use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;

use crate::{Error, Result};
use midly::{
    Format, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind,
    num::{u4, u7, u28},
};

/// Machine-readable edit command help.
pub const EDIT_FORMAT_HELP: &str = "\
MIDY_EDIT_FORMAT v1

Read:
  midy read input.mid

Write/apply:
  midy apply input.mid edits.txt -o output.mid
  midy apply input.mid edited.json -o output.mid
  midy apply input.mid edited.csv -o output.mid
  cat edits.txt | midy apply input.mid output.mid
  cat edits.txt | midy apply input.mid

Accepted edit lines:
  ADD_NOTE track=0 ch=0 key=60 start=0 dur=480 vel=96 [off_vel=64]
  ADD_NOTE track=0 ch=0 key=C4 at=2:1 dur=1/4 vel=96
  NOTE id=t0n0 track=0 ch=0 key=62 start=0 dur=240 vel=96 off_vel=64
  SET_NOTE id=t0n0 key=62 start=0 dur=240 vel=96 off_vel=64
  DELETE_NOTE id=t0n0
  DELETE_NOTES [track=0] [ch=0] [key=60] [start=0] [end=960]
  DELETE_NOTES [track=0] [ch=0] [key=C4] [bar=1|bars=1..4]
  TRANSPOSE semitones=2 [track=0] [ch=0] [key=60] [start=0] [end=960]
  SHIFT ticks=120 [track=0] [ch=0] [key=60] [start=0] [end=960]
  SHIFT by=1/8 [track=0] [ch=0] [bars=1..4]
  SCALE_TIME factor=2/1 [track=0] [ch=0] [key=60] [start=0] [end=960]
  SCALE_DURATION factor=1/2 [track=0] [ch=0] [key=60] [start=0] [end=960]
  QUANTIZE grid=120 mode=both [track=0] [ch=0] [key=60] [start=0] [end=960]
  QUANTIZE grid=1/16 mode=both [track=0] [ch=0] [bars=1..4]
  HUMANIZE timing=12 velocity=8 seed=1 [track=0] [ch=0] [start=0] [end=960]
  DEHUMANIZE grid=1/16 [mode=start|duration|both] [track=0] [ch=0]
  SWING amount=55 grid=1/8 [track=0] [ch=0] [bars=1..4]
  VELOCITY scale=0.8 [track=0] [ch=0] [key=60] [start=0] [end=960]
  VELOCITY add=10 [track=0] [ch=0]
  VELOCITY set=96 [track=0] [ch=0]
  VELOCITY compress=0.5 center=80 [track=0] [ch=0]
  CRESCENDO start_vel=40 end_vel=110 start=0 end=1920 [track=0] [ch=0]
  CHORDIZE quality=maj [track=0] [ch=0]
  CHORDIZE intervals=0,3,7 [track=0] [ch=0]
  ARPEGGIATE grid=1/16 order=up [track=0] [ch=0]
  BLOCK_CHORD grid=1/8 [track=0] [ch=0]
  INVERT_CHORDS inversion=1 [track=0] [ch=0]
  DOUBLE octave=-1 [track=0] [ch=0] [key=60]
  VOICE_LEAD max_jump=7 [track=0] [ch=0]
  MUTE [track=0] [ch=0]
  SOLO [track=0] [ch=0]
  MOVE_TRACK from=2 to=1 [ch=0]
  SET_CHANNEL track=1 ch=0 [from_ch=1]

Notes:
  - time fields are integer ticks
  - metrical files also accept at/pos=BAR:BEAT[:TICK] and bar/bars filters
    using the MIDI time-signature map, plus dur/grid/by fractions such as
    1/4 and 1/16 and durations beat/bar
  - ch is MIDI channel 0..15
  - key accepts MIDI values 0..127 or note names like C4/F#3/Bb2
  - velocities are MIDI values 0..127
  - filter start/end select notes by note start tick, with end exclusive
  - factor accepts N, N/D, or decimal forms like 0.5
  - read-only timeline lines HEADER, SONG, TRACK, META, EVENT are ignored by apply
";

/// Small parsed-MIDI summary used by the initial project skeleton.
#[derive(Debug, Eq, PartialEq)]
pub struct MidiSummary {
    /// Number of tracks in the standard MIDI file.
    pub tracks: usize,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct TimeSignatureValue {
    numerator: u8,
    denominator: u32,
}

impl Default for TimeSignatureValue {
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator: 4,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct TimeSignatureSegment {
    start_tick: u64,
    start_bar: u64,
    signature: TimeSignatureValue,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct TimeSignatureMap {
    ticks_per_quarter: u16,
    segments: Vec<TimeSignatureSegment>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct Note {
    id: String,
    track: usize,
    channel: u8,
    key: u8,
    start: u64,
    duration: u64,
    velocity: u8,
    off_velocity: u8,
    on_event_index: usize,
    off_event_index: usize,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct PendingNote {
    id: String,
    channel: u8,
    key: u8,
    start: u64,
    velocity: u8,
    on_event_index: usize,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct StuckNote {
    id: String,
    track: usize,
    channel: u8,
    key: u8,
    start: u64,
    on_event_index: usize,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct ConcreteNote {
    track: usize,
    channel: u8,
    key: u8,
    start: u64,
    duration: u64,
    velocity: u8,
    off_velocity: u8,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct StructuredNote {
    id: Option<String>,
    note: ConcreteNote,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct EditableNote {
    id: Option<String>,
    note: ConcreteNote,
    on_order: usize,
    off_order: usize,
    deleted: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct ScheduledNote {
    note: ConcreteNote,
    on_order: usize,
    off_order: usize,
}

#[derive(Debug, Default)]
struct TrackPatch {
    remove_indices: HashSet<usize>,
    add_notes: Vec<ScheduledNote>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum EditCommand {
    Add(ConcreteNote),
    Delete {
        id: String,
    },
    Set {
        id: String,
        patch: NotePatchFields,
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
    },
    ScaleTime {
        factor: Ratio,
        filter: NoteFilter,
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
    Dehumanize {
        grid: u64,
        mode: QuantizeMode,
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
    Mute {
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
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
struct NotePatchFields {
    track: Option<usize>,
    channel: Option<u8>,
    key: Option<u8>,
    start: Option<u64>,
    duration: Option<u64>,
    velocity: Option<u8>,
    off_velocity: Option<u8>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
struct NoteFilter {
    track: Option<usize>,
    channel: Option<u8>,
    key: Option<u8>,
    start: Option<u64>,
    end: Option<u64>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct Ratio {
    numerator: u64,
    denominator: u64,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum QuantizeMode {
    Start,
    Duration,
    Both,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum VelocityCommand {
    Scale(Ratio),
    Add(i16),
    Set(u8),
    Compress { factor: Ratio, center: u8 },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ArpeggioOrder {
    Up,
    Down,
    UpDown,
}

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

/// Common note-selection options used by read-only analysis commands.
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
    /// Destination track for generated bass notes.
    pub output_track: usize,
    /// Destination MIDI channel for generated bass notes.
    pub output_channel: u8,
    /// Octave for generated root notes, using the same convention as C4=60.
    pub octave: i16,
    /// Generated note-on velocity.
    pub velocity: u8,
    /// Optional fixed duration in ticks or a metrical fraction like 1/4.
    pub duration: Option<String>,
}

impl Default for BasslineOptions {
    fn default() -> Self {
        Self {
            query: QueryOptions::default(),
            window: 0,
            output_track: 0,
            output_channel: 0,
            octave: 2,
            velocity: 80,
            duration: None,
        }
    }
}

/// Options for ASCII piano-roll rendering.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RollOptions {
    /// Selection filter.
    pub query: QueryOptions,
    /// Grid as ticks (`120`) or a musical fraction (`1/16`).
    pub grid: Option<String>,
    /// Output verbosity.
    pub mode: RollMode,
}

impl Default for RollOptions {
    fn default() -> Self {
        Self {
            query: QueryOptions::default(),
            grid: None,
            mode: RollMode::Compact,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RollMode {
    Compact,
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

/// How to split a MIDI file into multiple outputs.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SplitMode {
    /// Create one output per source track.
    Track,
    /// Create one output per MIDI channel found in note events.
    Channel,
}

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

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum StuckNoteFixMode {
    Remove,
    Close,
}

/// Parses a standard MIDI file from memory using `midly`.
pub fn parse_smf(bytes: &[u8]) -> Result<midly::Smf<'_>> {
    Ok(midly::Smf::parse(bytes)?)
}

/// Parses a standard MIDI file and returns a tiny summary.
///
/// This intentionally stays small until the real CLI workflow is defined.
pub fn summarize_smf(bytes: &[u8]) -> Result<MidiSummary> {
    let smf = parse_smf(bytes)?;

    Ok(MidiSummary {
        tracks: smf.tracks.len(),
    })
}

/// Renders a deterministic ASCII timeline for a standard MIDI file.
///
/// The format is intentionally line-oriented so another program or model can
/// read it, edit note commands, and send those commands back to `midy`.
pub fn render_timeline(bytes: &[u8]) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let notes = collect_notes(&smf);
    let mut out = String::new();
    let length_ticks = song_length_ticks(&smf, &notes);
    let ticks_per_beat = ticks_per_beat(smf.header.timing);
    let time_map = TimeSignatureMap::from_smf(&smf);

    writeln!(out, "MIDY_TIMELINE v1").expect("writing to String cannot fail");
    writeln!(
        out,
        "HEADER format={} {} tracks={}",
        format_name(smf.header.format),
        timing_fields(smf.header.timing),
        smf.tracks.len(),
    )
    .expect("writing to String cannot fail");
    if let Some(ticks_per_beat) = ticks_per_beat {
        writeln!(
            out,
            "SONG length_ticks={} length_beats={:.3}",
            length_ticks,
            length_ticks as f64 / f64::from(ticks_per_beat),
        )
        .expect("writing to String cannot fail");
    } else {
        writeln!(out, "SONG length_ticks={length_ticks}").expect("writing to String cannot fail");
    }
    writeln!(
        out,
        "# Edit commands accepted by `midy apply INPUT.mid EDITS.txt -o OUTPUT.mid`:",
    )
    .expect("writing to String cannot fail");
    writeln!(out, "# ADD_NOTE track=0 ch=0 key=60 start=0 dur=480 vel=96",)
        .expect("writing to String cannot fail");
    writeln!(out, "# ADD_NOTE track=0 ch=0 key=C4 at=2:1 dur=1/4 vel=96")
        .expect("writing to String cannot fail");
    writeln!(out, "# SET_NOTE id=t0n0 key=62 dur=240").expect("writing to String cannot fail");
    writeln!(out, "# DELETE_NOTE id=t0n0").expect("writing to String cannot fail");
    writeln!(out, "# TRANSPOSE semitones=2 track=0 ch=0").expect("writing to String cannot fail");
    writeln!(out, "# SHIFT ticks=120 start=480 end=960").expect("writing to String cannot fail");
    writeln!(out, "# SHIFT by=1/8 bars=1..4").expect("writing to String cannot fail");
    writeln!(out, "# SCALE_TIME factor=2/1").expect("writing to String cannot fail");
    writeln!(out, "# SCALE_DURATION factor=1/2 key=60").expect("writing to String cannot fail");
    writeln!(out, "# QUANTIZE grid=1/16 mode=both").expect("writing to String cannot fail");
    writeln!(out, "# DELETE_NOTES track=0 start=0 end=480").expect("writing to String cannot fail");

    render_track_summaries(&smf, &notes, &mut out);
    render_timeline_events(&smf, &mut out);

    let mut sorted_notes = notes;
    sorted_notes.sort_by_key(|note| {
        (
            note.track,
            note.start,
            note.channel,
            note.key,
            note.id.clone(),
        )
    });
    for note in sorted_notes {
        writeln!(
            out,
            "NOTE id={} track={} ch={} key={} name={} start={} dur={} end={}{} vel={} off_vel={}",
            note.id,
            note.track,
            note.channel,
            note.key,
            note_name(note.key),
            note.start,
            note.duration,
            note.start.saturating_add(note.duration),
            note_position_fields(
                note.start,
                note.start.saturating_add(note.duration),
                time_map.as_ref()
            ),
            note.velocity,
            note.off_velocity,
        )
        .expect("writing to String cannot fail");
    }

    Ok(out)
}

/// Renders the timeline as structured JSON for scripts.
pub fn render_timeline_json(bytes: &[u8]) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let notes = collect_notes(&smf);
    let length_ticks = song_length_ticks(&smf, &notes);
    let ticks_per_beat = ticks_per_beat(smf.header.timing);
    let mut out = String::new();

    writeln!(out, "{{").expect("writing to String cannot fail");
    writeln!(out, "  \"kind\": \"MIDY_TIMELINE\",").expect("writing to String cannot fail");
    writeln!(out, "  \"version\": 1,").expect("writing to String cannot fail");
    writeln!(
        out,
        "  \"format\": {},",
        json_string(format_name(smf.header.format))
    )
    .expect("writing to String cannot fail");
    writeln!(
        out,
        "  \"timing\": {},",
        json_string(&timing_fields(smf.header.timing))
    )
    .expect("writing to String cannot fail");
    writeln!(out, "  \"tracks\": {},", smf.tracks.len()).expect("writing to String cannot fail");
    writeln!(out, "  \"length_ticks\": {},", length_ticks).expect("writing to String cannot fail");
    if let Some(ticks_per_beat) = ticks_per_beat {
        writeln!(
            out,
            "  \"length_beats\": {:.6},",
            length_ticks as f64 / f64::from(ticks_per_beat),
        )
        .expect("writing to String cannot fail");
    } else {
        writeln!(out, "  \"length_beats\": null,").expect("writing to String cannot fail");
    }
    writeln!(out, "  \"notes\": [").expect("writing to String cannot fail");
    let mut sorted_notes = notes;
    sorted_notes.sort_by_key(|note| {
        (
            note.track,
            note.start,
            note.channel,
            note.key,
            note.id.clone(),
        )
    });
    for (index, note) in sorted_notes.iter().enumerate() {
        writeln!(
            out,
            "    {{\"id\": {}, \"track\": {}, \"ch\": {}, \"key\": {}, \"name\": {}, \"start\": {}, \"dur\": {}, \"end\": {}, \"vel\": {}, \"off_vel\": {}}}{}",
            json_string(&note.id),
            note.track,
            note.channel,
            note.key,
            json_string(&note_name(note.key)),
            note.start,
            note.duration,
            note.start.saturating_add(note.duration),
            note.velocity,
            note.off_velocity,
            if index + 1 == sorted_notes.len() { "" } else { "," },
        )
        .expect("writing to String cannot fail");
    }
    writeln!(out, "  ]").expect("writing to String cannot fail");
    writeln!(out, "}}").expect("writing to String cannot fail");

    Ok(out)
}

/// Renders timeline notes as CSV for spreadsheets and simple scripts.
pub fn render_timeline_csv(bytes: &[u8]) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let mut notes = collect_notes(&smf);
    notes.sort_by_key(|note| {
        (
            note.track,
            note.start,
            note.channel,
            note.key,
            note.id.clone(),
        )
    });
    let mut out = String::new();

    writeln!(out, "id,track,ch,key,name,start,dur,end,vel,off_vel")
        .expect("writing to String cannot fail");
    for note in notes {
        writeln!(
            out,
            "{},{},{},{},{},{},{},{},{},{}",
            csv_field(&note.id),
            note.track,
            note.channel,
            note.key,
            csv_field(&note_name(note.key)),
            note.start,
            note.duration,
            note.start.saturating_add(note.duration),
            note.velocity,
            note.off_velocity,
        )
        .expect("writing to String cannot fail");
    }

    Ok(out)
}

/// Applies note rows from `midy read --format json`.
pub fn apply_timeline_json_edits(bytes: &[u8], json: &str) -> Result<Vec<u8>> {
    apply_structured_note_rows(bytes, parse_timeline_json_notes(json)?)
}

/// Applies note rows from `midy read --format csv`.
pub fn apply_timeline_csv_edits(bytes: &[u8], csv: &str) -> Result<Vec<u8>> {
    apply_structured_note_rows(bytes, parse_timeline_csv_notes(csv)?)
}

/// Renders a note-level diff as edit commands that can be piped into apply.
pub fn render_diff(before: &[u8], after: &[u8]) -> Result<String> {
    let before_smf = parse_smf(before)?;
    let after_smf = parse_smf(after)?;
    let mut before_notes = collect_notes(&before_smf);
    let mut after_notes = collect_notes(&after_smf);
    before_notes.sort_by_key(note_sort_key);
    after_notes.sort_by_key(note_sort_key);
    let mut matched_after = vec![false; after_notes.len()];
    let mut unmatched_before = Vec::new();

    for before_note in &before_notes {
        if let Some(after_index) = after_notes
            .iter()
            .enumerate()
            .position(|(index, after_note)| {
                !matched_after[index] && note_signature(before_note) == note_signature(after_note)
            })
        {
            matched_after[after_index] = true;
        } else {
            unmatched_before.push(before_note);
        }
    }

    let unmatched_after = after_notes
        .iter()
        .enumerate()
        .filter_map(|(index, note)| (!matched_after[index]).then_some(note))
        .collect::<Vec<_>>();

    let mut out = String::new();
    if !unmatched_before.is_empty() || !unmatched_after.is_empty() {
        writeln!(out, "# MIDY_DIFF v1").expect("writing to String cannot fail");
    }
    for note in unmatched_before {
        writeln!(out, "DELETE_NOTE id={}", note.id).expect("writing to String cannot fail");
    }
    for note in unmatched_after {
        writeln!(
            out,
            "ADD_NOTE track={} ch={} key={} start={} dur={} vel={} off_vel={}",
            note.track,
            note.channel,
            note.key,
            note.start,
            note.duration,
            note.velocity,
            note.off_velocity,
        )
        .expect("writing to String cannot fail");
    }
    render_non_note_event_diff(&before_smf, &after_smf, &mut out);

    Ok(out)
}

fn render_non_note_event_diff(before: &Smf<'_>, after: &Smf<'_>, out: &mut String) {
    let before_lines = rendered_non_note_event_lines(before);
    let after_lines = rendered_non_note_event_lines(after);
    if before_lines == after_lines {
        return;
    }

    let mut removed = before_lines.clone();
    let mut added = Vec::new();
    for line in after_lines {
        if let Some(index) = removed.iter().position(|candidate| candidate == &line) {
            removed.remove(index);
        } else {
            added.push(line);
        }
    }
    removed.sort();
    added.sort();

    if out.is_empty() {
        writeln!(out, "# MIDY_DIFF v1").expect("writing to String cannot fail");
    }
    writeln!(
        out,
        "# NON_NOTE_DIFF removed={} added={}",
        removed.len(),
        added.len()
    )
    .expect("writing to String cannot fail");
    for line in removed {
        writeln!(out, "# DELETE_EVENT {line}").expect("writing to String cannot fail");
    }
    for line in added {
        writeln!(out, "# ADD_EVENT {line}").expect("writing to String cannot fail");
    }
}

fn rendered_non_note_event_lines(smf: &Smf<'_>) -> Vec<String> {
    let mut rendered = String::new();
    render_timeline_events(smf, &mut rendered);
    rendered
        .lines()
        .filter(|line| !line.contains("kind=end_of_track"))
        .map(ToOwned::to_owned)
        .collect()
}

fn apply_structured_note_rows(bytes: &[u8], rows: Vec<StructuredNote>) -> Result<Vec<u8>> {
    let original = collect_notes(&parse_smf(bytes)?);
    let original_ids = original
        .iter()
        .map(|note| note.id.as_str())
        .collect::<HashSet<_>>();
    let present_ids = rows
        .iter()
        .filter_map(|row| row.id.as_deref())
        .collect::<HashSet<_>>();
    let mut edits = String::new();

    for note in &original {
        if !present_ids.contains(note.id.as_str()) {
            writeln!(edits, "DELETE_NOTE id={}", note.id).expect("writing to String cannot fail");
        }
    }

    for row in rows {
        if let Some(id) = row.id.filter(|id| original_ids.contains(id.as_str())) {
            writeln!(
                edits,
                "SET_NOTE id={} track={} ch={} key={} start={} dur={} vel={} off_vel={}",
                id,
                row.note.track,
                row.note.channel,
                row.note.key,
                row.note.start,
                row.note.duration,
                row.note.velocity,
                row.note.off_velocity,
            )
            .expect("writing to String cannot fail");
        } else {
            writeln!(
                edits,
                "ADD_NOTE track={} ch={} key={} start={} dur={} vel={} off_vel={}",
                row.note.track,
                row.note.channel,
                row.note.key,
                row.note.start,
                row.note.duration,
                row.note.velocity,
                row.note.off_velocity,
            )
            .expect("writing to String cannot fail");
        }
    }

    apply_edits(bytes, &edits)
}

fn parse_timeline_json_notes(json: &str) -> Result<Vec<StructuredNote>> {
    let mut notes = Vec::new();
    for (index, line) in json.lines().enumerate() {
        let line_no = index + 1;
        let trimmed = line.trim();
        if !trimmed.starts_with('{') || !trimmed.contains("\"id\"") || !trimmed.contains("\"dur\"")
        {
            continue;
        }
        let id = json_string_field(trimmed, "id").map_err(|message| {
            Error::Usage(format!(
                "invalid timeline JSON note on line {line_no}: {message}"
            ))
        })?;
        let note = ConcreteNote {
            track: json_usize_field(trimmed, "track").map_err(|message| {
                Error::Usage(format!(
                    "invalid timeline JSON note on line {line_no}: {message}"
                ))
            })?,
            channel: json_u8_field(trimmed, "ch", 15).map_err(|message| {
                Error::Usage(format!(
                    "invalid timeline JSON note on line {line_no}: {message}"
                ))
            })?,
            key: json_u8_field(trimmed, "key", 127).map_err(|message| {
                Error::Usage(format!(
                    "invalid timeline JSON note on line {line_no}: {message}"
                ))
            })?,
            start: json_u64_field(trimmed, "start").map_err(|message| {
                Error::Usage(format!(
                    "invalid timeline JSON note on line {line_no}: {message}"
                ))
            })?,
            duration: json_u64_field(trimmed, "dur").map_err(|message| {
                Error::Usage(format!(
                    "invalid timeline JSON note on line {line_no}: {message}"
                ))
            })?,
            velocity: json_u8_field(trimmed, "vel", 127).map_err(|message| {
                Error::Usage(format!(
                    "invalid timeline JSON note on line {line_no}: {message}"
                ))
            })?,
            off_velocity: json_u8_field(trimmed, "off_vel", 127).map_err(|message| {
                Error::Usage(format!(
                    "invalid timeline JSON note on line {line_no}: {message}"
                ))
            })?,
        };
        notes.push(StructuredNote {
            id: nonempty_id(id),
            note,
        });
    }
    if notes.is_empty() {
        return Err(Error::Usage(
            "timeline JSON did not contain any note rows".to_owned(),
        ));
    }
    Ok(notes)
}

fn parse_timeline_csv_notes(csv: &str) -> Result<Vec<StructuredNote>> {
    let mut lines = csv.lines();
    let header = lines
        .next()
        .ok_or_else(|| Error::Usage("timeline CSV is empty".to_owned()))?;
    let headers = parse_csv_row(header);
    let required = ["id", "track", "ch", "key", "start", "dur", "vel", "off_vel"];
    for name in required {
        if !headers.iter().any(|header| header == name) {
            return Err(Error::Usage(format!(
                "timeline CSV is missing required column '{name}'"
            )));
        }
    }

    let mut notes = Vec::new();
    for (index, line) in lines.enumerate() {
        let line_no = index + 2;
        if line.trim().is_empty() {
            continue;
        }
        let row = parse_csv_row(line);
        let value = |name: &str| -> Result<&str> {
            let column = headers
                .iter()
                .position(|header| header == name)
                .ok_or_else(|| Error::Usage(format!("timeline CSV missing column '{name}'")))?;
            row.get(column).map(String::as_str).ok_or_else(|| {
                Error::Usage(format!("timeline CSV line {line_no} missing '{name}'"))
            })
        };
        let id = value("id")?.to_owned();
        let note = ConcreteNote {
            track: parse_csv_usize(line_no, "track", value("track")?)?,
            channel: parse_csv_u8(line_no, "ch", value("ch")?, 15)?,
            key: parse_csv_u8(line_no, "key", value("key")?, 127)?,
            start: parse_csv_u64(line_no, "start", value("start")?)?,
            duration: parse_csv_u64(line_no, "dur", value("dur")?)?,
            velocity: parse_csv_u8(line_no, "vel", value("vel")?, 127)?,
            off_velocity: parse_csv_u8(line_no, "off_vel", value("off_vel")?, 127)?,
        };
        notes.push(StructuredNote {
            id: nonempty_id(id),
            note,
        });
    }
    if notes.is_empty() {
        return Err(Error::Usage(
            "timeline CSV did not contain any note rows".to_owned(),
        ));
    }
    Ok(notes)
}

/// Applies ASCII edit commands to a standard MIDI file and returns rewritten MIDI bytes.
pub fn apply_edits(bytes: &[u8], edits: &str) -> Result<Vec<u8>> {
    let mut smf = parse_smf(bytes)?;
    let notes = collect_notes(&smf);
    let time_map = TimeSignatureMap::from_smf(&smf);
    let commands = parse_edit_commands(
        edits,
        EditParseContext {
            ticks_per_beat: ticks_per_beat(smf.header.timing),
            time_map: time_map.as_ref(),
        },
    )?;
    if commands.is_empty() {
        return Ok(bytes.to_owned());
    }

    let mut editable_notes = notes
        .iter()
        .map(EditableNote::from_note)
        .collect::<Vec<_>>();
    let mut next_order = smf
        .tracks
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or_default()
        .saturating_add(1);

    for command in commands {
        match command {
            EditCommand::Add(note) => {
                validate_note(&note, smf.tracks.len())?;
                editable_notes.push(EditableNote {
                    id: None,
                    note,
                    on_order: next_order,
                    off_order: next_order.saturating_add(1),
                    deleted: false,
                });
                next_order = next_order.saturating_add(2);
            }
            EditCommand::Delete { id } => {
                find_editable_note_mut(&mut editable_notes, &id)?.deleted = true;
            }
            EditCommand::Set { id, patch } => {
                let editable = find_editable_note_mut(&mut editable_notes, &id)?;
                patch.apply_to(&mut editable.note);
                validate_note(&editable.note, smf.tracks.len())?;
            }
            EditCommand::DeleteMatching { filter } => {
                for editable in editable_notes
                    .iter_mut()
                    .filter(|editable| !editable.deleted && filter.matches(&editable.note))
                {
                    editable.deleted = true;
                }
            }
            EditCommand::Transpose { semitones, filter } => {
                for editable in matching_notes_mut(&mut editable_notes, &filter) {
                    editable.note.key = transpose_key(editable.note.key, semitones)?;
                }
            }
            EditCommand::Shift { ticks, filter } => {
                for editable in matching_notes_mut(&mut editable_notes, &filter) {
                    editable.note.start = shift_tick(editable.note.start, ticks)?;
                    validate_note(&editable.note, smf.tracks.len())?;
                }
            }
            EditCommand::ScaleTime { factor, filter } => {
                for editable in matching_notes_mut(&mut editable_notes, &filter) {
                    editable.note.start = factor.scale(editable.note.start)?;
                    editable.note.duration = factor.scale(editable.note.duration)?;
                    validate_note(&editable.note, smf.tracks.len())?;
                }
            }
            EditCommand::ScaleDuration { factor, filter } => {
                for editable in matching_notes_mut(&mut editable_notes, &filter) {
                    editable.note.duration = factor.scale(editable.note.duration)?;
                    validate_note(&editable.note, smf.tracks.len())?;
                }
            }
            EditCommand::Quantize { grid, mode, filter } => {
                for editable in matching_notes_mut(&mut editable_notes, &filter) {
                    quantize_note(&mut editable.note, grid, mode)?;
                    validate_note(&editable.note, smf.tracks.len())?;
                }
            }
            EditCommand::Humanize {
                timing,
                velocity,
                seed,
                filter,
            } => {
                apply_humanize(&mut editable_notes, &filter, timing, velocity, seed)?;
                for editable in editable_notes.iter().filter(|editable| !editable.deleted) {
                    validate_note(&editable.note, smf.tracks.len())?;
                }
            }
            EditCommand::Dehumanize { grid, mode, filter } => {
                for editable in matching_notes_mut(&mut editable_notes, &filter) {
                    quantize_note(&mut editable.note, grid, mode)?;
                    validate_note(&editable.note, smf.tracks.len())?;
                }
            }
            EditCommand::Swing {
                amount,
                grid,
                filter,
            } => {
                apply_swing(&mut editable_notes, &filter, amount, grid)?;
                for editable in editable_notes.iter().filter(|editable| !editable.deleted) {
                    validate_note(&editable.note, smf.tracks.len())?;
                }
            }
            EditCommand::Velocity { command, filter } => {
                for editable in matching_notes_mut(&mut editable_notes, &filter) {
                    editable.note.velocity = apply_velocity(editable.note.velocity, command)?;
                    validate_note(&editable.note, smf.tracks.len())?;
                }
            }
            EditCommand::Crescendo {
                start_velocity,
                end_velocity,
                filter,
            } => {
                apply_crescendo(&mut editable_notes, &filter, start_velocity, end_velocity)?;
                for editable in editable_notes.iter().filter(|editable| !editable.deleted) {
                    validate_note(&editable.note, smf.tracks.len())?;
                }
            }
            EditCommand::Chordize { intervals, filter } => {
                apply_chordize(
                    &mut editable_notes,
                    &filter,
                    &intervals,
                    &mut next_order,
                    smf.tracks.len(),
                )?;
            }
            EditCommand::Arpeggiate {
                grid,
                order,
                filter,
            } => {
                apply_arpeggiate(&mut editable_notes, &filter, grid, order)?;
                for editable in editable_notes.iter().filter(|editable| !editable.deleted) {
                    validate_note(&editable.note, smf.tracks.len())?;
                }
            }
            EditCommand::BlockChord { grid, filter } => {
                apply_block_chord(&mut editable_notes, &filter, grid)?;
                for editable in editable_notes.iter().filter(|editable| !editable.deleted) {
                    validate_note(&editable.note, smf.tracks.len())?;
                }
            }
            EditCommand::InvertChords { inversion, filter } => {
                apply_invert_chords(&mut editable_notes, &filter, inversion)?;
                for editable in editable_notes.iter().filter(|editable| !editable.deleted) {
                    validate_note(&editable.note, smf.tracks.len())?;
                }
            }
            EditCommand::Double { octave, filter } => {
                apply_double(
                    &mut editable_notes,
                    &filter,
                    octave,
                    &mut next_order,
                    smf.tracks.len(),
                )?;
            }
            EditCommand::VoiceLead { max_jump, filter } => {
                apply_voice_lead(&mut editable_notes, &filter, max_jump)?;
                for editable in editable_notes.iter().filter(|editable| !editable.deleted) {
                    validate_note(&editable.note, smf.tracks.len())?;
                }
            }
            EditCommand::Mute { filter } => {
                for editable in matching_notes_mut(&mut editable_notes, &filter) {
                    editable.deleted = true;
                }
            }
            EditCommand::Solo { filter } => {
                for editable in editable_notes
                    .iter_mut()
                    .filter(|editable| !editable.deleted && !filter.matches(&editable.note))
                {
                    editable.deleted = true;
                }
            }
            EditCommand::MoveTrack { from, to, filter } => {
                if from >= smf.tracks.len() || to >= smf.tracks.len() {
                    return Err(Error::Edit(format!(
                        "MOVE_TRACK requires existing tracks; file has {} tracks",
                        smf.tracks.len()
                    )));
                }
                for editable in matching_notes_mut(&mut editable_notes, &filter) {
                    if editable.note.track == from {
                        editable.note.track = to;
                        validate_note(&editable.note, smf.tracks.len())?;
                    }
                }
            }
            EditCommand::SetChannel { channel, filter } => {
                for editable in matching_notes_mut(&mut editable_notes, &filter) {
                    editable.note.channel = channel;
                    validate_note(&editable.note, smf.tracks.len())?;
                }
            }
        }
    }

    let patches = build_track_patches(smf.tracks.len(), &notes, &editable_notes)?;
    for (track_index, patch) in patches.iter().enumerate() {
        let track = std::mem::take(&mut smf.tracks[track_index]);
        smf.tracks[track_index] = rebuild_track(track, patch)?;
    }

    let mut out = Vec::new();
    smf.write_std(&mut out)?;
    Ok(out)
}

/// Suggests edit commands that reduce chord stacks to one note per slice.
pub fn suggest_reduce_chords(bytes: &[u8], options: &ReduceChordsOptions) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let time_map = TimeSignatureMap::from_smf(&smf);
    let mut query = options.query.clone();
    apply_bars_to_query(&mut query, time_map.as_ref())?;
    let mut notes = collect_notes(&smf)
        .into_iter()
        .filter(|note| query.matches_note(note))
        .collect::<Vec<_>>();
    let groups = chord_groups(&mut notes, options.window);
    let mut out = String::new();

    for group in groups.into_iter().filter(|group| group.len() > 1) {
        let keep_index = choose_chord_note(&group, &options.keep);
        for (index, note) in group.iter().enumerate() {
            if index != keep_index {
                writeln!(out, "DELETE_NOTE id={}", note.id).expect("writing to String cannot fail");
            }
        }
    }

    Ok(out)
}

/// Suggests ADD_NOTE commands for a simple bassline using detected chord roots.
pub fn suggest_bassline(bytes: &[u8], options: &BasslineOptions) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let ticks_per_beat = ticks_per_beat(smf.header.timing);
    let time_map = TimeSignatureMap::from_smf(&smf);
    let fixed_duration = options
        .duration
        .as_deref()
        .map(|duration| resolve_grid(Some(duration), ticks_per_beat))
        .transpose()?;
    let mut query = options.query.clone();
    apply_bars_to_query(&mut query, time_map.as_ref())?;
    let mut notes = collect_notes(&smf)
        .into_iter()
        .filter(|note| query.matches_note(note))
        .collect::<Vec<_>>();
    let groups = chord_groups(&mut notes, options.window);
    let mut out = String::new();

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
        let root_pc = analyze_chord(&keys).root_pc;
        let key = midi_key_for_octave(root_pc, options.octave)?;
        writeln!(
            out,
            "ADD_NOTE track={} ch={} key={} start={} dur={} vel={}",
            options.output_track, options.output_channel, key, start, duration, options.velocity,
        )
        .expect("writing to String cannot fail");
    }

    Ok(out)
}

/// Renders detected chord slices.
pub fn render_chords(bytes: &[u8], options: &ChordsOptions) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let time_map = TimeSignatureMap::from_smf(&smf);
    let mut query = options.query.clone();
    apply_bars_to_query(&mut query, time_map.as_ref())?;
    let mut notes = collect_notes(&smf)
        .into_iter()
        .filter(|note| query.matches_note(note))
        .collect::<Vec<_>>();
    let groups = chord_groups(&mut notes, options.window);
    let mut out = String::new();

    writeln!(
        out,
        "MIDY_CHORDS v1 window={} chords={}",
        options.window,
        groups.iter().filter(|group| group.len() > 1).count()
    )
    .expect("writing to String cannot fail");

    for group in groups.into_iter().filter(|group| group.len() > 1) {
        let tick = group
            .iter()
            .map(|note| note.start)
            .min()
            .unwrap_or_default();
        let mut keys = group.iter().map(|note| note.key).collect::<Vec<_>>();
        keys.sort_unstable();
        keys.dedup();
        let notes_text = keys
            .iter()
            .map(|key| note_name(*key))
            .collect::<Vec<_>>()
            .join(",");
        let keys_text = keys
            .iter()
            .map(|key| key.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let analysis = analyze_chord(&keys);
        let bass = keys.first().copied().map(note_name).unwrap_or_default();
        let position = time_map
            .as_ref()
            .map(|map| format!(" pos={}", map.tick_position(tick)))
            .unwrap_or_default();

        writeln!(
            out,
            "CHORD tick={}{} notes={} keys={} name={} inversion={} bass={} confidence={:.2}",
            tick,
            position,
            notes_text,
            keys_text,
            analysis.name,
            analysis.inversion,
            bass,
            analysis.confidence,
        )
        .expect("writing to String cannot fail");
    }

    Ok(out)
}

/// Renders a compact ASCII piano roll.
pub fn render_roll(bytes: &[u8], options: &RollOptions) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let ticks_per_beat = ticks_per_beat(smf.header.timing);
    let time_map = TimeSignatureMap::from_smf(&smf);
    let mut query = options.query.clone();
    apply_bars_to_query(&mut query, time_map.as_ref())?;
    let notes = collect_notes(&smf)
        .into_iter()
        .filter(|note| query.matches_note(note))
        .collect::<Vec<_>>();
    let grid = resolve_grid(options.grid.as_deref(), ticks_per_beat)?;
    let start = query.start.unwrap_or_else(|| {
        notes
            .iter()
            .map(|note| note.start)
            .min()
            .unwrap_or_default()
    });
    let end = query.end.unwrap_or_else(|| {
        notes
            .iter()
            .map(|note| note.start.saturating_add(note.duration))
            .max()
            .unwrap_or(start.saturating_add(grid))
    });
    let end = end.max(start.saturating_add(grid));
    let columns = (end - start).div_ceil(grid).min(256);
    let low = notes.iter().map(|note| note.key).min().unwrap_or(60);
    let high = notes.iter().map(|note| note.key).max().unwrap_or(60);
    let mut out = String::new();

    writeln!(
        out,
        "MIDY_ROLL v1 start={} end={} grid_ticks={} columns={} low={} high={} mode={}",
        start,
        end,
        grid,
        columns,
        low,
        high,
        match options.mode {
            RollMode::Compact => "compact",
            RollMode::Verbose => "verbose",
        },
    )
    .expect("writing to String cannot fail");

    for key in (low..=high).rev() {
        let mut row = String::with_capacity(columns as usize);
        for column in 0..columns {
            let cell_start = start + column * grid;
            let cell_end = cell_start.saturating_add(grid);
            let active = notes.iter().any(|note| {
                note.key == key
                    && note.start < cell_end
                    && note.start.saturating_add(note.duration) > cell_start
            });
            row.push(if active { '#' } else { '.' });
        }
        writeln!(out, "{:>4} | {} |", note_name(key), row).expect("writing to String cannot fail");
    }

    if options.mode == RollMode::Verbose {
        for column in 0..columns {
            let cell_start = start + column * grid;
            let cell_end = cell_start.saturating_add(grid);
            let mut active = notes
                .iter()
                .filter(|note| {
                    note.start < cell_end && note.start.saturating_add(note.duration) > cell_start
                })
                .map(|note| format!("{}:{}:{}", note_name(note.key), note.track, note.channel))
                .collect::<Vec<_>>();
            active.sort();
            active.dedup();
            let position = time_map
                .as_ref()
                .map(|map| format!(" pos={}", map.tick_position(cell_start)))
                .unwrap_or_default();
            writeln!(
                out,
                "CELL column={} tick={}{} notes={}",
                column,
                cell_start,
                position,
                if active.is_empty() {
                    "-".to_owned()
                } else {
                    active.join(",")
                },
            )
            .expect("writing to String cannot fail");
        }
    }

    Ok(out)
}

/// Renders a high-level MIDI analysis.
pub fn render_analysis(bytes: &[u8], file_name: Option<&str>) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let notes = collect_notes(&smf);
    let time_map = TimeSignatureMap::from_smf(&smf);
    let mut out = String::new();

    writeln!(
        out,
        "ANALYZE{}",
        file_name
            .map(|file| format!(" file={file}"))
            .unwrap_or_default()
    )
    .expect("writing to String cannot fail");
    writeln!(
        out,
        "header format={} {} tracks={}",
        format_name(smf.header.format),
        timing_fields(smf.header.timing),
        smf.tracks.len(),
    )
    .expect("writing to String cannot fail");
    render_tempo_summary(&smf, &mut out);
    render_time_signature_summary(&smf, &mut out);
    writeln!(
        out,
        "song length_ticks={} notes={} key_guess={}",
        song_length_ticks(&smf, &notes),
        notes.len(),
        guess_key_name(&notes),
    )
    .expect("writing to String cannot fail");

    for (track_index, track) in smf.tracks.iter().enumerate() {
        let track_notes = notes
            .iter()
            .filter(|note| note.track == track_index)
            .collect::<Vec<_>>();
        let stats = TrackStats::from_notes(&track_notes);
        writeln!(
            out,
            "track={} role={} events={} notes={} range={} avg_vel={:.1} avg_dur={:.1} polyphony_max={} density={:.3}",
            track_index,
            classify_track(&track_notes),
            track.len(),
            track_notes.len(),
            stats.range,
            stats.avg_velocity,
            stats.avg_duration,
            stats.polyphony_max,
            stats.density(song_length_ticks(&smf, &notes)),
        )
        .expect("writing to String cannot fail");
    }

    let chord_options = ChordsOptions {
        window: 0,
        ..ChordsOptions::default()
    };
    let chords = render_chords(bytes, &chord_options)?;
    if chords.lines().any(|line| line.starts_with("CHORD ")) {
        writeln!(out, "chords:").expect("writing to String cannot fail");
        for line in chords.lines().filter(|line| line.starts_with("CHORD ")) {
            if let Some(map) = time_map.as_ref() {
                writeln!(
                    out,
                    "  {line} bar_pos={}",
                    tick_position_field_from_line(line, map)
                )
                .expect("writing to String cannot fail");
            } else {
                writeln!(out, "  {line}").expect("writing to String cannot fail");
            }
        }
    }

    Ok(out)
}

/// Renders a compact JSON analysis for scripts.
pub fn render_analysis_json(bytes: &[u8], file_name: Option<&str>) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let notes = collect_notes(&smf);
    let song_length = song_length_ticks(&smf, &notes);
    let mut out = String::new();

    writeln!(out, "{{").expect("writing to String cannot fail");
    writeln!(
        out,
        "  \"file\": {},",
        file_name
            .map(json_string)
            .unwrap_or_else(|| "null".to_owned())
    )
    .expect("writing to String cannot fail");
    writeln!(
        out,
        "  \"format\": {},",
        json_string(format_name(smf.header.format))
    )
    .expect("writing to String cannot fail");
    writeln!(
        out,
        "  \"timing\": {},",
        json_string(&timing_fields(smf.header.timing))
    )
    .expect("writing to String cannot fail");
    writeln!(out, "  \"tracks\": {},", smf.tracks.len()).expect("writing to String cannot fail");
    writeln!(out, "  \"length_ticks\": {},", song_length).expect("writing to String cannot fail");
    writeln!(out, "  \"notes\": {},", notes.len()).expect("writing to String cannot fail");
    writeln!(
        out,
        "  \"key_guess\": {},",
        json_string(&guess_key_name(&notes))
    )
    .expect("writing to String cannot fail");
    writeln!(out, "  \"track_summaries\": [").expect("writing to String cannot fail");
    for (track_index, track) in smf.tracks.iter().enumerate() {
        let track_notes = notes
            .iter()
            .filter(|note| note.track == track_index)
            .collect::<Vec<_>>();
        let stats = TrackStats::from_notes(&track_notes);
        writeln!(
            out,
            "    {{\"track\": {}, \"role\": {}, \"events\": {}, \"notes\": {}, \"range\": {}, \"avg_velocity\": {:.3}, \"avg_duration\": {:.3}, \"polyphony_max\": {}, \"density\": {:.6}}}{}",
            track_index,
            json_string(classify_track(&track_notes)),
            track.len(),
            track_notes.len(),
            json_string(&stats.range),
            stats.avg_velocity,
            stats.avg_duration,
            stats.polyphony_max,
            stats.density(song_length),
            if track_index + 1 == smf.tracks.len() { "" } else { "," },
        )
        .expect("writing to String cannot fail");
    }
    writeln!(out, "  ]").expect("writing to String cannot fail");
    writeln!(out, "}}").expect("writing to String cannot fail");

    Ok(out)
}

/// Renders a track/channel summary.
pub fn render_tracks(bytes: &[u8]) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let notes = collect_notes(&smf);
    let song_length = song_length_ticks(&smf, &notes);
    let mut out = String::new();

    writeln!(out, "MIDY_TRACKS v1 tracks={}", smf.tracks.len())
        .expect("writing to String cannot fail");
    for (track_index, track) in smf.tracks.iter().enumerate() {
        let track_notes = notes
            .iter()
            .filter(|note| note.track == track_index)
            .collect::<Vec<_>>();
        let stats = TrackStats::from_notes(&track_notes);
        let channels = channels_text(&track_notes);
        let names = track_names(track);
        writeln!(
            out,
            "TRACK index={} role={} events={} notes={} channels={} range={} avg_vel={:.1} avg_dur={:.1} polyphony_max={} density={:.3} names={}",
            track_index,
            classify_track(&track_notes),
            track.len(),
            track_notes.len(),
            channels,
            stats.range,
            stats.avg_velocity,
            stats.avg_duration,
            stats.polyphony_max,
            stats.density(song_length),
            names,
        )
        .expect("writing to String cannot fail");
    }

    Ok(out)
}

/// Renders MIDI lint warnings.
pub fn render_lint(bytes: &[u8]) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let notes = collect_notes(&smf);
    let stuck_notes = collect_stuck_notes(&smf);
    let warnings = lint_warnings(&smf, &notes, &stuck_notes);
    let mut out = String::new();

    writeln!(out, "MIDY_LINT v1 warnings={}", warnings.len())
        .expect("writing to String cannot fail");
    if warnings.is_empty() {
        writeln!(out, "OK").expect("writing to String cannot fail");
    } else {
        for warning in warnings {
            writeln!(out, "{warning}").expect("writing to String cannot fail");
        }
    }

    Ok(out)
}

/// Repairs common note-level MIDI problems while preserving other events.
pub fn fix_midi(bytes: &[u8]) -> Result<Vec<u8>> {
    fix_midi_with_options(bytes, FixOptions::default())
}

/// Repairs common note-level MIDI problems with explicit fix options.
pub fn fix_midi_with_options(bytes: &[u8], options: FixOptions) -> Result<Vec<u8>> {
    let mut smf = parse_smf(bytes)?;
    let notes = collect_notes(&smf);
    let stuck_notes = collect_stuck_notes(&smf);
    let mut editable_notes = notes
        .iter()
        .map(EditableNote::from_note)
        .collect::<Vec<_>>();

    mark_zero_duration_notes(&mut editable_notes);
    mark_duplicate_notes(&mut editable_notes);
    trim_overlapping_notes(&mut editable_notes);

    for editable in editable_notes.iter().filter(|editable| !editable.deleted) {
        validate_note(&editable.note, smf.tracks.len())?;
    }

    let mut patches = build_track_patches(smf.tracks.len(), &notes, &editable_notes)?;
    for stuck in stuck_notes {
        patches[stuck.track]
            .remove_indices
            .insert(stuck.on_event_index);
        if options.stuck_note_mode == StuckNoteFixMode::Close {
            patches[stuck.track].add_notes.push(ScheduledNote {
                note: ConcreteNote {
                    track: stuck.track,
                    channel: stuck.channel,
                    key: stuck.key,
                    start: stuck.start,
                    duration: options.stuck_note_duration.max(1),
                    velocity: 64,
                    off_velocity: 64,
                },
                on_order: stuck.on_event_index,
                off_order: stuck.on_event_index.saturating_add(1),
            });
        }
    }
    for (track_index, patch) in patches.iter().enumerate() {
        let track = std::mem::take(&mut smf.tracks[track_index]);
        smf.tracks[track_index] = rebuild_track(track, patch)?;
    }
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
    match mode {
        SplitMode::Track => {
            let mut outputs = Vec::new();
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
            Ok(outputs)
        }
        SplitMode::Channel => {
            let mut channels = notes.iter().map(|note| note.channel).collect::<Vec<_>>();
            channels.sort_unstable();
            channels.dedup();
            let mut outputs = Vec::new();
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
            Ok(outputs)
        }
    }
}

/// Merges multiple MIDI files into one parallel-track MIDI file.
pub fn merge_midi(inputs: &[&[u8]]) -> Result<Vec<u8>> {
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
            return Err(Error::Usage(format!(
                "merge input {index} has timing '{}', expected '{}'",
                timing_fields(smf.header.timing),
                timing_fields(timing),
            )));
        }
    }

    let mut tracks = Vec::new();
    for smf in &smfs {
        for track in &smf.tracks {
            tracks.push(track.clone());
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
    Ok(out)
}

fn collect_notes(smf: &Smf<'_>) -> Vec<Note> {
    let mut notes = Vec::new();

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
                let id = format!("t{track_index}n{note_ordinal}");
                note_ordinal += 1;
                active
                    .entry((channel, key))
                    .or_default()
                    .push_back(PendingNote {
                        id,
                        channel,
                        key,
                        start: absolute_tick,
                        velocity,
                        on_event_index: event_index,
                    });
            } else if let Some(pending) = active
                .get_mut(&(channel, key))
                .and_then(VecDeque::pop_front)
            {
                notes.push(Note {
                    id: pending.id,
                    track: track_index,
                    channel: pending.channel,
                    key: pending.key,
                    start: pending.start,
                    duration: absolute_tick.saturating_sub(pending.start),
                    velocity: pending.velocity,
                    off_velocity: velocity,
                    on_event_index: pending.on_event_index,
                    off_event_index: event_index,
                });
            }
        }
    }

    notes
}

fn note_sort_key(note: &Note) -> (usize, u64, u8, u8, u64, u8, u8, String) {
    (
        note.track,
        note.start,
        note.channel,
        note.key,
        note.duration,
        note.velocity,
        note.off_velocity,
        note.id.clone(),
    )
}

fn note_signature(note: &Note) -> (usize, u8, u8, u64, u64, u8, u8) {
    (
        note.track,
        note.channel,
        note.key,
        note.start,
        note.duration,
        note.velocity,
        note.off_velocity,
    )
}

fn collect_stuck_notes(smf: &Smf<'_>) -> Vec<StuckNote> {
    let mut stuck = Vec::new();

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
                let id = format!("t{track_index}n{note_ordinal}");
                note_ordinal += 1;
                active
                    .entry((channel, key))
                    .or_default()
                    .push_back(PendingNote {
                        id,
                        channel,
                        key,
                        start: absolute_tick,
                        velocity,
                        on_event_index: event_index,
                    });
            } else {
                let _ = active
                    .get_mut(&(channel, key))
                    .and_then(VecDeque::pop_front);
            }
        }

        for pending in active.into_values().flatten() {
            stuck.push(StuckNote {
                id: pending.id,
                track: track_index,
                channel: pending.channel,
                key: pending.key,
                start: pending.start,
                on_event_index: pending.on_event_index,
            });
        }
    }

    stuck
}

impl EditableNote {
    fn from_note(note: &Note) -> Self {
        Self {
            id: Some(note.id.clone()),
            note: note.as_concrete(),
            on_order: note.on_event_index,
            off_order: note.off_event_index,
            deleted: false,
        }
    }
}

impl Note {
    fn as_concrete(&self) -> ConcreteNote {
        ConcreteNote {
            track: self.track,
            channel: self.channel,
            key: self.key,
            start: self.start,
            duration: self.duration,
            velocity: self.velocity,
            off_velocity: self.off_velocity,
        }
    }
}

fn build_track_patches(
    track_count: usize,
    original_notes: &[Note],
    editable_notes: &[EditableNote],
) -> Result<Vec<TrackPatch>> {
    let mut patches = (0..track_count)
        .map(|_| TrackPatch::default())
        .collect::<Vec<_>>();

    for note in original_notes {
        patches[note.track]
            .remove_indices
            .insert(note.on_event_index);
        patches[note.track]
            .remove_indices
            .insert(note.off_event_index);
    }

    for editable in editable_notes.iter().filter(|editable| !editable.deleted) {
        validate_note(&editable.note, track_count)?;
        patches[editable.note.track].add_notes.push(ScheduledNote {
            note: editable.note.clone(),
            on_order: editable.on_order,
            off_order: editable.off_order,
        });
    }

    Ok(patches)
}

fn note_event(kind: TrackEventKind<'_>) -> Option<(u8, u8, u8, bool)> {
    match kind {
        TrackEventKind::Midi { channel, message } => match message {
            MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                Some((channel.as_int(), key.as_int(), vel.as_int(), true))
            }
            MidiMessage::NoteOn { key, vel } => {
                Some((channel.as_int(), key.as_int(), vel.as_int(), false))
            }
            MidiMessage::NoteOff { key, vel } => {
                Some((channel.as_int(), key.as_int(), vel.as_int(), false))
            }
            _ => None,
        },
        _ => None,
    }
}

fn lint_warnings(smf: &Smf<'_>, notes: &[Note], stuck_notes: &[StuckNote]) -> Vec<String> {
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

    let mut sorted_notes = notes.iter().collect::<Vec<_>>();
    sorted_notes.sort_by_key(|note| {
        (
            note.track,
            note.channel,
            note.key,
            note.start,
            note.start.saturating_add(note.duration),
            note.id.clone(),
        )
    });
    let mut previous = None::<&Note>;
    for note in sorted_notes {
        if let Some(first) = previous
            && first.track == note.track
            && first.channel == note.channel
            && first.key == note.key
            && first.start.saturating_add(first.duration) > note.start
        {
            warnings.push(format!(
                "WARN overlap track={} ch={} key={} first={} second={} first_end={} second_start={}",
                note.track,
                note.channel,
                note.key,
                first.id,
                note.id,
                first.start.saturating_add(first.duration),
                note.start,
            ));
        }
        previous = Some(note);
    }

    let mut stuck_notes = stuck_notes.iter().collect::<Vec<_>>();
    stuck_notes.sort_by_key(|note| {
        (
            note.track,
            note.channel,
            note.key,
            note.start,
            note.id.clone(),
        )
    });
    for note in stuck_notes {
        warnings.push(format!(
            "WARN stuck_note id={} track={} ch={} key={} start={}",
            note.id, note.track, note.channel, note.key, note.start,
        ));
    }

    warnings
}

fn is_empty_track(track: &[TrackEvent<'_>]) -> bool {
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

fn render_track_summaries(smf: &Smf<'_>, notes: &[Note], out: &mut String) {
    for (track_index, track) in smf.tracks.iter().enumerate() {
        let note_count = notes
            .iter()
            .filter(|note| note.track == track_index)
            .count();
        writeln!(
            out,
            "TRACK index={} events={} notes={}",
            track_index,
            track.len(),
            note_count,
        )
        .expect("writing to String cannot fail");
    }
}

fn render_timeline_events(smf: &Smf<'_>, out: &mut String) {
    for (track_index, track) in smf.tracks.iter().enumerate() {
        let mut absolute_tick = 0_u64;

        for event in track {
            absolute_tick += u64::from(event.delta.as_int());
            match event.kind {
                TrackEventKind::Midi { channel, message } => {
                    if let MidiMessage::ProgramChange { program } = message {
                        writeln!(
                            out,
                            "EVENT track={} tick={} kind=program_change ch={} program={}",
                            track_index,
                            absolute_tick,
                            channel.as_int(),
                            program.as_int(),
                        )
                        .expect("writing to String cannot fail");
                    }
                }
                TrackEventKind::Meta(meta) => render_meta(track_index, absolute_tick, meta, out),
                TrackEventKind::SysEx(bytes) => {
                    writeln!(
                        out,
                        "EVENT track={} tick={} kind=sysex bytes={}",
                        track_index,
                        absolute_tick,
                        bytes.len(),
                    )
                    .expect("writing to String cannot fail");
                }
                TrackEventKind::Escape(bytes) => {
                    writeln!(
                        out,
                        "EVENT track={} tick={} kind=escape bytes={}",
                        track_index,
                        absolute_tick,
                        bytes.len(),
                    )
                    .expect("writing to String cannot fail");
                }
            }
        }
    }
}

fn render_meta(track: usize, tick: u64, meta: MetaMessage<'_>, out: &mut String) {
    match meta {
        MetaMessage::TrackName(bytes) => {
            writeln!(
                out,
                "META track={} tick={} kind=track_name text={}",
                track,
                tick,
                quote_ascii(bytes),
            )
            .expect("writing to String cannot fail");
        }
        MetaMessage::InstrumentName(bytes) => {
            writeln!(
                out,
                "META track={} tick={} kind=instrument_name text={}",
                track,
                tick,
                quote_ascii(bytes),
            )
            .expect("writing to String cannot fail");
        }
        MetaMessage::Tempo(micros_per_quarter) => {
            let micros = micros_per_quarter.as_int();
            let bpm = 60_000_000.0 / f64::from(micros);
            writeln!(
                out,
                "META track={} tick={} kind=tempo us_per_quarter={} bpm={:.3}",
                track, tick, micros, bpm,
            )
            .expect("writing to String cannot fail");
        }
        MetaMessage::TimeSignature(numerator, denominator_power, clocks, thirtyseconds) => {
            let denominator = 2_u32.pow(u32::from(denominator_power));
            writeln!(
                out,
                "META track={} tick={} kind=time_signature numerator={} denominator={} denominator_power={} clocks_per_click={} thirtyseconds_per_quarter={}",
                track, tick, numerator, denominator, denominator_power, clocks, thirtyseconds,
            )
            .expect("writing to String cannot fail");
        }
        MetaMessage::KeySignature(sharps, minor) => {
            writeln!(
                out,
                "META track={} tick={} kind=key_signature sharps={} minor={}",
                track, tick, sharps, minor,
            )
            .expect("writing to String cannot fail");
        }
        MetaMessage::EndOfTrack => {
            writeln!(out, "META track={} tick={} kind=end_of_track", track, tick)
                .expect("writing to String cannot fail");
        }
        _ => {}
    }
}

fn rebuild_track<'a>(
    track: Vec<TrackEvent<'a>>,
    patch: &TrackPatch,
) -> Result<Vec<TrackEvent<'a>>> {
    let mut timed = Vec::<TimedEvent<'a>>::new();
    let mut absolute_tick = 0_u64;
    let mut end_of_track_tick = None::<u64>;

    for (event_index, event) in track.into_iter().enumerate() {
        absolute_tick += u64::from(event.delta.as_int());

        if patch.remove_indices.contains(&event_index) {
            continue;
        }

        if matches!(event.kind, TrackEventKind::Meta(MetaMessage::EndOfTrack)) {
            end_of_track_tick =
                Some(end_of_track_tick.map_or(absolute_tick, |tick| tick.max(absolute_tick)));
            continue;
        }

        timed.push(TimedEvent {
            absolute_tick,
            order: event_index,
            kind: event.kind,
        });
    }

    for scheduled in &patch.add_notes {
        let note = &scheduled.note;
        let note_end = note
            .start
            .checked_add(note.duration)
            .ok_or_else(|| Error::Edit("note end tick overflowed u64".to_owned()))?;
        timed.push(TimedEvent {
            absolute_tick: note.start,
            order: scheduled.on_order,
            kind: TrackEventKind::Midi {
                channel: u4::from(note.channel),
                message: MidiMessage::NoteOn {
                    key: u7::from(note.key),
                    vel: u7::from(note.velocity),
                },
            },
        });
        timed.push(TimedEvent {
            absolute_tick: note_end,
            order: scheduled.off_order,
            kind: TrackEventKind::Midi {
                channel: u4::from(note.channel),
                message: MidiMessage::NoteOff {
                    key: u7::from(note.key),
                    vel: u7::from(note.off_velocity),
                },
            },
        });
    }

    timed.sort_by_key(|event| (event.absolute_tick, event.order));

    let last_event_tick = timed
        .iter()
        .map(|event| event.absolute_tick)
        .max()
        .unwrap_or(0);
    let end_of_track_tick = end_of_track_tick
        .unwrap_or(last_event_tick)
        .max(last_event_tick);
    let mut rebuilt = Vec::with_capacity(timed.len() + 1);
    let mut previous_tick = 0_u64;

    for event in timed {
        let delta = event.absolute_tick.saturating_sub(previous_tick);
        rebuilt.push(TrackEvent {
            delta: checked_delta(delta)?,
            kind: event.kind,
        });
        previous_tick = event.absolute_tick;
    }

    rebuilt.push(TrackEvent {
        delta: checked_delta(end_of_track_tick.saturating_sub(previous_tick))?,
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });

    Ok(rebuilt)
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum SelectionAction {
    Mute,
    Solo,
}

fn transform_selection(
    bytes: &[u8],
    selector: &TrackChannelSelector,
    action: SelectionAction,
) -> Result<Vec<u8>> {
    let mut smf = parse_smf(bytes)?;
    validate_selector(selector, smf.tracks.len())?;
    for (track_index, track) in smf.tracks.iter_mut().enumerate() {
        let source = std::mem::take(track);
        *track = filter_track_events(source, track_index, selector, action)?;
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

fn filter_track_events<'a>(
    track: Vec<TrackEvent<'a>>,
    track_index: usize,
    selector: &TrackChannelSelector,
    action: SelectionAction,
) -> Result<Vec<TrackEvent<'a>>> {
    let mut timed = Vec::<TimedEvent<'a>>::new();
    let mut absolute_tick = 0_u64;
    let mut end_of_track_tick = None::<u64>;

    for (event_index, event) in track.into_iter().enumerate() {
        absolute_tick += u64::from(event.delta.as_int());
        if matches!(event.kind, TrackEventKind::Meta(MetaMessage::EndOfTrack)) {
            end_of_track_tick =
                Some(end_of_track_tick.map_or(absolute_tick, |tick| tick.max(absolute_tick)));
            continue;
        }

        if keep_event_for_selection(track_index, event.kind, selector, action) {
            timed.push(TimedEvent {
                absolute_tick,
                order: event_index,
                kind: event.kind,
            });
        }
    }

    let end_of_track_tick = end_of_track_tick.unwrap_or(absolute_tick);
    timed.sort_by_key(|event| (event.absolute_tick, event.order));
    let mut rebuilt = Vec::with_capacity(timed.len() + 1);
    let mut previous_tick = 0_u64;
    for event in timed {
        rebuilt.push(TrackEvent {
            delta: checked_delta(event.absolute_tick.saturating_sub(previous_tick))?,
            kind: event.kind,
        });
        previous_tick = event.absolute_tick;
    }
    rebuilt.push(TrackEvent {
        delta: checked_delta(end_of_track_tick.saturating_sub(previous_tick))?,
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });

    Ok(rebuilt)
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

#[derive(Debug)]
struct TimedEvent<'a> {
    absolute_tick: u64,
    order: usize,
    kind: TrackEventKind<'a>,
}

#[derive(Debug, Clone, Copy)]
struct EditParseContext<'a> {
    ticks_per_beat: Option<u16>,
    time_map: Option<&'a TimeSignatureMap>,
}

fn parse_edit_commands(edits: &str, context: EditParseContext<'_>) -> Result<Vec<EditCommand>> {
    let mut commands = Vec::new();

    for (index, raw_line) in edits.lines().enumerate() {
        let line_no = index + 1;
        let line = strip_inline_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        let mut parts = line.split_whitespace();
        let keyword = parts.next().expect("non-empty line has a keyword");
        let mut positional = Vec::new();
        let mut fields = HashMap::new();

        for part in parts {
            if let Some((key, value)) = part.split_once('=') {
                let key = normalize_key(key);
                if fields.insert(key.clone(), value).is_some() {
                    return Err(edit_error(line_no, format!("duplicate field '{key}'")));
                }
            } else {
                positional.push(part);
            }
        }

        let normalized_keyword = normalize_key(keyword);
        if matches!(
            normalized_keyword.as_str(),
            "midy_timeline" | "header" | "song" | "track" | "meta" | "event"
        ) {
            continue;
        }

        let command = match normalized_keyword.as_str() {
            "add" | "add_note" => EditCommand::Add(parse_add_note(line_no, &fields, context)?),
            "note" => EditCommand::Set {
                id: parse_id(line_no, &fields, &positional)?,
                patch: parse_note_patch(line_no, &fields, context)?,
            },
            "del" | "delete" | "delete_note" | "del_note" => EditCommand::Delete {
                id: parse_id(line_no, &fields, &positional)?,
            },
            "set" | "change" | "set_note" | "change_note" => EditCommand::Set {
                id: parse_id(line_no, &fields, &positional)?,
                patch: parse_note_patch(line_no, &fields, context)?,
            },
            "delete_notes" | "del_notes" | "delete_range" => EditCommand::DeleteMatching {
                filter: parse_filter(line_no, &fields, context)?,
            },
            "transpose" | "transpose_notes" => EditCommand::Transpose {
                semitones: required_i16_alias(line_no, &fields, &["semitones", "semi", "by"])?,
                filter: parse_filter(line_no, &fields, context)?,
            },
            "shift" | "shift_notes" | "move" | "move_notes" => EditCommand::Shift {
                ticks: required_signed_ticks_alias(line_no, &fields, &["ticks", "by"], context)?,
                filter: parse_filter(line_no, &fields, context)?,
            },
            "scale_time" | "stretch" | "stretch_notes" => EditCommand::ScaleTime {
                factor: required_ratio(line_no, &fields, "factor")?,
                filter: parse_filter(line_no, &fields, context)?,
            },
            "scale_duration" | "scale_length" | "length" | "stretch_duration" => {
                EditCommand::ScaleDuration {
                    factor: required_ratio(line_no, &fields, "factor")?,
                    filter: parse_filter(line_no, &fields, context)?,
                }
            }
            "quantize" | "quantize_notes" => EditCommand::Quantize {
                grid: required_duration(line_no, &fields, "grid", context)?,
                mode: parse_quantize_mode(line_no, &fields)?,
                filter: parse_filter(line_no, &fields, context)?,
            },
            "humanize" | "humanise" => EditCommand::Humanize {
                timing: fields
                    .get("timing")
                    .map(|value| parse_u64(line_no, "timing", value))
                    .transpose()?
                    .unwrap_or(12),
                velocity: fields
                    .get("velocity")
                    .or_else(|| fields.get("vel"))
                    .map(|value| parse_u7(line_no, "velocity", value))
                    .transpose()?
                    .unwrap_or(8),
                seed: fields
                    .get("seed")
                    .map(|value| parse_u64(line_no, "seed", value))
                    .transpose()?
                    .unwrap_or(1),
                filter: parse_filter(line_no, &fields, context)?,
            },
            "dehumanize" | "dehumanise" => EditCommand::Dehumanize {
                grid: required_duration(line_no, &fields, "grid", context)?,
                mode: parse_quantize_mode(line_no, &fields)?,
                filter: parse_filter(line_no, &fields, context)?,
            },
            "swing" => EditCommand::Swing {
                amount: parse_swing_amount(line_no, &fields)?,
                grid: required_duration(line_no, &fields, "grid", context)?,
                filter: parse_filter(line_no, &fields, context)?,
            },
            "velocity" | "velocities" | "dynamics" => EditCommand::Velocity {
                command: parse_velocity_command(line_no, &fields)?,
                filter: parse_filter(line_no, &fields, context)?,
            },
            "crescendo" | "velocity_ramp" => EditCommand::Crescendo {
                start_velocity: required_u7_alias(line_no, &fields, &["start_vel", "from_vel"])?,
                end_velocity: required_u7_alias(line_no, &fields, &["end_vel", "to_vel"])?,
                filter: parse_filter(line_no, &fields, context)?,
            },
            "chordize" | "chordise" | "harmonize" | "harmonise" => EditCommand::Chordize {
                intervals: parse_chordize_intervals(line_no, &fields)?,
                filter: parse_filter(line_no, &fields, context)?,
            },
            "arpeggiate" | "arpeggio" | "arp" => EditCommand::Arpeggiate {
                grid: required_duration(line_no, &fields, "grid", context)?,
                order: parse_arpeggio_order(line_no, &fields)?,
                filter: parse_filter(line_no, &fields, context)?,
            },
            "block_chord" | "block_chords" | "block" => EditCommand::BlockChord {
                grid: required_duration(line_no, &fields, "grid", context)?,
                filter: parse_filter(line_no, &fields, context)?,
            },
            "invert_chord" | "invert_chords" | "invert" => EditCommand::InvertChords {
                inversion: fields
                    .get("inversion")
                    .or_else(|| fields.get("by"))
                    .map(|value| parse_i16(line_no, "inversion", value))
                    .transpose()?
                    .unwrap_or(1),
                filter: parse_filter(line_no, &fields, context)?,
            },
            "double" | "double_octave" => EditCommand::Double {
                octave: required_i16_alias(line_no, &fields, &["octave", "oct"])?,
                filter: parse_filter(line_no, &fields, context)?,
            },
            "voice_lead" | "voicelead" | "voice_leading" => EditCommand::VoiceLead {
                max_jump: fields
                    .get("max_jump")
                    .or_else(|| fields.get("max"))
                    .map(|value| parse_u7(line_no, "max_jump", value))
                    .transpose()?
                    .unwrap_or(7),
                filter: parse_filter(line_no, &fields, context)?,
            },
            "mute" | "mute_notes" => EditCommand::Mute {
                filter: parse_filter(line_no, &fields, context)?,
            },
            "solo" | "solo_notes" => EditCommand::Solo {
                filter: parse_filter(line_no, &fields, context)?,
            },
            "move_track" | "move_to_track" => {
                let from = required_usize(line_no, &fields, "from")?;
                let to = required_usize(line_no, &fields, "to")?;
                EditCommand::MoveTrack {
                    from,
                    to,
                    filter: parse_move_track_filter(line_no, &fields, from, context)?,
                }
            }
            "set_channel" | "set_ch" | "channel" => {
                let (channel, filter) = parse_set_channel(line_no, &fields, context)?;
                EditCommand::SetChannel { channel, filter }
            }
            unknown => {
                return Err(edit_error(
                    line_no,
                    format!(
                        "unknown command '{unknown}'; expected ADD_NOTE, SET_NOTE, DELETE_NOTE, TRANSPOSE, SHIFT, SCALE_TIME, SCALE_DURATION, QUANTIZE, DELETE_NOTES, HUMANIZE, DEHUMANIZE, SWING, VELOCITY, CRESCENDO, CHORDIZE, ARPEGGIATE, BLOCK_CHORD, INVERT_CHORDS, DOUBLE, VOICE_LEAD, MUTE, SOLO, MOVE_TRACK, SET_CHANNEL"
                    ),
                ));
            }
        };
        commands.push(command);
    }

    Ok(commands)
}

fn parse_add_note(
    line_no: usize,
    fields: &HashMap<String, &str>,
    context: EditParseContext<'_>,
) -> Result<ConcreteNote> {
    Ok(ConcreteNote {
        track: required_usize(line_no, fields, "track")?,
        channel: required_u4(line_no, fields, &["ch", "channel"])?,
        key: required_key(line_no, fields, "key")?,
        start: required_time_alias(line_no, fields, &["start", "at", "pos"], context)?,
        duration: required_duration_alias(
            line_no,
            fields,
            &["dur", "duration", "len", "length"],
            context,
        )?,
        velocity: optional_u7_alias(line_no, fields, &["vel", "velocity"])?.unwrap_or(64),
        off_velocity: optional_u7_alias(line_no, fields, &["off_vel", "off_velocity"])?
            .unwrap_or(64),
    })
}

fn parse_note_patch(
    line_no: usize,
    fields: &HashMap<String, &str>,
    context: EditParseContext<'_>,
) -> Result<NotePatchFields> {
    let patch = NotePatchFields {
        track: optional_usize(line_no, fields, "track")?,
        channel: optional_u4_alias(line_no, fields, &["ch", "channel"])?,
        key: optional_key(line_no, fields, "key")?,
        start: optional_time_alias(line_no, fields, &["start", "at", "pos"], context)?,
        duration: optional_duration_alias(
            line_no,
            fields,
            &["dur", "duration", "len", "length"],
            context,
        )?,
        velocity: optional_u7_alias(line_no, fields, &["vel", "velocity"])?,
        off_velocity: optional_u7_alias(line_no, fields, &["off_vel", "off_velocity"])?,
    };

    if patch == NotePatchFields::default() {
        return Err(edit_error(
            line_no,
            "SET_NOTE must change at least one field",
        ));
    }

    Ok(patch)
}

fn parse_filter(
    line_no: usize,
    fields: &HashMap<String, &str>,
    context: EditParseContext<'_>,
) -> Result<NoteFilter> {
    let bar_range = optional_bar_filter(line_no, fields, context)?;
    let filter = NoteFilter {
        track: optional_usize(line_no, fields, "track")?,
        channel: optional_u4_alias(line_no, fields, &["ch", "channel"])?,
        key: optional_key(line_no, fields, "key")?,
        start: optional_time_alias(line_no, fields, &["from", "start", "at", "pos"], context)?
            .or_else(|| bar_range.map(|(start, _)| start)),
        end: optional_time_alias(line_no, fields, &["to", "end"], context)?
            .or_else(|| bar_range.map(|(_, end)| end)),
    };

    if let (Some(start), Some(end)) = (filter.start, filter.end)
        && start > end
    {
        return Err(edit_error(line_no, "filter start/from must be <= end/to"));
    }

    Ok(filter)
}

fn parse_move_track_filter(
    line_no: usize,
    fields: &HashMap<String, &str>,
    from: usize,
    context: EditParseContext<'_>,
) -> Result<NoteFilter> {
    let mut filter_fields = fields_without(fields, &["from", "to"]);
    if let Some(track) = filter_fields.get("track").copied() {
        let parsed_track = parse_usize(line_no, "track", track)?;
        if parsed_track != from {
            return Err(edit_error(
                line_no,
                "MOVE_TRACK track filter must match from=...",
            ));
        }
    } else {
        filter_fields.insert(
            "track".to_owned(),
            fields.get("from").copied().unwrap_or("0"),
        );
    }
    parse_filter(line_no, &filter_fields, context)
}

fn parse_set_channel(
    line_no: usize,
    fields: &HashMap<String, &str>,
    context: EditParseContext<'_>,
) -> Result<(u8, NoteFilter)> {
    let channel = required_u4(line_no, fields, &["ch", "channel", "to_ch", "to_channel"])?;
    let mut filter_fields = fields_without(fields, &["ch", "channel", "to_ch", "to_channel"]);
    if let Some(from_channel) = fields
        .get("from_ch")
        .or_else(|| fields.get("from_channel"))
        .copied()
    {
        filter_fields.insert("ch".to_owned(), from_channel);
    }
    Ok((channel, parse_filter(line_no, &filter_fields, context)?))
}

fn fields_without<'a>(
    fields: &HashMap<String, &'a str>,
    excluded: &[&str],
) -> HashMap<String, &'a str> {
    fields
        .iter()
        .filter(|(key, _)| !excluded.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), *value))
        .collect()
}

fn parse_quantize_mode(line_no: usize, fields: &HashMap<String, &str>) -> Result<QuantizeMode> {
    match fields.get("mode").copied().unwrap_or("start") {
        "start" | "starts" => Ok(QuantizeMode::Start),
        "dur" | "duration" | "length" => Ok(QuantizeMode::Duration),
        "both" | "start_end" | "start_duration" => Ok(QuantizeMode::Both),
        value => Err(edit_error(
            line_no,
            format!("field 'mode' must be start, duration, or both; got '{value}'"),
        )),
    }
}

fn parse_swing_amount(line_no: usize, fields: &HashMap<String, &str>) -> Result<u8> {
    let value = fields.get("amount").copied().unwrap_or("55");
    let amount = parse_u7(line_no, "amount", value)?;
    if (50..=100).contains(&amount) {
        Ok(amount)
    } else {
        Err(edit_error(
            line_no,
            "field 'amount' must be in 50..100 for delayed swing",
        ))
    }
}

fn parse_arpeggio_order(line_no: usize, fields: &HashMap<String, &str>) -> Result<ArpeggioOrder> {
    match fields.get("order").copied().unwrap_or("up") {
        "up" | "asc" | "ascending" => Ok(ArpeggioOrder::Up),
        "down" | "desc" | "descending" => Ok(ArpeggioOrder::Down),
        "updown" | "up_down" | "up-down" | "bounce" => Ok(ArpeggioOrder::UpDown),
        value => Err(edit_error(
            line_no,
            format!("field 'order' must be up, down, or updown; got '{value}'"),
        )),
    }
}

fn parse_chordize_intervals(line_no: usize, fields: &HashMap<String, &str>) -> Result<Vec<i16>> {
    if let Some(value) = fields.get("intervals").or_else(|| fields.get("ints")) {
        let intervals = value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| parse_i16(line_no, "intervals", value))
            .collect::<Result<Vec<_>>>()?;
        if intervals.is_empty() {
            return Err(edit_error(line_no, "field 'intervals' must not be empty"));
        }
        return Ok(intervals);
    }

    let quality = fields
        .get("quality")
        .or_else(|| fields.get("chord"))
        .copied()
        .unwrap_or("maj");
    let intervals = match quality {
        "maj" | "major" => &[0, 4, 7][..],
        "min" | "minor" | "m" => &[0, 3, 7],
        "dim" | "diminished" => &[0, 3, 6],
        "aug" | "augmented" => &[0, 4, 8],
        "sus2" => &[0, 2, 7],
        "sus4" => &[0, 5, 7],
        "power" | "5" => &[0, 7],
        "maj7" | "major7" => &[0, 4, 7, 11],
        "min7" | "minor7" | "m7" => &[0, 3, 7, 10],
        "dom7" | "7" => &[0, 4, 7, 10],
        "minmaj7" | "mmaj7" => &[0, 3, 7, 11],
        "add9" => &[0, 4, 7, 14],
        value => {
            return Err(edit_error(
                line_no,
                format!(
                    "field 'quality' must be maj, min, dim, aug, sus2, sus4, power, maj7, min7, dom7, minmaj7, add9, or use intervals=0,4,7; got '{value}'"
                ),
            ));
        }
    };
    Ok(intervals.to_vec())
}

fn parse_velocity_command(
    line_no: usize,
    fields: &HashMap<String, &str>,
) -> Result<VelocityCommand> {
    let mut command = None::<VelocityCommand>;
    if let Some(value) = fields.get("scale") {
        set_velocity_command(
            line_no,
            &mut command,
            VelocityCommand::Scale(parse_ratio(line_no, "scale", value)?),
        )?;
    }
    if let Some(value) = fields.get("add") {
        set_velocity_command(
            line_no,
            &mut command,
            VelocityCommand::Add(parse_i16(line_no, "add", value)?),
        )?;
    }
    if let Some(value) = fields.get("set") {
        set_velocity_command(
            line_no,
            &mut command,
            VelocityCommand::Set(parse_u7(line_no, "set", value)?),
        )?;
    }
    if let Some(value) = fields.get("compress") {
        let center = fields
            .get("center")
            .map(|value| parse_u7(line_no, "center", value))
            .transpose()?
            .unwrap_or(80);
        set_velocity_command(
            line_no,
            &mut command,
            VelocityCommand::Compress {
                factor: parse_ratio(line_no, "compress", value)?,
                center,
            },
        )?;
    }
    command.ok_or_else(|| {
        edit_error(
            line_no,
            "VELOCITY needs exactly one of scale=, add=, set=, or compress=",
        )
    })
}

fn set_velocity_command(
    line_no: usize,
    command: &mut Option<VelocityCommand>,
    value: VelocityCommand,
) -> Result<()> {
    if command.replace(value).is_some() {
        Err(edit_error(
            line_no,
            "VELOCITY accepts only one operation: scale, add, set, or compress",
        ))
    } else {
        Ok(())
    }
}

fn parse_id(line_no: usize, fields: &HashMap<String, &str>, positional: &[&str]) -> Result<String> {
    if let Some(id) = fields.get("id") {
        Ok((*id).to_owned())
    } else if let Some(id) = positional.first() {
        Ok((*id).to_owned())
    } else {
        Err(edit_error(line_no, "missing note id"))
    }
}

fn required_usize(line_no: usize, fields: &HashMap<String, &str>, key: &str) -> Result<usize> {
    let value = fields
        .get(key)
        .ok_or_else(|| edit_error(line_no, format!("missing field '{key}'")))?;
    parse_usize(line_no, key, value)
}

fn optional_usize(
    line_no: usize,
    fields: &HashMap<String, &str>,
    key: &str,
) -> Result<Option<usize>> {
    fields
        .get(key)
        .map(|value| parse_usize(line_no, key, value))
        .transpose()
}

fn required_u4(line_no: usize, fields: &HashMap<String, &str>, keys: &[&str]) -> Result<u8> {
    optional_u4_alias(line_no, fields, keys)?
        .ok_or_else(|| edit_error(line_no, format!("missing field '{}'", keys[0])))
}

fn optional_u4_alias(
    line_no: usize,
    fields: &HashMap<String, &str>,
    keys: &[&str],
) -> Result<Option<u8>> {
    optional_alias(line_no, fields, keys, parse_u4)
}

fn required_u7_alias(line_no: usize, fields: &HashMap<String, &str>, keys: &[&str]) -> Result<u8> {
    optional_u7_alias(line_no, fields, keys)?
        .ok_or_else(|| edit_error(line_no, format!("missing field '{}'", keys[0])))
}

fn optional_u7_alias(
    line_no: usize,
    fields: &HashMap<String, &str>,
    keys: &[&str],
) -> Result<Option<u8>> {
    optional_alias(line_no, fields, keys, parse_u7)
}

fn required_key(line_no: usize, fields: &HashMap<String, &str>, key: &str) -> Result<u8> {
    optional_key(line_no, fields, key)?
        .ok_or_else(|| edit_error(line_no, format!("missing field '{key}'")))
}

fn optional_key(line_no: usize, fields: &HashMap<String, &str>, key: &str) -> Result<Option<u8>> {
    fields
        .get(key)
        .map(|value| parse_key(line_no, key, value))
        .transpose()
}

fn required_time_alias(
    line_no: usize,
    fields: &HashMap<String, &str>,
    keys: &[&str],
    context: EditParseContext<'_>,
) -> Result<u64> {
    optional_time_alias(line_no, fields, keys, context)?
        .ok_or_else(|| edit_error(line_no, format!("missing field '{}'", keys[0])))
}

fn optional_time_alias(
    line_no: usize,
    fields: &HashMap<String, &str>,
    keys: &[&str],
    context: EditParseContext<'_>,
) -> Result<Option<u64>> {
    let mut found = None::<(&str, u64)>;
    for key in keys {
        if let Some(value) = fields.get(*key) {
            let parsed = parse_time_value(line_no, key, value, context)?;
            if let Some((first_key, first_value)) = found {
                if first_value != parsed {
                    return Err(edit_error(
                        line_no,
                        format!("fields '{first_key}' and '{key}' disagree"),
                    ));
                }
            } else {
                found = Some((key, parsed));
            }
        }
    }
    Ok(found.map(|(_, value)| value))
}

fn required_duration(
    line_no: usize,
    fields: &HashMap<String, &str>,
    key: &str,
    context: EditParseContext<'_>,
) -> Result<u64> {
    fields
        .get(key)
        .map(|value| parse_duration_value(line_no, key, value, context))
        .transpose()?
        .ok_or_else(|| edit_error(line_no, format!("missing field '{key}'")))
}

fn required_duration_alias(
    line_no: usize,
    fields: &HashMap<String, &str>,
    keys: &[&str],
    context: EditParseContext<'_>,
) -> Result<u64> {
    optional_duration_alias(line_no, fields, keys, context)?
        .ok_or_else(|| edit_error(line_no, format!("missing field '{}'", keys[0])))
}

fn optional_duration_alias(
    line_no: usize,
    fields: &HashMap<String, &str>,
    keys: &[&str],
    context: EditParseContext<'_>,
) -> Result<Option<u64>> {
    let mut found = None::<(&str, u64)>;
    for key in keys {
        if let Some(value) = fields.get(*key) {
            let parsed = parse_duration_value(line_no, key, value, context)?;
            if let Some((first_key, first_value)) = found {
                if first_value != parsed {
                    return Err(edit_error(
                        line_no,
                        format!("fields '{first_key}' and '{key}' disagree"),
                    ));
                }
            } else {
                found = Some((key, parsed));
            }
        }
    }
    Ok(found.map(|(_, value)| value))
}

fn required_signed_ticks_alias(
    line_no: usize,
    fields: &HashMap<String, &str>,
    keys: &[&str],
    context: EditParseContext<'_>,
) -> Result<i64> {
    let mut found = None::<(&str, i64)>;
    for key in keys {
        if let Some(value) = fields.get(*key) {
            let parsed = parse_signed_ticks_value(line_no, key, value, context)?;
            if let Some((first_key, first_value)) = found {
                if first_value != parsed {
                    return Err(edit_error(
                        line_no,
                        format!("fields '{first_key}' and '{key}' disagree"),
                    ));
                }
            } else {
                found = Some((key, parsed));
            }
        }
    }
    found
        .map(|(_, value)| value)
        .ok_or_else(|| edit_error(line_no, format!("missing field '{}'", keys[0])))
}

fn optional_bar_filter(
    line_no: usize,
    fields: &HashMap<String, &str>,
    context: EditParseContext<'_>,
) -> Result<Option<(u64, u64)>> {
    let mut found = None::<(&str, (u64, u64))>;
    for key in ["bar", "bars"] {
        if let Some(value) = fields.get(key) {
            let parsed = parse_edit_bar_range(line_no, key, value, context)?;
            if let Some((first_key, first_value)) = found {
                if first_value != parsed {
                    return Err(edit_error(
                        line_no,
                        format!("fields '{first_key}' and '{key}' disagree"),
                    ));
                }
            } else {
                found = Some((key, parsed));
            }
        }
    }
    Ok(found.map(|(_, value)| value))
}

fn required_i16_alias(
    line_no: usize,
    fields: &HashMap<String, &str>,
    keys: &[&str],
) -> Result<i16> {
    optional_alias(line_no, fields, keys, parse_i16)?
        .ok_or_else(|| edit_error(line_no, format!("missing field '{}'", keys[0])))
}

fn required_ratio(line_no: usize, fields: &HashMap<String, &str>, key: &str) -> Result<Ratio> {
    let value = fields
        .get(key)
        .ok_or_else(|| edit_error(line_no, format!("missing field '{key}'")))?;
    parse_ratio(line_no, key, value)
}

fn optional_alias<T>(
    line_no: usize,
    fields: &HashMap<String, &str>,
    keys: &[&str],
    parse: fn(usize, &str, &str) -> Result<T>,
) -> Result<Option<T>> {
    let mut found = None;
    for key in keys {
        if let Some(value) = fields.get(*key) {
            if found.is_some() {
                return Err(edit_error(
                    line_no,
                    format!("duplicate aliases for '{}'", keys[0]),
                ));
            }
            found = Some(parse(line_no, key, value)?);
        }
    }
    Ok(found)
}

fn parse_usize(line_no: usize, key: &str, value: &str) -> Result<usize> {
    value
        .parse::<usize>()
        .map_err(|_| edit_error(line_no, format!("field '{key}' must be an integer")))
}

fn parse_u64(line_no: usize, key: &str, value: &str) -> Result<u64> {
    value
        .parse::<u64>()
        .map_err(|_| edit_error(line_no, format!("field '{key}' must be an integer")))
}

fn parse_i16(line_no: usize, key: &str, value: &str) -> Result<i16> {
    value
        .parse::<i16>()
        .map_err(|_| edit_error(line_no, format!("field '{key}' must be a signed integer")))
}

fn parse_ratio(line_no: usize, key: &str, value: &str) -> Result<Ratio> {
    let (numerator, denominator) = if let Some((numerator, denominator)) = value.split_once('/') {
        (
            parse_u64(line_no, key, numerator)?,
            parse_u64(line_no, key, denominator)?,
        )
    } else if let Some((whole, fraction)) = value.split_once('.') {
        let whole = parse_u64(line_no, key, whole)?;
        if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(edit_error(
                line_no,
                format!("field '{key}' must be a positive ratio or decimal"),
            ));
        }
        let denominator = 10_u64
            .checked_pow(fraction.len() as u32)
            .ok_or_else(|| edit_error(line_no, format!("field '{key}' decimal is too precise")))?;
        let fraction = parse_u64(line_no, key, fraction)?;
        let numerator = whole
            .checked_mul(denominator)
            .and_then(|whole| whole.checked_add(fraction))
            .ok_or_else(|| edit_error(line_no, format!("field '{key}' is too large")))?;
        (numerator, denominator)
    } else {
        (parse_u64(line_no, key, value)?, 1)
    };

    Ratio::new(numerator, denominator)
        .ok_or_else(|| edit_error(line_no, format!("field '{key}' must be greater than zero")))
}

fn parse_u4(line_no: usize, key: &str, value: &str) -> Result<u8> {
    let parsed = parse_u64(line_no, key, value)?;
    if parsed <= 15 {
        Ok(parsed as u8)
    } else {
        Err(edit_error(line_no, format!("field '{key}' must be 0..15")))
    }
}

fn parse_u7(line_no: usize, key: &str, value: &str) -> Result<u8> {
    let parsed = parse_u64(line_no, key, value)?;
    if parsed <= 127 {
        Ok(parsed as u8)
    } else {
        Err(edit_error(line_no, format!("field '{key}' must be 0..127")))
    }
}

fn parse_key(line_no: usize, key: &str, value: &str) -> Result<u8> {
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        return parse_u7(line_no, key, value);
    }

    let mut chars = value.chars();
    let Some(letter) = chars.next() else {
        return Err(edit_error(
            line_no,
            format!("field '{key}' cannot be empty"),
        ));
    };
    let base_pc = match letter.to_ascii_uppercase() {
        'C' => 0_i16,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => {
            return Err(edit_error(
                line_no,
                format!("field '{key}' must be 0..127 or a note name like C4/F#3/Bb2"),
            ));
        }
    };

    let rest = chars.as_str();
    let (accidental, octave_text) = if let Some(octave) = rest.strip_prefix('#') {
        (1_i16, octave)
    } else if let Some(octave) = rest
        .strip_prefix('b')
        .or_else(|| rest.strip_prefix('B'))
        .or_else(|| rest.strip_prefix('♭'))
    {
        (-1_i16, octave)
    } else {
        (0_i16, rest)
    };

    if octave_text.is_empty() {
        return Err(edit_error(
            line_no,
            format!("field '{key}' note name must include an octave, for example C4"),
        ));
    }
    let octave = octave_text
        .parse::<i16>()
        .map_err(|_| edit_error(line_no, format!("field '{key}' has an invalid octave")))?;
    let midi_key = (octave + 1)
        .checked_mul(12)
        .and_then(|base| base.checked_add(base_pc + accidental))
        .ok_or_else(|| edit_error(line_no, format!("field '{key}' note name overflowed")))?;
    if (0..=127).contains(&midi_key) {
        Ok(midi_key as u8)
    } else {
        Err(edit_error(
            line_no,
            format!("field '{key}' note name is outside MIDI range 0..127"),
        ))
    }
}

fn parse_time_value(
    line_no: usize,
    key: &str,
    value: &str,
    context: EditParseContext<'_>,
) -> Result<u64> {
    if value.contains(':') {
        parse_bar_beat_tick(line_no, key, value, context)
    } else {
        parse_u64(line_no, key, value)
    }
}

fn parse_bar_beat_tick(
    line_no: usize,
    key: &str,
    value: &str,
    context: EditParseContext<'_>,
) -> Result<u64> {
    let time_map = context.time_map.ok_or_else(|| {
        edit_error(
            line_no,
            format!("field '{key}' uses musical position but MIDI timing is not metrical"),
        )
    })?;
    let parts = value.split(':').collect::<Vec<_>>();
    if !(2..=3).contains(&parts.len()) {
        return Err(edit_error(
            line_no,
            format!("field '{key}' must be BAR:BEAT or BAR:BEAT:TICK"),
        ));
    }
    let bar = parse_u64(line_no, key, parts[0])?;
    let beat = parse_u64(line_no, key, parts[1])?;
    let tick = if let Some(tick) = parts.get(2) {
        parse_u64(line_no, key, tick)?
    } else {
        0
    };
    if bar == 0 || beat == 0 {
        return Err(edit_error(
            line_no,
            format!("field '{key}' bar and beat are 1-based"),
        ));
    }
    time_map
        .position_to_tick(bar, beat, tick)
        .map_err(|message| edit_error(line_no, format!("field '{key}' {message}")))
}

fn parse_duration_value(
    line_no: usize,
    key: &str,
    value: &str,
    context: EditParseContext<'_>,
) -> Result<u64> {
    match value.to_ascii_lowercase().as_str() {
        "beat" => context
            .ticks_per_beat
            .map(u64::from)
            .ok_or_else(|| musical_timing_error(line_no, key)),
        "bar" => context
            .time_map
            .map(TimeSignatureMap::initial_bar_ticks)
            .ok_or_else(|| musical_timing_error(line_no, key)),
        value if value.contains('/') => parse_fractional_ticks(line_no, key, value, context),
        _ => parse_u64(line_no, key, value),
    }
}

fn parse_signed_ticks_value(
    line_no: usize,
    key: &str,
    value: &str,
    context: EditParseContext<'_>,
) -> Result<i64> {
    let (negative, value) = if let Some(value) = value.strip_prefix('-') {
        (true, value)
    } else if let Some(value) = value.strip_prefix('+') {
        (false, value)
    } else {
        (false, value)
    };
    let magnitude = parse_duration_value(line_no, key, value, context)?;
    if magnitude > i64::MAX as u64 {
        return Err(edit_error(
            line_no,
            format!("field '{key}' is too large for a signed tick shift"),
        ));
    }
    if negative {
        Ok(-(magnitude as i64))
    } else {
        Ok(magnitude as i64)
    }
}

fn parse_fractional_ticks(
    line_no: usize,
    key: &str,
    value: &str,
    context: EditParseContext<'_>,
) -> Result<u64> {
    let ticks_per_beat = context
        .ticks_per_beat
        .map(u64::from)
        .ok_or_else(|| musical_timing_error(line_no, key))?;
    let (numerator, denominator) = value
        .split_once('/')
        .ok_or_else(|| edit_error(line_no, format!("field '{key}' must be a fraction")))?;
    let numerator = parse_u64(line_no, key, numerator)?;
    let (denominator, triplet) = if let Some(denominator) = denominator.strip_suffix('t') {
        (denominator, true)
    } else {
        (denominator, false)
    };
    let denominator = parse_u64(line_no, key, denominator)?;
    if numerator == 0 || denominator == 0 {
        return Err(edit_error(
            line_no,
            format!("field '{key}' fraction must be greater than zero"),
        ));
    }
    let whole_note_ticks = ticks_per_beat
        .checked_mul(4)
        .ok_or_else(|| edit_error(line_no, format!("field '{key}' fraction overflowed")))?;
    let numerator_ticks = whole_note_ticks
        .checked_mul(numerator)
        .and_then(|ticks| {
            if triplet {
                ticks.checked_mul(2)
            } else {
                Some(ticks)
            }
        })
        .ok_or_else(|| edit_error(line_no, format!("field '{key}' fraction overflowed")))?;
    let denominator_ticks = denominator
        .checked_mul(if triplet { 3 } else { 1 })
        .ok_or_else(|| edit_error(line_no, format!("field '{key}' fraction overflowed")))?;
    Ok(((numerator_ticks + denominator_ticks / 2) / denominator_ticks).max(1))
}

fn parse_edit_bar_range(
    line_no: usize,
    key: &str,
    value: &str,
    context: EditParseContext<'_>,
) -> Result<(u64, u64)> {
    let time_map = context
        .time_map
        .ok_or_else(|| musical_timing_error(line_no, key))?;
    time_map
        .bar_range_ticks(value)
        .map_err(|error| match error {
            Error::Usage(message) => edit_error(line_no, message),
            error => error,
        })
}

fn musical_timing_error(line_no: usize, key: &str) -> Error {
    edit_error(
        line_no,
        format!("field '{key}' requires metrical MIDI timing"),
    )
}

impl NotePatchFields {
    fn apply_to(&self, note: &mut ConcreteNote) {
        if let Some(track) = self.track {
            note.track = track;
        }
        if let Some(channel) = self.channel {
            note.channel = channel;
        }
        if let Some(key) = self.key {
            note.key = key;
        }
        if let Some(start) = self.start {
            note.start = start;
        }
        if let Some(duration) = self.duration {
            note.duration = duration;
        }
        if let Some(velocity) = self.velocity {
            note.velocity = velocity;
        }
        if let Some(off_velocity) = self.off_velocity {
            note.off_velocity = off_velocity;
        }
    }
}

fn validate_note(note: &ConcreteNote, track_count: usize) -> Result<()> {
    if note.track >= track_count {
        return Err(Error::Edit(format!(
            "track {} does not exist; file has {} tracks",
            note.track, track_count,
        )));
    }
    if note.duration == 0 {
        return Err(Error::Edit(
            "note duration must be greater than zero".to_owned(),
        ));
    }
    let end = note
        .start
        .checked_add(note.duration)
        .ok_or_else(|| Error::Edit("note end tick overflowed u64".to_owned()))?;
    checked_delta(note.start)?;
    checked_delta(note.duration)?;
    checked_delta(end)?;
    Ok(())
}

impl NoteFilter {
    fn matches(&self, note: &ConcreteNote) -> bool {
        if self.track.is_some_and(|track| note.track != track) {
            return false;
        }
        if self.channel.is_some_and(|channel| note.channel != channel) {
            return false;
        }
        if self.key.is_some_and(|key| note.key != key) {
            return false;
        }
        if self.start.is_some_and(|start| note.start < start) {
            return false;
        }
        if self.end.is_some_and(|end| note.start >= end) {
            return false;
        }
        true
    }
}

impl QueryOptions {
    fn matches_note(&self, note: &Note) -> bool {
        if self.track.is_some_and(|track| note.track != track) {
            return false;
        }
        if self.channel.is_some_and(|channel| note.channel != channel) {
            return false;
        }
        if self.key.is_some_and(|key| note.key != key) {
            return false;
        }
        if self.start.is_some_and(|start| note.start < start) {
            return false;
        }
        if self.end.is_some_and(|end| note.start >= end) {
            return false;
        }
        true
    }
}

impl TrackChannelSelector {
    fn matches_midi(&self, track: usize, channel: u8) -> bool {
        self.track.is_none_or(|wanted| wanted == track)
            && self.channel.is_none_or(|wanted| wanted == channel)
    }
}

impl Ratio {
    fn new(numerator: u64, denominator: u64) -> Option<Self> {
        if numerator == 0 || denominator == 0 {
            return None;
        }

        let divisor = greatest_common_divisor(numerator, denominator);
        Some(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    fn scale(self, value: u64) -> Result<u64> {
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

fn find_editable_note_mut<'a>(
    notes: &'a mut [EditableNote],
    id: &str,
) -> Result<&'a mut EditableNote> {
    notes
        .iter_mut()
        .find(|note| note.id.as_deref() == Some(id))
        .ok_or_else(|| Error::Edit(format!("unknown note id '{id}'")))
}

fn matching_notes_mut<'a>(
    notes: &'a mut [EditableNote],
    filter: &NoteFilter,
) -> Vec<&'a mut EditableNote> {
    notes
        .iter_mut()
        .filter(|editable| !editable.deleted && filter.matches(&editable.note))
        .collect()
}

fn transpose_key(key: u8, semitones: i16) -> Result<u8> {
    let transposed = i16::from(key) + semitones;
    if (0..=127).contains(&transposed) {
        Ok(transposed as u8)
    } else {
        Err(Error::Edit(format!(
            "transpose would move key {key} outside 0..127"
        )))
    }
}

fn shift_tick(tick: u64, by: i64) -> Result<u64> {
    if by >= 0 {
        tick.checked_add(by as u64)
            .ok_or_else(|| Error::Edit("shifted tick overflowed u64".to_owned()))
    } else {
        tick.checked_sub(by.unsigned_abs()).ok_or_else(|| {
            Error::Edit(format!(
                "shift by {by} would move tick {tick} before the start"
            ))
        })
    }
}

fn quantize_note(note: &mut ConcreteNote, grid: u64, mode: QuantizeMode) -> Result<()> {
    if grid == 0 {
        return Err(Error::Edit(
            "quantize grid must be greater than zero".to_owned(),
        ));
    }

    match mode {
        QuantizeMode::Start => {
            note.start = quantize_tick(note.start, grid)?;
        }
        QuantizeMode::Duration => {
            note.duration = quantize_tick(note.duration, grid)?.max(1);
        }
        QuantizeMode::Both => {
            let end = note
                .start
                .checked_add(note.duration)
                .ok_or_else(|| Error::Edit("note end tick overflowed u64".to_owned()))?;
            let start = quantize_tick(note.start, grid)?;
            let end = quantize_tick(end, grid)?;
            if end <= start {
                return Err(Error::Edit(
                    "quantize would make a note duration zero".to_owned(),
                ));
            }
            note.start = start;
            note.duration = end - start;
        }
    }

    Ok(())
}

fn quantize_tick(tick: u64, grid: u64) -> Result<u64> {
    tick.checked_add(grid / 2)
        .map(|tick| (tick / grid) * grid)
        .ok_or_else(|| Error::Edit("quantized tick overflowed u64".to_owned()))
}

fn apply_humanize(
    notes: &mut [EditableNote],
    filter: &NoteFilter,
    timing: u64,
    velocity: u8,
    seed: u64,
) -> Result<()> {
    if timing > i64::MAX as u64 {
        return Err(Error::Edit("humanize timing is too large".to_owned()));
    }
    let timing_span = timing
        .checked_mul(2)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| Error::Edit("humanize timing range overflowed".to_owned()))?;
    let velocity_span = u64::from(velocity) * 2 + 1;

    for (index, editable) in notes.iter_mut().enumerate() {
        if editable.deleted || !filter.matches(&editable.note) {
            continue;
        }
        if timing > 0 {
            let raw = stable_random(seed, index as u64, 0) % timing_span;
            let offset = raw as i64 - timing as i64;
            editable.note.start = shift_tick_clamped(editable.note.start, offset)?;
        }
        if velocity > 0 {
            let raw = stable_random(seed, index as u64, 1) % velocity_span;
            let offset = raw as i16 - i16::from(velocity);
            editable.note.velocity =
                clamp_note_on_velocity(i64::from(editable.note.velocity) + i64::from(offset));
        }
    }

    Ok(())
}

fn apply_swing(
    notes: &mut [EditableNote],
    filter: &NoteFilter,
    amount: u8,
    grid: u64,
) -> Result<()> {
    if grid == 0 {
        return Err(Error::Edit(
            "swing grid must be greater than zero".to_owned(),
        ));
    }
    let delay = grid
        .checked_mul(u64::from(amount.saturating_sub(50)))
        .map(|value| (value + 50) / 100)
        .ok_or_else(|| Error::Edit("swing delay overflowed".to_owned()))?;
    if delay == 0 {
        return Ok(());
    }

    for editable in notes.iter_mut() {
        if editable.deleted || !filter.matches(&editable.note) {
            continue;
        }
        let cell = editable.note.start / grid;
        if cell % 2 == 1 {
            editable.note.start = editable
                .note
                .start
                .checked_add(delay)
                .ok_or_else(|| Error::Edit("swing moved a note past u64 range".to_owned()))?;
        }
    }

    Ok(())
}

fn apply_arpeggiate(
    notes: &mut [EditableNote],
    filter: &NoteFilter,
    grid: u64,
    order: ArpeggioOrder,
) -> Result<()> {
    if grid == 0 {
        return Err(Error::Edit(
            "arpeggiate grid must be greater than zero".to_owned(),
        ));
    }
    for group in chord_groups_indices(notes, filter, 2) {
        let base_start = group
            .iter()
            .map(|index| notes[*index].note.start)
            .min()
            .unwrap_or_default();
        let ordered = ordered_chord_indices(notes, &group, order);
        for (position, index) in ordered.into_iter().enumerate() {
            notes[index].note.start = base_start
                .checked_add(
                    grid.checked_mul(position as u64)
                        .ok_or_else(|| Error::Edit("arpeggio tick overflowed".to_owned()))?,
                )
                .ok_or_else(|| Error::Edit("arpeggio tick overflowed".to_owned()))?;
        }
    }
    Ok(())
}

fn apply_block_chord(notes: &mut [EditableNote], filter: &NoteFilter, grid: u64) -> Result<()> {
    if grid == 0 {
        return Err(Error::Edit(
            "block chord grid must be greater than zero".to_owned(),
        ));
    }
    for editable in notes
        .iter_mut()
        .filter(|editable| !editable.deleted && filter.matches(&editable.note))
    {
        editable.note.start = (editable.note.start / grid) * grid;
    }
    Ok(())
}

fn apply_invert_chords(
    notes: &mut [EditableNote],
    filter: &NoteFilter,
    inversion: i16,
) -> Result<()> {
    for group in chord_groups_indices(notes, filter, 2) {
        for _ in 0..inversion.unsigned_abs() {
            let Some(index) = inversion_index(notes, &group, inversion >= 0) else {
                continue;
            };
            let semitones = if inversion >= 0 { 12 } else { -12 };
            notes[index].note.key = transpose_key(notes[index].note.key, semitones)?;
        }
    }
    Ok(())
}

fn apply_chordize(
    notes: &mut Vec<EditableNote>,
    filter: &NoteFilter,
    intervals: &[i16],
    next_order: &mut usize,
    track_count: usize,
) -> Result<()> {
    let selected = notes
        .iter()
        .filter(|editable| !editable.deleted && filter.matches(&editable.note))
        .map(|editable| editable.note.clone())
        .collect::<Vec<_>>();
    for source in selected {
        for interval in intervals.iter().copied().filter(|interval| *interval != 0) {
            let mut note = source.clone();
            note.key = transpose_key(note.key, interval)?;
            validate_note(&note, track_count)?;
            notes.push(EditableNote {
                id: None,
                note,
                on_order: *next_order,
                off_order: next_order.saturating_add(1),
                deleted: false,
            });
            *next_order = next_order.saturating_add(2);
        }
    }
    Ok(())
}

fn apply_double(
    notes: &mut Vec<EditableNote>,
    filter: &NoteFilter,
    octave: i16,
    next_order: &mut usize,
    track_count: usize,
) -> Result<()> {
    let semitones = octave
        .checked_mul(12)
        .ok_or_else(|| Error::Edit("DOUBLE octave is too large".to_owned()))?;
    let selected = notes
        .iter()
        .filter(|editable| !editable.deleted && filter.matches(&editable.note))
        .map(|editable| editable.note.clone())
        .collect::<Vec<_>>();
    for mut note in selected {
        note.key = transpose_key(note.key, semitones)?;
        validate_note(&note, track_count)?;
        notes.push(EditableNote {
            id: None,
            note,
            on_order: *next_order,
            off_order: next_order.saturating_add(1),
            deleted: false,
        });
        *next_order = next_order.saturating_add(2);
    }
    Ok(())
}

fn apply_voice_lead(notes: &mut [EditableNote], filter: &NoteFilter, max_jump: u8) -> Result<()> {
    let groups = chord_groups_indices(notes, filter, 2);
    let mut previous_key = None::<(usize, u8, Vec<u8>)>;
    for group in groups {
        let track = notes[group[0]].note.track;
        let channel = notes[group[0]].note.channel;
        let mut ordered = group.clone();
        ordered.sort_by_key(|index| (notes[*index].note.key, *index));

        if let Some((previous_track, previous_channel, previous_keys)) = previous_key.as_ref()
            && *previous_track == track
            && *previous_channel == channel
            && !previous_keys.is_empty()
        {
            for (position, index) in ordered.iter().enumerate() {
                let target = previous_keys[position.min(previous_keys.len() - 1)];
                notes[*index].note.key =
                    best_voice_led_key(notes[*index].note.key, target, max_jump);
            }
        }

        let mut current_keys = ordered
            .iter()
            .map(|index| notes[*index].note.key)
            .collect::<Vec<_>>();
        current_keys.sort_unstable();
        previous_key = Some((track, channel, current_keys));
    }
    Ok(())
}

fn chord_groups_indices(
    notes: &[EditableNote],
    filter: &NoteFilter,
    minimum_len: usize,
) -> Vec<Vec<usize>> {
    let mut indices = notes
        .iter()
        .enumerate()
        .filter(|(_, editable)| !editable.deleted && filter.matches(&editable.note))
        .map(|(index, editable)| {
            (
                editable.note.track,
                editable.note.channel,
                editable.note.start,
                editable.note.key,
                index,
            )
        })
        .collect::<Vec<_>>();
    indices.sort_unstable();

    let mut groups = Vec::<Vec<usize>>::new();
    let mut current_key = None::<(usize, u8, u64)>;
    let mut current = Vec::<usize>::new();
    for (track, channel, start, _, index) in indices {
        let key = (track, channel, start);
        if current_key.is_some_and(|current_key| current_key != key) {
            if current.len() >= minimum_len {
                groups.push(current);
            }
            current = Vec::new();
        }
        current_key = Some(key);
        current.push(index);
    }
    if current.len() >= minimum_len {
        groups.push(current);
    }
    groups
}

fn ordered_chord_indices(
    notes: &[EditableNote],
    group: &[usize],
    order: ArpeggioOrder,
) -> Vec<usize> {
    let mut ordered = group.to_vec();
    ordered.sort_by_key(|index| (notes[*index].note.key, *index));
    match order {
        ArpeggioOrder::Up => ordered,
        ArpeggioOrder::Down => {
            ordered.reverse();
            ordered
        }
        ArpeggioOrder::UpDown => {
            let mut alternated = Vec::with_capacity(ordered.len());
            let mut left = 0_usize;
            let mut right = ordered.len().saturating_sub(1);
            while left <= right && !ordered.is_empty() {
                alternated.push(ordered[left]);
                if left != right {
                    alternated.push(ordered[right]);
                }
                left += 1;
                right = right.saturating_sub(1);
            }
            alternated
        }
    }
}

fn inversion_index(notes: &[EditableNote], group: &[usize], move_lowest: bool) -> Option<usize> {
    if move_lowest {
        group
            .iter()
            .copied()
            .min_by_key(|index| notes[*index].note.key)
    } else {
        group
            .iter()
            .copied()
            .max_by_key(|index| notes[*index].note.key)
    }
}

fn best_voice_led_key(key: u8, target: u8, max_jump: u8) -> u8 {
    let mut nearest = key;
    let mut nearest_distance = u8::MAX;
    let mut constrained = None::<(u8, u8)>;
    for octave in -10..=10 {
        let candidate = i16::from(key) + octave * 12;
        if !(0..=127).contains(&candidate) {
            continue;
        }
        let candidate = candidate as u8;
        let distance = candidate.abs_diff(target);
        if distance < nearest_distance {
            nearest = candidate;
            nearest_distance = distance;
        }
        if distance <= max_jump && constrained.is_none_or(|(_, best)| distance < best) {
            constrained = Some((candidate, distance));
        }
    }
    constrained
        .map(|(candidate, _)| candidate)
        .unwrap_or(nearest)
}

fn stable_random(seed: u64, index: u64, salt: u64) -> u64 {
    let mut value =
        seed ^ index.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ salt.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn shift_tick_clamped(tick: u64, by: i64) -> Result<u64> {
    if by >= 0 {
        tick.checked_add(by as u64)
            .ok_or_else(|| Error::Edit("shifted tick overflowed u64".to_owned()))
    } else {
        Ok(tick.saturating_sub(by.unsigned_abs()))
    }
}

fn apply_velocity(value: u8, command: VelocityCommand) -> Result<u8> {
    match command {
        VelocityCommand::Scale(factor) => {
            let scaled = factor.scale(u64::from(value))?;
            Ok(clamp_note_on_velocity(scaled as i64))
        }
        VelocityCommand::Add(delta) => {
            Ok(clamp_note_on_velocity(i64::from(value) + i64::from(delta)))
        }
        VelocityCommand::Set(value) => Ok(value),
        VelocityCommand::Compress { factor, center } => {
            let delta = i64::from(value) - i64::from(center);
            let magnitude = delta.unsigned_abs();
            let scaled = factor.scale(magnitude)? as i64;
            let signed = if delta < 0 { -scaled } else { scaled };
            Ok(clamp_note_on_velocity(i64::from(center) + signed))
        }
    }
}

fn clamp_note_on_velocity(value: i64) -> u8 {
    value.clamp(1, 127) as u8
}

fn apply_crescendo(
    notes: &mut [EditableNote],
    filter: &NoteFilter,
    start_velocity: u8,
    end_velocity: u8,
) -> Result<()> {
    let matching = notes
        .iter()
        .filter(|editable| !editable.deleted && filter.matches(&editable.note))
        .map(|editable| editable.note.start)
        .collect::<Vec<_>>();
    if matching.is_empty() {
        return Ok(());
    }
    let start_tick = filter
        .start
        .unwrap_or_else(|| matching.iter().copied().min().unwrap_or_default());
    let end_tick = filter.end.unwrap_or_else(|| {
        matching
            .iter()
            .copied()
            .max()
            .unwrap_or(start_tick)
            .max(start_tick.saturating_add(1))
    });
    let span = end_tick.saturating_sub(start_tick).max(1);
    let span_i64 =
        i64::try_from(span).map_err(|_| Error::Edit("crescendo span is too large".to_owned()))?;
    let delta = i64::from(end_velocity) - i64::from(start_velocity);

    for editable in notes
        .iter_mut()
        .filter(|editable| !editable.deleted && filter.matches(&editable.note))
    {
        let offset = editable.note.start.saturating_sub(start_tick).min(span);
        let offset_i64 = i64::try_from(offset)
            .map_err(|_| Error::Edit("crescendo offset is too large".to_owned()))?;
        let numerator = delta
            .checked_mul(offset_i64)
            .ok_or_else(|| Error::Edit("crescendo velocity overflowed".to_owned()))?;
        let interpolated = i64::from(start_velocity) + rounded_div_i64(numerator, span_i64);
        editable.note.velocity = clamp_note_on_velocity(interpolated);
    }

    Ok(())
}

fn rounded_div_i64(numerator: i64, denominator: i64) -> i64 {
    if numerator >= 0 {
        (numerator + denominator / 2) / denominator
    } else {
        (numerator - denominator / 2) / denominator
    }
}

fn checked_delta(delta: u64) -> Result<u28> {
    if delta <= u64::from(u28::max_value().as_int()) {
        Ok(u28::from(delta as u32))
    } else {
        Err(Error::Edit(format!(
            "delta {delta} is too large for a MIDI variable-length tick value"
        )))
    }
}

fn format_name(format: Format) -> &'static str {
    match format {
        Format::SingleTrack => "single",
        Format::Parallel => "parallel",
        Format::Sequential => "sequential",
    }
}

fn timing_fields(timing: Timing) -> String {
    match timing {
        Timing::Metrical(ticks) => format!("timing=metrical ticks_per_beat={}", ticks.as_int()),
        Timing::Timecode(fps, subframes) => {
            format!(
                "timing=timecode fps={} ticks_per_frame={}",
                fps.as_int(),
                subframes
            )
        }
    }
}

fn ticks_per_beat(timing: Timing) -> Option<u16> {
    match timing {
        Timing::Metrical(ticks) if ticks.as_int() > 0 => Some(ticks.as_int()),
        Timing::Metrical(_) => None,
        Timing::Timecode(_, _) => None,
    }
}

impl TimeSignatureMap {
    fn from_smf(smf: &Smf<'_>) -> Option<Self> {
        let ticks_per_quarter = ticks_per_beat(smf.header.timing)?;
        let mut changes = meta_events(smf)
            .into_iter()
            .filter_map(|(_, tick, meta)| {
                if let MetaMessage::TimeSignature(numerator, denominator_power, _, _) = meta {
                    TimeSignatureValue::from_meta(numerator, denominator_power)
                        .map(|signature| (tick, signature))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        changes.sort_by_key(|(tick, _)| *tick);

        let mut map = Self {
            ticks_per_quarter,
            segments: vec![TimeSignatureSegment {
                start_tick: 0,
                start_bar: 1,
                signature: TimeSignatureValue::default(),
            }],
        };

        for (tick, signature) in changes {
            if let Some(last) = map.segments.last_mut()
                && last.start_tick == tick
            {
                last.signature = signature;
                continue;
            }

            let last = *map
                .segments
                .last()
                .expect("time signature map always has a default segment");
            let delta = tick.saturating_sub(last.start_tick);
            let bar_ticks = map.bar_ticks_for(last.signature);
            let bars_advanced = if delta == 0 {
                0
            } else {
                delta.div_ceil(bar_ticks)
            };
            map.segments.push(TimeSignatureSegment {
                start_tick: tick,
                start_bar: last.start_bar.saturating_add(bars_advanced),
                signature,
            });
        }

        Some(map)
    }

    fn initial_bar_ticks(&self) -> u64 {
        self.bar_ticks_for(self.segments[0].signature)
    }

    fn tick_position(&self, tick: u64) -> String {
        let segment = self.segment_for_tick(tick);
        let delta = tick.saturating_sub(segment.start_tick);
        let beat_ticks = self.beat_ticks_for(segment.signature);
        let bar_ticks = beat_ticks.saturating_mul(u64::from(segment.signature.numerator));
        let bar = segment.start_bar + delta / bar_ticks;
        let tick_in_bar = delta % bar_ticks;
        let beat = tick_in_bar / beat_ticks + 1;
        let tick_in_beat = tick_in_bar % beat_ticks;

        format!("{bar}:{beat}:{tick_in_beat}")
    }

    fn position_to_tick(&self, bar: u64, beat: u64, tick: u64) -> std::result::Result<u64, String> {
        if bar == 0 || beat == 0 {
            return Err("bar and beat are 1-based".to_owned());
        }
        let segment = self.segment_for_bar(bar)?;
        if beat > u64::from(segment.signature.numerator) {
            return Err(format!(
                "beat {beat} exceeds time signature {}/{} at bar {bar}",
                segment.signature.numerator, segment.signature.denominator
            ));
        }
        let beat_ticks = self.beat_ticks_for(segment.signature);
        if tick >= beat_ticks {
            return Err(format!(
                "tick-in-beat must be less than beat length {beat_ticks}"
            ));
        }
        let bar_start = self
            .bar_start_tick(bar)
            .map_err(|error| error.to_string())?;
        bar_start
            .checked_add((beat - 1).saturating_mul(beat_ticks))
            .and_then(|base| base.checked_add(tick))
            .ok_or_else(|| "position overflowed".to_owned())
    }

    fn bar_range_ticks(&self, value: &str) -> Result<(u64, u64)> {
        let parse_bar = |raw: &str| {
            raw.parse::<u64>()
                .map_err(|_| Error::Usage(format!("invalid bar number '{raw}'")))
                .and_then(|bar| {
                    if bar == 0 {
                        Err(Error::Usage("bar numbers are 1-based".to_owned()))
                    } else {
                        Ok(bar)
                    }
                })
        };

        if let Some((start, end)) = value.split_once("..") {
            let start = parse_bar(start)?;
            let end = parse_bar(end)?;
            if start > end {
                return Err(Error::Usage("--bars start must be <= end".to_owned()));
            }
            let end_exclusive = end
                .checked_add(1)
                .ok_or_else(|| Error::Usage("--bars end overflowed".to_owned()))?;
            Ok((
                self.bar_start_tick(start)?,
                self.bar_start_tick(end_exclusive)?,
            ))
        } else {
            let bar = parse_bar(value)?;
            let end_exclusive = bar
                .checked_add(1)
                .ok_or_else(|| Error::Usage("--bars end overflowed".to_owned()))?;
            Ok((
                self.bar_start_tick(bar)?,
                self.bar_start_tick(end_exclusive)?,
            ))
        }
    }

    fn bar_start_tick(&self, bar: u64) -> Result<u64> {
        if bar == 0 {
            return Err(Error::Usage("bar numbers are 1-based".to_owned()));
        }
        let segment = self
            .segment_for_bar(bar)
            .map_err(|message| Error::Usage(message.to_owned()))?;
        let bars_after_segment_start = bar.saturating_sub(segment.start_bar);
        let bar_ticks = self.bar_ticks_for(segment.signature);
        segment
            .start_tick
            .checked_add(bars_after_segment_start.saturating_mul(bar_ticks))
            .ok_or_else(|| Error::Usage("bar range overflowed".to_owned()))
    }

    fn segment_for_tick(&self, tick: u64) -> TimeSignatureSegment {
        let index = self
            .segments
            .iter()
            .rposition(|segment| segment.start_tick <= tick)
            .unwrap_or(0);
        self.segments[index]
    }

    fn segment_for_bar(&self, bar: u64) -> std::result::Result<TimeSignatureSegment, String> {
        let Some(index) = self
            .segments
            .iter()
            .rposition(|segment| segment.start_bar <= bar)
        else {
            return Err(format!("bar {bar} is before the first bar"));
        };
        Ok(self.segments[index])
    }

    fn beat_ticks_for(&self, signature: TimeSignatureValue) -> u64 {
        let quarter_ticks = u64::from(self.ticks_per_quarter);
        let denominator = u64::from(signature.denominator);
        ((quarter_ticks * 4 + denominator / 2) / denominator).max(1)
    }

    fn bar_ticks_for(&self, signature: TimeSignatureValue) -> u64 {
        self.beat_ticks_for(signature)
            .saturating_mul(u64::from(signature.numerator))
            .max(1)
    }
}

impl TimeSignatureValue {
    fn from_meta(numerator: u8, denominator_power: u8) -> Option<Self> {
        if numerator == 0 || denominator_power > 31 {
            return None;
        }
        Some(Self {
            numerator,
            denominator: 1_u32 << denominator_power,
        })
    }
}

fn song_length_ticks(smf: &Smf<'_>, notes: &[Note]) -> u64 {
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

fn note_position_fields(start: u64, end: u64, time_map: Option<&TimeSignatureMap>) -> String {
    let Some(time_map) = time_map else {
        return String::new();
    };

    format!(
        " pos={} end_pos={}",
        time_map.tick_position(start),
        time_map.tick_position(end),
    )
}

fn apply_bars_to_query(
    query: &mut QueryOptions,
    time_map: Option<&TimeSignatureMap>,
) -> Result<()> {
    let Some(bars) = query.bars.as_deref() else {
        return Ok(());
    };
    let time_map =
        time_map.ok_or_else(|| Error::Usage("--bars requires metrical MIDI timing".to_owned()))?;
    let (start, end) = time_map.bar_range_ticks(bars)?;
    query.start.get_or_insert(start);
    query.end.get_or_insert(end);
    Ok(())
}

fn resolve_grid(raw: Option<&str>, ticks_per_beat: Option<u16>) -> Result<u64> {
    match raw {
        Some(value) if value.contains('/') => {
            let Some(ticks_per_beat) = ticks_per_beat else {
                return Err(Error::Usage(
                    "fractional grid requires metrical MIDI timing".to_owned(),
                ));
            };
            let (numerator, denominator) = value
                .split_once('/')
                .ok_or_else(|| Error::Usage(format!("invalid grid '{value}'")))?;
            let numerator = numerator
                .parse::<u64>()
                .map_err(|_| Error::Usage(format!("invalid grid numerator '{numerator}'")))?;
            let denominator = denominator
                .parse::<u64>()
                .map_err(|_| Error::Usage(format!("invalid grid denominator '{denominator}'")))?;
            if numerator == 0 || denominator == 0 {
                return Err(Error::Usage(
                    "grid fraction must be greater than zero".to_owned(),
                ));
            }
            let whole_note_ticks = u64::from(ticks_per_beat) * 4;
            Ok(((whole_note_ticks * numerator) / denominator).max(1))
        }
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| Error::Usage(format!("invalid grid '{value}'")))
            .and_then(|grid| {
                if grid == 0 {
                    Err(Error::Usage("grid must be greater than zero".to_owned()))
                } else {
                    Ok(grid)
                }
            }),
        None => Ok(ticks_per_beat
            .map(|ticks| u64::from(ticks) / 4)
            .unwrap_or(120)
            .max(1)),
    }
}

fn chord_groups(notes: &mut [Note], window: u64) -> Vec<Vec<&Note>> {
    notes.sort_by_key(|note| {
        (
            note.start,
            note.track,
            note.channel,
            note.key,
            note.id.clone(),
        )
    });
    let mut groups = Vec::<Vec<&Note>>::new();
    let mut current = Vec::<&Note>::new();
    let mut group_start = None::<u64>;

    for note in notes.iter() {
        if group_start.is_some_and(|start| note.start > start.saturating_add(window)) {
            groups.push(current);
            current = Vec::new();
            group_start = Some(note.start);
        } else if group_start.is_none() {
            group_start = Some(note.start);
        }
        current.push(note);
    }

    if !current.is_empty() {
        groups.push(current);
    }

    groups
}

fn choose_chord_note(group: &[&Note], keep: &ChordKeep) -> usize {
    match keep {
        ChordKeep::Highest => group
            .iter()
            .enumerate()
            .max_by_key(|(_, note)| note.key)
            .map(|(index, _)| index)
            .unwrap_or(0),
        ChordKeep::Lowest => group
            .iter()
            .enumerate()
            .min_by_key(|(_, note)| note.key)
            .map(|(index, _)| index)
            .unwrap_or(0),
        ChordKeep::Root => {
            let keys = group.iter().map(|note| note.key).collect::<Vec<_>>();
            let root = analyze_chord(&keys).root_pc;
            group
                .iter()
                .enumerate()
                .filter(|(_, note)| note.key % 12 == root)
                .min_by_key(|(_, note)| note.key)
                .map(|(index, _)| index)
                .unwrap_or_else(|| choose_chord_note(group, &ChordKeep::Lowest))
        }
        ChordKeep::Nth(nth) => {
            let mut indexed = group.iter().enumerate().collect::<Vec<_>>();
            indexed.sort_by_key(|(_, note)| note.key);
            let index = nth.saturating_sub(1).min(indexed.len().saturating_sub(1));
            indexed.get(index).map(|(index, _)| *index).unwrap_or(0)
        }
    }
}

#[derive(Debug, Clone)]
struct ChordAnalysis {
    name: String,
    root_pc: u8,
    inversion: String,
    confidence: f64,
}

fn analyze_chord(keys: &[u8]) -> ChordAnalysis {
    let mut pitch_classes = keys.iter().map(|key| key % 12).collect::<Vec<_>>();
    pitch_classes.sort_unstable();
    pitch_classes.dedup();
    let bass_pc = keys.iter().min().copied().unwrap_or(60) % 12;
    let patterns: &[(&str, &[u8])] = &[
        ("maj7", &[0, 4, 7, 11]),
        ("7", &[0, 4, 7, 10]),
        ("min7", &[0, 3, 7, 10]),
        ("mMaj7", &[0, 3, 7, 11]),
        ("m7b5", &[0, 3, 6, 10]),
        ("dim7", &[0, 3, 6, 9]),
        ("maj", &[0, 4, 7]),
        ("min", &[0, 3, 7]),
        ("dim", &[0, 3, 6]),
        ("aug", &[0, 4, 8]),
        ("sus2", &[0, 2, 7]),
        ("sus4", &[0, 5, 7]),
    ];

    let mut best = None::<(u8, &str, usize)>;
    for root in 0..12 {
        let intervals = pitch_classes
            .iter()
            .map(|pc| (pc + 12 - root) % 12)
            .collect::<Vec<_>>();
        for (name, required) in patterns {
            if required.iter().all(|interval| intervals.contains(interval)) {
                let score = required.len();
                if best.is_none_or(|(_, _, best_score)| score > best_score) {
                    best = Some((root, *name, score));
                }
            }
        }
    }

    let (root_pc, quality, score) = best.unwrap_or((bass_pc, "unknown", 1));
    let name = if quality == "unknown" {
        format!("{}unknown", pc_name(root_pc))
    } else {
        format!("{}{}", pc_name(root_pc), quality)
    };
    let inversion = if bass_pc == root_pc {
        "root".to_owned()
    } else {
        format!("over_{}", pc_name(bass_pc))
    };
    let confidence = score as f64 / pitch_classes.len().max(1) as f64;

    ChordAnalysis {
        name,
        root_pc,
        inversion,
        confidence,
    }
}

fn pc_name(pc: u8) -> &'static str {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    NAMES[usize::from(pc % 12)]
}

#[derive(Debug)]
struct TrackStats {
    range: String,
    avg_velocity: f64,
    avg_duration: f64,
    polyphony_max: usize,
}

impl TrackStats {
    fn from_notes(notes: &[&Note]) -> Self {
        if notes.is_empty() {
            return Self {
                range: "-".to_owned(),
                avg_velocity: 0.0,
                avg_duration: 0.0,
                polyphony_max: 0,
            };
        }

        let low = notes.iter().map(|note| note.key).min().unwrap_or_default();
        let high = notes.iter().map(|note| note.key).max().unwrap_or_default();
        let avg_velocity = notes
            .iter()
            .map(|note| f64::from(note.velocity))
            .sum::<f64>()
            / notes.len() as f64;
        let avg_duration =
            notes.iter().map(|note| note.duration as f64).sum::<f64>() / notes.len() as f64;
        let polyphony_max = max_polyphony(notes);

        Self {
            range: format!("{}..{}", note_name(low), note_name(high)),
            avg_velocity,
            avg_duration,
            polyphony_max,
        }
    }

    fn density(&self, song_length: u64) -> f64 {
        if song_length == 0 {
            0.0
        } else {
            self.avg_duration / song_length as f64
        }
    }
}

fn max_polyphony(notes: &[&Note]) -> usize {
    let mut events = Vec::<(u64, i32)>::new();
    for note in notes {
        events.push((note.start, 1));
        events.push((note.start.saturating_add(note.duration), -1));
    }
    events.sort_by_key(|(tick, delta)| (*tick, *delta));
    let mut active = 0_i32;
    let mut max_active = 0_i32;
    for (_, delta) in events {
        active += delta;
        max_active = max_active.max(active);
    }
    max_active.max(0) as usize
}

fn classify_track(notes: &[&Note]) -> &'static str {
    if notes.is_empty() {
        return "meta";
    }
    if notes.iter().any(|note| note.channel == 9) {
        return "drums";
    }
    let stats = TrackStats::from_notes(notes);
    let avg_key = notes.iter().map(|note| f64::from(note.key)).sum::<f64>() / notes.len() as f64;
    if stats.polyphony_max >= 3 {
        "chords"
    } else if avg_key < 48.0 {
        "bass"
    } else if stats.polyphony_max <= 2 && avg_key >= 55.0 {
        "melody"
    } else {
        "notes"
    }
}

fn render_tempo_summary(smf: &Smf<'_>, out: &mut String) {
    let tempos = meta_events(smf)
        .into_iter()
        .filter_map(|(track, tick, meta)| {
            if let MetaMessage::Tempo(us) = meta {
                Some((track, tick, us.as_int()))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if let Some((track, tick, micros)) = tempos.first().copied() {
        writeln!(
            out,
            "tempo initial_bpm={:.3} us_per_quarter={} changes={} first_track={} first_tick={}",
            60_000_000.0 / f64::from(micros),
            micros,
            tempos.len(),
            track,
            tick,
        )
        .expect("writing to String cannot fail");
    } else {
        writeln!(
            out,
            "tempo initial_bpm=120.000 us_per_quarter=500000 changes=0 inferred=true"
        )
        .expect("writing to String cannot fail");
    }
}

fn render_time_signature_summary(smf: &Smf<'_>, out: &mut String) {
    let signatures = meta_events(smf)
        .into_iter()
        .filter_map(|(track, tick, meta)| {
            if let MetaMessage::TimeSignature(num, den_power, _, _) = meta {
                Some((track, tick, num, 2_u32.pow(u32::from(den_power))))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if let Some((track, tick, num, den)) = signatures.first().copied() {
        writeln!(
            out,
            "time_signature initial={}/{} changes={} first_track={} first_tick={}",
            num,
            den,
            signatures.len(),
            track,
            tick,
        )
        .expect("writing to String cannot fail");
    } else {
        writeln!(out, "time_signature initial=4/4 changes=0 inferred=true")
            .expect("writing to String cannot fail");
    }
}

fn meta_events<'a>(smf: &Smf<'a>) -> Vec<(usize, u64, MetaMessage<'a>)> {
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

fn channels_text(notes: &[&Note]) -> String {
    let mut channels = notes.iter().map(|note| note.channel).collect::<Vec<_>>();
    channels.sort_unstable();
    channels.dedup();
    if channels.is_empty() {
        "-".to_owned()
    } else {
        channels
            .iter()
            .map(|channel| channel.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn track_names(track: &[TrackEvent<'_>]) -> String {
    let names = track
        .iter()
        .filter_map(|event| {
            if let TrackEventKind::Meta(MetaMessage::TrackName(bytes)) = event.kind {
                Some(String::from_utf8_lossy(bytes).into_owned())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    if names.is_empty() {
        "-".to_owned()
    } else {
        names.join("|")
    }
}

fn guess_key_name(notes: &[Note]) -> String {
    if notes.is_empty() {
        return "unknown".to_owned();
    }
    let mut counts = [0_u64; 12];
    for note in notes {
        counts[usize::from(note.key % 12)] += note.duration.max(1);
    }
    let (pc, _) = counts
        .iter()
        .enumerate()
        .max_by_key(|(_, count)| *count)
        .unwrap_or((0, &0));
    format!("{} major_or_relative", pc_name(pc as u8))
}

fn tick_position_field_from_line(line: &str, time_map: &TimeSignatureMap) -> String {
    let tick = line
        .split_whitespace()
        .find_map(|part| part.strip_prefix("tick="))
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    time_map.tick_position(tick)
}

fn note_name(key: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = i16::from(key) / 12 - 1;
    format!("{}{}", NAMES[usize::from(key % 12)], octave)
}

fn midi_key_for_octave(pc: u8, octave: i16) -> Result<u8> {
    let key = (octave + 1)
        .checked_mul(12)
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

fn quote_ascii(bytes: &[u8]) -> String {
    let mut quoted = String::from("\"");
    for byte in bytes {
        for escaped in std::ascii::escape_default(*byte) {
            quoted.push(char::from(escaped));
        }
    }
    quoted.push('"');
    quoted
}

fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if character.is_control() => {
                write!(out, "\\u{:04x}", character as u32).expect("writing to String cannot fail");
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

fn csv_field(value: &str) -> String {
    if value
        .chars()
        .any(|character| matches!(character, ',' | '"' | '\n' | '\r'))
    {
        let mut out = String::from("\"");
        for character in value.chars() {
            if character == '"' {
                out.push('"');
            }
            out.push(character);
        }
        out.push('"');
        out
    } else {
        value.to_owned()
    }
}

fn json_string_field(line: &str, key: &str) -> std::result::Result<String, String> {
    let marker = format!("\"{key}\":");
    let after = line
        .split_once(&marker)
        .ok_or_else(|| format!("missing field '{key}'"))?
        .1
        .trim_start();
    let mut chars = after.chars();
    if chars.next() != Some('"') {
        return Err(format!("field '{key}' must be a JSON string"));
    }
    let mut value = String::new();
    let mut escaped = false;
    for character in chars {
        if escaped {
            value.push(match character {
                '"' => '"',
                '\\' => '\\',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            return Ok(value);
        } else {
            value.push(character);
        }
    }
    Err(format!("field '{key}' string was not terminated"))
}

fn json_u64_field(line: &str, key: &str) -> std::result::Result<u64, String> {
    let marker = format!("\"{key}\":");
    let after = line
        .split_once(&marker)
        .ok_or_else(|| format!("missing field '{key}'"))?
        .1
        .trim_start();
    let number = after
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    if number.is_empty() {
        return Err(format!("field '{key}' must be an unsigned integer"));
    }
    number
        .parse::<u64>()
        .map_err(|_| format!("field '{key}' is too large"))
}

fn json_usize_field(line: &str, key: &str) -> std::result::Result<usize, String> {
    let value = json_u64_field(line, key)?;
    usize::try_from(value).map_err(|_| format!("field '{key}' is too large"))
}

fn json_u8_field(line: &str, key: &str, max: u8) -> std::result::Result<u8, String> {
    let value = json_u64_field(line, key)?;
    let value = u8::try_from(value).map_err(|_| format!("field '{key}' is too large"))?;
    if value <= max {
        Ok(value)
    } else {
        Err(format!("field '{key}' must be <= {max}"))
    }
}

fn parse_csv_row(line: &str) -> Vec<String> {
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = line.chars().peekable();
    let mut quoted = false;
    while let Some(character) = chars.next() {
        if quoted {
            if character == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(character);
            }
        } else if character == '"' && field.is_empty() {
            quoted = true;
        } else if character == ',' {
            row.push(std::mem::take(&mut field));
        } else {
            field.push(character);
        }
    }
    row.push(field);
    row
}

fn parse_csv_u64(line_no: usize, key: &str, value: &str) -> Result<u64> {
    value.parse::<u64>().map_err(|_| {
        Error::Usage(format!(
            "timeline CSV line {line_no} field '{key}' must be an integer"
        ))
    })
}

fn parse_csv_usize(line_no: usize, key: &str, value: &str) -> Result<usize> {
    value.parse::<usize>().map_err(|_| {
        Error::Usage(format!(
            "timeline CSV line {line_no} field '{key}' must be an integer"
        ))
    })
}

fn parse_csv_u8(line_no: usize, key: &str, value: &str, max: u8) -> Result<u8> {
    let parsed = value.parse::<u8>().map_err(|_| {
        Error::Usage(format!(
            "timeline CSV line {line_no} field '{key}' must be an integer"
        ))
    })?;
    if parsed <= max {
        Ok(parsed)
    } else {
        Err(Error::Usage(format!(
            "timeline CSV line {line_no} field '{key}' must be <= {max}"
        )))
    }
}

fn nonempty_id(id: String) -> Option<String> {
    let trimmed = id.trim();
    if trimmed.is_empty() || trimmed == "-" {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn normalize_key(value: &str) -> String {
    value.to_ascii_lowercase().replace('-', "_")
}

fn strip_inline_comment(line: &str) -> &str {
    let mut previous_was_whitespace = true;
    for (index, character) in line.char_indices() {
        if character == '#' && previous_was_whitespace {
            return &line[..index];
        }
        previous_was_whitespace = character.is_whitespace();
    }
    line
}

fn edit_error(line_no: usize, message: impl Into<String>) -> Error {
    Error::Edit(format!("edit line {line_no}: {}", message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_SINGLE_TRACK_MIDI: &[u8] =
        b"MThd\x00\x00\x00\x06\x00\x00\x00\x01\x00\x60MTrk\x00\x00\x00\x04\x00\xff\x2f\x00";
    const ONE_NOTE_MIDI: &[u8] = b"MThd\x00\x00\x00\x06\x00\x00\x00\x01\x00\x60MTrk\x00\x00\x00\x0c\x00\x90\x3c\x40\x60\x80\x3c\x40\x00\xff\x2f\x00";
    const ONE_CHORD_MIDI: &[u8] = b"MThd\x00\x00\x00\x06\x00\x00\x00\x01\x00\x60MTrk\x00\x00\x00\x1c\x00\x90\x3c\x40\x00\x90\x40\x40\x00\x90\x43\x40\x60\x80\x3c\x40\x00\x80\x40\x40\x00\x80\x43\x40\x00\xff\x2f\x00";
    const TWO_CHANNEL_MIDI: &[u8] = b"MThd\x00\x00\x00\x06\x00\x00\x00\x01\x00\x60MTrk\x00\x00\x00\x14\x00\x90\x3c\x40\x00\x91\x40\x40\x60\x80\x3c\x40\x00\x81\x40\x40\x00\xff\x2f\x00";
    const STUCK_NOTE_MIDI: &[u8] = b"MThd\x00\x00\x00\x06\x00\x00\x00\x01\x00\x60MTrk\x00\x00\x00\x08\x00\x90\x3c\x40\x60\xff\x2f\x00";
    const TIME_SIGNATURE_CHANGE_MIDI: &[u8] = b"MThd\x00\x00\x00\x06\x00\x00\x00\x01\x00\x60MTrk\x00\x00\x00\x15\x00\xff\x58\x04\x03\x02\x18\x08\x82\x20\xff\x58\x04\x04\x02\x18\x08\x00\xff\x2f\x00";

    #[test]
    fn summarizes_track_count_with_midly() {
        assert_eq!(
            summarize_smf(EMPTY_SINGLE_TRACK_MIDI).unwrap(),
            MidiSummary { tracks: 1 },
        );
    }

    #[test]
    fn renders_note_timeline() {
        let text = render_timeline(ONE_NOTE_MIDI).unwrap();

        assert!(text.contains("MIDY_TIMELINE v1"));
        assert!(text.contains("HEADER format=single timing=metrical ticks_per_beat=96 tracks=1"));
        assert!(text.contains("SONG length_ticks=96 length_beats=1.000"));
        assert!(text.contains(
            "NOTE id=t0n0 track=0 ch=0 key=60 name=C4 start=0 dur=96 end=96 pos=1:1:0 end_pos=1:2:0 vel=64 off_vel=64"
        ));
    }

    #[test]
    fn renders_timeline_json() {
        let text = render_timeline_json(ONE_NOTE_MIDI).unwrap();

        assert!(text.contains("\"kind\": \"MIDY_TIMELINE\""));
        assert!(text.contains("\"format\": \"single\""));
        assert!(text.contains("\"notes\""));
        assert!(text.contains("\"id\": \"t0n0\""));
        assert!(text.contains("\"key\": 60"));
    }

    #[test]
    fn renders_timeline_csv() {
        let text = render_timeline_csv(ONE_NOTE_MIDI).unwrap();

        assert!(text.starts_with("id,track,ch,key,name,start,dur,end,vel,off_vel\n"));
        assert!(text.contains("t0n0,0,0,60,C4,0,96,96,64,64"));
    }

    #[test]
    fn applies_timeline_json_rows() {
        let json = render_timeline_json(ONE_NOTE_MIDI)
            .unwrap()
            .replace("\"key\": 60", "\"key\": 62")
            .replace("\"name\": \"C4\"", "\"name\": \"D4\"");
        let rewritten = apply_timeline_json_edits(ONE_NOTE_MIDI, &json).unwrap();
        let text = render_timeline(&rewritten).unwrap();

        assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=62 name=D4"));
    }

    #[test]
    fn applies_timeline_csv_rows_and_deletes_missing_ids() {
        let csv = render_timeline_csv(ONE_CHORD_MIDI).unwrap();
        let edited_csv = csv
            .lines()
            .filter(|line| !line.contains(",64,E4,"))
            .collect::<Vec<_>>()
            .join("\n");
        let rewritten = apply_timeline_csv_edits(ONE_CHORD_MIDI, &edited_csv).unwrap();
        let text = render_timeline(&rewritten).unwrap();

        assert!(text.contains("key=60 name=C4"));
        assert!(!text.contains("key=64 name=E4"));
        assert!(text.contains("key=67 name=G4"));
    }

    #[test]
    fn renders_empty_diff_for_equal_notes() {
        let diff = render_diff(ONE_NOTE_MIDI, ONE_NOTE_MIDI).unwrap();

        assert!(diff.is_empty());
    }

    #[test]
    fn renders_diff_that_recreates_after_notes() {
        let after = apply_edits(ONE_NOTE_MIDI, "SET_NOTE id=t0n0 key=62 vel=90\n").unwrap();
        let diff = render_diff(ONE_NOTE_MIDI, &after).unwrap();
        let recreated = apply_edits(ONE_NOTE_MIDI, &diff).unwrap();

        assert!(diff.contains("DELETE_NOTE id=t0n0"));
        assert!(diff.contains("ADD_NOTE track=0 ch=0 key=62 start=0 dur=96 vel=90 off_vel=64"));
        assert_eq!(
            render_timeline(&recreated).unwrap(),
            render_timeline(&after).unwrap()
        );
    }

    #[test]
    fn renders_non_note_event_diff_comments() {
        let diff = render_diff(EMPTY_SINGLE_TRACK_MIDI, TIME_SIGNATURE_CHANGE_MIDI).unwrap();

        assert!(diff.contains("# NON_NOTE_DIFF removed=0 added=2"));
        assert!(diff.contains("# ADD_EVENT META track=0 tick=0 kind=time_signature"));
    }

    #[test]
    fn applies_add_note_command() {
        let rewritten = apply_edits(
            EMPTY_SINGLE_TRACK_MIDI,
            "ADD_NOTE track=0 ch=0 key=64 start=0 dur=48 vel=80\n",
        )
        .unwrap();
        let text = render_timeline(&rewritten).unwrap();

        assert!(text.contains(
            "NOTE id=t0n0 track=0 ch=0 key=64 name=E4 start=0 dur=48 end=48 pos=1:1:0 end_pos=1:1:48 vel=80 off_vel=64"
        ));
    }

    #[test]
    fn applies_set_note_command() {
        let rewritten = apply_edits(ONE_NOTE_MIDI, "SET_NOTE id=t0n0 key=61 dur=48\n").unwrap();
        let text = render_timeline(&rewritten).unwrap();

        assert!(text.contains(
            "NOTE id=t0n0 track=0 ch=0 key=61 name=C#4 start=0 dur=48 end=48 pos=1:1:0 end_pos=1:1:48 vel=64 off_vel=64"
        ));
    }

    #[test]
    fn applies_delete_note_command() {
        let rewritten = apply_edits(ONE_NOTE_MIDI, "DELETE_NOTE id=t0n0\n").unwrap();
        let text = render_timeline(&rewritten).unwrap();

        assert!(!text.lines().any(|line| line.starts_with("NOTE ")));
    }

    #[test]
    fn applies_whole_file_transpose() {
        let rewritten = apply_edits(ONE_NOTE_MIDI, "TRANSPOSE semitones=7\n").unwrap();
        let text = render_timeline(&rewritten).unwrap();

        assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=67 name=G4"));
    }

    #[test]
    fn applies_filtered_shift() {
        let rewritten = apply_edits(ONE_NOTE_MIDI, "SHIFT ticks=24 track=0 ch=0\n").unwrap();
        let text = render_timeline(&rewritten).unwrap();

        assert!(text.contains("start=24 dur=96 end=120 pos=1:1:24 end_pos=1:2:24"));
    }

    #[test]
    fn applies_scale_duration() {
        let rewritten = apply_edits(ONE_NOTE_MIDI, "SCALE_DURATION factor=1/2\n").unwrap();
        let text = render_timeline(&rewritten).unwrap();

        assert!(text.contains("start=0 dur=48 end=48"));
    }

    #[test]
    fn applies_quantize_both() {
        let shifted = apply_edits(ONE_NOTE_MIDI, "SHIFT ticks=17\n").unwrap();
        let quantized = apply_edits(&shifted, "QUANTIZE grid=24 mode=both\n").unwrap();
        let text = render_timeline(&quantized).unwrap();

        assert!(text.contains("start=24 dur=96 end=120"));
    }

    #[test]
    fn applies_delete_notes_filter() {
        let rewritten = apply_edits(ONE_NOTE_MIDI, "DELETE_NOTES start=0 end=96 key=60\n").unwrap();
        let text = render_timeline(&rewritten).unwrap();

        assert!(!text.lines().any(|line| line.starts_with("NOTE ")));
    }

    #[test]
    fn applies_modified_timeline_note_line() {
        let timeline = render_timeline(ONE_NOTE_MIDI).unwrap();
        let edited_timeline = timeline.replace("key=60 name=C4", "key=62 name=D4");
        let rewritten = apply_edits(ONE_NOTE_MIDI, &edited_timeline).unwrap();
        let text = render_timeline(&rewritten).unwrap();

        assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=62 name=D4"));
    }

    #[test]
    fn applies_musical_add_note_aliases() {
        let rewritten = apply_edits(
            EMPTY_SINGLE_TRACK_MIDI,
            "ADD_NOTE track=0 ch=0 key=C4 at=2:1 dur=1/4 vel=80\n",
        )
        .unwrap();
        let text = render_timeline(&rewritten).unwrap();

        assert!(text.contains(
            "NOTE id=t0n0 track=0 ch=0 key=60 name=C4 start=384 dur=96 end=480 pos=2:1:0 end_pos=2:2:0 vel=80 off_vel=64"
        ));
    }

    #[test]
    fn uses_time_signature_map_for_musical_positions() {
        let rewritten = apply_edits(
            TIME_SIGNATURE_CHANGE_MIDI,
            "\
ADD_NOTE track=0 ch=0 key=C4 at=1:3 dur=beat vel=80
ADD_NOTE track=0 ch=0 key=E4 at=2:4 dur=beat vel=80
",
        )
        .unwrap();
        let text = render_timeline(&rewritten).unwrap();

        assert!(text.contains("key=60 name=C4 start=192 dur=96 end=288 pos=1:3:0 end_pos=2:1:0"));
        assert!(text.contains("key=64 name=E4 start=576 dur=96 end=672 pos=2:4:0 end_pos=3:1:0"));
    }

    #[test]
    fn rejects_beats_outside_the_current_time_signature() {
        let error = apply_edits(
            TIME_SIGNATURE_CHANGE_MIDI,
            "ADD_NOTE track=0 ch=0 key=C4 at=1:4 dur=beat vel=80\n",
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("beat 4 exceeds time signature 3/4")
        );
    }

    #[test]
    fn uses_time_signature_map_for_bar_filters() {
        let notes = apply_edits(
            TIME_SIGNATURE_CHANGE_MIDI,
            "\
ADD_NOTE track=0 ch=0 key=C4 at=1:3 dur=beat vel=80
ADD_NOTE track=0 ch=0 key=E4 at=2:4 dur=beat vel=80
",
        )
        .unwrap();
        let shifted = apply_edits(&notes, "SHIFT by=beat bars=2\n").unwrap();
        let text = render_timeline(&shifted).unwrap();

        assert!(text.contains("key=60 name=C4 start=192"));
        assert!(text.contains("key=64 name=E4 start=672"));
    }

    #[test]
    fn applies_note_name_patch_and_fraction_duration() {
        let rewritten = apply_edits(ONE_NOTE_MIDI, "SET_NOTE id=t0n0 key=F#4 dur=1/8\n").unwrap();
        let text = render_timeline(&rewritten).unwrap();

        assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=66 name=F#4 start=0 dur=48"));
    }

    #[test]
    fn applies_bar_filter_and_fraction_shift() {
        let rewritten = apply_edits(ONE_NOTE_MIDI, "SHIFT by=1/4 bar=1\n").unwrap();
        let text = render_timeline(&rewritten).unwrap();

        assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=60 name=C4 start=96"));
    }

    #[test]
    fn quantizes_to_fractional_grid() {
        let shifted = apply_edits(ONE_NOTE_MIDI, "SHIFT ticks=73\n").unwrap();
        let quantized = apply_edits(&shifted, "QUANTIZE grid=1/4 mode=start\n").unwrap();
        let text = render_timeline(&quantized).unwrap();

        assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=60 name=C4 start=96"));
    }

    #[test]
    fn applies_humanize_reproducibly() {
        let first = apply_edits(ONE_NOTE_MIDI, "HUMANIZE timing=12 velocity=8 seed=7\n").unwrap();
        let second = apply_edits(ONE_NOTE_MIDI, "HUMANIZE timing=12 velocity=8 seed=7\n").unwrap();
        let notes = collect_notes(&parse_smf(&first).unwrap());
        let note = notes.first().unwrap();

        assert_eq!(first, second);
        assert!(note.start <= 12);
        assert!((56..=72).contains(&note.velocity));
    }

    #[test]
    fn applies_dehumanize() {
        let shifted = apply_edits(ONE_NOTE_MIDI, "SHIFT ticks=17\n").unwrap();
        let dehumanized = apply_edits(&shifted, "DEHUMANIZE grid=24 mode=both\n").unwrap();
        let text = render_timeline(&dehumanized).unwrap();

        assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=60 name=C4 start=24"));
    }

    #[test]
    fn applies_swing_to_odd_grid_subdivisions() {
        let notes = apply_edits(
            EMPTY_SINGLE_TRACK_MIDI,
            "\
ADD_NOTE track=0 ch=0 key=60 start=0 dur=24 vel=64
ADD_NOTE track=0 ch=0 key=62 start=48 dur=24 vel=64
",
        )
        .unwrap();
        let swung = apply_edits(&notes, "SWING amount=75 grid=1/8\n").unwrap();
        let text = render_timeline(&swung).unwrap();

        assert!(text.contains("key=60 name=C4 start=0"));
        assert!(text.contains("key=62 name=D4 start=60"));
    }

    #[test]
    fn applies_velocity_scale_add_set_and_compress() {
        let scaled = apply_edits(ONE_NOTE_MIDI, "VELOCITY scale=1/2\n").unwrap();
        assert!(render_timeline(&scaled).unwrap().contains("vel=32"));

        let added = apply_edits(ONE_NOTE_MIDI, "VELOCITY add=10\n").unwrap();
        assert!(render_timeline(&added).unwrap().contains("vel=74"));

        let set = apply_edits(ONE_NOTE_MIDI, "VELOCITY set=96\n").unwrap();
        assert!(render_timeline(&set).unwrap().contains("vel=96"));

        let compressed = apply_edits(ONE_NOTE_MIDI, "VELOCITY compress=1/2 center=80\n").unwrap();
        assert!(render_timeline(&compressed).unwrap().contains("vel=72"));
    }

    #[test]
    fn applies_crescendo() {
        let notes = apply_edits(
            EMPTY_SINGLE_TRACK_MIDI,
            "\
ADD_NOTE track=0 ch=0 key=60 start=0 dur=48 vel=64
ADD_NOTE track=0 ch=0 key=62 start=96 dur=48 vel=64
ADD_NOTE track=0 ch=0 key=64 start=192 dur=48 vel=64
",
        )
        .unwrap();
        let crescendo = apply_edits(
            &notes,
            "CRESCENDO start_vel=40 end_vel=100 start=0 end=193\n",
        )
        .unwrap();
        let text = render_timeline(&crescendo).unwrap();

        assert!(
            text.contains("key=60 name=C4 start=0 dur=48 end=48 pos=1:1:0 end_pos=1:1:48 vel=40")
        );
        assert!(
            text.contains("key=62 name=D4 start=96 dur=48 end=144 pos=1:2:0 end_pos=1:2:48 vel=70")
        );
        assert!(
            text.contains(
                "key=64 name=E4 start=192 dur=48 end=240 pos=1:3:0 end_pos=1:3:48 vel=100"
            )
        );
    }

    #[test]
    fn suggests_reducing_chord_to_highest_note() {
        let edits = suggest_reduce_chords(
            ONE_CHORD_MIDI,
            &ReduceChordsOptions {
                keep: ChordKeep::Highest,
                ..ReduceChordsOptions::default()
            },
        )
        .unwrap();

        assert!(edits.contains("DELETE_NOTE id=t0n0"));
        assert!(edits.contains("DELETE_NOTE id=t0n1"));
        assert!(!edits.contains("DELETE_NOTE id=t0n2"));
    }

    #[test]
    fn suggests_bassline_from_chord_roots() {
        let edits = suggest_bassline(
            ONE_CHORD_MIDI,
            &BasslineOptions {
                output_track: 0,
                output_channel: 0,
                octave: 2,
                velocity: 90,
                ..BasslineOptions::default()
            },
        )
        .unwrap();
        let rewritten = apply_edits(ONE_CHORD_MIDI, &edits).unwrap();
        let text = render_timeline(&rewritten).unwrap();

        assert!(edits.contains("ADD_NOTE track=0 ch=0 key=36 start=0 dur=96 vel=90"));
        assert!(text.contains("key=36 name=C2"));
    }

    #[test]
    fn renders_chord_names() {
        let text = render_chords(ONE_CHORD_MIDI, &ChordsOptions::default()).unwrap();

        assert!(text.contains("MIDY_CHORDS v1"));
        assert!(text.contains("name=Cmaj"));
        assert!(text.contains("notes=C4,E4,G4"));
    }

    #[test]
    fn arpeggiates_block_chords() {
        let arpeggiated = apply_edits(ONE_CHORD_MIDI, "ARPEGGIATE grid=24 order=up\n").unwrap();
        let text = render_timeline(&arpeggiated).unwrap();

        assert!(text.contains("key=60 name=C4 start=0"));
        assert!(text.contains("key=64 name=E4 start=24"));
        assert!(text.contains("key=67 name=G4 start=48"));
    }

    #[test]
    fn chordizes_single_notes() {
        let chordized = apply_edits(ONE_NOTE_MIDI, "CHORDIZE quality=maj\n").unwrap();
        let text = render_timeline(&chordized).unwrap();
        let note_lines = text
            .lines()
            .filter(|line| line.starts_with("NOTE "))
            .collect::<Vec<_>>();

        assert_eq!(note_lines.len(), 3);
        assert!(text.contains("key=60 name=C4"));
        assert!(text.contains("key=64 name=E4"));
        assert!(text.contains("key=67 name=G4"));
    }

    #[test]
    fn chordizes_with_custom_intervals() {
        let chordized = apply_edits(ONE_NOTE_MIDI, "CHORDIZE intervals=0,3,7,10\n").unwrap();
        let text = render_timeline(&chordized).unwrap();

        assert!(text.contains("key=60 name=C4"));
        assert!(text.contains("key=63 name=D#4"));
        assert!(text.contains("key=67 name=G4"));
        assert!(text.contains("key=70 name=A#4"));
    }

    #[test]
    fn blocks_arpeggios_to_grid_start() {
        let arpeggiated = apply_edits(ONE_CHORD_MIDI, "ARPEGGIATE grid=24 order=up\n").unwrap();
        let blocked = apply_edits(&arpeggiated, "BLOCK_CHORD grid=96\n").unwrap();
        let text = render_timeline(&blocked).unwrap();

        assert!(text.contains("key=60 name=C4 start=0"));
        assert!(text.contains("key=64 name=E4 start=0"));
        assert!(text.contains("key=67 name=G4 start=0"));
    }

    #[test]
    fn inverts_chords() {
        let inverted = apply_edits(ONE_CHORD_MIDI, "INVERT_CHORDS inversion=1\n").unwrap();
        let text = render_timeline(&inverted).unwrap();

        assert!(
            !text
                .lines()
                .any(|line| line.starts_with("NOTE ") && line.contains("key=60"))
        );
        assert!(text.contains("key=64 name=E4"));
        assert!(text.contains("key=67 name=G4"));
        assert!(text.contains("key=72 name=C5"));
    }

    #[test]
    fn doubles_notes_by_octave() {
        let doubled = apply_edits(ONE_NOTE_MIDI, "DOUBLE octave=-1\n").unwrap();
        let text = render_timeline(&doubled).unwrap();
        let note_lines = text
            .lines()
            .filter(|line| line.starts_with("NOTE "))
            .collect::<Vec<_>>();

        assert_eq!(note_lines.len(), 2);
        assert!(
            note_lines
                .iter()
                .any(|line| line.contains("key=60 name=C4"))
        );
        assert!(
            note_lines
                .iter()
                .any(|line| line.contains("key=48 name=C3"))
        );
    }

    #[test]
    fn voice_leads_chords_by_octave() {
        let chords = apply_edits(
            EMPTY_SINGLE_TRACK_MIDI,
            "\
ADD_NOTE track=0 ch=0 key=60 start=0 dur=48 vel=64
ADD_NOTE track=0 ch=0 key=64 start=0 dur=48 vel=64
ADD_NOTE track=0 ch=0 key=67 start=0 dur=48 vel=64
ADD_NOTE track=0 ch=0 key=67 start=96 dur=48 vel=64
ADD_NOTE track=0 ch=0 key=71 start=96 dur=48 vel=64
ADD_NOTE track=0 ch=0 key=74 start=96 dur=48 vel=64
",
        )
        .unwrap();
        let voiced = apply_edits(&chords, "VOICE_LEAD max_jump=7\n").unwrap();
        let text = render_timeline(&voiced).unwrap();

        assert!(text.contains("key=55 name=G3 start=96"));
        assert!(text.contains("key=59 name=B3 start=96"));
        assert!(text.contains("key=62 name=D4 start=96"));
    }

    #[test]
    fn renders_ascii_roll() {
        let text = render_roll(ONE_CHORD_MIDI, &RollOptions::default()).unwrap();

        assert!(text.contains("MIDY_ROLL v1"));
        assert!(text.contains("  G4 | #"));
        assert!(text.contains("  C4 | #"));
    }

    #[test]
    fn renders_verbose_ascii_roll_cells() {
        let text = render_roll(
            ONE_CHORD_MIDI,
            &RollOptions {
                mode: RollMode::Verbose,
                ..RollOptions::default()
            },
        )
        .unwrap();

        assert!(text.contains("mode=verbose"));
        assert!(text.contains("CELL column=0 tick=0 pos=1:1:0 notes=C4:0:0,E4:0:0,G4:0:0"));
    }

    #[test]
    fn renders_analysis() {
        let text = render_analysis(ONE_CHORD_MIDI, Some("chord.mid")).unwrap();

        assert!(text.contains("ANALYZE file=chord.mid"));
        assert!(text.contains("track=0 role=chords"));
        assert!(text.contains("key_guess="));
        assert!(text.contains("chords:"));
    }

    #[test]
    fn renders_analysis_json() {
        let text = render_analysis_json(ONE_CHORD_MIDI, Some("chord.mid")).unwrap();

        assert!(text.contains("\"file\": \"chord.mid\""));
        assert!(text.contains("\"track_summaries\""));
        assert!(text.contains("\"role\": \"chords\""));
    }

    #[test]
    fn renders_tracks() {
        let text = render_tracks(ONE_CHORD_MIDI).unwrap();

        assert!(text.contains("MIDY_TRACKS v1 tracks=1"));
        assert!(text.contains("TRACK index=0 role=chords"));
        assert!(text.contains("channels=0"));
    }

    #[test]
    fn lints_duplicate_overlap_empty_and_stuck_notes() {
        let empty_lint = render_lint(EMPTY_SINGLE_TRACK_MIDI).unwrap();
        assert!(empty_lint.contains("WARN empty_track track=0"));

        let messy = apply_edits(
            EMPTY_SINGLE_TRACK_MIDI,
            "\
ADD_NOTE track=0 ch=0 key=60 start=0 dur=96 vel=80
ADD_NOTE track=0 ch=0 key=60 start=0 dur=96 vel=80
ADD_NOTE track=0 ch=0 key=60 start=48 dur=96 vel=80
",
        )
        .unwrap();
        let messy_lint = render_lint(&messy).unwrap();
        assert!(messy_lint.contains("WARN duplicate_note"));
        assert!(messy_lint.contains("WARN overlap"));

        let stuck_lint = render_lint(STUCK_NOTE_MIDI).unwrap();
        assert!(stuck_lint.contains("WARN stuck_note"));
    }

    #[test]
    fn fixes_duplicate_and_overlapping_notes() {
        let messy = apply_edits(
            EMPTY_SINGLE_TRACK_MIDI,
            "\
ADD_NOTE track=0 ch=0 key=60 start=0 dur=96 vel=80
ADD_NOTE track=0 ch=0 key=60 start=0 dur=96 vel=80
ADD_NOTE track=0 ch=0 key=60 start=48 dur=96 vel=80
",
        )
        .unwrap();
        let fixed = fix_midi(&messy).unwrap();
        let text = render_timeline(&fixed).unwrap();
        let note_lines = text
            .lines()
            .filter(|line| line.starts_with("NOTE "))
            .collect::<Vec<_>>();

        assert_eq!(note_lines.len(), 2);
        assert!(
            note_lines
                .iter()
                .any(|line| line.contains("start=0 dur=48"))
        );
        assert!(
            note_lines
                .iter()
                .any(|line| line.contains("start=48 dur=96"))
        );
        assert!(!render_lint(&fixed).unwrap().contains("WARN duplicate_note"));
    }

    #[test]
    fn fixes_stuck_note_on() {
        let fixed = fix_midi(STUCK_NOTE_MIDI).unwrap();
        let text = render_timeline(&fixed).unwrap();

        assert!(!text.lines().any(|line| line.starts_with("NOTE ")));
        assert!(!render_lint(&fixed).unwrap().contains("WARN stuck_note"));
    }

    #[test]
    fn can_close_stuck_note_on_with_missing_note_off() {
        let fixed = fix_midi_with_options(
            STUCK_NOTE_MIDI,
            FixOptions {
                stuck_note_mode: StuckNoteFixMode::Close,
                stuck_note_duration: 48,
                ..FixOptions::default()
            },
        )
        .unwrap();
        let text = render_timeline(&fixed).unwrap();

        assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=60 name=C4 start=0 dur=48"));
        assert!(!render_lint(&fixed).unwrap().contains("WARN stuck_note"));
    }

    #[test]
    fn can_remove_empty_tracks_during_fix() {
        let merged = merge_midi(&[EMPTY_SINGLE_TRACK_MIDI, ONE_NOTE_MIDI]).unwrap();
        let fixed = fix_midi_with_options(
            &merged,
            FixOptions {
                remove_empty_tracks: true,
                ..FixOptions::default()
            },
        )
        .unwrap();
        let text = render_timeline(&fixed).unwrap();

        assert!(text.contains("HEADER format=single timing=metrical ticks_per_beat=96 tracks=1"));
        assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=60"));
    }

    #[test]
    fn mutes_selected_channel() {
        let rewritten = mute_selection(
            TWO_CHANNEL_MIDI,
            &TrackChannelSelector {
                track: None,
                channel: Some(0),
            },
        )
        .unwrap();
        let text = render_timeline(&rewritten).unwrap();
        let note_lines = text
            .lines()
            .filter(|line| line.starts_with("NOTE "))
            .collect::<Vec<_>>();

        assert!(!note_lines.iter().any(|line| line.contains("key=60")));
        assert!(note_lines.iter().any(|line| line.contains("key=64")));
    }

    #[test]
    fn applies_mute_and_solo_edit_commands() {
        let muted = apply_edits(TWO_CHANNEL_MIDI, "MUTE ch=0\n").unwrap();
        let muted_text = render_timeline(&muted).unwrap();
        let muted_lines = muted_text
            .lines()
            .filter(|line| line.starts_with("NOTE "))
            .collect::<Vec<_>>();

        assert!(!muted_lines.iter().any(|line| line.contains("key=60")));
        assert!(muted_lines.iter().any(|line| line.contains("key=64")));

        let soloed = apply_edits(TWO_CHANNEL_MIDI, "SOLO ch=1\n").unwrap();
        let soloed_text = render_timeline(&soloed).unwrap();
        let soloed_lines = soloed_text
            .lines()
            .filter(|line| line.starts_with("NOTE "))
            .collect::<Vec<_>>();

        assert!(!soloed_lines.iter().any(|line| line.contains("key=60")));
        assert!(soloed_lines.iter().any(|line| line.contains("key=64")));
    }

    #[test]
    fn applies_move_track_and_set_channel_edit_commands() {
        let merged = merge_midi(&[ONE_NOTE_MIDI, ONE_CHORD_MIDI]).unwrap();
        let moved = apply_edits(&merged, "MOVE_TRACK from=1 to=0 key=67\n").unwrap();
        let moved_text = render_timeline(&moved).unwrap();

        assert!(
            moved_text
                .lines()
                .any(|line| line.starts_with("NOTE ") && line.contains("track=0 ch=0 key=67"))
        );
        assert!(
            !moved_text
                .lines()
                .any(|line| { line.starts_with("NOTE ") && line.contains("track=1 ch=0 key=67") })
        );

        let changed =
            apply_edits(TWO_CHANNEL_MIDI, "SET_CHANNEL track=0 ch=2 from_ch=1\n").unwrap();
        let changed_text = render_timeline(&changed).unwrap();
        assert!(
            changed_text
                .lines()
                .any(|line| line.starts_with("NOTE ") && line.contains("ch=2 key=64"))
        );
    }

    #[test]
    fn extracts_selected_channel() {
        let rewritten = extract_selection(
            TWO_CHANNEL_MIDI,
            &TrackChannelSelector {
                track: None,
                channel: Some(1),
            },
        )
        .unwrap();
        let text = render_timeline(&rewritten).unwrap();
        let note_lines = text
            .lines()
            .filter(|line| line.starts_with("NOTE "))
            .collect::<Vec<_>>();

        assert!(!note_lines.iter().any(|line| line.contains("key=60")));
        assert!(note_lines.iter().any(|line| line.contains("key=64")));
    }

    #[test]
    fn splits_by_channel() {
        let outputs = split_selection(TWO_CHANNEL_MIDI, SplitMode::Channel).unwrap();

        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].0, "ch-0.mid");
        assert_eq!(outputs[1].0, "ch-1.mid");
        let ch0 = render_timeline(&outputs[0].1).unwrap();
        let ch1 = render_timeline(&outputs[1].1).unwrap();
        assert!(
            ch0.lines()
                .any(|line| line.starts_with("NOTE ") && line.contains("key=60"))
        );
        assert!(
            ch1.lines()
                .any(|line| line.starts_with("NOTE ") && line.contains("key=64"))
        );
    }

    #[test]
    fn merges_midi_files_as_parallel_tracks() {
        let merged = merge_midi(&[ONE_NOTE_MIDI, ONE_CHORD_MIDI]).unwrap();
        let text = render_timeline(&merged).unwrap();

        assert!(text.contains("HEADER format=parallel timing=metrical ticks_per_beat=96 tracks=2"));
        assert!(text.contains("TRACK index=0"));
        assert!(text.contains("TRACK index=1"));
        assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=60"));
        assert!(text.contains("NOTE id=t1n0 track=1 ch=0 key=60"));
        assert!(text.contains("NOTE id=t1n2 track=1 ch=0 key=67"));
    }
}
