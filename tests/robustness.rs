//! Mutated files and extreme edits must fail cleanly, never panic.
//!
//! Set `MIDY_FUZZ_ITERATIONS` to run longer, and `MIDY_CORPUS` to a directory
//! of MIDI files to run the round-trip checks over them.

mod common;

use std::path::Path;

use common::band;
use midy::midi;

const EDITS: &[&str] = &[
    "TRANSPOSE semitones=1\n",
    "QUANTIZE grid=1/16 mode=both\n",
    "SWING amount=60 grid=1/8\n",
    "HUMANIZE timing=12 velocity=8 seed=3\n",
    "ARPEGGIATE grid=1/16 order=updown\n",
    "INVERT_CHORDS inversion=-1\n",
    "VOICE_LEAD max_jump=7\n",
    "CRESCENDO start_vel=40 end_vel=110\n",
    "SCALE_TIME factor=3/2\n",
    "SHIFT by=1/8 bars=1..2\n",
    "ADD_NOTE track=0 ch=0 key=C4 at=2:1 dur=bar vel=90\n",
    "SET_TEMPO bpm=90\nDELETE_EVENTS kind=cc\n",
];

const EXTREME_EDITS: &[&str] = &[
    "TRANSPOSE semitones=32767\n",
    "TRANSPOSE semitones=-32768\n",
    "ADD_NOTE track=0 ch=0 key=C32767 start=0 dur=1 vel=1\n",
    "ADD_NOTE track=0 ch=0 key=C-32768 start=0 dur=1 vel=1\n",
    "ADD_NOTE track=0 ch=0 key=60 start=18446744073709551615 dur=18446744073709551615 vel=1\n",
    "SHIFT ticks=-9223372036854775808\n",
    "SHIFT ticks=9223372036854775807\n",
    "SHIFT by=18446744073709551615/1\n",
    "QUANTIZE grid=18446744073709551615 mode=both\n",
    "QUANTIZE grid=18446744073709551615/1 mode=both\n",
    "QUANTIZE grid=1/18446744073709551615t\n",
    "SWING amount=100 grid=18446744073709551615\n",
    "ARPEGGIATE grid=18446744073709551615\n",
    "HUMANIZE timing=9223372036854775807 velocity=127\n",
    "HUMANIZE timing=18446744073709551615\n",
    "VELOCITY scale=18446744073709551615\n",
    "VELOCITY compress=18446744073709551615 center=0\n",
    "SCALE_TIME factor=18446744073709551615/1\n",
    "SCALE_TIME factor=0.00000000000000000000001\n",
    "INVERT_CHORDS inversion=-32768\n",
    "DOUBLE octave=32767\n",
    "DOUBLE octave=2731\n",
    "CHORDIZE intervals=32767\n",
    "VOICE_LEAD max_jump=127\n",
    "DELETE_NOTES bars=1..18446744073709551615\n",
    "DELETE_NOTES bars=18446744073709551615\n",
    "SHIFT by=1 at=999999999:1\n",
    "CRESCENDO start_vel=0 end_vel=127 start=0 end=18446744073709551615\n",
    "SET_NOTE id=t0n0 start=268435455 dur=1\n",
    "SET_NOTE id=t0n0 start=268435456 dur=1\n",
    "SET_TEMPO bpm=0.0000000001\n",
    "SET_TEMPO bpm=1e308\n",
    "ADD_EVENT track=0 tick=18446744073709551615 kind=cc ch=0 controller=1 value=1\n",
    "ADD_EVENT track=0 tick=0 kind=pitch_bend ch=0 value=8191\n",
    "ADD_EVENT track=0 tick=0 kind=smpte_offset fps=30 hour=99 minute=99 second=99 frame=99 subframe=99\n",
];

fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn exercise(bytes: &[u8]) {
    let _ = midi::render_timeline(bytes);
    let _ = midi::render_timeline_json(bytes);
    let _ = midi::render_timeline_csv(bytes);
    let _ = midi::render_tracks(bytes);
    let _ = midi::render_lint(bytes);
    let _ = midi::render_analysis(bytes, Some("f"));
    let _ = midi::render_analysis_json(bytes, Some("f"));
    let _ = midi::render_chords(bytes, &midi::ChordsOptions::default());
    let _ = midi::render_roll(bytes, &midi::RollOptions::default());
    let _ = midi::render_diff(bytes, bytes);
    let _ = midi::fix_midi(bytes);
    let _ = midi::split_selection(bytes, midi::SplitMode::Channel);
    let _ = midi::suggest_reduce_chords(bytes, &midi::ReduceChordsOptions::default());
    let _ = midi::suggest_bassline(bytes, &midi::BasslineOptions::default());
    for edit in EDITS {
        let _ = midi::apply_edits(bytes, edit);
    }
}

#[test]
fn mutated_files_never_panic() {
    let iterations = std::env::var("MIDY_FUZZ_ITERATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1_500_u64);
    let seed = band();
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;

    for _ in 0..iterations {
        let mut bytes = seed.clone();
        for _ in 0..1 + next(&mut state) % 8 {
            let index = 14 + (next(&mut state) as usize) % (bytes.len() - 14);
            bytes[index] = match next(&mut state) % 4 {
                0 => next(&mut state) as u8,
                1 => 0xff,
                2 => 0x00,
                _ => bytes[index] ^ (1 << (next(&mut state) % 8)),
            };
        }
        if next(&mut state).is_multiple_of(8) {
            let keep = 14 + (next(&mut state) as usize) % (bytes.len() - 14);
            bytes.truncate(keep);
        }
        exercise(&bytes);
    }
}

#[test]
fn extreme_edit_values_never_panic() {
    let song = band();
    for edit in EXTREME_EDITS {
        let _ = midi::apply_edits(&song, edit);
        let _ = midi::apply_edits(&song, &edit.replace("t0n0", "t1n0"));
    }
}

/// Checks a file survives read, edit and re-read without unintended changes.
fn check_round_trip(bytes: &[u8], label: &str) {
    let timeline = midi::render_timeline(bytes).unwrap_or_else(|error| panic!("{label}: {error}"));

    let unchanged = midi::apply_edits(bytes, &timeline)
        .unwrap_or_else(|error| panic!("{label}: unmodified timeline failed: {error}"));
    assert_eq!(
        midi::render_diff(bytes, &unchanged).unwrap(),
        "",
        "{label}: unmodified timeline changed the file"
    );

    let json = midi::render_timeline_json(bytes).unwrap();
    if json.contains("\"id\"") {
        let unchanged = midi::apply_timeline_json_edits(bytes, &json)
            .unwrap_or_else(|error| panic!("{label}: JSON failed: {error}"));
        assert_eq!(
            midi::render_diff(bytes, &unchanged).unwrap(),
            "",
            "{label}: JSON changed the file"
        );
    }

    let up = midi::apply_edits(bytes, "TRANSPOSE semitones=0\nSHIFT ticks=0\n")
        .unwrap_or_else(|error| panic!("{label}: no-op edit failed: {error}"));
    assert_eq!(
        midi::render_diff(bytes, &up).unwrap(),
        "",
        "{label}: no-op edit changed the file"
    );

    let empty = midi::apply_edits(bytes, "ADD_TRACK\n").unwrap();
    let diff = midi::render_diff(&empty, bytes).unwrap();
    assert!(
        !diff.contains("ADD_NOTE"),
        "{label}: ADD_TRACK changed notes"
    );
}

#[test]
fn generated_song_round_trips() {
    check_round_trip(&band(), "band");
}

#[test]
fn corpus_round_trips() {
    let Ok(directory) = std::env::var("MIDY_CORPUS") else {
        return;
    };
    let mut checked = 0;
    visit(Path::new(&directory), &mut checked);
    assert!(checked > 0, "no MIDI files under {directory}");
}

fn visit(path: &Path, checked: &mut usize) {
    for entry in std::fs::read_dir(path).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit(&path, checked);
        } else if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mid"))
        {
            let bytes = std::fs::read(&path).unwrap();
            if midi::parse_smf(&bytes).is_ok() {
                check_round_trip(&bytes, &path.display().to_string());
                *checked += 1;
            }
        }
    }
}
