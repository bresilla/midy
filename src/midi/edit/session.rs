//! In-memory edit state and writing it back to MIDI tracks.

use std::collections::{HashMap, HashSet};

use midly::{MetaMessage, MidiMessage, Smf, TrackEvent, TrackEventKind, num::u4, num::u7};

use crate::midi::format::checked_delta;
use crate::midi::model::{ConcreteNote, Note, StuckNote, scan_notes};
use crate::{Error, Result};

use super::command::describe_note;

const RANK_NOTE_OFF: u8 = 0;
const RANK_ADDED_EVENT: u8 = 1;
const RANK_DEFAULT: u8 = 2;

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::midi) struct EditableNote {
    pub(in crate::midi) id: Option<String>,
    pub(in crate::midi) note: ConcreteNote,
    pub(in crate::midi) original: Option<ConcreteNote>,
    pub(in crate::midi) on_order: usize,
    pub(in crate::midi) off_order: usize,
    /// A stuck note: its note-on is written without a note-off.
    pub(in crate::midi) open: bool,
    pub(in crate::midi) deleted: bool,
}

impl EditableNote {
    fn from_note(note: &Note) -> Self {
        let concrete = ConcreteNote {
            track: note.track,
            channel: note.channel,
            key: note.key,
            start: note.start,
            duration: note.duration,
            velocity: note.velocity,
            off_velocity: note.off_velocity,
        };
        Self {
            id: Some(note.id.clone()),
            original: Some(concrete.clone()),
            note: concrete,
            on_order: note.on_event_index,
            off_order: note.off_event_index,
            open: false,
            deleted: false,
        }
    }

    fn from_stuck(note: &StuckNote) -> Self {
        let concrete = ConcreteNote {
            track: note.track,
            channel: note.channel,
            key: note.key,
            start: note.start,
            duration: 0,
            velocity: note.velocity,
            off_velocity: 64,
        };
        Self {
            id: Some(note.id.clone()),
            original: Some(concrete.clone()),
            note: concrete,
            on_order: note.on_event_index,
            off_order: note.on_event_index,
            open: true,
            deleted: false,
        }
    }
}

#[derive(Debug, Clone)]
pub(in crate::midi) struct EditableEvent<'a> {
    pub(in crate::midi) track: usize,
    pub(in crate::midi) tick: u64,
    pub(in crate::midi) order: usize,
    pub(in crate::midi) kind: TrackEventKind<'a>,
    pub(in crate::midi) added: bool,
    pub(in crate::midi) deleted: bool,
}

/// Every note and event of a file, with notes addressable by id.
#[derive(Debug)]
pub(in crate::midi) struct EditSession<'a> {
    pub(in crate::midi) notes: Vec<EditableNote>,
    pub(in crate::midi) events: Vec<EditableEvent<'a>>,
    pub(in crate::midi) track_ends: Vec<u64>,
    index: HashMap<String, usize>,
    next_order: usize,
}

impl<'a> EditSession<'a> {
    pub(in crate::midi) fn from_smf(smf: &Smf<'a>) -> Self {
        let scan = scan_notes(smf);
        let note_events = scan
            .notes
            .iter()
            .flat_map(|note| {
                [
                    (note.track, note.on_event_index),
                    (note.track, note.off_event_index),
                ]
            })
            .chain(
                scan.stuck
                    .iter()
                    .map(|note| (note.track, note.on_event_index)),
            )
            .collect::<HashSet<_>>();

        let mut events = Vec::new();
        let mut track_ends = vec![0_u64; smf.tracks.len()];
        for (track_index, track) in smf.tracks.iter().enumerate() {
            let mut tick = 0_u64;
            for (event_index, event) in track.iter().enumerate() {
                tick += u64::from(event.delta.as_int());
                if matches!(event.kind, TrackEventKind::Meta(MetaMessage::EndOfTrack)) {
                    track_ends[track_index] = track_ends[track_index].max(tick);
                } else if !note_events.contains(&(track_index, event_index)) {
                    events.push(EditableEvent {
                        track: track_index,
                        tick,
                        order: event_index,
                        kind: event.kind,
                        added: false,
                        deleted: false,
                    });
                }
            }
        }

        let notes = scan
            .notes
            .iter()
            .map(EditableNote::from_note)
            .chain(scan.stuck.iter().map(EditableNote::from_stuck))
            .collect::<Vec<_>>();
        let index = notes
            .iter()
            .enumerate()
            .filter_map(|(position, note)| Some((note.id.clone()?, position)))
            .collect();
        let next_order = smf.tracks.iter().map(Vec::len).max().unwrap_or_default() + 1;

        Self {
            notes,
            events,
            track_ends,
            index,
            next_order,
        }
    }

    pub(in crate::midi) fn track_count(&self) -> usize {
        self.track_ends.len()
    }

    pub(super) fn check_track(&self, track: usize) -> Result<()> {
        if track < self.track_count() {
            Ok(())
        } else {
            Err(Error::Edit(format!(
                "track {track} does not exist; file has {} tracks",
                self.track_count()
            )))
        }
    }

    /// Finds a live note by id.
    pub(super) fn note_position(&self, id: &str) -> Result<usize> {
        let position = *self
            .index
            .get(id)
            .ok_or_else(|| Error::Edit(format!("unknown note id '{id}'")))?;
        if self.notes[position].deleted {
            return Err(Error::Edit(format!(
                "note '{id}' was deleted by an earlier line"
            )));
        }
        Ok(position)
    }

    pub(super) fn add_note(&mut self, note: ConcreteNote) -> usize {
        let on_order = self.take_order();
        let off_order = self.take_order();
        self.notes.push(EditableNote {
            id: None,
            note,
            original: None,
            on_order,
            off_order,
            open: false,
            deleted: false,
        });
        self.notes.len() - 1
    }

    pub(super) fn add_event(&mut self, track: usize, tick: u64, kind: TrackEventKind<'a>) {
        let order = self.take_order();
        self.events.push(EditableEvent {
            track,
            tick,
            order,
            kind,
            added: true,
            deleted: false,
        });
    }

    fn take_order(&mut self) -> usize {
        let order = self.next_order;
        self.next_order += 1;
        order
    }

    pub(super) fn validate_note(&self, position: usize) -> Result<()> {
        let editable = &self.notes[position];
        let note = &editable.note;
        let describe = || describe_note(editable.id.as_deref(), note);
        self.check_track(note.track)
            .map_err(|error| Error::Edit(format!("{}: {error}", describe())))?;

        let kept_zero_length = editable
            .original
            .as_ref()
            .is_some_and(|original| original.duration == 0);
        if note.duration == 0 && !editable.open && !kept_zero_length {
            return Err(Error::Edit(format!(
                "{}: note duration must be greater than zero",
                describe()
            )));
        }
        let end = note
            .start
            .checked_add(note.duration)
            .ok_or_else(|| Error::Edit(format!("{}: note end tick overflowed", describe())))?;
        checked_delta(end).map_err(|error| Error::Edit(format!("{}: {error}", describe())))?;
        Ok(())
    }

    /// Validates every note that differs from the input file.
    pub(super) fn validate_changed(&self) -> Result<()> {
        for (position, editable) in self.notes.iter().enumerate() {
            if !editable.deleted && editable.original.as_ref() != Some(&editable.note) {
                self.validate_note(position)?;
            }
        }
        Ok(())
    }

    pub(in crate::midi) fn into_tracks(mut self) -> Result<Vec<Vec<TrackEvent<'a>>>> {
        align_note_pairs(&mut self.notes);
        let mut timed = vec![Vec::<TimedEvent<'a>>::new(); self.track_count()];

        for event in self.events.into_iter().filter(|event| !event.deleted) {
            timed[event.track].push(TimedEvent {
                tick: event.tick,
                rank: if event.added {
                    RANK_ADDED_EVENT
                } else {
                    RANK_DEFAULT
                },
                order: event.order,
                kind: event.kind,
            });
        }

        for editable in self.notes.iter().filter(|note| !note.deleted) {
            let note = &editable.note;
            let end = note
                .start
                .checked_add(note.duration)
                .ok_or_else(|| Error::Edit("note end tick overflowed".to_owned()))?;
            let channel = u4::from(note.channel);
            let key = u7::from(note.key);
            timed[note.track].push(TimedEvent {
                tick: note.start,
                rank: RANK_DEFAULT,
                order: editable.on_order,
                kind: TrackEventKind::Midi {
                    channel,
                    message: MidiMessage::NoteOn {
                        key,
                        vel: u7::from(note.velocity),
                    },
                },
            });
            if !editable.open {
                timed[note.track].push(TimedEvent {
                    tick: end,
                    rank: if note.duration > 0 {
                        RANK_NOTE_OFF
                    } else {
                        RANK_DEFAULT
                    },
                    order: editable.off_order,
                    kind: TrackEventKind::Midi {
                        channel,
                        message: MidiMessage::NoteOff {
                            key,
                            vel: u7::from(note.off_velocity),
                        },
                    },
                });
            }
        }

        timed
            .into_iter()
            .zip(self.track_ends)
            .map(|(events, end)| finish_track(events, end))
            .collect()
    }
}

/// Orders note-ons that share a tick and pitch by when each note ends.
///
/// Readers pair the first note-on with the first note-off, so this keeps each
/// note's length with its own velocity.
fn align_note_pairs(notes: &mut [EditableNote]) {
    let mut groups = HashMap::<(usize, u8, u8, u64), Vec<usize>>::new();
    for (position, editable) in notes.iter().enumerate() {
        if !editable.deleted {
            let note = &editable.note;
            groups
                .entry((note.track, note.channel, note.key, note.start))
                .or_default()
                .push(position);
        }
    }

    for group in groups.values_mut().filter(|group| group.len() > 1) {
        let mut orders = group
            .iter()
            .map(|position| notes[*position].on_order)
            .collect::<Vec<_>>();
        orders.sort_unstable();
        group.sort_by_key(|position| {
            let editable = &notes[*position];
            (
                editable.open,
                editable.note.start.saturating_add(editable.note.duration),
                editable.off_order,
            )
        });
        for (position, order) in group.iter().zip(orders) {
            notes[*position].on_order = order;
        }
    }
}

#[derive(Debug, Clone)]
pub(in crate::midi) struct TimedEvent<'a> {
    pub(in crate::midi) tick: u64,
    pub(in crate::midi) rank: u8,
    pub(in crate::midi) order: usize,
    pub(in crate::midi) kind: TrackEventKind<'a>,
}

impl<'a> TimedEvent<'a> {
    /// An event that keeps its place among others on the same tick.
    pub(in crate::midi) fn kept(tick: u64, order: usize, kind: TrackEventKind<'a>) -> Self {
        Self {
            tick,
            rank: RANK_DEFAULT,
            order,
            kind,
        }
    }
}

/// Sorts events, converts ticks to deltas and closes the track.
pub(in crate::midi) fn finish_track<'a>(
    mut events: Vec<TimedEvent<'a>>,
    end_tick: u64,
) -> Result<Vec<TrackEvent<'a>>> {
    events.sort_by_key(|event| (event.tick, event.rank, event.order));

    let last_event_tick = events.last().map_or(0, |event| event.tick);
    let mut track = Vec::with_capacity(events.len() + 1);
    let mut previous_tick = 0_u64;
    for event in events {
        track.push(TrackEvent {
            delta: checked_delta(event.tick - previous_tick)?,
            kind: event.kind,
        });
        previous_tick = event.tick;
    }
    track.push(TrackEvent {
        delta: checked_delta(end_tick.max(last_event_tick) - previous_tick)?,
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });
    Ok(track)
}
