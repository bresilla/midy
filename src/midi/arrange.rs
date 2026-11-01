//! Structural operations: selecting, splitting and merging tracks.

use midly::{Format, MetaMessage, Smf, TrackEvent, TrackEventKind};

use super::edit::{TimedEvent, finish_track};
use super::format::timing_fields;
use super::model::{collect_notes, meta_events};
use super::options::{SplitMode, TrackChannelSelector};
use super::parse_smf;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum SelectionAction {
    Mute,
    Solo,
}

/// Extracts matching MIDI channel events while preserving non-MIDI/meta events.
pub fn extract_selection(bytes: &[u8], selector: &TrackChannelSelector) -> Result<Vec<u8>> {
    transform_selection(bytes, selector, SelectionAction::Solo)
}

/// Removes matching MIDI channel events while preserving everything else.
pub fn mute_selection(bytes: &[u8], selector: &TrackChannelSelector) -> Result<Vec<u8>> {
    transform_selection(bytes, selector, SelectionAction::Mute)
}

/// Keeps only matching MIDI channel events while preserving non-MIDI/meta events.
pub fn solo_selection(bytes: &[u8], selector: &TrackChannelSelector) -> Result<Vec<u8>> {
    transform_selection(bytes, selector, SelectionAction::Solo)
}

/// Splits a MIDI file into one filtered MIDI file per track or channel.
pub fn split_selection(bytes: &[u8], mode: SplitMode) -> Result<Vec<(String, Vec<u8>)>> {
    let smf = parse_smf(bytes)?;
    let notes = collect_notes(&smf);
    let mut outputs = Vec::new();
    match mode {
        SplitMode::Track => {
            for track in 0..smf.tracks.len() {
                if notes.iter().any(|note| note.track == track) {
                    let selector = TrackChannelSelector {
                        track: Some(track),
                        channel: None,
                    };
                    outputs.push((
                        format!("track-{track}.mid"),
                        extract_selection(bytes, &selector)?,
                    ));
                }
            }
        }
        SplitMode::Channel => {
            let mut channels = notes.iter().map(|note| note.channel).collect::<Vec<_>>();
            channels.sort_unstable();
            channels.dedup();
            for channel in channels {
                let selector = TrackChannelSelector {
                    track: None,
                    channel: Some(channel),
                };
                outputs.push((
                    format!("ch-{channel}.mid"),
                    extract_selection(bytes, &selector)?,
                ));
            }
        }
    }
    Ok(outputs)
}

/// Merges multiple MIDI files into one parallel-track MIDI file.
pub fn merge_midi(inputs: &[&[u8]]) -> Result<Vec<u8>> {
    Ok(merge_midi_report(inputs)?.0)
}

/// Merges files and describes anything dropped to keep one tempo map.
///
/// The first input with tempo changes sets the tempo map. Tempo events from
/// inputs that disagree with it are removed.
pub fn merge_midi_report(inputs: &[&[u8]]) -> Result<(Vec<u8>, Vec<String>)> {
    if inputs.is_empty() {
        return Err(Error::Usage(
            "merge requires at least one input MIDI".to_owned(),
        ));
    }
    let smfs = inputs
        .iter()
        .map(|bytes| parse_smf(bytes))
        .collect::<Result<Vec<_>>>()?;
    let timing = smfs[0].header.timing;
    for (index, smf) in smfs.iter().enumerate().skip(1) {
        if smf.header.timing != timing {
            return Err(Error::Input(format!(
                "merge input {index} has timing '{}', expected '{}'",
                timing_fields(smf.header.timing),
                timing_fields(timing),
            )));
        }
    }

    let reference = smfs
        .iter()
        .map(tempo_map)
        .find(|tempos| !tempos.is_empty())
        .unwrap_or_default();
    let mut warnings = Vec::new();
    let mut tracks = Vec::new();
    for (index, smf) in smfs.iter().enumerate() {
        let tempos = tempo_map(smf);
        let conflicting = !tempos.is_empty() && tempos != reference;
        if conflicting {
            warnings.push(format!(
                "merge input {index}: dropped {} tempo events that conflict with the first tempo map",
                tempos.len()
            ));
        }
        for track in &smf.tracks {
            tracks.push(if conflicting {
                without_tempo(track)?
            } else {
                track.clone()
            });
        }
    }

    let format = if tracks.len() <= 1 {
        Format::SingleTrack
    } else {
        Format::Parallel
    };
    let smf = Smf {
        header: midly::Header { format, timing },
        tracks,
    };
    let mut out = Vec::new();
    smf.write_std(&mut out)?;
    Ok((out, warnings))
}

fn tempo_map(smf: &Smf<'_>) -> Vec<(u64, u32)> {
    let mut tempos = meta_events(smf)
        .into_iter()
        .filter_map(|(_, tick, meta)| match meta {
            MetaMessage::Tempo(micros) => Some((tick, micros.as_int())),
            _ => None,
        })
        .collect::<Vec<_>>();
    tempos.sort_unstable();
    tempos
}

fn without_tempo<'a>(track: &[TrackEvent<'a>]) -> Result<Vec<TrackEvent<'a>>> {
    rebuild_with(track, |kind| {
        !matches!(kind, TrackEventKind::Meta(MetaMessage::Tempo(_)))
    })
}

fn transform_selection(
    bytes: &[u8],
    selector: &TrackChannelSelector,
    action: SelectionAction,
) -> Result<Vec<u8>> {
    let mut smf = parse_smf(bytes)?;
    validate_selector(selector, smf.tracks.len())?;
    for (track_index, track) in smf.tracks.iter_mut().enumerate() {
        *track = rebuild_with(track, |kind| {
            keep_event_for_selection(track_index, kind, selector, action)
        })?;
    }
    let mut out = Vec::new();
    smf.write_std(&mut out)?;
    Ok(out)
}

fn validate_selector(selector: &TrackChannelSelector, track_count: usize) -> Result<()> {
    if let Some(track) = selector.track
        && track >= track_count
    {
        return Err(Error::Usage(format!(
            "track {track} does not exist; file has {track_count} tracks"
        )));
    }
    if selector.track.is_none() && selector.channel.is_none() {
        return Err(Error::Usage(
            "select at least one of --track or --ch/--channel".to_owned(),
        ));
    }
    Ok(())
}

/// Copies the events `keep` accepts, preserving the timing of those that remain.
fn rebuild_with<'a>(
    track: &[TrackEvent<'a>],
    keep: impl Fn(TrackEventKind<'a>) -> bool,
) -> Result<Vec<TrackEvent<'a>>> {
    let mut timed = Vec::with_capacity(track.len());
    let mut tick = 0_u64;
    let mut end_tick = 0_u64;

    for (index, event) in track.iter().enumerate() {
        tick += u64::from(event.delta.as_int());
        if matches!(event.kind, TrackEventKind::Meta(MetaMessage::EndOfTrack)) {
            end_tick = end_tick.max(tick);
        } else if keep(event.kind) {
            timed.push(TimedEvent::kept(tick, index, event.kind));
        }
    }

    finish_track(timed, end_tick)
}

fn keep_event_for_selection(
    track_index: usize,
    kind: TrackEventKind<'_>,
    selector: &TrackChannelSelector,
    action: SelectionAction,
) -> bool {
    let TrackEventKind::Midi { channel, .. } = kind else {
        return true;
    };
    let matches = selector.matches_midi(track_index, channel.as_int());
    match action {
        SelectionAction::Mute => !matches,
        SelectionAction::Solo => matches,
    }
}
