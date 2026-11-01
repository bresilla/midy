use super::args::{Command, EditInput, ReadFormat, parse};
use crate::midi;

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn defaults_to_help() {
    assert_eq!(parse(args(&["midy"])).unwrap(), Command::Help);
}

#[test]
fn parses_help() {
    assert_eq!(parse(args(&["midy", "--help"])).unwrap(), Command::Help);
    assert_eq!(parse(args(&["midy", "-h"])).unwrap(), Command::Help);
}

#[test]
fn parses_man() {
    assert_eq!(parse(args(&["midy", "--man"])).unwrap(), Command::Man);
    assert_eq!(parse(args(&["midy", "man"])).unwrap(), Command::Man);
}

#[test]
fn parses_version() {
    assert_eq!(
        parse(args(&["midy", "--version"])).unwrap(),
        Command::Version
    );
    assert_eq!(parse(args(&["midy", "-V"])).unwrap(), Command::Version);
}

#[test]
fn parses_read() {
    assert_eq!(
        parse(args(&["midy", "read", "song.mid"])).unwrap(),
        Command::Read {
            input: "song.mid".into(),
            format: ReadFormat::Midy,
            options: midi::ReadOptions::default(),
        }
    );
    assert_eq!(
        parse(args(&["midy", "read", "song.mid", "--format", "json"])).unwrap(),
        Command::Read {
            input: "song.mid".into(),
            format: ReadFormat::Json,
            options: midi::ReadOptions::default(),
        }
    );
    assert_eq!(
        parse(args(&["midy", "read", "--format=csv", "song.mid"])).unwrap(),
        Command::Read {
            input: "song.mid".into(),
            format: ReadFormat::Csv,
            options: midi::ReadOptions::default(),
        }
    );
}

#[test]
fn parses_schema() {
    assert_eq!(parse(args(&["midy", "schema"])).unwrap(), Command::Schema);
    assert_eq!(parse(args(&["midy", "format"])).unwrap(), Command::Schema);
}

#[test]
fn parses_apply() {
    assert_eq!(
        parse(args(&[
            "midy",
            "apply",
            "song.mid",
            "edits.txt",
            "-o",
            "out.mid"
        ]))
        .unwrap(),
        Command::Apply {
            input: "song.mid".into(),
            edits: EditInput::File("edits.txt".into()),
            output: "out.mid".into(),
            format: midi::EditFormat::Auto,
            options: midi::ApplyOptions::default(),
            dry_run: false,
        }
    );
}

#[test]
fn parses_apply_with_stdin_and_output_path() {
    assert_eq!(
        parse(args(&["midy", "apply", "song.mid", "out.mid"])).unwrap(),
        Command::Apply {
            input: "song.mid".into(),
            edits: EditInput::Stdin,
            output: "out.mid".into(),
            format: midi::EditFormat::Auto,
            options: midi::ApplyOptions::default(),
            dry_run: false,
        }
    );
}

#[test]
fn parses_apply_with_stdin_and_overwrite() {
    assert_eq!(
        parse(args(&["midy", "apply", "song.mid"])).unwrap(),
        Command::Apply {
            input: "song.mid".into(),
            edits: EditInput::Stdin,
            output: "song.mid".into(),
            format: midi::EditFormat::Auto,
            options: midi::ApplyOptions::default(),
            dry_run: false,
        }
    );
}

#[test]
fn parses_apply_with_stdin_and_dash_edit_file() {
    assert_eq!(
        parse(args(&["midy", "apply", "song.mid", "-", "-o", "out.mid"])).unwrap(),
        Command::Apply {
            input: "song.mid".into(),
            edits: EditInput::Stdin,
            output: "out.mid".into(),
            format: midi::EditFormat::Auto,
            options: midi::ApplyOptions::default(),
            dry_run: false,
        }
    );
}

#[test]
fn parses_suggest_reduce_chords() {
    assert_eq!(
        parse(args(&[
            "midy",
            "suggest",
            "reduce-chords",
            "song.mid",
            "--keep",
            "lowest",
            "--track=1",
            "--window",
            "8",
        ]))
        .unwrap(),
        Command::SuggestReduceChords {
            input: "song.mid".into(),
            options: midi::ReduceChordsOptions {
                keep: midi::ChordKeep::Lowest,
                query: midi::QueryOptions {
                    track: Some(1),
                    ..midi::QueryOptions::default()
                },
                window: 8,
            },
        }
    );
}

#[test]
fn parses_suggest_bass() {
    assert_eq!(
        parse(args(&[
            "midy",
            "suggest",
            "bass",
            "song.mid",
            "--track",
            "1",
            "--out-track",
            "2",
            "--out-ch",
            "0",
            "--octave",
            "2",
            "--vel",
            "90",
            "--dur",
            "1/4",
        ]))
        .unwrap(),
        Command::SuggestBass {
            input: "song.mid".into(),
            options: midi::BasslineOptions {
                query: midi::QueryOptions {
                    track: Some(1),
                    ..midi::QueryOptions::default()
                },
                output_track: Some(2),
                output_channel: 0,
                octave: 2,
                velocity: 90,
                duration: Some("1/4".to_owned()),
                ..midi::BasslineOptions::default()
            },
        }
    );
}

#[test]
fn parses_chords_roll_and_analyze() {
    assert!(matches!(
        parse(args(&["midy", "chords", "song.mid", "--bars", "1..4"])).unwrap(),
        Command::Chords { .. }
    ));
    assert!(matches!(
        parse(args(&["midy", "roll", "song.mid", "--grid", "1/16"])).unwrap(),
        Command::Roll { .. }
    ));
    assert_eq!(
        parse(args(&["midy", "roll", "song.mid", "--mode", "verbose"])).unwrap(),
        Command::Roll {
            input: "song.mid".into(),
            options: midi::RollOptions {
                mode: midi::RollMode::Verbose,
                ..midi::RollOptions::default()
            },
        }
    );
    assert_eq!(
        parse(args(&["midy", "analyze", "song.mid"])).unwrap(),
        Command::Analyze {
            input: "song.mid".into(),
            json: false,
        }
    );
    assert_eq!(
        parse(args(&["midy", "analyze", "song.mid", "--json"])).unwrap(),
        Command::Analyze {
            input: "song.mid".into(),
            json: true,
        }
    );
    assert_eq!(
        parse(args(&["midy", "tracks", "song.mid"])).unwrap(),
        Command::Tracks {
            input: "song.mid".into()
        }
    );
    assert_eq!(
        parse(args(&["midy", "lint", "song.mid"])).unwrap(),
        Command::Lint {
            input: "song.mid".into(),
            exit_code: false,
        }
    );
    assert_eq!(
        parse(args(&["midy", "fix", "song.mid", "fixed.mid"])).unwrap(),
        Command::Fix {
            input: "song.mid".into(),
            output: "fixed.mid".into(),
            options: midi::FixOptions::default(),
        }
    );
    assert_eq!(
        parse(args(&[
            "midy",
            "fix",
            "song.mid",
            "fixed.mid",
            "--stuck-mode",
            "close",
            "--stuck-duration",
            "240",
            "--remove-empty-tracks",
        ]))
        .unwrap(),
        Command::Fix {
            input: "song.mid".into(),
            output: "fixed.mid".into(),
            options: midi::FixOptions {
                stuck_note_mode: midi::StuckNoteFixMode::Close,
                stuck_note_duration: 240,
                remove_empty_tracks: true,
            },
        }
    );
}

#[test]
fn parses_humanize_dehumanize_and_swing() {
    assert_eq!(
        parse(args(&[
            "midy",
            "humanize",
            "song.mid",
            "human.mid",
            "--timing",
            "12",
            "--velocity",
            "8",
            "--seed",
            "2",
            "--track",
            "1",
        ]))
        .unwrap(),
        Command::Humanize {
            input: "song.mid".into(),
            output: "human.mid".into(),
            edit: "HUMANIZE timing=12 velocity=8 seed=2 track=1\n".to_owned(),
        }
    );
    assert_eq!(
        parse(args(&[
            "midy",
            "dehumanize",
            "song.mid",
            "tight.mid",
            "--grid",
            "1/16",
            "--mode",
            "both",
        ]))
        .unwrap(),
        Command::Dehumanize {
            input: "song.mid".into(),
            output: "tight.mid".into(),
            edit: "DEHUMANIZE grid=1/16 mode=both\n".to_owned(),
        }
    );
    assert_eq!(
        parse(args(&[
            "midy",
            "swing",
            "song.mid",
            "swung.mid",
            "--amount",
            "55",
            "--grid",
            "1/8",
            "--bars",
            "1..4",
        ]))
        .unwrap(),
        Command::Swing {
            input: "song.mid".into(),
            output: "swung.mid".into(),
            edit: "SWING amount=55 grid=1/8 bars=1..4\n".to_owned(),
        }
    );
}

#[test]
fn parses_chordize_and_arpeggiate() {
    assert_eq!(
        parse(args(&[
            "midy",
            "chordize",
            "melody.mid",
            "chords.mid",
            "--quality",
            "min7",
            "--track",
            "1",
        ]))
        .unwrap(),
        Command::Chordize {
            input: "melody.mid".into(),
            output: "chords.mid".into(),
            edit: "CHORDIZE quality=min7 track=1\n".to_owned(),
        }
    );
    assert_eq!(
        parse(args(&[
            "midy",
            "arpeggiate",
            "chords.mid",
            "arp.mid",
            "--grid",
            "1/16",
            "--order",
            "updown",
            "--bars",
            "1..4",
        ]))
        .unwrap(),
        Command::Arpeggiate {
            input: "chords.mid".into(),
            output: "arp.mid".into(),
            edit: "ARPEGGIATE grid=1/16 order=updown bars=1..4\n".to_owned(),
        }
    );
}

#[test]
fn parses_track_channel_transforms() {
    assert_eq!(
        parse(args(&[
            "midy", "extract", "song.mid", "--track", "1", "out.mid"
        ]))
        .unwrap(),
        Command::Extract {
            input: "song.mid".into(),
            output: "out.mid".into(),
            selector: midi::TrackChannelSelector {
                track: Some(1),
                channel: None,
            },
        }
    );
    assert_eq!(
        parse(args(&["midy", "mute", "song.mid", "--ch", "9", "out.mid"])).unwrap(),
        Command::Mute {
            input: "song.mid".into(),
            output: "out.mid".into(),
            selector: midi::TrackChannelSelector {
                track: None,
                channel: Some(9),
            },
        }
    );
    assert!(matches!(
        parse(args(&["midy", "solo", "song.mid", "--track=0", "out.mid"])).unwrap(),
        Command::Solo { .. }
    ));
}

#[test]
fn parses_split() {
    assert_eq!(
        parse(args(&[
            "midy",
            "split",
            "song.mid",
            "--by",
            "channel",
            "--out-dir",
            "split"
        ]))
        .unwrap(),
        Command::Split {
            input: "song.mid".into(),
            out_dir: "split".into(),
            mode: midi::SplitMode::Channel,
        }
    );
}

#[test]
fn parses_merge() {
    assert_eq!(
        parse(args(&[
            "midy",
            "merge",
            "drums.mid",
            "bass.mid",
            "chords.mid",
            "-o",
            "song.mid",
        ]))
        .unwrap(),
        Command::Merge {
            inputs: vec!["drums.mid".into(), "bass.mid".into(), "chords.mid".into()],
            output: "song.mid".into(),
        }
    );
    assert!(parse(args(&["midy", "merge", "only.mid", "-o", "out.mid"])).is_err());
    assert!(parse(args(&["midy", "merge", "a.mid", "b.mid"])).is_err());
}

#[test]
fn parses_diff() {
    assert_eq!(
        parse(args(&["midy", "diff", "before.mid", "after.mid"])).unwrap(),
        Command::Diff {
            before: "before.mid".into(),
            after: "after.mid".into(),
            exit_code: false,
        }
    );
}

#[test]
fn parses_render() {
    assert_eq!(
        parse(args(&[
            "midy",
            "render",
            "song.mid",
            "song.flac",
            "--soundfont",
            "gm.sf2",
            "--synth",
            "fluidsynth",
            "--sample-rate",
            "48000",
        ]))
        .unwrap(),
        Command::Render {
            input: "song.mid".into(),
            output: "song.flac".into(),
            soundfont: "gm.sf2".into(),
            synth: "fluidsynth".to_owned(),
            sample_rate: 48_000,
        }
    );
    assert!(parse(args(&["midy", "render", "song.mid", "song.wav"])).is_err());
}

#[test]
fn rejects_unknown_args() {
    assert!(parse(args(&["midy", "inspect"])).is_err());
    assert!(parse(args(&["midy", "apply", "song.mid", "edits.txt", "out.mid"])).is_err());
}
