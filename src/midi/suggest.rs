//! Generated edit commands.

use super::chords::{choose_chord_note, chord_groups, chord_groups_by_voice, chord_root};
use super::format::ticks_per_beat;
use super::model::collect_notes;
use super::options::{BasslineOptions, ReduceChordsOptions};
use super::parse_smf;
use super::time::{TimeSignatureMap, option_ticks};
use crate::{Error, Result};

/// Suggests edit commands that reduce chord stacks to one note per slice.
///
/// Each track and channel is reduced on its own.
pub fn suggest_reduce_chords(bytes: &[u8], options: &ReduceChordsOptions) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let time_map = TimeSignatureMap::from_smf(&smf);
    let query = options.query.resolved(&smf, time_map.as_ref())?;
    let notes = collect_notes(&smf)
        .into_iter()
        .filter(|note| query.matches_pitched(note))
        .collect::<Vec<_>>();
    let mut out = String::new();

    for group in chord_groups_by_voice(notes, options.window) {
        let references = group.iter().collect::<Vec<_>>();
        let keep_index = choose_chord_note(&references, &options.keep);
        for (index, note) in group.iter().enumerate() {
            if index != keep_index {
                emit!(out, "DELETE_NOTE id={}", note.id);
            }
        }
    }

    Ok(out)
}

/// Suggests ADD_NOTE commands for a simple bassline using detected chord roots.
pub fn suggest_bassline(bytes: &[u8], options: &BasslineOptions) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let time_map = TimeSignatureMap::from_smf(&smf);
    let fixed_duration = options
        .duration
        .as_deref()
        .map(|value| {
            option_ticks(
                "dur",
                value,
                ticks_per_beat(smf.header.timing),
                time_map.as_ref(),
            )
        })
        .transpose()?;
    let query = options.query.resolved(&smf, time_map.as_ref())?;
    let mut notes = collect_notes(&smf)
        .into_iter()
        .filter(|note| query.matches_pitched(note))
        .collect::<Vec<_>>();
    let groups = chord_groups(&mut notes, options.window);
    let mut out = String::new();

    let track_count = smf.tracks.len();
    let output_track = options.output_track.unwrap_or(track_count);
    for track in track_count..=output_track {
        if track == output_track && options.output_track.is_none() {
            emit!(out, "ADD_TRACK name=Bass");
        } else {
            emit!(out, "ADD_TRACK");
        }
    }

    for group in groups.into_iter().filter(|group| group.len() > 1) {
        let start = group
            .iter()
            .map(|note| note.start)
            .min()
            .unwrap_or_default();
        let duration = fixed_duration.unwrap_or_else(|| {
            group
                .iter()
                .map(|note| note.duration)
                .max()
                .unwrap_or(1)
                .max(1)
        });
        let mut keys = group.iter().map(|note| note.key).collect::<Vec<_>>();
        keys.sort_unstable();
        keys.dedup();
        let key = midi_key_for_octave(chord_root(&keys), options.octave)?;
        emit!(
            out,
            "ADD_NOTE track={} ch={} key={} start={} dur={} vel={}",
            output_track,
            options.output_channel,
            key,
            start,
            duration,
            options.velocity,
        );
    }

    Ok(out)
}

fn midi_key_for_octave(pc: u8, octave: i16) -> Result<u8> {
    let key = octave
        .checked_add(1)
        .and_then(|octave| octave.checked_mul(12))
        .and_then(|base| base.checked_add(i16::from(pc % 12)))
        .ok_or_else(|| Error::Usage("generated bass key overflowed".to_owned()))?;
    if (0..=127).contains(&key) {
        Ok(key as u8)
    } else {
        Err(Error::Usage(format!(
            "generated bass key {key} is outside MIDI range 0..127; change --octave"
        )))
    }
}
