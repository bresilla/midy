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
  cat edits.txt | midy apply input.mid output.mid
  cat edits.txt | midy apply input.mid

Accepted edit lines:
  ADD_NOTE track=0 ch=0 key=60 start=0 dur=480 vel=96 [off_vel=64]
  NOTE id=t0n0 track=0 ch=0 key=62 start=0 dur=240 vel=96 off_vel=64
  SET_NOTE id=t0n0 key=62 start=0 dur=240 vel=96 off_vel=64
  DELETE_NOTE id=t0n0
  DELETE_NOTES [track=0] [ch=0] [key=60] [start=0] [end=960]
  TRANSPOSE semitones=2 [track=0] [ch=0] [key=60] [start=0] [end=960]
  SHIFT ticks=120 [track=0] [ch=0] [key=60] [start=0] [end=960]
  SCALE_TIME factor=2/1 [track=0] [ch=0] [key=60] [start=0] [end=960]
  SCALE_DURATION factor=1/2 [track=0] [ch=0] [key=60] [start=0] [end=960]
  QUANTIZE grid=120 mode=both [track=0] [ch=0] [key=60] [start=0] [end=960]

Notes:
  - time fields are integer ticks
  - ch is MIDI channel 0..15
  - key and velocities are MIDI values 0..127
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
    writeln!(out, "# SET_NOTE id=t0n0 key=62 dur=240").expect("writing to String cannot fail");
    writeln!(out, "# DELETE_NOTE id=t0n0").expect("writing to String cannot fail");
    writeln!(out, "# TRANSPOSE semitones=2 track=0 ch=0").expect("writing to String cannot fail");
    writeln!(out, "# SHIFT ticks=120 start=480 end=960").expect("writing to String cannot fail");
    writeln!(out, "# SCALE_TIME factor=2/1").expect("writing to String cannot fail");
    writeln!(out, "# SCALE_DURATION factor=1/2 key=60").expect("writing to String cannot fail");
    writeln!(out, "# QUANTIZE grid=120 mode=both").expect("writing to String cannot fail");
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
                ticks_per_beat
            ),
            note.velocity,
            note.off_velocity,
        )
        .expect("writing to String cannot fail");
    }

    Ok(out)
}

/// Applies ASCII edit commands to a standard MIDI file and returns rewritten MIDI bytes.
pub fn apply_edits(bytes: &[u8], edits: &str) -> Result<Vec<u8>> {
    let mut smf = parse_smf(bytes)?;
    let notes = collect_notes(&smf);
    let commands = parse_edit_commands(edits)?;
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

#[derive(Debug)]
struct TimedEvent<'a> {
    absolute_tick: u64,
    order: usize,
    kind: TrackEventKind<'a>,
}

fn parse_edit_commands(edits: &str) -> Result<Vec<EditCommand>> {
    let mut commands = Vec::new();

    for (index, raw_line) in edits.lines().enumerate() {
        let line_no = index + 1;
        let line = raw_line.split('#').next().unwrap_or("").trim();
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
            "add" | "add_note" => EditCommand::Add(parse_add_note(line_no, &fields)?),
            "note" => EditCommand::Set {
                id: parse_id(line_no, &fields, &positional)?,
                patch: parse_note_patch(line_no, &fields)?,
            },
            "del" | "delete" | "delete_note" | "del_note" => EditCommand::Delete {
                id: parse_id(line_no, &fields, &positional)?,
            },
            "set" | "change" | "set_note" | "change_note" => EditCommand::Set {
                id: parse_id(line_no, &fields, &positional)?,
                patch: parse_note_patch(line_no, &fields)?,
            },
            "delete_notes" | "del_notes" | "delete_range" => EditCommand::DeleteMatching {
                filter: parse_filter(line_no, &fields)?,
            },
            "transpose" | "transpose_notes" => EditCommand::Transpose {
                semitones: required_i16_alias(line_no, &fields, &["semitones", "semi", "by"])?,
                filter: parse_filter(line_no, &fields)?,
            },
            "shift" | "shift_notes" | "move" | "move_notes" => EditCommand::Shift {
                ticks: required_i64_alias(line_no, &fields, &["ticks", "by"])?,
                filter: parse_filter(line_no, &fields)?,
            },
            "scale_time" | "stretch" | "stretch_notes" => EditCommand::ScaleTime {
                factor: required_ratio(line_no, &fields, "factor")?,
                filter: parse_filter(line_no, &fields)?,
            },
            "scale_duration" | "scale_length" | "length" | "stretch_duration" => {
                EditCommand::ScaleDuration {
                    factor: required_ratio(line_no, &fields, "factor")?,
                    filter: parse_filter(line_no, &fields)?,
                }
            }
            "quantize" | "quantize_notes" => EditCommand::Quantize {
                grid: required_u64(line_no, &fields, "grid")?,
                mode: parse_quantize_mode(line_no, &fields)?,
                filter: parse_filter(line_no, &fields)?,
            },
            unknown => {
                return Err(edit_error(
                    line_no,
                    format!(
                        "unknown command '{unknown}'; expected ADD_NOTE, SET_NOTE, DELETE_NOTE, TRANSPOSE, SHIFT, SCALE_TIME, SCALE_DURATION, QUANTIZE, DELETE_NOTES"
                    ),
                ));
            }
        };
        commands.push(command);
    }

    Ok(commands)
}

fn parse_add_note(line_no: usize, fields: &HashMap<String, &str>) -> Result<ConcreteNote> {
    Ok(ConcreteNote {
        track: required_usize(line_no, fields, "track")?,
        channel: required_u4(line_no, fields, &["ch", "channel"])?,
        key: required_u7(line_no, fields, "key")?,
        start: required_u64(line_no, fields, "start")?,
        duration: required_u64_alias(line_no, fields, &["dur", "duration", "len", "length"])?,
        velocity: optional_u7_alias(line_no, fields, &["vel", "velocity"])?.unwrap_or(64),
        off_velocity: optional_u7_alias(line_no, fields, &["off_vel", "off_velocity"])?
            .unwrap_or(64),
    })
}

fn parse_note_patch(line_no: usize, fields: &HashMap<String, &str>) -> Result<NotePatchFields> {
    let patch = NotePatchFields {
        track: optional_usize(line_no, fields, "track")?,
        channel: optional_u4_alias(line_no, fields, &["ch", "channel"])?,
        key: optional_u7(line_no, fields, "key")?,
        start: optional_u64(line_no, fields, "start")?,
        duration: optional_u64_alias(line_no, fields, &["dur", "duration", "len", "length"])?,
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

fn parse_filter(line_no: usize, fields: &HashMap<String, &str>) -> Result<NoteFilter> {
    let filter = NoteFilter {
        track: optional_usize(line_no, fields, "track")?,
        channel: optional_u4_alias(line_no, fields, &["ch", "channel"])?,
        key: optional_u7(line_no, fields, "key")?,
        start: optional_u64_alias(line_no, fields, &["from", "start"])?,
        end: optional_u64_alias(line_no, fields, &["to", "end"])?,
    };

    if let (Some(start), Some(end)) = (filter.start, filter.end)
        && start > end
    {
        return Err(edit_error(line_no, "filter start/from must be <= end/to"));
    }

    Ok(filter)
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

fn required_u7(line_no: usize, fields: &HashMap<String, &str>, key: &str) -> Result<u8> {
    optional_u7(line_no, fields, key)?
        .ok_or_else(|| edit_error(line_no, format!("missing field '{key}'")))
}

fn optional_u7(line_no: usize, fields: &HashMap<String, &str>, key: &str) -> Result<Option<u8>> {
    fields
        .get(key)
        .map(|value| parse_u7(line_no, key, value))
        .transpose()
}

fn optional_u7_alias(
    line_no: usize,
    fields: &HashMap<String, &str>,
    keys: &[&str],
) -> Result<Option<u8>> {
    optional_alias(line_no, fields, keys, parse_u7)
}

fn required_u64(line_no: usize, fields: &HashMap<String, &str>, key: &str) -> Result<u64> {
    optional_u64(line_no, fields, key)?
        .ok_or_else(|| edit_error(line_no, format!("missing field '{key}'")))
}

fn optional_u64(line_no: usize, fields: &HashMap<String, &str>, key: &str) -> Result<Option<u64>> {
    fields
        .get(key)
        .map(|value| parse_u64(line_no, key, value))
        .transpose()
}

fn required_u64_alias(
    line_no: usize,
    fields: &HashMap<String, &str>,
    keys: &[&str],
) -> Result<u64> {
    optional_u64_alias(line_no, fields, keys)?
        .ok_or_else(|| edit_error(line_no, format!("missing field '{}'", keys[0])))
}

fn optional_u64_alias(
    line_no: usize,
    fields: &HashMap<String, &str>,
    keys: &[&str],
) -> Result<Option<u64>> {
    optional_alias(line_no, fields, keys, parse_u64)
}

fn required_i16_alias(
    line_no: usize,
    fields: &HashMap<String, &str>,
    keys: &[&str],
) -> Result<i16> {
    optional_alias(line_no, fields, keys, parse_i16)?
        .ok_or_else(|| edit_error(line_no, format!("missing field '{}'", keys[0])))
}

fn required_i64_alias(
    line_no: usize,
    fields: &HashMap<String, &str>,
    keys: &[&str],
) -> Result<i64> {
    optional_alias(line_no, fields, keys, parse_i64)?
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

fn parse_i64(line_no: usize, key: &str, value: &str) -> Result<i64> {
    value
        .parse::<i64>()
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

fn note_position_fields(start: u64, end: u64, ticks_per_beat: Option<u16>) -> String {
    let Some(ticks_per_beat) = ticks_per_beat else {
        return String::new();
    };

    format!(
        " pos={} end_pos={}",
        tick_position(start, ticks_per_beat),
        tick_position(end, ticks_per_beat),
    )
}

fn tick_position(tick: u64, ticks_per_beat: u16) -> String {
    let ticks_per_beat = u64::from(ticks_per_beat);
    let ticks_per_bar = ticks_per_beat * 4;
    let bar = tick / ticks_per_bar + 1;
    let tick_in_bar = tick % ticks_per_bar;
    let beat = tick_in_bar / ticks_per_beat + 1;
    let tick_in_beat = tick_in_bar % ticks_per_beat;

    format!("{bar}:{beat}:{tick_in_beat}")
}

fn note_name(key: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = i16::from(key) / 12 - 1;
    format!("{}{}", NAMES[usize::from(key % 12)], octave)
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

fn normalize_key(value: &str) -> String {
    value.to_ascii_lowercase().replace('-', "_")
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
}
