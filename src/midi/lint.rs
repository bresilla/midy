//! Lint warnings and note repairs.

use std::collections::HashMap;

use midly::{Format, MetaMessage, Smf, TrackEvent, TrackEventKind, num::u28};

use super::edit::{EditSession, EditableNote};
use super::model::{Note, NoteScan, scan_notes};
use super::options::{FixOptions, StuckNoteFixMode};
use super::parse_smf;
use crate::Result;

/// Renders MIDI lint warnings.
pub fn render_lint(bytes: &[u8]) -> Result<String> {
    Ok(lint_report(bytes)?.0)
}

/// Renders MIDI lint warnings and returns how many there are.
pub fn lint_report(bytes: &[u8]) -> Result<(String, usize)> {
    let smf = parse_smf(bytes)?;
    let warnings = lint_warnings(&smf, &scan_notes(&smf));
    let mut out = String::new();

    emit!(out, "MIDY_LINT v1 warnings={}", warnings.len());
    if warnings.is_empty() {
        emit!(out, "OK");
    }
    for warning in &warnings {
        emit!(out, "{warning}");
    }

    Ok((out, warnings.len()))
}

/// Repairs common note-level MIDI problems while preserving other events.
pub fn fix_midi(bytes: &[u8]) -> Result<Vec<u8>> {
    fix_midi_with_options(bytes, FixOptions::default())
}

/// Repairs common note-level MIDI problems with explicit fix options.
pub fn fix_midi_with_options(bytes: &[u8], options: FixOptions) -> Result<Vec<u8>> {
    let mut smf = parse_smf(bytes)?;
    let orphans = scan_notes(&smf).orphan_events();
    let mut session = EditSession::from_smf(&smf);

    session
        .notes
        .iter_mut()
        .filter(|note| note.open)
        .for_each(|note| match options.stuck_note_mode {
            StuckNoteFixMode::Remove => note.deleted = true,
            StuckNoteFixMode::Close => {
                note.open = false;
                note.note.duration = options.stuck_note_duration.max(1);
            }
        });
    session
        .events
        .iter_mut()
        .filter(|event| orphans.contains(&(event.track, event.order)))
        .for_each(|event| event.deleted = true);

    mark_zero_duration_notes(&mut session.notes);
    mark_duplicate_notes(&mut session.notes);
    trim_overlapping_notes(&mut session.notes);

    smf.tracks = session.into_tracks()?;
    if options.remove_empty_tracks && smf.tracks.len() > 1 {
        smf.tracks.retain(|track| !is_empty_track(track));
        if smf.tracks.is_empty() {
            smf.tracks.push(vec![TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
            }]);
        }
        smf.header.format = if smf.tracks.len() <= 1 {
            Format::SingleTrack
        } else {
            Format::Parallel
        };
    }

    let mut out = Vec::new();
    smf.write_std(&mut out)?;
    Ok(out)
}

fn lint_warnings(smf: &Smf<'_>, scan: &NoteScan) -> Vec<String> {
    let notes = scan.notes.as_slice();
    let mut warnings = Vec::new();

    for (track_index, track) in smf.tracks.iter().enumerate() {
        if is_empty_track(track) {
            warnings.push(format!("WARN empty_track track={track_index}"));
        }
    }

    for note in notes.iter().filter(|note| note.duration == 0) {
        warnings.push(format!(
            "WARN zero_duration id={} track={} ch={} key={} start={}",
            note.id, note.track, note.channel, note.key, note.start,
        ));
    }

    let mut duplicate_groups = HashMap::<(usize, u8, u8, u64, u64), Vec<&Note>>::new();
    for note in notes {
        duplicate_groups
            .entry((
                note.track,
                note.channel,
                note.key,
                note.start,
                note.duration,
            ))
            .or_default()
            .push(note);
    }
    let mut duplicate_groups = duplicate_groups
        .into_iter()
        .filter(|(_, group)| group.len() > 1)
        .collect::<Vec<_>>();
    duplicate_groups.sort_by_key(|((track, channel, key, start, duration), _)| {
        (*track, *channel, *key, *start, *duration)
    });
    for ((track, channel, key, start, duration), mut group) in duplicate_groups {
        group.sort_by_key(|note| note.id.clone());
        warnings.push(format!(
            "WARN duplicate_note track={} ch={} key={} start={} dur={} count={} ids={}",
            track,
            channel,
            key,
            start,
            duration,
            group.len(),
            group
                .iter()
                .map(|note| note.id.as_str())
                .collect::<Vec<_>>()
                .join(","),
        ));
    }

    warnings.extend(overlap_warnings(notes));

    let mut stuck_notes = scan.stuck.iter().collect::<Vec<_>>();
    stuck_notes.sort_by_key(|note| (note.track, note.channel, note.key, note.start));
    for note in stuck_notes {
        warnings.push(format!(
            "WARN stuck_note id={} track={} ch={} key={} start={}",
            note.id, note.track, note.channel, note.key, note.start,
        ));
    }

    for orphan in &scan.orphans {
        warnings.push(format!(
            "WARN orphan_note_off track={} ch={} key={} tick={}",
            orphan.track, orphan.channel, orphan.key, orphan.tick,
        ));
    }

    if smf.header.format == Format::Sequential {
        warnings.push(
            "WARN sequential_format tracks are independent songs but midy shares one tempo and time-signature map"
                .to_owned(),
        );
    }

    warnings
}

pub(super) fn is_empty_track(track: &[TrackEvent<'_>]) -> bool {
    track
        .iter()
        .all(|event| matches!(event.kind, TrackEventKind::Meta(MetaMessage::EndOfTrack)))
}

fn mark_zero_duration_notes(notes: &mut [EditableNote]) {
    for note in notes {
        if note.note.duration == 0 {
            note.deleted = true;
        }
    }
}

fn mark_duplicate_notes(notes: &mut [EditableNote]) {
    let mut seen = HashMap::<(usize, u8, u8, u64, u64), usize>::new();
    for (index, editable) in notes.iter_mut().enumerate() {
        if editable.deleted {
            continue;
        }
        let note = &editable.note;
        let key = (
            note.track,
            note.channel,
            note.key,
            note.start,
            note.duration,
        );
        if seen.insert(key, index).is_some() {
            editable.deleted = true;
        }
    }
}

fn trim_overlapping_notes(notes: &mut [EditableNote]) {
    let mut indices = (0..notes.len()).collect::<Vec<_>>();
    indices.sort_by_key(|index| {
        let note = &notes[*index].note;
        (
            note.track,
            note.channel,
            note.key,
            note.start,
            note.start.saturating_add(note.duration),
            notes[*index].on_order,
        )
    });

    let mut previous = None::<usize>;
    for index in indices {
        if notes[index].deleted {
            continue;
        }
        if let Some(previous_index) = previous {
            let same_pitch = {
                let previous_note = &notes[previous_index].note;
                let note = &notes[index].note;
                previous_note.track == note.track
                    && previous_note.channel == note.channel
                    && previous_note.key == note.key
            };
            if same_pitch && !notes[previous_index].deleted {
                let previous_end = notes[previous_index]
                    .note
                    .start
                    .saturating_add(notes[previous_index].note.duration);
                if previous_end > notes[index].note.start {
                    if notes[index].note.start <= notes[previous_index].note.start {
                        notes[previous_index].deleted = true;
                    } else {
                        notes[previous_index].note.duration =
                            notes[index].note.start - notes[previous_index].note.start;
                    }
                }
            }
        }
        previous = Some(index);
    }
}

/// Finds same-pitch overlaps, comparing each note with the longest one before it.
fn overlap_warnings(notes: &[Note]) -> Vec<String> {
    let mut sorted = notes.iter().collect::<Vec<_>>();
    sorted.sort_by_key(|note| {
        (
            note.track,
            note.channel,
            note.key,
            note.start,
            note.start.saturating_add(note.duration),
            note.on_event_index,
        )
    });

    let mut warnings = Vec::new();
    let mut longest = None::<&Note>;
    for note in sorted {
        let same_pitch = longest.filter(|first| {
            first.track == note.track && first.channel == note.channel && first.key == note.key
        });
        let end = |note: &Note| note.start.saturating_add(note.duration);
        if let Some(first) = same_pitch
            && end(first) > note.start
        {
            warnings.push(format!(
                "WARN overlap track={} ch={} key={} first={} second={} first_end={} second_start={}",
                note.track,
                note.channel,
                note.key,
                first.id,
                note.id,
                end(first),
                note.start,
            ));
        }
        longest = match same_pitch {
            Some(first) if end(first) >= end(note) => Some(first),
            _ => Some(note),
        };
    }
    warnings
}
