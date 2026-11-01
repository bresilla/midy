//! Running parsed commands against a file.

use std::collections::HashSet;

use midly::{
    Arena, Format, Fps, Header, MetaMessage, MidiMessage, PitchBend, Smf, SmpteTime,
    TrackEventKind,
    num::{u4, u7, u14, u24},
};

use crate::midi::format::ticks_per_beat;
use crate::midi::model::scan_notes;
use crate::midi::options::{
    ApplyOptions, ApplyOutput, CommandReport, EditFormat, EditReport, EventDetail, QueryOptions,
};
use crate::midi::parse_smf;
use crate::midi::time::TimeSignatureMap;
use crate::midi::timeline::{
    event_channel, event_controller, event_kind_name, timeline_event_lines,
};
use crate::{Error, Result};

use super::command::{EditCommand, EventFilter, LineCommand, NewEvent, NoteFilter, TextKind};
use super::fields::EditContext;
use super::parse::parse_edit_text;
use super::session::{EditSession, EditableEvent};
use super::structured::{detect_format, parse_rows};
use super::transform::{
    apply_arpeggiate, apply_block_chord, apply_crescendo, apply_humanize, apply_invert_chords,
    apply_swing, apply_velocity, apply_voice_lead, chord_groups_indices, matching_notes_mut,
    quantize_note, shift_tick, transpose_key,
};

/// Which notes an input is expected to list, and what happens to the others.
#[derive(Debug)]
pub(super) struct Membership {
    pub(super) scope: NoteFilter,
    pub(super) listed: HashSet<String>,
    pub(super) delete_missing: bool,
}

/// Commands ready to run.
#[derive(Debug, Default)]
pub(super) struct Plan {
    pub(super) commands: Vec<LineCommand>,
    pub(super) membership: Option<Membership>,
    pub(super) warnings: Vec<String>,
}

/// Applies ASCII edit commands to a standard MIDI file and returns rewritten MIDI bytes.
pub fn apply_edits(bytes: &[u8], edits: &str) -> Result<Vec<u8>> {
    Ok(apply_edits_with(bytes, edits, EditFormat::Text, ApplyOptions::default())?.bytes)
}

/// Applies edit input in any supported format and reports what changed.
pub fn apply_edits_with(
    bytes: &[u8],
    edits: &str,
    format: EditFormat,
    options: ApplyOptions,
) -> Result<ApplyOutput> {
    let smf = parse_smf(bytes)?;
    let time_map = TimeSignatureMap::from_smf(&smf);
    let context = EditContext {
        ticks_per_beat: ticks_per_beat(smf.header.timing),
        time_map: time_map.as_ref(),
    };

    let plan = match detect_format(format, edits) {
        EditFormat::Json | EditFormat::Csv if options.prune => {
            return Err(Error::Usage(
                "--prune only applies to text timelines; JSON and CSV rows already delete missing ids"
                    .to_owned(),
            ));
        }
        EditFormat::Json => parse_rows(&smf, edits, EditFormat::Json, context)?,
        EditFormat::Csv => parse_rows(&smf, edits, EditFormat::Csv, context)?,
        EditFormat::Text | EditFormat::Auto => plan_text(&smf, edits, context, options)?,
    };
    execute(bytes, smf, time_map.as_ref(), plan, options)
}

fn plan_text(
    smf: &Smf<'_>,
    edits: &str,
    context: EditContext<'_>,
    options: ApplyOptions,
) -> Result<Plan> {
    let parsed = parse_edit_text(edits, context)?;
    if options.prune && !parsed.timeline {
        return Err(Error::Usage(
            "--prune needs a timeline from 'midy read', starting with MIDY_TIMELINE".to_owned(),
        ));
    }
    if options.prune && parsed.note_ids.is_empty() {
        return Err(Error::Usage(
            "refusing to prune: the timeline lists no NOTE lines".to_owned(),
        ));
    }

    let mut warnings = Vec::new();
    if !parsed.event_lines.is_empty() {
        let scan = scan_notes(smf);
        let current = timeline_event_lines(smf, &scan, EventDetail::All, &QueryOptions::default());
        let current = current.iter().map(String::as_str).collect::<HashSet<_>>();
        for (line, text) in &parsed.event_lines {
            if !current.contains(text.as_str()) {
                warnings.push(format!(
                    "line {line}: META and EVENT lines are read-only and this one differs from the file; use ADD_EVENT, SET_TEMPO or SET_PROGRAM to change events"
                ));
            }
        }
    }

    let membership = parsed.timeline.then(|| Membership {
        scope: parsed.scope.unwrap_or_default(),
        listed: parsed.note_ids.iter().cloned().collect(),
        delete_missing: options.prune,
    });
    Ok(Plan {
        commands: parsed.commands,
        membership,
        warnings,
    })
}

fn execute(
    bytes: &[u8],
    smf: Smf<'_>,
    time_map: Option<&TimeSignatureMap>,
    plan: Plan,
    options: ApplyOptions,
) -> Result<ApplyOutput> {
    let arena = Arena::new();
    let mut session = EditSession::from_smf(&smf);
    let mut report = EditReport {
        warnings: plan.warnings,
        notes_before: session.notes.iter().filter(|note| !note.open).count(),
        ..EditReport::default()
    };

    for entry in plan.commands {
        let LineCommand {
            line,
            name,
            command,
        } = entry;
        let matched = run_command(&mut session, &arena, time_map, command)
            .map_err(|error| Error::Edit(with_line(line, name, &error)))?;
        if matched == 0 {
            report
                .warnings
                .push(format!("line {line}: {name} matched nothing"));
        }
        report.commands.push(CommandReport {
            line,
            command: name.to_owned(),
            matched,
        });
    }

    if let Some(membership) = plan.membership {
        settle_membership(&mut session, &membership, &mut report);
    }
    summarize(&session, &mut report);

    if options.strict && !report.warnings.is_empty() {
        return Err(Error::Edit(format!(
            "strict mode: {}",
            report.warnings.join("; ")
        )));
    }

    if report.commands.is_empty() {
        report.warnings.push("no edit commands found".to_owned());
        return Ok(ApplyOutput {
            bytes: bytes.to_owned(),
            report,
        });
    }

    let track_count = session.track_count();
    let tracks = session.into_tracks()?;
    let format = match smf.header.format {
        Format::SingleTrack if track_count > 1 => Format::Parallel,
        format => format,
    };
    let rewritten = Smf {
        header: Header {
            format,
            timing: smf.header.timing,
        },
        tracks,
    };
    let mut out = Vec::new();
    rewritten.write_std(&mut out)?;
    Ok(ApplyOutput { bytes: out, report })
}

fn with_line(line: usize, name: &str, error: &Error) -> String {
    let text = error.to_string();
    if text.starts_with("edit line") {
        text
    } else {
        format!("edit line {line} ({name}): {text}")
    }
}

fn settle_membership(
    session: &mut EditSession<'_>,
    membership: &Membership,
    report: &mut EditReport,
) {
    let mut missing = 0;
    for editable in &mut session.notes {
        let listed = editable
            .id
            .as_ref()
            .is_some_and(|id| membership.listed.contains(id));
        let in_scope = editable
            .original
            .as_ref()
            .is_some_and(|original| membership.scope.matches(original));
        if !editable.open && !editable.deleted && in_scope && !listed {
            missing += 1;
            if membership.delete_missing {
                editable.deleted = true;
            }
        }
    }

    if membership.delete_missing {
        report.commands.push(CommandReport {
            line: 0,
            command: "PRUNE".to_owned(),
            matched: missing,
        });
    } else if missing > 0 {
        report.warnings.push(format!(
            "{missing} notes are missing from the timeline and were left unchanged; use --prune to delete them"
        ));
    }
}

fn summarize(session: &EditSession<'_>, report: &mut EditReport) {
    for editable in &session.notes {
        match (&editable.original, editable.deleted) {
            (None, false) => report.notes_added += 1,
            (Some(_), true) if !editable.open => report.notes_deleted += 1,
            (Some(original), false) if !editable.open && *original != editable.note => {
                report.notes_changed += 1;
            }
            _ => {}
        }
        if !editable.deleted && !editable.open {
            report.notes_after += 1;
        }
    }
    for event in &session.events {
        report.events_added += usize::from(event.added && !event.deleted);
        report.events_deleted += usize::from(!event.added && event.deleted);
    }
}

fn run_command<'a>(
    session: &mut EditSession<'a>,
    arena: &'a Arena,
    time_map: Option<&TimeSignatureMap>,
    command: EditCommand,
) -> Result<usize> {
    if let Some(track) = command_filter(&command).and_then(|filter| filter.track) {
        session.check_track(track)?;
    }

    let matched = match command {
        EditCommand::Add(note) => {
            let position = session.add_note(note);
            session.validate_note(position)?;
            return Ok(1);
        }
        EditCommand::Delete { id } => {
            let position = session.note_position(&id)?;
            session.notes[position].deleted = true;
            return Ok(1);
        }
        EditCommand::Set { id, patch } => {
            let position = session.note_position(&id)?;
            let editable = &mut session.notes[position];
            patch.apply_to(&mut editable.note, time_map)?;
            editable.open &= patch.duration.is_none();
            session.validate_note(position)?;
            return Ok(1);
        }
        EditCommand::Note { id, row } => {
            let position = session.note_position(&id)?;
            let patch = row.patch_for(&session.notes[position].note, time_map)?;
            patch.apply_to(&mut session.notes[position].note, time_map)?;
            session.validate_note(position)?;
            return Ok(1);
        }
        EditCommand::DeleteMatching { filter } => {
            let mut matching = matching_notes_mut(&mut session.notes, &filter);
            matching
                .iter_mut()
                .for_each(|editable| editable.deleted = true);
            matching.len()
        }
        EditCommand::Transpose { semitones, filter } => {
            let mut matching = matching_notes_mut(&mut session.notes, &filter);
            for editable in &mut matching {
                editable.note.key = transpose_key(editable.note.key, semitones)?;
            }
            matching.len()
        }
        EditCommand::Shift {
            ticks,
            filter,
            notes_only,
        } => {
            let mut matching = matching_notes_mut(&mut session.notes, &filter);
            for editable in &mut matching {
                editable.note.start = shift_tick(editable.note.start, ticks)?;
            }
            let matched = matching.len();
            if !notes_only {
                retime_events(session, &filter, |tick| {
                    Ok(if tick == 0 {
                        0
                    } else if ticks >= 0 {
                        tick.saturating_add(ticks as u64)
                    } else {
                        tick.saturating_sub(ticks.unsigned_abs())
                    })
                })?;
            }
            matched
        }
        EditCommand::ScaleTime {
            factor,
            filter,
            notes_only,
        } => {
            let mut matching = matching_notes_mut(&mut session.notes, &filter);
            for editable in &mut matching {
                let note = &mut editable.note;
                note.start = factor.scale(note.start)?;
                if note.duration > 0 {
                    note.duration = factor.scale(note.duration)?.max(1);
                }
            }
            let matched = matching.len();
            if !notes_only {
                retime_events(session, &filter, |tick| factor.scale(tick))?;
            }
            matched
        }
        EditCommand::ScaleDuration { factor, filter } => {
            let mut matching = matching_notes_mut(&mut session.notes, &filter);
            for editable in &mut matching {
                if editable.note.duration > 0 {
                    editable.note.duration = factor.scale(editable.note.duration)?.max(1);
                }
            }
            matching.len()
        }
        EditCommand::Quantize { grid, mode, filter } => {
            let mut matching = matching_notes_mut(&mut session.notes, &filter);
            for editable in &mut matching {
                quantize_note(&mut editable.note, grid, mode)?;
            }
            matching.len()
        }
        EditCommand::Humanize {
            timing,
            velocity,
            seed,
            filter,
        } => apply_humanize(&mut session.notes, &filter, timing, velocity, seed)?,
        EditCommand::Swing {
            amount,
            grid,
            filter,
        } => apply_swing(&mut session.notes, &filter, amount, grid)?,
        EditCommand::Velocity { command, filter } => {
            let mut matching = matching_notes_mut(&mut session.notes, &filter);
            for editable in &mut matching {
                editable.note.velocity = apply_velocity(editable.note.velocity, command)?;
            }
            matching.len()
        }
        EditCommand::Crescendo {
            start_velocity,
            end_velocity,
            filter,
        } => apply_crescendo(&mut session.notes, &filter, start_velocity, end_velocity)?,
        EditCommand::Chordize { intervals, filter } => {
            let selected = selected_notes(session, &filter);
            for source in &selected {
                for interval in intervals.iter().copied().filter(|interval| *interval != 0) {
                    let mut note = source.clone();
                    note.key = transpose_key(note.key, interval)?;
                    session.add_note(note);
                }
            }
            selected.len()
        }
        EditCommand::Arpeggiate {
            grid,
            order,
            filter,
        } => apply_arpeggiate(session, &filter, grid, order)?,
        EditCommand::BlockChord { grid, filter } => {
            apply_block_chord(&mut session.notes, &filter, grid)
        }
        EditCommand::InvertChords { inversion, filter } => {
            apply_invert_chords(&mut session.notes, &filter, inversion)?
        }
        EditCommand::Double { octave, filter } => {
            let semitones = octave
                .checked_mul(12)
                .ok_or_else(|| Error::Edit("DOUBLE octave is too large".to_owned()))?;
            let selected = selected_notes(session, &filter);
            for source in &selected {
                let mut note = source.clone();
                note.key = transpose_key(note.key, semitones)?;
                session.add_note(note);
            }
            selected.len()
        }
        EditCommand::VoiceLead { max_jump, filter } => {
            let matched = chord_groups_indices(&session.notes, &filter, 2)
                .iter()
                .map(Vec::len)
                .sum();
            apply_voice_lead(&mut session.notes, &filter, max_jump);
            matched
        }
        EditCommand::Solo { filter } => {
            let mut matched = 0;
            for editable in session.notes.iter_mut().filter(|note| !note.deleted) {
                if filter.matches(&editable.note) {
                    matched += 1;
                } else {
                    editable.deleted = true;
                }
            }
            matched
        }
        EditCommand::MoveTrack { from, to, filter } => {
            session.check_track(from)?;
            session.check_track(to)?;
            let mut matching = matching_notes_mut(&mut session.notes, &filter);
            matching
                .iter_mut()
                .for_each(|editable| editable.note.track = to);
            matching.len()
        }
        EditCommand::SetChannel { channel, filter } => {
            let mut matching = matching_notes_mut(&mut session.notes, &filter);
            matching
                .iter_mut()
                .for_each(|editable| editable.note.channel = channel);
            matching.len()
        }
        EditCommand::SetTempo { tick, micros } => {
            session.check_track(0)?;
            let kind = event_kind(&NewEvent::Tempo { micros }, arena)?;
            replace_event(session, 0, tick, kind, |event| {
                event.tick == tick
                    && matches!(event.kind, TrackEventKind::Meta(MetaMessage::Tempo(_)))
            });
            return Ok(1);
        }
        EditCommand::SetProgram {
            track,
            channel,
            program,
            tick,
        } => {
            session.check_track(track)?;
            let kind = event_kind(&NewEvent::ProgramChange { channel, program }, arena)?;
            replace_event(session, track, tick, kind, |event| {
                event.track == track
                    && event.tick == tick
                    && event_kind_name(event.kind) == "program_change"
                    && event_channel(event.kind) == Some(channel)
            });
            return Ok(1);
        }
        EditCommand::AddEvent { track, tick, event } => {
            session.check_track(track)?;
            let kind = event_kind(&event, arena)?;
            session.add_event(track, tick, kind);
            return Ok(1);
        }
        EditCommand::DeleteEvent { track, tick, event } => {
            let kind = event_kind(&event, arena)?;
            let found = session.events.iter_mut().find(|candidate| {
                !candidate.deleted
                    && candidate.track == track
                    && candidate.tick == tick
                    && candidate.kind == kind
            });
            let found = found.ok_or_else(|| {
                Error::Edit(format!("no matching event on track {track} at tick {tick}"))
            })?;
            found.deleted = true;
            return Ok(1);
        }
        EditCommand::DeleteEvents { filter } => {
            if let Some(track) = filter.track {
                session.check_track(track)?;
            }
            let mut matched = 0;
            for event in session.events.iter_mut().filter(|event| !event.deleted) {
                if event_matches(event, &filter) {
                    event.deleted = true;
                    matched += 1;
                }
            }
            return Ok(matched);
        }
        EditCommand::AddTrack { name } => {
            session.track_ends.push(0);
            if let Some(name) = name {
                let track = session.track_count() - 1;
                let kind = text_kind(TextKind::TrackName, arena.add(&name));
                session.add_event(track, 0, kind);
            }
            return Ok(1);
        }
        EditCommand::SetTrackName { track, name } => {
            session.check_track(track)?;
            let kind = text_kind(TextKind::TrackName, arena.add(&name));
            replace_event(session, track, 0, kind, |event| {
                event.track == track
                    && matches!(event.kind, TrackEventKind::Meta(MetaMessage::TrackName(_)))
            });
            return Ok(1);
        }
    };

    session.validate_changed()?;
    Ok(matched)
}

fn command_filter(command: &EditCommand) -> Option<&NoteFilter> {
    match command {
        EditCommand::DeleteMatching { filter }
        | EditCommand::Transpose { filter, .. }
        | EditCommand::Shift { filter, .. }
        | EditCommand::ScaleTime { filter, .. }
        | EditCommand::ScaleDuration { filter, .. }
        | EditCommand::Quantize { filter, .. }
        | EditCommand::Humanize { filter, .. }
        | EditCommand::Swing { filter, .. }
        | EditCommand::Velocity { filter, .. }
        | EditCommand::Crescendo { filter, .. }
        | EditCommand::Chordize { filter, .. }
        | EditCommand::Arpeggiate { filter, .. }
        | EditCommand::BlockChord { filter, .. }
        | EditCommand::InvertChords { filter, .. }
        | EditCommand::Double { filter, .. }
        | EditCommand::VoiceLead { filter, .. }
        | EditCommand::Solo { filter }
        | EditCommand::SetChannel { filter, .. } => Some(filter),
        _ => None,
    }
}

fn selected_notes(
    session: &EditSession<'_>,
    filter: &NoteFilter,
) -> Vec<crate::midi::model::ConcreteNote> {
    session
        .notes
        .iter()
        .filter(|editable| !editable.deleted && filter.matches(&editable.note))
        .map(|editable| editable.note.clone())
        .collect()
}

/// Moves non-note events along with a time edit.
///
/// A key filter selects pitches, which events do not have. Channel messages
/// follow the track, channel and range filters; meta and system events and the
/// end of each track follow only whole-song edits.
fn retime_events(
    session: &mut EditSession<'_>,
    filter: &NoteFilter,
    retime: impl Fn(u64) -> Result<u64>,
) -> Result<()> {
    if filter.key.is_some() {
        return Ok(());
    }
    let whole_song = filter.is_whole_song();
    for event in session.events.iter_mut().filter(|event| !event.deleted) {
        let selected = match event_channel(event.kind) {
            Some(channel) => {
                filter.track.is_none_or(|track| track == event.track)
                    && filter.allows_channel(channel)
                    && filter.allows_tick(event.tick)
            }
            None => whole_song,
        };
        if selected {
            event.tick = retime(event.tick)?;
        }
    }
    if whole_song {
        for end in &mut session.track_ends {
            *end = retime(*end)?;
        }
    }
    Ok(())
}

fn replace_event<'a>(
    session: &mut EditSession<'a>,
    default_track: usize,
    tick: u64,
    kind: TrackEventKind<'a>,
    is_target: impl Fn(&EditableEvent<'a>) -> bool,
) {
    let mut track = default_track;
    let mut tick = tick;
    if let Some(old) = session
        .events
        .iter_mut()
        .find(|event| !event.deleted && is_target(event))
    {
        old.deleted = true;
        track = old.track;
        tick = old.tick;
    }
    session.add_event(track, tick, kind);
}

fn event_matches(event: &EditableEvent<'_>, filter: &EventFilter) -> bool {
    filter
        .kind
        .as_deref()
        .is_none_or(|kind| event_kind_name(event.kind) == kind)
        && filter
            .controller
            .is_none_or(|controller| event_controller(event.kind) == Some(controller))
        && filter.track.is_none_or(|track| event.track == track)
        && filter
            .channel
            .is_none_or(|channel| event_channel(event.kind) == Some(channel))
        && filter.start.is_none_or(|start| event.tick >= start)
        && filter.end.is_none_or(|end| event.tick < end)
}

fn text_kind<'a>(kind: TextKind, bytes: &'a [u8]) -> TrackEventKind<'a> {
    TrackEventKind::Meta(match kind {
        TextKind::Text => MetaMessage::Text(bytes),
        TextKind::Copyright => MetaMessage::Copyright(bytes),
        TextKind::TrackName => MetaMessage::TrackName(bytes),
        TextKind::InstrumentName => MetaMessage::InstrumentName(bytes),
        TextKind::Lyric => MetaMessage::Lyric(bytes),
        TextKind::Marker => MetaMessage::Marker(bytes),
        TextKind::CuePoint => MetaMessage::CuePoint(bytes),
        TextKind::ProgramName => MetaMessage::ProgramName(bytes),
        TextKind::DeviceName => MetaMessage::DeviceName(bytes),
    })
}

fn midi_kind<'a>(channel: u8, message: MidiMessage) -> TrackEventKind<'a> {
    TrackEventKind::Midi {
        channel: u4::from(channel),
        message,
    }
}

fn event_kind<'a>(event: &NewEvent, arena: &'a Arena) -> Result<TrackEventKind<'a>> {
    Ok(match event {
        NewEvent::Controller {
            channel,
            controller,
            value,
        } => midi_kind(
            *channel,
            MidiMessage::Controller {
                controller: u7::from(*controller),
                value: u7::from(*value),
            },
        ),
        NewEvent::PitchBend { channel, value } => midi_kind(
            *channel,
            MidiMessage::PitchBend {
                bend: PitchBend(u14::from((*value + 8192) as u16)),
            },
        ),
        NewEvent::ProgramChange { channel, program } => midi_kind(
            *channel,
            MidiMessage::ProgramChange {
                program: u7::from(*program),
            },
        ),
        NewEvent::ChannelAftertouch { channel, pressure } => midi_kind(
            *channel,
            MidiMessage::ChannelAftertouch {
                vel: u7::from(*pressure),
            },
        ),
        NewEvent::PolyAftertouch {
            channel,
            key,
            pressure,
        } => midi_kind(
            *channel,
            MidiMessage::Aftertouch {
                key: u7::from(*key),
                vel: u7::from(*pressure),
            },
        ),
        NewEvent::OrphanOff {
            channel,
            key,
            velocity,
        } => midi_kind(
            *channel,
            MidiMessage::NoteOff {
                key: u7::from(*key),
                vel: u7::from(*velocity),
            },
        ),
        NewEvent::Tempo { micros } => TrackEventKind::Meta(MetaMessage::Tempo(u24::from(*micros))),
        NewEvent::TimeSignature {
            numerator,
            power,
            clocks,
            thirtyseconds,
        } => TrackEventKind::Meta(MetaMessage::TimeSignature(
            *numerator,
            *power,
            *clocks,
            *thirtyseconds,
        )),
        NewEvent::KeySignature { sharps, minor } => {
            TrackEventKind::Meta(MetaMessage::KeySignature(*sharps, *minor))
        }
        NewEvent::Text { kind, bytes } => text_kind(*kind, arena.add(bytes)),
        NewEvent::SequenceNumber { value } => {
            TrackEventKind::Meta(MetaMessage::TrackNumber(*value))
        }
        NewEvent::MidiChannel { channel } => {
            TrackEventKind::Meta(MetaMessage::MidiChannel(u4::from(*channel)))
        }
        NewEvent::MidiPort { port } => TrackEventKind::Meta(MetaMessage::MidiPort(u7::from(*port))),
        NewEvent::SmpteOffset {
            fps,
            hour,
            minute,
            second,
            frame,
            subframe,
        } => {
            let invalid = || Error::Edit("invalid smpte_offset values".to_owned());
            let fps = Fps::from_int(*fps).ok_or_else(invalid)?;
            let time = SmpteTime::new(*hour, *minute, *second, *frame, *subframe, fps)
                .ok_or_else(invalid)?;
            TrackEventKind::Meta(MetaMessage::SmpteOffset(time))
        }
        NewEvent::SequencerSpecific { bytes } => {
            TrackEventKind::Meta(MetaMessage::SequencerSpecific(arena.add(bytes)))
        }
        NewEvent::UnknownMeta { kind, bytes } => {
            TrackEventKind::Meta(MetaMessage::Unknown(*kind, arena.add(bytes)))
        }
        NewEvent::SysEx { bytes } => TrackEventKind::SysEx(arena.add(bytes)),
        NewEvent::Escape { bytes } => TrackEventKind::Escape(arena.add(bytes)),
    })
}

/// Renders an edit report as line-oriented text.
pub fn render_edit_report(report: &EditReport) -> String {
    let mut out = String::new();
    emit!(out, "MIDY_APPLY v1 commands={}", report.commands.len());
    for command in &report.commands {
        emit!(
            out,
            "LINE line={} command={} matched={}",
            command.line,
            command.command,
            command.matched
        );
    }
    emit!(
        out,
        "SUMMARY notes_before={} notes_after={} added={} deleted={} changed={} events_added={} events_deleted={}",
        report.notes_before,
        report.notes_after,
        report.notes_added,
        report.notes_deleted,
        report.notes_changed,
        report.events_added,
        report.events_deleted,
    );
    for warning in &report.warnings {
        emit!(out, "WARN {warning}");
    }
    out
}
