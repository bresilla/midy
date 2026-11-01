//! Parsed edit commands and the filters they select with.

use crate::Error;
use crate::Result;
use crate::midi::format::note_name;
use crate::midi::model::{ConcreteNote, DRUM_CHANNEL};
use crate::midi::time::{Span, TimeSignatureMap};

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) struct LineCommand {
    pub(super) line: usize,
    pub(super) name: &'static str,
    pub(super) command: EditCommand,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) enum EditCommand {
    Add(ConcreteNote),
    Delete {
        id: String,
    },
    Set {
        id: String,
        patch: NotePatch,
    },
    Note {
        id: String,
        row: NoteRow,
    },
    DeleteMatching {
        filter: NoteFilter,
    },
    Transpose {
        semitones: i16,
        filter: NoteFilter,
    },
    Shift {
        ticks: i64,
        filter: NoteFilter,
        notes_only: bool,
    },
    ScaleTime {
        factor: Ratio,
        filter: NoteFilter,
        notes_only: bool,
    },
    ScaleDuration {
        factor: Ratio,
        filter: NoteFilter,
    },
    Quantize {
        grid: u64,
        mode: QuantizeMode,
        filter: NoteFilter,
    },
    Humanize {
        timing: u64,
        velocity: u8,
        seed: u64,
        filter: NoteFilter,
    },
    Swing {
        amount: u8,
        grid: u64,
        filter: NoteFilter,
    },
    Velocity {
        command: VelocityCommand,
        filter: NoteFilter,
    },
    Crescendo {
        start_velocity: u8,
        end_velocity: u8,
        filter: NoteFilter,
    },
    Chordize {
        intervals: Vec<i16>,
        filter: NoteFilter,
    },
    Arpeggiate {
        grid: u64,
        order: ArpeggioOrder,
        filter: NoteFilter,
    },
    BlockChord {
        grid: u64,
        filter: NoteFilter,
    },
    InvertChords {
        inversion: i16,
        filter: NoteFilter,
    },
    Double {
        octave: i16,
        filter: NoteFilter,
    },
    VoiceLead {
        max_jump: u8,
        filter: NoteFilter,
    },
    Solo {
        filter: NoteFilter,
    },
    MoveTrack {
        from: usize,
        to: usize,
        filter: NoteFilter,
    },
    SetChannel {
        channel: u8,
        filter: NoteFilter,
    },
    SetTempo {
        tick: u64,
        micros: u32,
    },
    SetProgram {
        track: usize,
        channel: u8,
        program: u8,
        tick: u64,
    },
    AddEvent {
        track: usize,
        tick: u64,
        event: NewEvent,
    },
    DeleteEvent {
        track: usize,
        tick: u64,
        event: NewEvent,
    },
    DeleteEvents {
        filter: EventFilter,
    },
    AddTrack {
        name: Option<Vec<u8>>,
    },
    SetTrackName {
        track: usize,
        name: Vec<u8>,
    },
}

/// Changes requested by `SET_NOTE`; only named fields change.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub(super) struct NotePatch {
    pub(super) track: Option<usize>,
    pub(super) channel: Option<u8>,
    pub(super) key: Option<u8>,
    pub(super) start: Option<u64>,
    pub(super) duration: Option<Span>,
    pub(super) velocity: Option<u8>,
    pub(super) off_velocity: Option<u8>,
}

impl NotePatch {
    pub(super) fn apply_to(
        &self,
        note: &mut ConcreteNote,
        time_map: Option<&TimeSignatureMap>,
    ) -> Result<()> {
        note.track = self.track.unwrap_or(note.track);
        note.channel = self.channel.unwrap_or(note.channel);
        note.key = self.key.unwrap_or(note.key);
        note.start = self.start.unwrap_or(note.start);
        if let Some(duration) = self.duration {
            note.duration = duration
                .ticks_at(time_map, note.start)
                .map_err(Error::Edit)?;
        }
        note.velocity = self.velocity.unwrap_or(note.velocity);
        note.off_velocity = self.off_velocity.unwrap_or(note.off_velocity);
        Ok(())
    }
}

/// A full note row, where some fields restate others.
///
/// `name`, `pos`, `end` and `end_pos` are derived from `key`, `start` and `dur`.
/// Whichever spelling differs from the current note is taken as the edit.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub(super) struct NoteRow {
    pub(super) track: Option<usize>,
    pub(super) channel: Option<u8>,
    pub(super) key: Option<u8>,
    pub(super) name: Option<u8>,
    pub(super) start: Option<u64>,
    pub(super) pos: Option<u64>,
    pub(super) duration: Option<Span>,
    pub(super) end: Option<u64>,
    pub(super) end_pos: Option<u64>,
    pub(super) velocity: Option<u8>,
    pub(super) off_velocity: Option<u8>,
}

impl NoteRow {
    pub(super) fn patch_for(
        &self,
        current: &ConcreteNote,
        time_map: Option<&TimeSignatureMap>,
    ) -> Result<NotePatch> {
        let key = reconcile(self.key, self.name, current.key, ("key", "name"))?;
        let start = reconcile(self.start, self.pos, current.start, ("start", "pos"))?;
        let new_start = start.unwrap_or(current.start);

        let current_end = current.start.saturating_add(current.duration);
        let end = reconcile(self.end, self.end_pos, current_end, ("end", "end_pos"))?
            .filter(|end| *end != current_end);
        let end_duration = end
            .map(|end| {
                end.checked_sub(new_start)
                    .filter(|duration| *duration > 0)
                    .ok_or_else(|| {
                        Error::Edit(format!("note end {end} must be after start {new_start}"))
                    })
            })
            .transpose()?;
        let duration = self
            .duration
            .map(|duration| duration.ticks_at(time_map, new_start).map_err(Error::Edit))
            .transpose()?;
        let duration = reconcile(duration, end_duration, current.duration, ("dur", "end"))?;

        Ok(NotePatch {
            track: self.track,
            channel: self.channel,
            key,
            start,
            duration: duration.map(Span::Ticks),
            velocity: self.velocity,
            off_velocity: self.off_velocity,
        })
    }
}

fn reconcile<T: Copy + Eq>(
    primary: Option<T>,
    derived: Option<T>,
    current: T,
    names: (&str, &str),
) -> Result<Option<T>> {
    match (primary, derived) {
        (Some(primary), Some(derived)) if primary != derived => {
            if primary != current && derived != current {
                Err(Error::Edit(format!(
                    "fields '{}' and '{}' disagree",
                    names.0, names.1
                )))
            } else if primary != current {
                Ok(Some(primary))
            } else {
                Ok(Some(derived))
            }
        }
        (primary, derived) => Ok(primary.or(derived)),
    }
}

/// How a filter treats channel 10 percussion.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub(in crate::midi) enum DrumMode {
    #[default]
    Include,
    Exclude,
    Only,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub(in crate::midi) struct NoteFilter {
    pub(super) track: Option<usize>,
    pub(super) channel: Option<u8>,
    pub(super) key: Option<u8>,
    pub(super) start: Option<u64>,
    pub(super) end: Option<u64>,
    pub(super) drums: DrumMode,
}

impl NoteFilter {
    pub(in crate::midi) fn matches(&self, note: &ConcreteNote) -> bool {
        self.track.is_none_or(|track| note.track == track)
            && self.allows_channel(note.channel)
            && self.key.is_none_or(|key| note.key == key)
            && self.allows_tick(note.start)
    }

    pub(super) fn allows_channel(&self, channel: u8) -> bool {
        self.channel.is_none_or(|wanted| channel == wanted)
            && match self.drums {
                DrumMode::Include => true,
                DrumMode::Exclude => channel != DRUM_CHANNEL,
                DrumMode::Only => channel == DRUM_CHANNEL,
            }
    }

    pub(super) fn allows_tick(&self, tick: u64) -> bool {
        self.start.is_none_or(|start| tick >= start) && self.end.is_none_or(|end| tick < end)
    }

    /// True when the filter names nothing, so the command covers the whole song.
    pub(super) fn is_whole_song(&self) -> bool {
        self.track.is_none()
            && self.channel.is_none()
            && self.key.is_none()
            && self.start.is_none()
            && self.end.is_none()
    }
}

/// Selects non-note events.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub(super) struct EventFilter {
    pub(super) kind: Option<String>,
    pub(super) controller: Option<u8>,
    pub(super) track: Option<usize>,
    pub(super) channel: Option<u8>,
    pub(super) start: Option<u64>,
    pub(super) end: Option<u64>,
}

/// A non-note event described by an edit line.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) enum NewEvent {
    Controller {
        channel: u8,
        controller: u8,
        value: u8,
    },
    PitchBend {
        channel: u8,
        value: i16,
    },
    ProgramChange {
        channel: u8,
        program: u8,
    },
    ChannelAftertouch {
        channel: u8,
        pressure: u8,
    },
    PolyAftertouch {
        channel: u8,
        key: u8,
        pressure: u8,
    },
    OrphanOff {
        channel: u8,
        key: u8,
        velocity: u8,
    },
    Tempo {
        micros: u32,
    },
    TimeSignature {
        numerator: u8,
        power: u8,
        clocks: u8,
        thirtyseconds: u8,
    },
    KeySignature {
        sharps: i8,
        minor: bool,
    },
    Text {
        kind: TextKind,
        bytes: Vec<u8>,
    },
    SequenceNumber {
        value: Option<u16>,
    },
    MidiChannel {
        channel: u8,
    },
    MidiPort {
        port: u8,
    },
    SmpteOffset {
        fps: u8,
        hour: u8,
        minute: u8,
        second: u8,
        frame: u8,
        subframe: u8,
    },
    SequencerSpecific {
        bytes: Vec<u8>,
    },
    UnknownMeta {
        kind: u8,
        bytes: Vec<u8>,
    },
    SysEx {
        bytes: Vec<u8>,
    },
    Escape {
        bytes: Vec<u8>,
    },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum TextKind {
    Text,
    Copyright,
    TrackName,
    InstrumentName,
    Lyric,
    Marker,
    CuePoint,
    ProgramName,
    DeviceName,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) struct Ratio {
    numerator: u64,
    denominator: u64,
}

impl Ratio {
    pub(super) fn new(numerator: u64, denominator: u64) -> Option<Self> {
        if numerator == 0 || denominator == 0 {
            return None;
        }
        let divisor = greatest_common_divisor(numerator, denominator);
        Some(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    pub(super) fn scale(self, value: u64) -> Result<u64> {
        value
            .checked_mul(self.numerator)
            .and_then(|value| value.checked_add(self.denominator / 2))
            .map(|value| value / self.denominator)
            .ok_or_else(|| Error::Edit("scaled tick overflowed u64".to_owned()))
    }
}

fn greatest_common_divisor(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum QuantizeMode {
    Start,
    Duration,
    Both,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum VelocityCommand {
    Scale(Ratio),
    Add(i16),
    Set(u8),
    Compress { factor: Ratio, center: u8 },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum ArpeggioOrder {
    Up,
    Down,
    UpDown,
    Outside,
}

/// Names a note for error messages.
pub(super) fn describe_note(id: Option<&str>, note: &ConcreteNote) -> String {
    match id {
        Some(id) => format!("note {id}"),
        None => format!(
            "new note {} at tick {} on track {}",
            note_name(note.key),
            note.start,
            note.track
        ),
    }
}
