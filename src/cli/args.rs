//! Command-line argument parsing.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::text::closest_match;
use crate::{Error, Result, midi};

use super::{check_edit_paths, has_midi_extension};

#[derive(Debug, Eq, PartialEq)]
pub(super) enum Command {
    Help,
    Man,
    Version,
    Schema,
    Read {
        input: PathBuf,
        format: ReadFormat,
        options: midi::ReadOptions,
    },
    Apply {
        input: PathBuf,
        edits: EditInput,
        output: PathBuf,
        format: midi::EditFormat,
        options: midi::ApplyOptions,
        dry_run: bool,
    },
    SuggestReduceChords {
        input: PathBuf,
        options: midi::ReduceChordsOptions,
    },
    SuggestBass {
        input: PathBuf,
        options: midi::BasslineOptions,
    },
    Chords {
        input: PathBuf,
        options: midi::ChordsOptions,
    },
    Roll {
        input: PathBuf,
        options: midi::RollOptions,
    },
    Analyze {
        input: PathBuf,
        json: bool,
    },
    Tracks {
        input: PathBuf,
    },
    Lint {
        input: PathBuf,
        exit_code: bool,
    },
    Fix {
        input: PathBuf,
        output: PathBuf,
        options: midi::FixOptions,
    },
    Humanize {
        input: PathBuf,
        output: PathBuf,
        edit: String,
    },
    Dehumanize {
        input: PathBuf,
        output: PathBuf,
        edit: String,
    },
    Swing {
        input: PathBuf,
        output: PathBuf,
        edit: String,
    },
    Chordize {
        input: PathBuf,
        output: PathBuf,
        edit: String,
    },
    Arpeggiate {
        input: PathBuf,
        output: PathBuf,
        edit: String,
    },
    Extract {
        input: PathBuf,
        output: PathBuf,
        selector: midi::TrackChannelSelector,
    },
    Mute {
        input: PathBuf,
        output: PathBuf,
        selector: midi::TrackChannelSelector,
    },
    Solo {
        input: PathBuf,
        output: PathBuf,
        selector: midi::TrackChannelSelector,
    },
    Split {
        input: PathBuf,
        out_dir: PathBuf,
        mode: midi::SplitMode,
    },
    Merge {
        inputs: Vec<PathBuf>,
        output: PathBuf,
    },
    Diff {
        before: PathBuf,
        after: PathBuf,
        exit_code: bool,
    },
    Render {
        input: PathBuf,
        output: PathBuf,
        soundfont: PathBuf,
        synth: String,
        sample_rate: u32,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum EditInput {
    File(PathBuf),
    Stdin,
    Inline(Vec<String>),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum ReadFormat {
    Midy,
    Json,
    Csv,
}

const QUERY_OPTIONS: &[&str] = &["track", "ch", "key", "start", "end", "bars"];
const EDIT_FILTER_OPTIONS: &[&str] =
    &["track", "ch", "key", "start", "end", "bar", "bars", "drums"];
const ALIASES: &[(&str, &str)] = &[
    ("channel", "ch"),
    ("velocity", "vel"),
    ("duration", "dur"),
    ("out-channel", "out-ch"),
    ("sf2", "soundfont"),
    ("output", "o"),
    ("edit", "e"),
];

/// The options one command accepts.
#[derive(Debug, Clone, Copy, Default)]
struct OptionSpec<'a> {
    command: &'a str,
    values: &'a [&'a [&'a str]],
    flags: &'a [&'a str],
    repeated: &'a [&'a str],
}

#[derive(Debug, Default)]
struct ParsedOptions {
    positional: Vec<String>,
    values: HashMap<String, String>,
    flags: HashSet<String>,
    repeated: Vec<String>,
}

impl ParsedOptions {
    fn value(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    fn flag(&self, key: &str) -> bool {
        self.flags.contains(key)
    }

    fn path(&self, index: usize) -> PathBuf {
        self.positional[index].clone().into()
    }
}

fn canonical_option(name: &str) -> String {
    let name = name.replace('_', "-");
    ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name, |(_, canonical)| (*canonical).to_owned())
}

fn parse_options(rest: &[String], spec: OptionSpec<'_>) -> Result<ParsedOptions> {
    let mut parsed = ParsedOptions::default();
    let takes_value = |name: &str| {
        spec.values
            .iter()
            .flat_map(|group| group.iter())
            .any(|known| *known == name)
            || spec.repeated.contains(&name)
    };

    let mut index = 0;
    while index < rest.len() {
        let arg = &rest[index];
        index += 1;
        let (raw, inline) = if let Some(long) = arg.strip_prefix("--") {
            match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            }
        } else if arg == "-" || !arg.starts_with('-') {
            parsed.positional.push(arg.clone());
            continue;
        } else {
            (arg.trim_start_matches('-'), None)
        };

        let name = canonical_option(raw);
        if spec.flags.contains(&name.as_str()) {
            if inline.is_some() {
                return Err(Error::Usage(format!("--{raw} does not take a value")));
            }
            parsed.flags.insert(name);
        } else if takes_value(&name) {
            let value = match inline {
                Some(value) => value.to_owned(),
                None => {
                    let value = rest
                        .get(index)
                        .ok_or_else(|| Error::Usage(format!("missing value after '{arg}'")))?;
                    index += 1;
                    value.clone()
                }
            };
            if spec.repeated.contains(&name.as_str()) {
                parsed.repeated.push(value);
            } else if parsed.values.insert(name.clone(), value).is_some() {
                return Err(Error::Usage(format!("duplicate option '{arg}'")));
            }
        } else {
            return Err(unknown_option(arg, raw, spec));
        }
    }

    Ok(parsed)
}

fn unknown_option(arg: &str, raw: &str, spec: OptionSpec<'_>) -> Error {
    let known = spec
        .values
        .iter()
        .flat_map(|group| group.iter().copied())
        .chain(spec.flags.iter().copied())
        .chain(spec.repeated.iter().copied());
    let hint = closest_match(raw, known)
        .map(|name| format!("; did you mean '--{name}'?"))
        .unwrap_or_default();
    Error::Usage(format!("unknown option '{arg}' for {}{hint}", spec.command))
}

pub(super) fn parse(args: Vec<String>) -> Result<Command> {
    let Some(command) = args.get(1) else {
        return Ok(Command::Help);
    };
    let rest = &args[2..];
    match command.as_str() {
        "-h" | "--help" | "help" if rest.is_empty() => Ok(Command::Help),
        "--man" | "man" if rest.is_empty() => Ok(Command::Man),
        "-V" | "--version" if rest.is_empty() => Ok(Command::Version),
        "schema" | "format" if rest.is_empty() => Ok(Command::Schema),
        "read" | "dump" => parse_read(rest),
        "apply" | "write" => parse_apply(rest),
        "suggest" => parse_suggest(rest),
        "chords" => parse_chords(rest),
        "roll" => parse_roll(rest),
        "analyze" | "analyse" => parse_analyze(rest),
        "tracks" => parse_tracks(rest),
        "lint" => parse_lint(rest),
        "fix" => parse_fix(rest),
        "humanize" | "humanise" => parse_humanize(rest),
        "dehumanize" | "dehumanise" => parse_dehumanize(rest),
        "swing" => parse_swing(rest),
        "chordize" | "chordise" => parse_chordize(rest),
        "arpeggiate" | "arp" => parse_arpeggiate(rest),
        "extract" | "mute" | "solo" => parse_transform(rest, command),
        "split" => parse_split(rest),
        "merge" => parse_merge(rest),
        "diff" => parse_diff(rest),
        "render" => parse_render(rest),
        unknown => {
            let hint = closest_match(unknown, COMMANDS.iter().copied())
                .map(|name| format!("did you mean '{name}'? "))
                .unwrap_or_default();
            Err(Error::Usage(format!(
                "unknown argument '{unknown}'. {hint}Use 'midy --help' for usage."
            )))
        }
    }
}

const COMMANDS: &[&str] = &[
    "read",
    "apply",
    "write",
    "suggest",
    "chords",
    "roll",
    "analyze",
    "tracks",
    "lint",
    "fix",
    "humanize",
    "dehumanize",
    "swing",
    "chordize",
    "arpeggiate",
    "extract",
    "mute",
    "solo",
    "split",
    "merge",
    "diff",
    "render",
    "schema",
];

fn parse_read(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(
        rest,
        OptionSpec {
            command: "read",
            values: &[QUERY_OPTIONS, &["format", "events"]],
            flags: &["compact"],
            ..OptionSpec::default()
        },
    )?;
    if parsed.positional.len() != 1 {
        return Err(Error::Usage(
            "read expects <input.mid> with optional --format midy|json|csv".to_owned(),
        ));
    }
    let format = match parsed.value("format").unwrap_or("midy") {
        "midy" | "ascii" | "text" => ReadFormat::Midy,
        "json" => ReadFormat::Json,
        "csv" => ReadFormat::Csv,
        value => {
            return Err(Error::Usage(format!(
                "--format must be midy, json, or csv, got '{value}'"
            )));
        }
    };
    let events = match parsed.value("events").unwrap_or("all") {
        "all" => midi::EventDetail::All,
        "basic" => midi::EventDetail::Basic,
        "none" => midi::EventDetail::None,
        value => {
            return Err(Error::Usage(format!(
                "--events must be all, basic, or none, got '{value}'"
            )));
        }
    };
    Ok(Command::Read {
        input: parsed.path(0),
        format,
        options: midi::ReadOptions {
            query: parse_query_options(&parsed)?,
            events,
            compact: parsed.flag("compact"),
        },
    })
}

fn parse_suggest(rest: &[String]) -> Result<Command> {
    let Some(kind) = rest.first() else {
        return Err(Error::Usage(
            "suggest expects a kind, for example: suggest reduce-chords <input.mid>".to_owned(),
        ));
    };
    match kind.as_str() {
        "reduce-chords" | "reduce_chords" => {
            let parsed = parse_options(
                &rest[1..],
                OptionSpec {
                    command: "suggest reduce-chords",
                    values: &[QUERY_OPTIONS, &["keep", "window"]],
                    flags: &["include-drums"],
                    ..OptionSpec::default()
                },
            )?;
            if parsed.positional.len() != 1 {
                return Err(Error::Usage(
                    "suggest reduce-chords expects <input.mid>".to_owned(),
                ));
            }
            Ok(Command::SuggestReduceChords {
                input: parsed.path(0),
                options: midi::ReduceChordsOptions {
                    keep: parse_keep(parsed.value("keep").unwrap_or("highest"))?,
                    query: parse_query_options(&parsed)?,
                    window: parse_number(parsed.value("window"), "window")?.unwrap_or(0),
                },
            })
        }
        "bass" | "bassline" | "bass-line" => {
            let parsed = parse_options(
                &rest[1..],
                OptionSpec {
                    command: "suggest bass",
                    values: &[
                        QUERY_OPTIONS,
                        &["window", "out-track", "out-ch", "octave", "vel", "dur"],
                    ],
                    flags: &["include-drums"],
                    ..OptionSpec::default()
                },
            )?;
            if parsed.positional.len() != 1 {
                return Err(Error::Usage("suggest bass expects <input.mid>".to_owned()));
            }
            let velocity = parse_u7(parsed.value("vel"), "vel", 127)?.unwrap_or(80);
            if velocity == 0 {
                return Err(Error::Usage("--vel must be 1..127".to_owned()));
            }
            Ok(Command::SuggestBass {
                input: parsed.path(0),
                options: midi::BasslineOptions {
                    query: parse_query_options(&parsed)?,
                    window: parse_number(parsed.value("window"), "window")?.unwrap_or(0),
                    output_track: parse_number(parsed.value("out-track"), "out-track")?,
                    output_channel: parse_u7(parsed.value("out-ch"), "out-ch", 15)?.unwrap_or(0),
                    octave: parse_number(parsed.value("octave"), "octave")?.unwrap_or(2),
                    velocity,
                    duration: parsed.value("dur").map(ToOwned::to_owned),
                },
            })
        }
        unknown => Err(Error::Usage(format!(
            "unknown suggest kind '{unknown}'. Try 'suggest reduce-chords' or 'suggest bass'."
        ))),
    }
}

fn parse_chords(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(
        rest,
        OptionSpec {
            command: "chords",
            values: &[QUERY_OPTIONS, &["window"]],
            flags: &["include-drums"],
            ..OptionSpec::default()
        },
    )?;
    if parsed.positional.len() != 1 {
        return Err(Error::Usage(
            "chords expects <input.mid> with optional --track/--ch/--start/--end/--bars/--window"
                .to_owned(),
        ));
    }
    Ok(Command::Chords {
        input: parsed.path(0),
        options: midi::ChordsOptions {
            query: parse_query_options(&parsed)?,
            window: parse_number(parsed.value("window"), "window")?.unwrap_or(0),
        },
    })
}

fn parse_roll(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(
        rest,
        OptionSpec {
            command: "roll",
            values: &[QUERY_OPTIONS, &["grid", "mode", "max-columns"]],
            ..OptionSpec::default()
        },
    )?;
    if parsed.positional.len() != 1 {
        return Err(Error::Usage(
            "roll expects <input.mid> with optional --track/--ch/--start/--end/--bars/--grid"
                .to_owned(),
        ));
    }
    Ok(Command::Roll {
        input: parsed.path(0),
        options: midi::RollOptions {
            query: parse_query_options(&parsed)?,
            grid: parsed.value("grid").map(ToOwned::to_owned),
            mode: parse_roll_mode(parsed.value("mode").unwrap_or("compact"))?,
            max_columns: parse_number(parsed.value("max-columns"), "max-columns")?
                .unwrap_or(midi::DEFAULT_ROLL_COLUMNS),
        },
    })
}

fn parse_roll_mode(value: &str) -> Result<midi::RollMode> {
    match value {
        "compact" => Ok(midi::RollMode::Compact),
        "verbose" => Ok(midi::RollMode::Verbose),
        value => Err(Error::Usage(format!(
            "--mode must be compact or verbose, got '{value}'"
        ))),
    }
}

fn parse_analyze(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(
        rest,
        OptionSpec {
            command: "analyze",
            flags: &["json"],
            ..OptionSpec::default()
        },
    )?;
    if parsed.positional.len() != 1 {
        return Err(Error::Usage(
            "analyze expects <input.mid> with optional --json".to_owned(),
        ));
    }
    Ok(Command::Analyze {
        input: parsed.path(0),
        json: parsed.flag("json"),
    })
}

fn parse_tracks(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(
        rest,
        OptionSpec {
            command: "tracks",
            ..OptionSpec::default()
        },
    )?;
    if parsed.positional.len() != 1 {
        return Err(Error::Usage("tracks expects <input.mid>".to_owned()));
    }
    Ok(Command::Tracks {
        input: parsed.path(0),
    })
}

fn parse_lint(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(
        rest,
        OptionSpec {
            command: "lint",
            flags: &["exit-code"],
            ..OptionSpec::default()
        },
    )?;
    if parsed.positional.len() != 1 {
        return Err(Error::Usage(
            "lint expects <input.mid> with optional --exit-code".to_owned(),
        ));
    }
    Ok(Command::Lint {
        input: parsed.path(0),
        exit_code: parsed.flag("exit-code"),
    })
}

fn parse_fix(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(
        rest,
        OptionSpec {
            command: "fix",
            values: &[&["stuck-mode", "stuck-duration"]],
            flags: &["remove-empty-tracks"],
            ..OptionSpec::default()
        },
    )?;
    if parsed.positional.len() != 2 {
        return Err(Error::Usage(
            "fix expects <input.mid> <output.mid> [--stuck-mode remove|close] [--remove-empty-tracks]"
                .to_owned(),
        ));
    }
    let stuck_note_mode = match parsed.value("stuck-mode").unwrap_or("remove") {
        "remove" => midi::StuckNoteFixMode::Remove,
        "close" => midi::StuckNoteFixMode::Close,
        value => {
            return Err(Error::Usage(format!(
                "--stuck-mode must be remove or close, got '{value}'"
            )));
        }
    };
    Ok(Command::Fix {
        input: parsed.path(0),
        output: parsed.path(1),
        options: midi::FixOptions {
            stuck_note_mode,
            stuck_note_duration: parse_number(parsed.value("stuck-duration"), "stuck-duration")?
                .unwrap_or(120),
            remove_empty_tracks: parsed.flag("remove-empty-tracks"),
        },
    })
}

fn edit_command_options(
    rest: &[String],
    command: &str,
    values: &[&str],
    usage: &str,
) -> Result<ParsedOptions> {
    let parsed = parse_options(
        rest,
        OptionSpec {
            command,
            values: &[EDIT_FILTER_OPTIONS, values],
            ..OptionSpec::default()
        },
    )?;
    if parsed.positional.len() != 2 {
        return Err(Error::Usage(usage.to_owned()));
    }
    Ok(parsed)
}

fn parse_humanize(rest: &[String]) -> Result<Command> {
    let parsed = edit_command_options(
        rest,
        "humanize",
        &["timing", "vel", "seed"],
        "humanize expects <input.mid> <output.mid> [--timing N] [--velocity N] [--seed N]",
    )?;
    let mut edit = format!(
        "HUMANIZE timing={} velocity={} seed={}",
        parsed.value("timing").unwrap_or("12"),
        parsed.value("vel").unwrap_or("8"),
        parsed.value("seed").unwrap_or("1"),
    );
    append_edit_filters(&mut edit, &parsed);
    Ok(Command::Humanize {
        input: parsed.path(0),
        output: parsed.path(1),
        edit,
    })
}

fn parse_dehumanize(rest: &[String]) -> Result<Command> {
    let parsed = edit_command_options(
        rest,
        "dehumanize",
        &["grid", "mode"],
        "dehumanize expects <input.mid> <output.mid> [--grid GRID] [--mode start|duration|both]",
    )?;
    let mut edit = format!(
        "DEHUMANIZE grid={} mode={}",
        parsed.value("grid").unwrap_or("1/16"),
        parsed.value("mode").unwrap_or("start"),
    );
    append_edit_filters(&mut edit, &parsed);
    Ok(Command::Dehumanize {
        input: parsed.path(0),
        output: parsed.path(1),
        edit,
    })
}

fn parse_swing(rest: &[String]) -> Result<Command> {
    let parsed = edit_command_options(
        rest,
        "swing",
        &["amount", "grid"],
        "swing expects <input.mid> <output.mid> [--amount 50..100] [--grid GRID]",
    )?;
    let mut edit = format!(
        "SWING amount={} grid={}",
        parsed.value("amount").unwrap_or("55"),
        parsed.value("grid").unwrap_or("1/8"),
    );
    append_edit_filters(&mut edit, &parsed);
    Ok(Command::Swing {
        input: parsed.path(0),
        output: parsed.path(1),
        edit,
    })
}

fn parse_chordize(rest: &[String]) -> Result<Command> {
    let parsed = edit_command_options(
        rest,
        "chordize",
        &["quality", "intervals"],
        "chordize expects <input.mid> <output.mid> [--quality QUALITY|--intervals 0,4,7]",
    )?;
    let mut edit = if let Some(intervals) = parsed.value("intervals") {
        format!("CHORDIZE intervals={intervals}")
    } else {
        format!(
            "CHORDIZE quality={}",
            parsed.value("quality").unwrap_or("maj")
        )
    };
    append_edit_filters(&mut edit, &parsed);
    Ok(Command::Chordize {
        input: parsed.path(0),
        output: parsed.path(1),
        edit,
    })
}

fn parse_arpeggiate(rest: &[String]) -> Result<Command> {
    let parsed = edit_command_options(
        rest,
        "arpeggiate",
        &["grid", "order"],
        "arpeggiate expects <input.mid> <output.mid> [--grid GRID] [--order up|down|updown|outside]",
    )?;
    let mut edit = format!(
        "ARPEGGIATE grid={} order={}",
        parsed.value("grid").unwrap_or("1/16"),
        parsed.value("order").unwrap_or("up"),
    );
    append_edit_filters(&mut edit, &parsed);
    Ok(Command::Arpeggiate {
        input: parsed.path(0),
        output: parsed.path(1),
        edit,
    })
}

fn append_edit_filters(edit: &mut String, parsed: &ParsedOptions) {
    for key in EDIT_FILTER_OPTIONS {
        if let Some(value) = parsed.value(key) {
            edit.push(' ');
            edit.push_str(key);
            edit.push('=');
            edit.push_str(value);
        }
    }
    edit.push('\n');
}

fn parse_transform(rest: &[String], command: &str) -> Result<Command> {
    let parsed = parse_options(
        rest,
        OptionSpec {
            command,
            values: &[&["track", "ch"]],
            ..OptionSpec::default()
        },
    )?;
    if parsed.positional.len() != 2 {
        return Err(Error::Usage(format!(
            "{command} expects <input.mid> (--track N|--ch N) <output.mid>"
        )));
    }
    let selector = parse_track_channel_selector(&parsed)?;
    let input = parsed.path(0);
    let output = parsed.path(1);
    match command {
        "extract" => Ok(Command::Extract {
            input,
            output,
            selector,
        }),
        "mute" => Ok(Command::Mute {
            input,
            output,
            selector,
        }),
        _ => Ok(Command::Solo {
            input,
            output,
            selector,
        }),
    }
}

fn parse_split(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(
        rest,
        OptionSpec {
            command: "split",
            values: &[&["by", "out-dir"]],
            ..OptionSpec::default()
        },
    )?;
    if parsed.positional.len() != 1 {
        return Err(Error::Usage(
            "split expects <input.mid> --by track|channel --out-dir <dir>".to_owned(),
        ));
    }
    let mode = match parsed.value("by").unwrap_or("track") {
        "track" | "tracks" => midi::SplitMode::Track,
        "channel" | "channels" | "ch" => midi::SplitMode::Channel,
        value => {
            return Err(Error::Usage(format!(
                "--by must be track or channel, got '{value}'"
            )));
        }
    };
    let out_dir = parsed
        .value("out-dir")
        .ok_or_else(|| Error::Usage("split requires --out-dir <dir>".to_owned()))?;

    Ok(Command::Split {
        input: parsed.path(0),
        out_dir: out_dir.into(),
        mode,
    })
}

fn parse_merge(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(
        rest,
        OptionSpec {
            command: "merge",
            values: &[&["o"]],
            ..OptionSpec::default()
        },
    )?;
    if parsed.positional.len() < 2 {
        return Err(Error::Usage(
            "merge expects at least two input MIDI files".to_owned(),
        ));
    }
    let output = parsed
        .value("o")
        .ok_or_else(|| Error::Usage("merge requires -o <output.mid>".to_owned()))?;
    Ok(Command::Merge {
        inputs: parsed.positional.iter().map(PathBuf::from).collect(),
        output: output.into(),
    })
}

fn parse_diff(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(
        rest,
        OptionSpec {
            command: "diff",
            flags: &["exit-code"],
            ..OptionSpec::default()
        },
    )?;
    if parsed.positional.len() != 2 {
        return Err(Error::Usage(
            "diff expects <before.mid> <after.mid> with optional --exit-code".to_owned(),
        ));
    }
    Ok(Command::Diff {
        before: parsed.path(0),
        after: parsed.path(1),
        exit_code: parsed.flag("exit-code"),
    })
}

fn parse_render(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(
        rest,
        OptionSpec {
            command: "render",
            values: &[&["soundfont", "synth", "sample-rate"]],
            ..OptionSpec::default()
        },
    )?;
    if parsed.positional.len() != 2 {
        return Err(Error::Usage(
            "render expects <input.mid> <output.wav|output.flac> --soundfont <file.sf2>".to_owned(),
        ));
    }
    let soundfont = parsed
        .value("soundfont")
        .ok_or_else(|| Error::Usage("render requires --soundfont <file.sf2>".to_owned()))?;
    let sample_rate: u32 =
        parse_number(parsed.value("sample-rate"), "sample-rate")?.unwrap_or(44_100);
    if sample_rate == 0 {
        return Err(Error::Usage(
            "--sample-rate must be greater than zero".to_owned(),
        ));
    }
    Ok(Command::Render {
        input: parsed.path(0),
        output: parsed.path(1),
        soundfont: soundfont.into(),
        synth: parsed.value("synth").unwrap_or("fluidsynth").to_owned(),
        sample_rate,
    })
}

fn parse_query_options(parsed: &ParsedOptions) -> Result<midi::QueryOptions> {
    Ok(midi::QueryOptions {
        track: parse_number(parsed.value("track"), "track")?,
        channel: parse_u7(parsed.value("ch"), "ch", 15)?,
        key: parsed
            .value("key")
            .map(|value| {
                midi::parse_note_key(value)
                    .map_err(|message| Error::Usage(format!("--key {message}")))
            })
            .transpose()?,
        start: parse_number(parsed.value("start"), "start")?,
        end: parse_number(parsed.value("end"), "end")?,
        bars: parsed.value("bars").map(ToOwned::to_owned),
        include_drums: parsed.flag("include-drums"),
    })
}

fn parse_track_channel_selector(parsed: &ParsedOptions) -> Result<midi::TrackChannelSelector> {
    let selector = midi::TrackChannelSelector {
        track: parse_number(parsed.value("track"), "track")?,
        channel: parse_u7(parsed.value("ch"), "ch", 15)?,
    };
    if selector.track.is_none() && selector.channel.is_none() {
        Err(Error::Usage(
            "select at least one of --track or --ch/--channel".to_owned(),
        ))
    } else {
        Ok(selector)
    }
}

fn parse_keep(value: &str) -> Result<midi::ChordKeep> {
    match value {
        "highest" | "top" => Ok(midi::ChordKeep::Highest),
        "lowest" | "bottom" => Ok(midi::ChordKeep::Lowest),
        "root" => Ok(midi::ChordKeep::Root),
        value if value.starts_with("nth=") => {
            let nth = value
                .trim_start_matches("nth=")
                .parse::<usize>()
                .map_err(|_| Error::Usage(format!("invalid keep value '{value}'")))?;
            if nth == 0 {
                Err(Error::Usage("nth keep value is 1-based".to_owned()))
            } else {
                Ok(midi::ChordKeep::Nth(nth))
            }
        }
        value => Err(Error::Usage(format!(
            "invalid keep value '{value}', expected highest, lowest, root, or nth=N"
        ))),
    }
}

fn parse_number<T: std::str::FromStr>(value: Option<&str>, name: &str) -> Result<Option<T>> {
    value
        .map(|value| {
            value
                .parse::<T>()
                .map_err(|_| Error::Usage(format!("--{name} must be an integer")))
        })
        .transpose()
}

fn parse_u7(value: Option<&str>, name: &str, max: u8) -> Result<Option<u8>> {
    let parsed = parse_number::<u8>(value, name)?;
    if parsed.is_some_and(|parsed| parsed > max) {
        return Err(Error::Usage(format!("--{name} must be <= {max}")));
    }
    Ok(parsed)
}

fn parse_apply(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(
        rest,
        OptionSpec {
            command: "apply",
            values: &[&["o", "format"]],
            flags: &["dry-run", "strict", "prune"],
            repeated: &["e"],
        },
    )?;
    let dry_run = parsed.flag("dry-run");
    let output = parsed.value("o").map(PathBuf::from);
    let inline = (!parsed.repeated.is_empty()).then(|| parsed.repeated.clone());
    let format = match parsed.value("format").unwrap_or("auto") {
        "auto" => midi::EditFormat::Auto,
        "text" | "midy" => midi::EditFormat::Text,
        "json" => midi::EditFormat::Json,
        "csv" => midi::EditFormat::Csv,
        value => {
            return Err(Error::Usage(format!(
                "--format must be auto, text, json, or csv, got '{value}'"
            )));
        }
    };

    let paths = parsed
        .positional
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let (input, edits, output) = match (paths.as_slice(), output, inline) {
        ([], ..) => {
            return Err(Error::Usage(
                "apply expects <input.mid>; pipe edits on stdin or use <input.mid> <edits.txt> -o <output.mid>"
                    .to_owned(),
            ));
        }
        ([input], output, inline) => {
            let target = output.unwrap_or_else(|| input.clone());
            let edits = inline.map_or(EditInput::Stdin, EditInput::Inline);
            (input.clone(), edits, target)
        }
        ([input, second], Some(output), None) => (input.clone(), parse_edit_input(second), output),
        ([_, _], Some(_), Some(_)) => {
            return Err(Error::Usage(
                "give edits once: as a file or with -e, not both".to_owned(),
            ));
        }
        ([input, second], None, inline) if has_midi_extension(second) => {
            let edits = inline.map_or(EditInput::Stdin, EditInput::Inline);
            (input.clone(), edits, second.clone())
        }
        ([input, second], None, None) if dry_run => {
            (input.clone(), parse_edit_input(second), input.clone())
        }
        ([input, second], None, _) => {
            return Err(Error::Usage(missing_output_message(input, second)));
        }
        _ => {
            return Err(Error::Usage(
                "too many apply arguments; use <input.mid> <edits.txt> -o <output.mid> or pipe edits into <input.mid> [output.mid]"
                    .to_owned(),
            ));
        }
    };

    if let EditInput::File(path) = &edits {
        check_edit_paths(path, &output)?;
    }
    Ok(Command::Apply {
        input,
        edits,
        output,
        format,
        options: midi::ApplyOptions {
            prune: parsed.flag("prune"),
            strict: parsed.flag("strict"),
        },
        dry_run,
    })
}

fn missing_output_message(input: &Path, second: &Path) -> String {
    format!(
        "'{second}' has no .mid extension, so it cannot be the stdin output path. To read edits from it, name the output: midy apply {input} {second} -o <output.mid>",
        second = second.display(),
        input = input.display(),
    )
}

fn parse_edit_input(path: &Path) -> EditInput {
    if path == Path::new("-") {
        EditInput::Stdin
    } else {
        EditInput::File(path.to_owned())
    }
}
