//! Rendering a file as a text, JSON or CSV timeline.

use midly::{MetaMessage, MidiMessage, Smf, TrackEventKind};

use super::format::{
    csv_field, format_name, hex_encode, json_string, note_name, quote_ascii, ticks_per_beat,
    timing_fields,
};
use super::model::{Note, NoteScan, StuckNote, note_event, scan_notes, song_length_ticks};
use super::options::{EventDetail, QueryOptions, ReadOptions};
use super::parse_smf;
use super::time::{TimeSignatureMap, note_position_fields};
use crate::Result;

/// Event kinds that `DELETE_EVENTS kind=` can name.
pub(super) const EVENT_KINDS: &[&str] = &[
    "cc",
    "pitch_bend",
    "program_change",
    "channel_aftertouch",
    "poly_aftertouch",
    "orphan_note_off",
    "sysex",
    "escape",
    "text",
    "copyright",
    "track_name",
    "instrument_name",
    "lyric",
    "marker",
    "cue_point",
    "program_name",
    "device_name",
    "sequence_number",
    "midi_channel",
    "midi_port",
    "tempo",
    "smpte_offset",
    "time_signature",
    "key_signature",
    "sequencer_specific",
    "unknown",
];

/// The notes and context a timeline shows.
struct View<'a> {
    smf: Smf<'a>,
    scan: NoteScan,
    query: QueryOptions,
    notes: Vec<Note>,
    stuck: Vec<StuckNote>,
    time_map: Option<TimeSignatureMap>,
    length_ticks: u64,
}

impl<'a> View<'a> {
    fn new(bytes: &'a [u8], options: &ReadOptions) -> Result<Self> {
        let smf = parse_smf(bytes)?;
        let time_map = TimeSignatureMap::from_smf(&smf);
        let query = options.query.resolved(&smf, time_map.as_ref())?;
        let scan = scan_notes(&smf);
        let length_ticks = song_length_ticks(&smf, &scan.notes);

        let mut notes = scan
            .notes
            .iter()
            .filter(|note| query.matches_note(note))
            .cloned()
            .collect::<Vec<_>>();
        notes.sort_by_key(|note| {
            (
                note.track,
                note.start,
                note.channel,
                note.key,
                note.on_event_index,
            )
        });
        let stuck = scan
            .stuck
            .iter()
            .filter(|note| {
                query.track.is_none_or(|track| note.track == track)
                    && query.channel.is_none_or(|channel| note.channel == channel)
                    && query.key.is_none_or(|key| note.key == key)
                    && query.matches_tick(note.start)
            })
            .cloned()
            .collect();

        Ok(Self {
            smf,
            scan,
            query,
            notes,
            stuck,
            time_map,
            length_ticks,
        })
    }
}

/// Renders a deterministic ASCII timeline for a standard MIDI file.
pub fn render_timeline(bytes: &[u8]) -> Result<String> {
    render_timeline_with(bytes, &ReadOptions::default())
}

/// Renders the timeline as structured JSON for scripts.
pub fn render_timeline_json(bytes: &[u8]) -> Result<String> {
    render_timeline_json_with(bytes, &ReadOptions::default())
}

/// Renders timeline notes as CSV for spreadsheets and simple scripts.
pub fn render_timeline_csv(bytes: &[u8]) -> Result<String> {
    render_timeline_csv_with(bytes, &ReadOptions::default())
}

/// Renders a selected part of the ASCII timeline.
pub fn render_timeline_with(bytes: &[u8], options: &ReadOptions) -> Result<String> {
    let view = View::new(bytes, options)?;
    let smf = &view.smf;
    let mut out = String::new();

    emit!(out, "MIDY_TIMELINE v1");
    emit!(
        out,
        "HEADER format={} {} tracks={}",
        format_name(smf.header.format),
        timing_fields(smf.header.timing),
        smf.tracks.len(),
    );
    if let Some(ticks_per_beat) = ticks_per_beat(smf.header.timing) {
        emit!(
            out,
            "SONG length_ticks={} length_beats={:.3}",
            view.length_ticks,
            view.length_ticks as f64 / f64::from(ticks_per_beat),
        );
    } else {
        emit!(out, "SONG length_ticks={}", view.length_ticks);
    }
    if let Some(scope) = view.query.scope_text() {
        emit!(out, "SCOPE {scope}");
    }
    emit!(out, "# Edit NOTE lines or add commands; see 'midy schema'.");

    for (track_index, track) in smf.tracks.iter().enumerate() {
        let note_count = view
            .scan
            .notes
            .iter()
            .filter(|note| note.track == track_index)
            .count();
        emit!(
            out,
            "TRACK index={} events={} notes={}",
            track_index,
            track.len(),
            note_count,
        );
    }
    for line in timeline_event_lines(smf, &view.scan, options.events, &view.query) {
        emit!(out, "{line}");
    }
    for note in &view.stuck {
        emit!(
            out,
            "STUCK id={} track={} ch={} key={} name={} start={} vel={}",
            note.id,
            note.track,
            note.channel,
            note.key,
            note_name(note.key),
            note.start,
            note.velocity,
        );
    }
    for note in &view.notes {
        if options.compact {
            emit!(
                out,
                "NOTE id={} track={} ch={} key={} start={} dur={} vel={}",
                note.id,
                note.track,
                note.channel,
                note.key,
                note.start,
                note.duration,
                note.velocity,
            );
            continue;
        }
        let end = note.start.saturating_add(note.duration);
        emit!(
            out,
            "NOTE id={} track={} ch={} key={} name={} start={} dur={} end={}{} vel={} off_vel={}",
            note.id,
            note.track,
            note.channel,
            note.key,
            note_name(note.key),
            note.start,
            note.duration,
            end,
            note_position_fields(note.start, end, view.time_map.as_ref()),
            note.velocity,
            note.off_velocity,
        );
    }

    Ok(out)
}

/// Renders a selected part of the timeline as JSON.
pub fn render_timeline_json_with(bytes: &[u8], options: &ReadOptions) -> Result<String> {
    let view = View::new(bytes, options)?;
    let smf = &view.smf;
    let tempo_map = ticks_per_beat(smf.header.timing);
    let mut out = String::new();

    emit!(out, "{{");
    emit!(out, "  \"kind\": \"MIDY_TIMELINE\",");
    emit!(out, "  \"version\": 1,");
    emit!(
        out,
        "  \"format\": {},",
        json_string(format_name(smf.header.format))
    );
    emit!(
        out,
        "  \"timing\": {},",
        json_string(&timing_fields(smf.header.timing))
    );
    match tempo_map {
        Some(ticks) => emit!(out, "  \"ticks_per_beat\": {ticks},"),
        None => emit!(out, "  \"ticks_per_beat\": null,"),
    }
    emit!(out, "  \"tracks\": {},", smf.tracks.len());
    emit!(out, "  \"length_ticks\": {},", view.length_ticks);
    match tempo_map {
        Some(ticks) => emit!(
            out,
            "  \"length_beats\": {:.6},",
            view.length_ticks as f64 / f64::from(ticks)
        ),
        None => emit!(out, "  \"length_beats\": null,"),
    }
    if let Some(scope) = view.query.scope_text() {
        emit!(out, "  \"scope\": {},", json_string(&scope));
    }

    let events = timeline_event_lines(smf, &view.scan, options.events, &view.query);
    emit!(out, "  \"events\": [");
    for (index, line) in events.iter().enumerate() {
        let comma = if index + 1 == events.len() { "" } else { "," };
        emit!(out, "    {}{comma}", json_string(line));
    }
    emit!(out, "  ],");

    emit!(out, "  \"stuck_notes\": [");
    for (index, note) in view.stuck.iter().enumerate() {
        emit!(
            out,
            "    {{\"id\": {}, \"track\": {}, \"ch\": {}, \"key\": {}, \"start\": {}, \"vel\": {}}}{}",
            json_string(&note.id),
            note.track,
            note.channel,
            note.key,
            note.start,
            note.velocity,
            if index + 1 == view.stuck.len() {
                ""
            } else {
                ","
            },
        );
    }
    emit!(out, "  ],");

    emit!(out, "  \"notes\": [");
    for (index, note) in view.notes.iter().enumerate() {
        emit!(
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
            if index + 1 == view.notes.len() {
                ""
            } else {
                ","
            },
        );
    }
    emit!(out, "  ]");
    emit!(out, "}}");

    Ok(out)
}

/// Renders a selected part of the notes as CSV.
pub fn render_timeline_csv_with(bytes: &[u8], options: &ReadOptions) -> Result<String> {
    let view = View::new(bytes, options)?;
    let scope = view
        .query
        .scope_text()
        .map(|scope| format!(",{}", csv_field(&scope.replace(' ', ";"))));
    let mut out = String::new();

    emit!(
        out,
        "id,track,ch,key,name,start,dur,end,vel,off_vel{}",
        if scope.is_some() { ",scope" } else { "" }
    );
    for note in &view.notes {
        emit!(
            out,
            "{},{},{},{},{},{},{},{},{},{}{}",
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
            scope.as_deref().unwrap_or_default(),
        );
    }

    Ok(out)
}

/// Renders the non-note lines of a timeline, in file order per track.
pub(super) fn timeline_event_lines(
    smf: &Smf<'_>,
    scan: &NoteScan,
    detail: EventDetail,
    query: &QueryOptions,
) -> Vec<String> {
    if detail == EventDetail::None {
        return Vec::new();
    }
    let orphans = scan.orphan_events();
    let mut lines = Vec::new();

    for (track_index, track) in smf.tracks.iter().enumerate() {
        if query.track.is_some_and(|wanted| wanted != track_index) {
            continue;
        }
        let mut tick = 0_u64;
        for (event_index, event) in track.iter().enumerate() {
            tick += u64::from(event.delta.as_int());
            let orphan = orphans.contains(&(track_index, event_index));
            if (note_event(event.kind).is_some() && !orphan) || !query.matches_tick(tick) {
                continue;
            }
            if let (Some(wanted), Some(channel)) = (query.channel, event_channel(event.kind))
                && wanted != channel
            {
                continue;
            }
            if detail == EventDetail::Basic && is_performance_event(event.kind) {
                continue;
            }
            lines.push(event_line(track_index, tick, event.kind));
        }
    }
    lines
}

fn is_performance_event(kind: TrackEventKind<'_>) -> bool {
    matches!(
        kind,
        TrackEventKind::Midi { message, .. } if !matches!(message, MidiMessage::ProgramChange { .. })
    )
}

/// Returns the channel of a channel-voice event.
pub(super) fn event_channel(kind: TrackEventKind<'_>) -> Option<u8> {
    match kind {
        TrackEventKind::Midi { channel, .. } => Some(channel.as_int()),
        _ => None,
    }
}

pub(super) fn event_controller(kind: TrackEventKind<'_>) -> Option<u8> {
    match kind {
        TrackEventKind::Midi {
            message: MidiMessage::Controller { controller, .. },
            ..
        } => Some(controller.as_int()),
        _ => None,
    }
}

/// Returns the `kind=` name of a non-note event.
pub(super) fn event_kind_name(kind: TrackEventKind<'_>) -> &'static str {
    match kind {
        TrackEventKind::Midi { message, .. } => match message {
            MidiMessage::Controller { .. } => "cc",
            MidiMessage::PitchBend { .. } => "pitch_bend",
            MidiMessage::ProgramChange { .. } => "program_change",
            MidiMessage::ChannelAftertouch { .. } => "channel_aftertouch",
            MidiMessage::Aftertouch { .. } => "poly_aftertouch",
            MidiMessage::NoteOn { .. } | MidiMessage::NoteOff { .. } => "orphan_note_off",
        },
        TrackEventKind::SysEx(_) => "sysex",
        TrackEventKind::Escape(_) => "escape",
        TrackEventKind::Meta(meta) => match meta {
            MetaMessage::TrackNumber(_) => "sequence_number",
            MetaMessage::Text(_) => "text",
            MetaMessage::Copyright(_) => "copyright",
            MetaMessage::TrackName(_) => "track_name",
            MetaMessage::InstrumentName(_) => "instrument_name",
            MetaMessage::Lyric(_) => "lyric",
            MetaMessage::Marker(_) => "marker",
            MetaMessage::CuePoint(_) => "cue_point",
            MetaMessage::ProgramName(_) => "program_name",
            MetaMessage::DeviceName(_) => "device_name",
            MetaMessage::MidiChannel(_) => "midi_channel",
            MetaMessage::MidiPort(_) => "midi_port",
            MetaMessage::EndOfTrack => "end_of_track",
            MetaMessage::Tempo(_) => "tempo",
            MetaMessage::SmpteOffset(_) => "smpte_offset",
            MetaMessage::TimeSignature(..) => "time_signature",
            MetaMessage::KeySignature(..) => "key_signature",
            MetaMessage::SequencerSpecific(_) => "sequencer_specific",
            MetaMessage::Unknown(..) => "unknown",
        },
    }
}

/// Renders one non-note event using the field names `ADD_EVENT` accepts.
pub(super) fn event_line(track: usize, tick: u64, kind: TrackEventKind<'_>) -> String {
    let prefix = if matches!(kind, TrackEventKind::Meta(_)) {
        "META"
    } else {
        "EVENT"
    };
    let name = event_kind_name(kind);
    let fields = event_fields(kind);
    let separator = if fields.is_empty() { "" } else { " " };
    format!("{prefix} track={track} tick={tick} kind={name}{separator}{fields}")
}

fn event_fields(kind: TrackEventKind<'_>) -> String {
    let bytes_fields = |bytes: &[u8]| format!("bytes={} hex={}", bytes.len(), hex_encode(bytes));
    match kind {
        TrackEventKind::Midi { channel, message } => {
            let ch = channel.as_int();
            match message {
                MidiMessage::Controller { controller, value } => {
                    format!("ch={ch} controller={controller} value={value}")
                }
                MidiMessage::PitchBend { bend } => format!("ch={ch} value={}", bend.as_int()),
                MidiMessage::ProgramChange { program } => format!("ch={ch} program={program}"),
                MidiMessage::ChannelAftertouch { vel } => format!("ch={ch} pressure={vel}"),
                MidiMessage::Aftertouch { key, vel } => {
                    format!("ch={ch} key={key} pressure={vel}")
                }
                MidiMessage::NoteOn { key, vel } | MidiMessage::NoteOff { key, vel } => {
                    format!("ch={ch} key={key} vel={vel}")
                }
            }
        }
        TrackEventKind::SysEx(bytes) | TrackEventKind::Escape(bytes) => bytes_fields(bytes),
        TrackEventKind::Meta(meta) => match meta {
            MetaMessage::Text(bytes)
            | MetaMessage::Copyright(bytes)
            | MetaMessage::TrackName(bytes)
            | MetaMessage::InstrumentName(bytes)
            | MetaMessage::Lyric(bytes)
            | MetaMessage::Marker(bytes)
            | MetaMessage::CuePoint(bytes)
            | MetaMessage::ProgramName(bytes)
            | MetaMessage::DeviceName(bytes) => format!("text={}", quote_ascii(bytes)),
            MetaMessage::TrackNumber(Some(value)) => format!("value={value}"),
            MetaMessage::TrackNumber(None) => "value=-".to_owned(),
            MetaMessage::MidiChannel(channel) => format!("ch={channel}"),
            MetaMessage::MidiPort(port) => format!("port={port}"),
            MetaMessage::EndOfTrack => String::new(),
            MetaMessage::Tempo(micros) => {
                let micros = micros.as_int();
                format!(
                    "us_per_quarter={micros} bpm={:.3}",
                    60_000_000.0 / f64::from(micros.max(1))
                )
            }
            MetaMessage::SmpteOffset(time) => format!(
                "fps={} hour={} minute={} second={} frame={} subframe={}",
                time.fps().as_int(),
                time.hour(),
                time.minute(),
                time.second(),
                time.frame(),
                time.subframe(),
            ),
            MetaMessage::TimeSignature(numerator, power, clocks, thirtyseconds) => {
                let denominator = 1_u64
                    .checked_shl(u32::from(power))
                    .filter(|_| power <= 31)
                    .map(|value| format!("denominator={value} "))
                    .unwrap_or_default();
                format!(
                    "numerator={numerator} {denominator}denominator_power={power} clocks_per_click={clocks} thirtyseconds_per_quarter={thirtyseconds}"
                )
            }
            MetaMessage::KeySignature(sharps, minor) => format!("sharps={sharps} minor={minor}"),
            MetaMessage::SequencerSpecific(bytes) => bytes_fields(bytes),
            MetaMessage::Unknown(kind, bytes) => format!("type={kind} {}", bytes_fields(bytes)),
        },
    }
}
