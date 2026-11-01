//! Public option and report types.

use midly::Smf;

use super::model::{DRUM_CHANNEL, Note};
use super::time::TimeSignatureMap;
use crate::{Error, Result};

/// Which note should be kept when reducing a simultaneous chord stack.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ChordKeep {
    /// Keep the highest note in each chord slice.
    Highest,
    /// Keep the lowest note in each chord slice.
    Lowest,
    /// Keep the detected root pitch class, preferring the lowest matching note.
    Root,
    /// Keep the Nth note from the bottom of the chord, using 1-based indexing.
    Nth(usize),
}

/// Common note-selection options used by read-only commands.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct QueryOptions {
    /// Restrict to one track.
    pub track: Option<usize>,
    /// Restrict to one MIDI channel.
    pub channel: Option<u8>,
    /// Restrict to one MIDI key.
    pub key: Option<u8>,
    /// Restrict to notes starting at or after this tick.
    pub start: Option<u64>,
    /// Restrict to notes starting before this tick.
    pub end: Option<u64>,
    /// Restrict to a musical bar range like `1..8` using the MIDI time-signature map.
    pub bars: Option<String>,
    /// Count channel 10 percussion in pitch analysis.
    pub include_drums: bool,
}

impl QueryOptions {
    /// Validates the selection against a file and converts bars to ticks.
    pub(super) fn resolved(
        &self,
        smf: &Smf<'_>,
        time_map: Option<&TimeSignatureMap>,
    ) -> Result<Self> {
        if let Some(track) = self.track
            && track >= smf.tracks.len()
        {
            return Err(Error::Usage(format!(
                "track {track} does not exist; file has {} tracks",
                smf.tracks.len()
            )));
        }
        let mut query = self.clone();
        if let Some(bars) = query.bars.take() {
            if query.start.is_some() || query.end.is_some() {
                return Err(Error::Usage(
                    "use either --bars or --start/--end, not both".to_owned(),
                ));
            }
            let time_map = time_map
                .ok_or_else(|| Error::Usage("--bars requires metrical MIDI timing".to_owned()))?;
            let (start, end) = time_map.bar_range_ticks(&bars)?;
            query.start = Some(start);
            query.end = Some(end);
        }
        Ok(query)
    }

    pub(super) fn matches_note(&self, note: &Note) -> bool {
        self.track.is_none_or(|track| note.track == track)
            && self.channel.is_none_or(|channel| note.channel == channel)
            && self.key.is_none_or(|key| note.key == key)
            && self.matches_tick(note.start)
    }

    pub(super) fn matches_tick(&self, tick: u64) -> bool {
        self.start.is_none_or(|start| tick >= start) && self.end.is_none_or(|end| tick < end)
    }

    /// Selects notes for pitch analysis, leaving percussion out unless requested.
    pub(super) fn matches_pitched(&self, note: &Note) -> bool {
        self.matches_note(note)
            && (note.channel != DRUM_CHANNEL
                || self.include_drums
                || self.channel == Some(DRUM_CHANNEL))
    }

    /// Renders the resolved selection using edit filter field names.
    pub(super) fn scope_text(&self) -> Option<String> {
        let mut fields = Vec::new();
        if let Some(track) = self.track {
            fields.push(format!("track={track}"));
        }
        if let Some(channel) = self.channel {
            fields.push(format!("ch={channel}"));
        }
        if let Some(key) = self.key {
            fields.push(format!("key={key}"));
        }
        if let Some(start) = self.start {
            fields.push(format!("start={start}"));
        }
        if let Some(end) = self.end {
            fields.push(format!("end={end}"));
        }
        (!fields.is_empty()).then(|| fields.join(" "))
    }
}

/// How much non-note data the timeline shows.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum EventDetail {
    /// Every event in the file.
    #[default]
    All,
    /// Meta events, program changes and system messages, without controller
    /// data, pitch bends or aftertouch.
    Basic,
    /// Note lines only.
    None,
}

/// Options for `midy read`.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ReadOptions {
    /// Selection filter.
    pub query: QueryOptions,
    /// Which non-note events to show.
    pub events: EventDetail,
    /// Leave derived note fields out of the text timeline.
    pub compact: bool,
}

/// Options for chord-reduction suggestions.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReduceChordsOptions {
    /// Which note to keep in each detected chord.
    pub keep: ChordKeep,
    /// Selection filter.
    pub query: QueryOptions,
    /// Group notes whose start ticks are this close together.
    pub window: u64,
}

impl Default for ReduceChordsOptions {
    fn default() -> Self {
        Self {
            keep: ChordKeep::Highest,
            query: QueryOptions::default(),
            window: 0,
        }
    }
}

/// Options for chord-slice rendering.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ChordsOptions {
    /// Selection filter.
    pub query: QueryOptions,
    /// Group notes whose start ticks are this close together.
    pub window: u64,
}

/// Options for generating simple bass notes from chord roots.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BasslineOptions {
    /// Selection filter for source chord notes.
    pub query: QueryOptions,
    /// Group notes whose start ticks are this close together.
    pub window: u64,
    /// Destination track. Missing tracks are created; `None` adds a new track.
    pub output_track: Option<usize>,
    /// Destination MIDI channel for generated bass notes.
    pub output_channel: u8,
    /// Octave for generated root notes, using the same convention as C4=60.
    pub octave: i16,
    /// Generated note-on velocity.
    pub velocity: u8,
    /// Optional fixed duration in ticks or a metrical length like `1/4` or `1/8t`.
    pub duration: Option<String>,
}

impl Default for BasslineOptions {
    fn default() -> Self {
        Self {
            query: QueryOptions::default(),
            window: 0,
            output_track: None,
            output_channel: 0,
            octave: 2,
            velocity: 80,
            duration: None,
        }
    }
}

/// Default limit on piano-roll width.
pub const DEFAULT_ROLL_COLUMNS: u64 = 256;

/// Options for ASCII piano-roll rendering.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RollOptions {
    /// Selection filter.
    pub query: QueryOptions,
    /// Grid as ticks (`120`) or a musical length (`1/16`, `1/8t`, `beat`).
    pub grid: Option<String>,
    /// Output verbosity.
    pub mode: RollMode,
    /// Widest roll to print; `0` removes the limit.
    pub max_columns: u64,
}

impl Default for RollOptions {
    fn default() -> Self {
        Self {
            query: QueryOptions::default(),
            grid: None,
            mode: RollMode::Compact,
            max_columns: DEFAULT_ROLL_COLUMNS,
        }
    }
}

/// Piano-roll verbosity.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RollMode {
    /// One text row per pitch.
    Compact,
    /// Rows plus one `CELL` line per grid column.
    Verbose,
}

/// Track/channel selection for structural MIDI operations.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TrackChannelSelector {
    /// Restrict to one source track.
    pub track: Option<usize>,
    /// Restrict to one MIDI channel.
    pub channel: Option<u8>,
}

impl TrackChannelSelector {
    pub(super) fn matches_midi(&self, track: usize, channel: u8) -> bool {
        self.track.is_none_or(|wanted| wanted == track)
            && self.channel.is_none_or(|wanted| wanted == channel)
    }
}

/// How to split a MIDI file into multiple outputs.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SplitMode {
    /// Create one output per source track.
    Track,
    /// Create one output per MIDI channel found in note events.
    Channel,
}

/// Options for `midy fix`.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct FixOptions {
    /// How to repair stuck note-on events.
    pub stuck_note_mode: StuckNoteFixMode,
    /// Duration to use when closing stuck notes.
    pub stuck_note_duration: u64,
    /// Remove empty tracks after note repairs.
    pub remove_empty_tracks: bool,
}

impl Default for FixOptions {
    fn default() -> Self {
        Self {
            stuck_note_mode: StuckNoteFixMode::Remove,
            stuck_note_duration: 120,
            remove_empty_tracks: false,
        }
    }
}

/// Repair applied to stuck note-on events.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum StuckNoteFixMode {
    /// Delete the note-on.
    Remove,
    /// Add the missing note-off.
    Close,
}

/// Options for applying edits.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct ApplyOptions {
    /// Delete notes that a full timeline no longer lists.
    pub prune: bool,
    /// Fail on anything that would otherwise only warn.
    pub strict: bool,
}

/// The format of edit input.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum EditFormat {
    /// Detect the format from the text.
    #[default]
    Auto,
    /// Edit commands or a timeline.
    Text,
    /// Note rows from `midy read --format json`.
    Json,
    /// Note rows from `midy read --format csv`.
    Csv,
}

/// What one edit line did.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CommandReport {
    /// One-based position in the edit input.
    pub line: usize,
    /// Canonical command name.
    pub command: String,
    /// Notes or events the command selected.
    pub matched: usize,
}

/// What a set of edits changed.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct EditReport {
    /// Per-line results, in input order.
    pub commands: Vec<CommandReport>,
    /// Notes in the input.
    pub notes_before: usize,
    /// Notes in the output.
    pub notes_after: usize,
    /// Notes created.
    pub notes_added: usize,
    /// Notes removed.
    pub notes_deleted: usize,
    /// Surviving notes with any changed field.
    pub notes_changed: usize,
    /// Non-note events created.
    pub events_added: usize,
    /// Non-note events removed.
    pub events_deleted: usize,
    /// Conditions that did not stop the edit.
    pub warnings: Vec<String>,
}

/// Rewritten MIDI plus a description of the edit.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ApplyOutput {
    /// The rewritten file.
    pub bytes: Vec<u8>,
    /// What changed.
    pub report: EditReport,
}
