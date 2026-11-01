//! Parsing edit text into commands.

use crate::midi::format::hex_decode;
use crate::midi::model::{ConcreteNote, DRUM_CHANNEL};
use crate::midi::timeline::EVENT_KINDS;
use crate::text::closest_match;
use crate::{Error, Result};

use super::command::{
    ArpeggioOrder, DrumMode, EditCommand, EventFilter, LineCommand, NewEvent, NoteFilter,
    NotePatch, NoteRow, QuantizeMode, Ratio, TextKind, VelocityCommand,
};
use super::fields::{EditContext, Fields, normalize_key, tokenize};

const COMMANDS: &[&str] = &[
    "ADD_NOTE",
    "SET_NOTE",
    "DELETE_NOTE",
    "DELETE_NOTES",
    "TRANSPOSE",
    "SHIFT",
    "SCALE_TIME",
    "SCALE_DURATION",
    "QUANTIZE",
    "HUMANIZE",
    "DEHUMANIZE",
    "SWING",
    "VELOCITY",
    "CRESCENDO",
    "CHORDIZE",
    "ARPEGGIATE",
    "BLOCK_CHORD",
    "INVERT_CHORDS",
    "DOUBLE",
    "VOICE_LEAD",
    "MUTE",
    "SOLO",
    "MOVE_TRACK",
    "SET_CHANNEL",
    "SET_TEMPO",
    "SET_PROGRAM",
    "ADD_EVENT",
    "DELETE_EVENT",
    "DELETE_EVENTS",
    "ADD_TRACK",
    "SET_TRACK_NAME",
];

const MAX_TIMING_TICKS: u64 = 0x0fff_ffff;

/// Edit text after parsing.
#[derive(Debug, Default)]
pub(super) struct ParsedEdits {
    pub(super) commands: Vec<LineCommand>,
    pub(super) timeline: bool,
    pub(super) scope: Option<NoteFilter>,
    pub(super) note_ids: Vec<String>,
    pub(super) event_lines: Vec<(usize, String)>,
}

pub(super) fn parse_edit_text(edits: &str, context: EditContext<'_>) -> Result<ParsedEdits> {
    let mut parsed = ParsedEdits::default();

    for (index, raw_line) in edits.lines().enumerate() {
        let line_no = index + 1;
        let line = raw_line.trim_start_matches('\u{feff}').trim();
        let Some(first) = line.split_whitespace().next() else {
            continue;
        };
        if first.starts_with('#') || first.starts_with("```") {
            continue;
        }

        let keyword = normalize_key(first);
        match keyword.as_str() {
            "midy_timeline" => {
                parsed.timeline = true;
                continue;
            }
            "header" | "song" | "track" | "stuck" => continue,
            "meta" | "event" => {
                parsed.event_lines.push((line_no, line.to_owned()));
                continue;
            }
            _ => {}
        }

        let label = format!("edit line {line_no}");
        let tokens = tokenize(line).map_err(|error| Error::Edit(format!("{label}: {error}")))?;
        let fields = Fields::from_tokens(label, &tokens[1..], context)?;

        if keyword == "scope" {
            parsed.scope = Some(fields.filter(FilterSpec::scope())?);
            fields.finish("SCOPE")?;
            continue;
        }

        let (name, command) = parse_command(&keyword, &fields)?;
        fields.finish(name)?;
        if let EditCommand::Note { id, .. } = &command {
            parsed.note_ids.push(id.clone());
        }
        parsed.commands.push(LineCommand {
            line: line_no,
            name,
            command,
        });
    }

    Ok(parsed)
}

/// Parses a scope such as `track=1 start=0 end=960`.
pub(super) fn parse_scope(text: &str, context: EditContext<'_>) -> Result<NoteFilter> {
    let tokens = text
        .split(|character: char| character.is_whitespace() || character == ';')
        .filter(|token| !token.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let fields = Fields::from_tokens("scope".to_owned(), &tokens, context)?;
    let filter = fields.filter(FilterSpec::scope())?;
    fields.finish("scope")?;
    Ok(filter)
}

fn parse_command(keyword: &str, fields: &Fields<'_>) -> Result<(&'static str, EditCommand)> {
    let timing = FilterSpec::notes(DrumMode::Include);
    let pitch = FilterSpec::notes(DrumMode::Exclude);

    let parsed = match keyword {
        "add" | "add_note" => ("ADD_NOTE", EditCommand::Add(parse_add_note(fields)?)),
        "note" => {
            let id = fields
                .id()
                .map_err(|_| fields.error("NOTE needs id=...; use ADD_NOTE to create a note"))?;
            fields.ignore(&["id"]);
            (
                "NOTE",
                EditCommand::Note {
                    id,
                    row: parse_note_row(fields)?,
                },
            )
        }
        "del" | "delete" | "delete_note" | "del_note" => {
            ("DELETE_NOTE", EditCommand::Delete { id: fields.id()? })
        }
        "set" | "change" | "set_note" | "change_note" => (
            "SET_NOTE",
            EditCommand::Set {
                id: fields.id()?,
                patch: parse_note_patch(fields)?,
            },
        ),
        "delete_notes" | "del_notes" | "delete_range" => (
            "DELETE_NOTES",
            EditCommand::DeleteMatching {
                filter: destructive_filter(fields, "DELETE_NOTES")?,
            },
        ),
        "transpose" | "transpose_notes" => (
            "TRANSPOSE",
            EditCommand::Transpose {
                semitones: required(
                    fields,
                    fields.i16(&["semitones", "semi", "by"])?,
                    "semitones",
                )?,
                filter: fields.filter(pitch)?,
            },
        ),
        "shift" | "shift_notes" | "move" | "move_notes" => {
            let filter = fields.filter(timing)?;
            (
                "SHIFT",
                EditCommand::Shift {
                    ticks: parse_shift(fields, &filter)?,
                    notes_only: fields.boolean(&["notes_only"])?.unwrap_or(false),
                    filter,
                },
            )
        }
        "scale_time" | "stretch" | "stretch_notes" => (
            "SCALE_TIME",
            EditCommand::ScaleTime {
                factor: parse_ratio(fields, "factor")?,
                notes_only: fields.boolean(&["notes_only"])?.unwrap_or(false),
                filter: fields.filter(timing)?,
            },
        ),
        "scale_duration" | "scale_length" | "length" | "stretch_duration" => (
            "SCALE_DURATION",
            EditCommand::ScaleDuration {
                factor: parse_ratio(fields, "factor")?,
                filter: fields.filter(timing)?,
            },
        ),
        "quantize" | "quantize_notes" => ("QUANTIZE", parse_quantize(fields, timing)?),
        "dehumanize" | "dehumanise" => ("DEHUMANIZE", parse_quantize(fields, timing)?),
        "humanize" | "humanise" => {
            let timing_ticks = fields.u64(&["timing"])?.unwrap_or(12);
            if timing_ticks > MAX_TIMING_TICKS {
                return Err(fields.error("field 'timing' is too large"));
            }
            (
                "HUMANIZE",
                EditCommand::Humanize {
                    timing: timing_ticks,
                    velocity: fields.u7(&["velocity", "vel"])?.unwrap_or(8),
                    seed: fields.u64(&["seed"])?.unwrap_or(1),
                    filter: fields.filter(timing)?,
                },
            )
        }
        "swing" => {
            let filter = fields.filter(timing)?;
            (
                "SWING",
                EditCommand::Swing {
                    amount: parse_swing_amount(fields)?,
                    grid: parse_grid(fields, &filter)?,
                    filter,
                },
            )
        }
        "velocity" | "velocities" | "dynamics" => (
            "VELOCITY",
            EditCommand::Velocity {
                command: parse_velocity_command(fields)?,
                filter: fields.filter(timing)?,
            },
        ),
        "crescendo" | "velocity_ramp" => (
            "CRESCENDO",
            EditCommand::Crescendo {
                start_velocity: required(
                    fields,
                    fields.u7(&["start_vel", "from_vel"])?,
                    "start_vel",
                )?,
                end_velocity: required(fields, fields.u7(&["end_vel", "to_vel"])?, "end_vel")?,
                filter: fields.filter(timing)?,
            },
        ),
        "chordize" | "chordise" | "harmonize" | "harmonise" => (
            "CHORDIZE",
            EditCommand::Chordize {
                intervals: parse_chordize_intervals(fields)?,
                filter: fields.filter(pitch)?,
            },
        ),
        "arpeggiate" | "arpeggio" | "arp" => {
            let filter = fields.filter(pitch)?;
            (
                "ARPEGGIATE",
                EditCommand::Arpeggiate {
                    grid: parse_grid(fields, &filter)?,
                    order: parse_arpeggio_order(fields)?,
                    filter,
                },
            )
        }
        "block_chord" | "block_chords" | "block" => {
            let filter = fields.filter(pitch)?;
            (
                "BLOCK_CHORD",
                EditCommand::BlockChord {
                    grid: parse_grid(fields, &filter)?,
                    filter,
                },
            )
        }
        "invert_chord" | "invert_chords" | "invert" => (
            "INVERT_CHORDS",
            EditCommand::InvertChords {
                inversion: fields.i16(&["inversion", "by"])?.unwrap_or(1),
                filter: fields.filter(pitch)?,
            },
        ),
        "double" | "double_octave" => (
            "DOUBLE",
            EditCommand::Double {
                octave: required(fields, fields.i16(&["octave", "oct"])?, "octave")?,
                filter: fields.filter(pitch)?,
            },
        ),
        "voice_lead" | "voicelead" | "voice_leading" => (
            "VOICE_LEAD",
            EditCommand::VoiceLead {
                max_jump: fields.u7(&["max_jump", "max"])?.unwrap_or(7),
                filter: fields.filter(pitch)?,
            },
        ),
        "mute" | "mute_notes" => (
            "MUTE",
            EditCommand::DeleteMatching {
                filter: destructive_filter(fields, "MUTE")?,
            },
        ),
        "solo" | "solo_notes" => (
            "SOLO",
            EditCommand::Solo {
                filter: destructive_filter(fields, "SOLO")?,
            },
        ),
        "move_track" | "move_to_track" => {
            let from = required(fields, fields.usize(&["from"])?, "from")?;
            let to = required(fields, fields.usize(&["to"])?, "to")?;
            if fields.usize(&["track"])?.is_some_and(|track| track != from) {
                return Err(fields.error("MOVE_TRACK track filter must match from=..."));
            }
            let mut filter = fields.filter(FilterSpec::fixed_range(&["ch", "channel"]))?;
            filter.track = Some(from);
            ("MOVE_TRACK", EditCommand::MoveTrack { from, to, filter })
        }
        "set_channel" | "set_ch" | "channel" => (
            "SET_CHANNEL",
            EditCommand::SetChannel {
                channel: required(
                    fields,
                    fields.channel(&["ch", "channel", "to_ch", "to_channel"])?,
                    "ch",
                )?,
                filter: fields.filter(FilterSpec::fixed_range(&["from_ch", "from_channel"]))?,
            },
        ),
        "set_tempo" | "tempo" => (
            "SET_TEMPO",
            EditCommand::SetTempo {
                micros: parse_tempo(fields)?,
                tick: fields.time(&["tick", "start", "at", "pos"])?.unwrap_or(0),
            },
        ),
        "set_program" | "program" => (
            "SET_PROGRAM",
            EditCommand::SetProgram {
                track: required(fields, fields.usize(&["track"])?, "track")?,
                channel: required(fields, fields.channel(&["ch", "channel"])?, "ch")?,
                program: required(fields, fields.u7(&["program"])?, "program")?,
                tick: fields.time(&["tick", "start", "at", "pos"])?.unwrap_or(0),
            },
        ),
        "add_event" => {
            let (track, tick, event) = parse_event(fields)?;
            ("ADD_EVENT", EditCommand::AddEvent { track, tick, event })
        }
        "delete_event" | "del_event" => {
            let (track, tick, event) = parse_event(fields)?;
            (
                "DELETE_EVENT",
                EditCommand::DeleteEvent { track, tick, event },
            )
        }
        "delete_events" | "del_events" => (
            "DELETE_EVENTS",
            EditCommand::DeleteEvents {
                filter: parse_event_filter(fields)?,
            },
        ),
        "add_track" => (
            "ADD_TRACK",
            EditCommand::AddTrack {
                name: fields.text("name")?,
            },
        ),
        "set_track_name" | "rename_track" => (
            "SET_TRACK_NAME",
            EditCommand::SetTrackName {
                track: required(fields, fields.usize(&["track"])?, "track")?,
                name: fields.text("name")?.ok_or_else(|| fields.missing("name"))?,
            },
        ),
        unknown => {
            let upper = unknown.to_ascii_uppercase();
            let hint = closest_match(&upper, COMMANDS.iter().copied())
                .map(|name| format!("did you mean {name}? "))
                .unwrap_or_default();
            return Err(fields.error(format!(
                "unknown command '{upper}'; {hint}expected one of {}",
                COMMANDS.join(", ")
            )));
        }
    };
    Ok(parsed)
}

fn required<T>(fields: &Fields<'_>, value: Option<T>, key: &str) -> Result<T> {
    value.ok_or_else(|| fields.missing(key))
}

pub(super) fn parse_add_note(fields: &Fields<'_>) -> Result<ConcreteNote> {
    let start = required(fields, fields.time(&["start", "at", "pos"])?, "start")?;
    let duration = required(
        fields,
        fields.span(&["dur", "duration", "len", "length"])?,
        "dur",
    )?;
    Ok(ConcreteNote {
        track: required(fields, fields.usize(&["track"])?, "track")?,
        channel: required(fields, fields.channel(&["ch", "channel"])?, "ch")?,
        key: required(fields, fields.key(&["key"])?, "key")?,
        start,
        duration: fields.resolve_span(duration, start)?,
        velocity: fields.velocity(&["vel", "velocity"])?.unwrap_or(64),
        off_velocity: fields.u7(&["off_vel", "off_velocity"])?.unwrap_or(64),
    })
}

fn parse_note_patch(fields: &Fields<'_>) -> Result<NotePatch> {
    let patch = NotePatch {
        track: fields.usize(&["track"])?,
        channel: fields.channel(&["ch", "channel"])?,
        key: fields.key(&["key"])?,
        start: fields.time(&["start", "at", "pos"])?,
        duration: fields.span(&["dur", "duration", "len", "length"])?,
        velocity: fields.velocity(&["vel", "velocity"])?,
        off_velocity: fields.u7(&["off_vel", "off_velocity"])?,
    };
    if patch == NotePatch::default() {
        return Err(fields.error("SET_NOTE must change at least one field"));
    }
    Ok(patch)
}

pub(super) fn parse_note_row(fields: &Fields<'_>) -> Result<NoteRow> {
    Ok(NoteRow {
        track: fields.usize(&["track"])?,
        channel: fields.channel(&["ch", "channel"])?,
        key: fields.key(&["key"])?,
        name: fields.key(&["name"])?,
        start: fields.time(&["start"])?,
        pos: fields.time(&["pos", "at"])?,
        duration: fields.span(&["dur", "duration", "len", "length"])?,
        end: fields.time(&["end"])?,
        end_pos: fields.time(&["end_pos"])?,
        velocity: fields.velocity(&["vel", "velocity"])?,
        off_velocity: fields.u7(&["off_vel", "off_velocity"])?,
    })
}

/// Field names a note filter reads.
#[derive(Debug, Clone, Copy)]
pub(super) struct FilterSpec {
    channel_keys: &'static [&'static str],
    range_aliases: bool,
    drums: DrumMode,
}

impl FilterSpec {
    const fn notes(drums: DrumMode) -> Self {
        Self {
            channel_keys: &["ch", "channel"],
            range_aliases: true,
            drums,
        }
    }

    const fn scope() -> Self {
        Self::notes(DrumMode::Include)
    }

    const fn fixed_range(channel_keys: &'static [&'static str]) -> Self {
        Self {
            channel_keys,
            range_aliases: false,
            drums: DrumMode::Include,
        }
    }
}

impl Fields<'_> {
    fn filter(&self, spec: FilterSpec) -> Result<NoteFilter> {
        let start_keys: &[&str] = if spec.range_aliases {
            &["from", "start", "at", "pos"]
        } else {
            &["start", "at", "pos"]
        };
        let end_keys: &[&str] = if spec.range_aliases {
            &["to", "end"]
        } else {
            &["end"]
        };

        let track = self.usize(&["track"])?;
        let channel = self.channel(spec.channel_keys)?;
        let key = self.key(&["key"])?;
        let bars = self.bar_range(&["bar", "bars"])?;
        let start = self.time(start_keys)?;
        let end = self.time(end_keys)?;
        if bars.is_some() && (start.is_some() || end.is_some()) {
            return Err(self.error("use either bar/bars or start/end, not both"));
        }
        let start = start.or(bars.map(|(start, _)| start));
        let end = end.or(bars.map(|(_, end)| end));
        if let (Some(start), Some(end)) = (start, end)
            && start > end
        {
            return Err(self.error("filter start/from must be <= end/to"));
        }

        let drums = match self.raw("drums") {
            Some("true" | "yes" | "include") => DrumMode::Include,
            Some("false" | "no" | "exclude") => DrumMode::Exclude,
            Some("only") => DrumMode::Only,
            Some(value) => {
                return Err(self.error(format!(
                    "field 'drums' must be true, false, or only; got '{value}'"
                )));
            }
            None if channel == Some(DRUM_CHANNEL) => DrumMode::Include,
            None => spec.drums,
        };

        Ok(NoteFilter {
            track,
            channel,
            key,
            start,
            end,
            drums,
        })
    }
}

fn destructive_filter(fields: &Fields<'_>, command: &str) -> Result<NoteFilter> {
    let filter = fields.filter(FilterSpec::notes(DrumMode::Include))?;
    let everything = fields.boolean(&["all"])?.unwrap_or(false);
    fields.finish(command)?;
    if filter.is_whole_song() && filter.drums == DrumMode::Include && !everything {
        return Err(fields.error(format!(
            "{command} with no filter would affect every note; add a filter or all=true"
        )));
    }
    Ok(filter)
}

fn parse_ratio(fields: &Fields<'_>, key: &str) -> Result<Ratio> {
    let value = fields.raw(key).ok_or_else(|| fields.missing(key))?;
    ratio_value(fields, key, value)
}

fn ratio_value(fields: &Fields<'_>, key: &str, value: &str) -> Result<Ratio> {
    let integer = |text: &str| {
        text.parse::<u64>()
            .map_err(|_| fields.error(format!("field '{key}' must be a positive ratio or decimal")))
    };
    let (numerator, denominator) = if let Some((numerator, denominator)) = value.split_once('/') {
        (integer(numerator)?, integer(denominator)?)
    } else if let Some((whole, fraction)) = value.split_once('.') {
        if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(fields.error(format!("field '{key}' must be a positive ratio or decimal")));
        }
        let denominator = 10_u64
            .checked_pow(fraction.len() as u32)
            .ok_or_else(|| fields.error(format!("field '{key}' decimal is too precise")))?;
        let numerator = integer(whole)?
            .checked_mul(denominator)
            .and_then(|whole| whole.checked_add(integer(fraction).ok()?))
            .ok_or_else(|| fields.error(format!("field '{key}' is too large")))?;
        (numerator, denominator)
    } else {
        (integer(value)?, 1)
    };

    Ratio::new(numerator, denominator)
        .ok_or_else(|| fields.error(format!("field '{key}' must be greater than zero")))
}

fn parse_shift(fields: &Fields<'_>, filter: &NoteFilter) -> Result<i64> {
    let (negative, span) = required(fields, fields.signed_span(&["ticks", "by"])?, "ticks")?;
    let magnitude = fields.resolve_span(span, filter.start.unwrap_or(0))?;
    let magnitude = i64::try_from(magnitude)
        .map_err(|_| fields.error("shift is too large for a signed tick value"))?;
    Ok(if negative { -magnitude } else { magnitude })
}

fn parse_grid(fields: &Fields<'_>, filter: &NoteFilter) -> Result<u64> {
    let span = required(fields, fields.span(&["grid"])?, "grid")?;
    let grid = fields.resolve_span(span, filter.start.unwrap_or(0))?;
    if grid == 0 {
        return Err(fields.error("field 'grid' must be greater than zero"));
    }
    Ok(grid)
}

fn parse_quantize(fields: &Fields<'_>, spec: FilterSpec) -> Result<EditCommand> {
    let filter = fields.filter(spec)?;
    let mode = match fields.raw("mode").unwrap_or("start") {
        "start" | "starts" => QuantizeMode::Start,
        "dur" | "duration" | "length" => QuantizeMode::Duration,
        "both" | "start_end" | "start_duration" => QuantizeMode::Both,
        value => {
            return Err(fields.error(format!(
                "field 'mode' must be start, duration, or both; got '{value}'"
            )));
        }
    };
    Ok(EditCommand::Quantize {
        grid: parse_grid(fields, &filter)?,
        mode,
        filter,
    })
}

fn parse_swing_amount(fields: &Fields<'_>) -> Result<u8> {
    let amount = fields.u7(&["amount"])?.unwrap_or(55);
    if (50..=100).contains(&amount) {
        Ok(amount)
    } else {
        Err(fields.error("field 'amount' must be in 50..100 for delayed swing"))
    }
}

fn parse_arpeggio_order(fields: &Fields<'_>) -> Result<ArpeggioOrder> {
    match fields.raw("order").unwrap_or("up") {
        "up" | "asc" | "ascending" => Ok(ArpeggioOrder::Up),
        "down" | "desc" | "descending" => Ok(ArpeggioOrder::Down),
        "updown" | "up_down" | "up-down" | "bounce" => Ok(ArpeggioOrder::UpDown),
        "outside" | "outside_in" | "outside-in" => Ok(ArpeggioOrder::Outside),
        value => Err(fields.error(format!(
            "field 'order' must be up, down, updown, or outside; got '{value}'"
        ))),
    }
}

fn parse_chordize_intervals(fields: &Fields<'_>) -> Result<Vec<i16>> {
    let intervals = fields.raw("intervals").or_else(|| fields.raw("ints"));
    if let Some(value) = intervals {
        let intervals = value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                value
                    .parse::<i16>()
                    .map_err(|_| fields.error("field 'intervals' must be signed integers"))
            })
            .collect::<Result<Vec<_>>>()?;
        if intervals.is_empty() {
            return Err(fields.error("field 'intervals' must not be empty"));
        }
        return Ok(intervals);
    }

    let quality = fields
        .raw("quality")
        .or_else(|| fields.raw("chord"))
        .unwrap_or("maj");
    let intervals: &[i16] = match quality {
        "maj" | "major" => &[0, 4, 7],
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
            return Err(fields.error(format!(
                "field 'quality' must be maj, min, dim, aug, sus2, sus4, power, maj7, min7, dom7, minmaj7, add9, or use intervals=0,4,7; got '{value}'"
            )));
        }
    };
    Ok(intervals.to_vec())
}

fn parse_velocity_command(fields: &Fields<'_>) -> Result<VelocityCommand> {
    let mut commands = Vec::new();
    if let Some(value) = fields.raw("scale") {
        commands.push(VelocityCommand::Scale(ratio_value(fields, "scale", value)?));
    }
    if let Some(value) = fields.i16(&["add"])? {
        commands.push(VelocityCommand::Add(value));
    }
    if let Some(value) = fields.velocity(&["set"])? {
        commands.push(VelocityCommand::Set(value));
    }
    if let Some(value) = fields.raw("compress") {
        commands.push(VelocityCommand::Compress {
            factor: ratio_value(fields, "compress", value)?,
            center: fields.u7(&["center"])?.unwrap_or(80),
        });
    }
    match commands.as_slice() {
        [command] => Ok(*command),
        [] => Err(fields.error("VELOCITY needs exactly one of scale=, add=, set=, or compress=")),
        _ => Err(fields.error("VELOCITY accepts only one operation: scale, add, set, or compress")),
    }
}

fn parse_tempo(fields: &Fields<'_>) -> Result<u32> {
    let micros = fields.u64(&["us_per_quarter"])?;
    let bpm = fields
        .raw("bpm")
        .map(|value| {
            value
                .parse::<f64>()
                .ok()
                .filter(|bpm| bpm.is_finite() && *bpm > 0.0)
                .ok_or_else(|| fields.error("field 'bpm' must be a positive number"))
        })
        .transpose()?;
    let micros = match (micros, bpm) {
        (Some(micros), _) => micros,
        (None, Some(bpm)) => (60_000_000.0 / bpm).round() as u64,
        (None, None) => return Err(fields.missing("bpm")),
    };
    if !(1..=0x00ff_ffff).contains(&micros) {
        return Err(fields.error("tempo must be 1..16777215 microseconds per quarter note"));
    }
    Ok(micros as u32)
}

fn parse_event(fields: &Fields<'_>) -> Result<(usize, u64, NewEvent)> {
    let track = required(fields, fields.usize(&["track"])?, "track")?;
    let tick = required(
        fields,
        fields.time(&["tick", "start", "at", "pos"])?,
        "tick",
    )?;
    let kind = fields.raw("kind").ok_or_else(|| fields.missing("kind"))?;
    let channel = || required(fields, fields.channel(&["ch", "channel"])?, "ch");
    let u7 = |key: &str| required(fields, fields.u7(&[key])?, key);
    let hex = || -> Result<Vec<u8>> {
        fields.ignore(&["bytes"]);
        let value = fields.raw("hex").ok_or_else(|| fields.missing("hex"))?;
        hex_decode(value).map_err(|message| fields.error(format!("field 'hex' {message}")))
    };

    let event = match kind {
        "cc" | "controller" => NewEvent::Controller {
            channel: channel()?,
            controller: u7("controller")?,
            value: u7("value")?,
        },
        "pitch_bend" => {
            let value = required(fields, fields.i16(&["value"])?, "value")?;
            if !(-8192..=8191).contains(&value) {
                return Err(fields.error("field 'value' must be -8192..8191"));
            }
            NewEvent::PitchBend {
                channel: channel()?,
                value,
            }
        }
        "program_change" => NewEvent::ProgramChange {
            channel: channel()?,
            program: u7("program")?,
        },
        "channel_aftertouch" => NewEvent::ChannelAftertouch {
            channel: channel()?,
            pressure: u7("pressure")?,
        },
        "poly_aftertouch" => NewEvent::PolyAftertouch {
            channel: channel()?,
            key: required(fields, fields.key(&["key"])?, "key")?,
            pressure: u7("pressure")?,
        },
        "orphan_note_off" => NewEvent::OrphanOff {
            channel: channel()?,
            key: required(fields, fields.key(&["key"])?, "key")?,
            velocity: u7("vel")?,
        },
        "tempo" => NewEvent::Tempo {
            micros: parse_tempo(fields)?,
        },
        "time_signature" => parse_time_signature(fields)?,
        "key_signature" => {
            let sharps = required(fields, fields.i16(&["sharps"])?, "sharps")?;
            if !(-7..=7).contains(&sharps) {
                return Err(fields.error("field 'sharps' must be -7..7"));
            }
            NewEvent::KeySignature {
                sharps: sharps as i8,
                minor: fields.boolean(&["minor"])?.unwrap_or(false),
            }
        }
        "text" => text_event(fields, TextKind::Text)?,
        "copyright" => text_event(fields, TextKind::Copyright)?,
        "track_name" => text_event(fields, TextKind::TrackName)?,
        "instrument_name" => text_event(fields, TextKind::InstrumentName)?,
        "lyric" => text_event(fields, TextKind::Lyric)?,
        "marker" => text_event(fields, TextKind::Marker)?,
        "cue_point" => text_event(fields, TextKind::CuePoint)?,
        "program_name" => text_event(fields, TextKind::ProgramName)?,
        "device_name" => text_event(fields, TextKind::DeviceName)?,
        "sequence_number" => NewEvent::SequenceNumber {
            value: match fields.raw("value") {
                None | Some("-") => None,
                Some(value) => Some(
                    value
                        .parse::<u16>()
                        .map_err(|_| fields.error("field 'value' must be 0..65535"))?,
                ),
            },
        },
        "midi_channel" => NewEvent::MidiChannel {
            channel: channel()?,
        },
        "midi_port" => NewEvent::MidiPort { port: u7("port")? },
        "smpte_offset" => NewEvent::SmpteOffset {
            fps: u7("fps")?,
            hour: u7("hour")?,
            minute: u7("minute")?,
            second: u7("second")?,
            frame: u7("frame")?,
            subframe: u7("subframe")?,
        },
        "sequencer_specific" => NewEvent::SequencerSpecific { bytes: hex()? },
        "unknown" => NewEvent::UnknownMeta {
            kind: required(fields, fields.bounded(&["type"], 255)?, "type")?,
            bytes: hex()?,
        },
        "sysex" => NewEvent::SysEx { bytes: hex()? },
        "escape" => NewEvent::Escape { bytes: hex()? },
        other => {
            return Err(fields.error(format!("unsupported event kind '{other}'")));
        }
    };
    Ok((track, tick, event))
}

fn parse_time_signature(fields: &Fields<'_>) -> Result<NewEvent> {
    let numerator = required(fields, fields.bounded(&["numerator"], 255)?, "numerator")?;
    if numerator == 0 {
        return Err(fields.error("field 'numerator' must be greater than zero"));
    }
    let power = fields.bounded(&["denominator_power"], 255)?;
    let power = match (fields.u64(&["denominator"])?, power) {
        (Some(denominator), derived) => {
            if !denominator.is_power_of_two() || denominator > 1 << 31 {
                return Err(fields.error("field 'denominator' must be a power of two"));
            }
            let power = denominator.trailing_zeros() as u8;
            if derived.is_some_and(|derived| derived != power) {
                return Err(fields.error("fields 'denominator' and 'denominator_power' disagree"));
            }
            power
        }
        (None, Some(power)) => power,
        (None, None) => return Err(fields.missing("denominator")),
    };
    Ok(NewEvent::TimeSignature {
        numerator,
        power,
        clocks: fields.bounded(&["clocks_per_click"], 255)?.unwrap_or(24),
        thirtyseconds: fields
            .bounded(&["thirtyseconds_per_quarter"], 255)?
            .unwrap_or(8),
    })
}

fn text_event(fields: &Fields<'_>, kind: TextKind) -> Result<NewEvent> {
    Ok(NewEvent::Text {
        kind,
        bytes: fields.text("text")?.ok_or_else(|| fields.missing("text"))?,
    })
}

fn parse_event_filter(fields: &Fields<'_>) -> Result<EventFilter> {
    let bars = fields.bar_range(&["bar", "bars"])?;
    let start = fields.time(&["start", "from"])?;
    let end = fields.time(&["end", "to"])?;
    if bars.is_some() && (start.is_some() || end.is_some()) {
        return Err(fields.error("use either bar/bars or start/end, not both"));
    }
    let kind = fields
        .raw("kind")
        .map(|kind| if kind == "controller" { "cc" } else { kind });
    if let Some(kind) = kind
        && !EVENT_KINDS.contains(&kind)
    {
        let hint = closest_match(kind, EVENT_KINDS.iter().copied())
            .map(|name| format!("; did you mean '{name}'?"))
            .unwrap_or_default();
        return Err(fields.error(format!("unknown event kind '{kind}'{hint}")));
    }
    let filter = EventFilter {
        kind: kind.map(ToOwned::to_owned),
        controller: fields.u7(&["controller"])?,
        track: fields.usize(&["track"])?,
        channel: fields.channel(&["ch", "channel"])?,
        start: start.or(bars.map(|(start, _)| start)),
        end: end.or(bars.map(|(_, end)| end)),
    };
    let everything = fields.boolean(&["all"])?.unwrap_or(false);
    fields.finish("DELETE_EVENTS")?;
    if filter == EventFilter::default() && !everything {
        return Err(fields.error(
            "DELETE_EVENTS with no filter would remove every non-note event; add a filter or all=true",
        ));
    }
    Ok(filter)
}
