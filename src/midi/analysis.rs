//! Song analysis and per-track statistics.

use midly::{MetaMessage, Smf, TrackEvent, TrackEventKind};

use super::chords::chord_slices;
use super::format::{format_name, json_string, note_name, ticks_per_beat, timing_fields};
use super::model::{DRUM_CHANNEL, Note, collect_notes, meta_events, song_length_ticks};
use super::parse_smf;
use super::time::TimeSignatureMap;
use crate::Result;

const MAJOR_PROFILE: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR_PROFILE: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];
const MAJOR_KEYS: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];
const MINOR_KEYS: [&str; 12] = [
    "C", "C#", "D", "Eb", "E", "F", "F#", "G", "G#", "A", "Bb", "B",
];

/// Renders a high-level MIDI analysis.
pub fn render_analysis(bytes: &[u8], file_name: Option<&str>) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let mut notes = collect_notes(&smf);
    let time_map = TimeSignatureMap::from_smf(&smf);
    let song_length = song_length_ticks(&smf, &notes);
    let beat_ticks = ticks_per_beat(smf.header.timing);
    let key = estimate_key(&notes);
    let mut out = String::new();

    emit!(
        out,
        "ANALYZE{}",
        file_name
            .map(|file| format!(" file={file}"))
            .unwrap_or_default()
    );
    emit!(
        out,
        "header format={} {} tracks={}",
        format_name(smf.header.format),
        timing_fields(smf.header.timing),
        smf.tracks.len(),
    );
    render_tempo_summary(&smf, &mut out);
    render_time_signature_summary(&smf, &mut out);
    emit!(
        out,
        "song length_ticks={} notes={} key_guess={} key_confidence={:.2} key_declared={}",
        song_length,
        notes.len(),
        key.name,
        key.confidence,
        declared_key(&smf).unwrap_or_else(|| "-".to_owned()),
    );

    for (track_index, track) in smf.tracks.iter().enumerate() {
        let track_notes = notes
            .iter()
            .filter(|note| note.track == track_index)
            .collect::<Vec<_>>();
        let stats = TrackStats::from_notes(&track_notes);
        emit!(
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
            stats.density(song_length, beat_ticks),
        );
    }

    notes.retain(|note| note.channel != DRUM_CHANNEL);
    let slices = chord_slices(&mut notes, 0);
    if !slices.is_empty() {
        emit!(out, "chords:");
        for slice in &slices {
            emit!(out, "  {}", slice.line(time_map.as_ref()));
        }
    }

    Ok(out)
}

/// Renders a JSON analysis for scripts.
pub fn render_analysis_json(bytes: &[u8], file_name: Option<&str>) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let mut notes = collect_notes(&smf);
    let time_map = TimeSignatureMap::from_smf(&smf);
    let song_length = song_length_ticks(&smf, &notes);
    let beat_ticks = ticks_per_beat(smf.header.timing);
    let key = estimate_key(&notes);
    let mut out = String::new();
    let json_or_null = |value: Option<String>| {
        value.map_or_else(|| "null".to_owned(), |value| json_string(&value))
    };

    emit!(out, "{{");
    emit!(
        out,
        "  \"file\": {},",
        json_or_null(file_name.map(ToOwned::to_owned))
    );
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
    match beat_ticks {
        Some(ticks) => emit!(out, "  \"ticks_per_beat\": {ticks},"),
        None => emit!(out, "  \"ticks_per_beat\": null,"),
    }
    emit!(out, "  \"tracks\": {},", smf.tracks.len());
    emit!(out, "  \"length_ticks\": {song_length},");
    emit!(out, "  \"notes\": {},", notes.len());
    emit!(out, "  \"key_guess\": {},", json_string(&key.name));
    emit!(out, "  \"key_confidence\": {:.3},", key.confidence);
    emit!(
        out,
        "  \"key_declared\": {},",
        json_or_null(declared_key(&smf))
    );

    let tempos = tempo_changes(&smf);
    emit!(out, "  \"tempos\": [");
    for (index, (track, tick, micros)) in tempos.iter().enumerate() {
        emit!(
            out,
            "    {{\"track\": {track}, \"tick\": {tick}, \"us_per_quarter\": {micros}, \"bpm\": {:.3}}}{}",
            60_000_000.0 / f64::from((*micros).max(1)),
            if index + 1 == tempos.len() { "" } else { "," },
        );
    }
    emit!(out, "  ],");

    let signatures = time_signature_changes(&smf);
    emit!(out, "  \"time_signatures\": [");
    for (index, (track, tick, numerator, denominator)) in signatures.iter().enumerate() {
        emit!(
            out,
            "    {{\"track\": {track}, \"tick\": {tick}, \"numerator\": {numerator}, \"denominator\": {denominator}}}{}",
            if index + 1 == signatures.len() {
                ""
            } else {
                ","
            },
        );
    }
    emit!(out, "  ],");

    emit!(out, "  \"track_summaries\": [");
    for (track_index, track) in smf.tracks.iter().enumerate() {
        let track_notes = notes
            .iter()
            .filter(|note| note.track == track_index)
            .collect::<Vec<_>>();
        let stats = TrackStats::from_notes(&track_notes);
        emit!(
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
            stats.density(song_length, beat_ticks),
            if track_index + 1 == smf.tracks.len() {
                ""
            } else {
                ","
            },
        );
    }
    emit!(out, "  ],");

    notes.retain(|note| note.channel != DRUM_CHANNEL);
    let slices = chord_slices(&mut notes, 0);
    emit!(out, "  \"chords\": [");
    for (index, slice) in slices.iter().enumerate() {
        emit!(
            out,
            "    {}{}",
            json_string(&slice.line(time_map.as_ref())),
            if index + 1 == slices.len() { "" } else { "," },
        );
    }
    emit!(out, "  ]");
    emit!(out, "}}");

    Ok(out)
}

/// Renders a track/channel summary.
pub fn render_tracks(bytes: &[u8]) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let notes = collect_notes(&smf);
    let song_length = song_length_ticks(&smf, &notes);
    let beat_ticks = ticks_per_beat(smf.header.timing);
    let mut out = String::new();

    emit!(out, "MIDY_TRACKS v1 tracks={}", smf.tracks.len());
    for (track_index, track) in smf.tracks.iter().enumerate() {
        let track_notes = notes
            .iter()
            .filter(|note| note.track == track_index)
            .collect::<Vec<_>>();
        let stats = TrackStats::from_notes(&track_notes);
        emit!(
            out,
            "TRACK index={} role={} events={} notes={} channels={} range={} avg_vel={:.1} avg_dur={:.1} polyphony_max={} density={:.3} names={}",
            track_index,
            classify_track(&track_notes),
            track.len(),
            track_notes.len(),
            channels_text(&track_notes),
            stats.range,
            stats.avg_velocity,
            stats.avg_duration,
            stats.polyphony_max,
            stats.density(song_length, beat_ticks),
            track_names(track),
        );
    }

    Ok(out)
}

struct KeyEstimate {
    name: String,
    confidence: f64,
}

/// Matches pitched notes against major and minor key profiles.
fn estimate_key(notes: &[Note]) -> KeyEstimate {
    let mut weights = [0.0_f64; 12];
    for note in notes.iter().filter(|note| note.channel != DRUM_CHANNEL) {
        weights[usize::from(note.key % 12)] += note.duration.max(1) as f64;
    }
    if weights.iter().all(|weight| *weight == 0.0) {
        return KeyEstimate {
            name: "unknown".to_owned(),
            confidence: 0.0,
        };
    }

    let mut best = (f64::MIN, 0_usize, false);
    for tonic in 0..12 {
        for (profile, minor) in [(&MAJOR_PROFILE, false), (&MINOR_PROFILE, true)] {
            let rotated = |pc: usize| weights[(pc + tonic) % 12];
            let score = correlation(&rotated, profile);
            if score > best.0 {
                best = (score, tonic, minor);
            }
        }
    }

    let (score, tonic, minor) = best;
    let name = if minor {
        format!("{}_minor", MINOR_KEYS[tonic])
    } else {
        format!("{}_major", MAJOR_KEYS[tonic])
    };
    KeyEstimate {
        name,
        confidence: score.clamp(0.0, 1.0),
    }
}

fn correlation(weight: &impl Fn(usize) -> f64, profile: &[f64; 12]) -> f64 {
    let weight_mean = (0..12).map(weight).sum::<f64>() / 12.0;
    let profile_mean = profile.iter().sum::<f64>() / 12.0;
    let (mut product, mut weight_power, mut profile_power) = (0.0, 0.0, 0.0);
    for (pc, target) in profile.iter().enumerate() {
        let left = weight(pc) - weight_mean;
        let right = target - profile_mean;
        product += left * right;
        weight_power += left * left;
        profile_power += right * right;
    }
    let scale = (weight_power * profile_power).sqrt();
    if scale == 0.0 { 0.0 } else { product / scale }
}

/// Names the first key signature in the file.
fn declared_key(smf: &Smf<'_>) -> Option<String> {
    meta_events(smf).into_iter().find_map(|(_, _, meta)| {
        let MetaMessage::KeySignature(sharps, minor) = meta else {
            return None;
        };
        let fifths = i32::from(sharps.clamp(-7, 7));
        let offset = if minor { 9 } else { 0 };
        let tonic = (fifths * 7 + offset).rem_euclid(12) as usize;
        Some(if minor {
            format!("{}_minor", MINOR_KEYS[tonic])
        } else {
            format!("{}_major", MAJOR_KEYS[tonic])
        })
    })
}

fn tempo_changes(smf: &Smf<'_>) -> Vec<(usize, u64, u32)> {
    meta_events(smf)
        .into_iter()
        .filter_map(|(track, tick, meta)| match meta {
            MetaMessage::Tempo(micros) => Some((track, tick, micros.as_int())),
            _ => None,
        })
        .collect()
}

fn time_signature_changes(smf: &Smf<'_>) -> Vec<(usize, u64, u8, u64)> {
    meta_events(smf)
        .into_iter()
        .filter_map(|(track, tick, meta)| match meta {
            MetaMessage::TimeSignature(numerator, power, _, _) => Some((
                track,
                tick,
                numerator,
                1_u64.checked_shl(u32::from(power)).unwrap_or(0),
            )),
            _ => None,
        })
        .collect()
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

#[derive(Debug)]
struct TrackStats {
    range: String,
    avg_velocity: f64,
    avg_duration: f64,
    polyphony_max: usize,
    note_count: usize,
}

impl TrackStats {
    fn from_notes(notes: &[&Note]) -> Self {
        if notes.is_empty() {
            return Self {
                range: "-".to_owned(),
                avg_velocity: 0.0,
                avg_duration: 0.0,
                polyphony_max: 0,
                note_count: 0,
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
            note_count: notes.len(),
        }
    }

    /// Notes per beat; files without musical timing use 1000 ticks as the unit.
    fn density(&self, song_length: u64, ticks_per_beat: Option<u16>) -> f64 {
        if song_length == 0 {
            return 0.0;
        }
        let unit = ticks_per_beat.map_or(1000.0, f64::from);
        self.note_count as f64 * unit / song_length as f64
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
    let tempos = tempo_changes(smf);
    if let Some((track, tick, micros)) = tempos.first().copied() {
        emit!(
            out,
            "tempo initial_bpm={:.3} us_per_quarter={} changes={} first_track={} first_tick={}",
            60_000_000.0 / f64::from(micros),
            micros,
            tempos.len(),
            track,
            tick,
        );
    } else {
        emit!(
            out,
            "tempo initial_bpm=120.000 us_per_quarter=500000 changes=0 inferred=true"
        );
    }
}

fn render_time_signature_summary(smf: &Smf<'_>, out: &mut String) {
    let signatures = time_signature_changes(smf);
    if let Some((track, tick, num, den)) = signatures.first().copied() {
        emit!(
            out,
            "time_signature initial={}/{} changes={} first_track={} first_tick={}",
            num,
            den,
            signatures.len(),
            track,
            tick,
        );
    } else {
        emit!(out, "time_signature initial=4/4 changes=0 inferred=true");
    }
}
