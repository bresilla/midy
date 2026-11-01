//! Note pairing and the note types shared across the crate.

use std::collections::{HashMap, HashSet, VecDeque};

use midly::{MetaMessage, MidiMessage, Smf, TrackEventKind};

/// MIDI channel reserved for percussion, zero-based.
pub(super) const DRUM_CHANNEL: u8 = 9;

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) struct Note {
    pub(super) id: String,
    pub(super) track: usize,
    pub(super) channel: u8,
    pub(super) key: u8,
    pub(super) start: u64,
    pub(super) duration: u64,
    pub(super) velocity: u8,
    pub(super) off_velocity: u8,
    pub(super) on_event_index: usize,
    pub(super) off_event_index: usize,
}

/// A note-on whose note-off never arrives.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) struct StuckNote {
    pub(super) id: String,
    pub(super) track: usize,
    pub(super) channel: u8,
    pub(super) key: u8,
    pub(super) start: u64,
    pub(super) velocity: u8,
    pub(super) on_event_index: usize,
}

/// A note-off with no matching note-on.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) struct OrphanOff {
    pub(super) track: usize,
    pub(super) event_index: usize,
    pub(super) tick: u64,
    pub(super) channel: u8,
    pub(super) key: u8,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) struct ConcreteNote {
    pub(super) track: usize,
    pub(super) channel: u8,
    pub(super) key: u8,
    pub(super) start: u64,
    pub(super) duration: u64,
    pub(super) velocity: u8,
    pub(super) off_velocity: u8,
}

/// Every note-related event in a file, classified by how it pairs.
#[derive(Debug, Default)]
pub(super) struct NoteScan {
    pub(super) notes: Vec<Note>,
    pub(super) stuck: Vec<StuckNote>,
    pub(super) orphans: Vec<OrphanOff>,
}

impl NoteScan {
    pub(super) fn orphan_events(&self) -> HashSet<(usize, usize)> {
        self.orphans
            .iter()
            .map(|orphan| (orphan.track, orphan.event_index))
            .collect()
    }
}

struct PendingNote {
    id: String,
    start: u64,
    velocity: u8,
    on_event_index: usize,
}

/// Pairs note-ons with note-offs first-in, first-out per track, channel and key.
pub(super) fn scan_notes(smf: &Smf<'_>) -> NoteScan {
    let mut scan = NoteScan::default();

    for (track_index, track) in smf.tracks.iter().enumerate() {
        let mut absolute_tick = 0_u64;
        let mut note_ordinal = 0_usize;
        let mut active = HashMap::<(u8, u8), VecDeque<PendingNote>>::new();

        for (event_index, event) in track.iter().enumerate() {
            absolute_tick += u64::from(event.delta.as_int());
            let Some((channel, key, velocity, is_on)) = note_event(event.kind) else {
                continue;
            };

            if is_on {
                active
                    .entry((channel, key))
                    .or_default()
                    .push_back(PendingNote {
                        id: format!("t{track_index}n{note_ordinal}"),
                        start: absolute_tick,
                        velocity,
                        on_event_index: event_index,
                    });
                note_ordinal += 1;
            } else if let Some(pending) = active
                .get_mut(&(channel, key))
                .and_then(VecDeque::pop_front)
            {
                scan.notes.push(Note {
                    id: pending.id,
                    track: track_index,
                    channel,
                    key,
                    start: pending.start,
                    duration: absolute_tick.saturating_sub(pending.start),
                    velocity: pending.velocity,
                    off_velocity: velocity,
                    on_event_index: pending.on_event_index,
                    off_event_index: event_index,
                });
            } else {
                scan.orphans.push(OrphanOff {
                    track: track_index,
                    event_index,
                    tick: absolute_tick,
                    channel,
                    key,
                });
            }
        }

        for ((channel, key), pending) in active {
            scan.stuck.extend(pending.into_iter().map(|note| StuckNote {
                id: note.id,
                track: track_index,
                channel,
                key,
                start: note.start,
                velocity: note.velocity,
                on_event_index: note.on_event_index,
            }));
        }
    }

    scan.stuck
        .sort_by_key(|note| (note.track, note.on_event_index));
    scan
}

pub(super) fn collect_notes(smf: &Smf<'_>) -> Vec<Note> {
    scan_notes(smf).notes
}

/// Returns `(channel, key, velocity, is_note_on)` for note messages.
pub(super) fn note_event(kind: TrackEventKind<'_>) -> Option<(u8, u8, u8, bool)> {
    match kind {
        TrackEventKind::Midi { channel, message } => match message {
            MidiMessage::NoteOn { key, vel } => Some((
                channel.as_int(),
                key.as_int(),
                vel.as_int(),
                vel.as_int() > 0,
            )),
            MidiMessage::NoteOff { key, vel } => {
                Some((channel.as_int(), key.as_int(), vel.as_int(), false))
            }
            _ => None,
        },
        _ => None,
    }
}

pub(super) fn meta_events<'a>(smf: &Smf<'a>) -> Vec<(usize, u64, MetaMessage<'a>)> {
    let mut events = Vec::new();
    for (track_index, track) in smf.tracks.iter().enumerate() {
        let mut tick = 0_u64;
        for event in track {
            tick += u64::from(event.delta.as_int());
            if let TrackEventKind::Meta(meta) = event.kind {
                events.push((track_index, tick, meta));
            }
        }
    }
    events
}

pub(super) fn song_length_ticks(smf: &Smf<'_>, notes: &[Note]) -> u64 {
    let event_length = smf
        .tracks
        .iter()
        .map(|track| {
            track
                .iter()
                .map(|event| u64::from(event.delta.as_int()))
                .sum::<u64>()
        })
        .max()
        .unwrap_or_default();
    let note_length = notes
        .iter()
        .map(|note| note.start.saturating_add(note.duration))
        .max()
        .unwrap_or_default();

    event_length.max(note_length)
}
