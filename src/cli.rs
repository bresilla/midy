use std::{io::Read, path::PathBuf};

use crate::{Error, Result, midi, name};

const HELP: &str = "\
midy

Usage:
  midy read <input.mid>
  midy apply <input.mid> <edits.txt> -o <output.mid>
  midy apply <input.mid> <output.mid>        # read edits from stdin
  midy apply <input.mid>                     # read edits from stdin and overwrite
  midy schema
  midy --man
  midy [OPTIONS]

Options:
  -h, --help       Show this help message
      --man        Show the detailed manual
  -V, --version    Show version information

Commands:
  read     Print a deterministic ASCII timeline for a MIDI file
  apply    Apply ASCII edit commands and write a new MIDI file
  write    Alias for apply
  schema   Print the ASCII edit format accepted by apply/write
";

const MANUAL: &str = "\
MIDY MANUAL
===========

Name
----
midy - convert MIDI files into editable ASCII timelines, then apply ASCII edits
back into MIDI files.

Purpose
-------
midy is designed as a bridge between binary MIDI data and text-based tools.
The main idea is:

  1. Read a .mid file.
  2. Print a stable, line-oriented ASCII timeline.
  3. Send that text to a human, script, or language model.
  4. Let that text system describe changes as edit lines.
  5. Apply those edits and write a new .mid file.

The ASCII format is intentionally plain text. It is not meant to be pretty
sheet music. It is meant to be explicit, deterministic, grep-friendly, and
easy for another machine to understand and rewrite.

Quick start
-----------
Print a MIDI file as ASCII:

  midy read song.mid

Save that ASCII to a file:

  midy read song.mid > song.midy.txt

Create a separate edit file:

  ADD_NOTE track=0 ch=0 key=60 start=0 dur=480 vel=96
  SET_NOTE id=t0n0 key=62 dur=240
  DELETE_NOTE id=t0n1
  TRANSPOSE semitones=2 track=0

Apply the edits and write a new MIDI file:

  midy apply song.mid edits.txt -o changed.mid

Or pipe edits through stdin and name only the input and output MIDI files:

  cat edits.txt | midy apply song.mid changed.mid

If you pipe edits and provide only the input MIDI file, midy overwrites that
input file after successfully parsing and rewriting the MIDI in memory:

  cat edits.txt | midy apply song.mid

The write command is an alias for apply:

  midy write song.mid edits.txt -o changed.mid

Commands
--------
midy --help
  Prints the short command summary.

midy --man
  Prints this detailed manual.

midy --version
  Prints the program version.

midy schema
  Prints the concise machine-readable edit grammar.

midy read <input.mid>
  Parses a MIDI file and prints the ASCII timeline to stdout.

midy apply <input.mid> <edits.txt> -o <output.mid>
  Parses the original MIDI file, reads edits from edits.txt, applies them, and
  writes output.mid.

midy apply <input.mid> <output.mid>
  Reads edit commands from stdin and writes the edited MIDI to output.mid.
  Example: cat edits.txt | midy apply input.mid output.mid

midy apply <input.mid>
  Reads edit commands from stdin and overwrites input.mid. The overwrite happens
  only after the input MIDI and edit text have both parsed successfully.

Timeline output
---------------
The first line identifies the format:

  MIDY_TIMELINE v1

The HEADER line describes the MIDI container:

  HEADER format=parallel timing=metrical ticks_per_beat=480 tracks=2

Important fields:

  format
    MIDI file layout: single, parallel, or sequential.

  timing=metrical ticks_per_beat=N
    The common musical timing mode. Tick positions in NOTE lines are integer
    MIDI ticks. If ticks_per_beat is 480, one quarter note is 480 ticks.

  tracks=N
    Number of MIDI tracks in the file.

The SONG line gives total length:

  SONG length_ticks=1920 length_beats=4.000

Track summaries look like:

  TRACK index=1 events=42 notes=12

Meta lines show important non-note MIDI events:

  META track=0 tick=0 kind=tempo us_per_quarter=500000 bpm=120.000
  META track=0 tick=0 kind=time_signature numerator=4 denominator=4 ...
  META track=0 tick=1920 kind=end_of_track

Program changes and system events are shown as EVENT lines:

  EVENT track=1 tick=0 kind=program_change ch=0 program=4

Notes are the main editable lines:

  NOTE id=t1n0 track=1 ch=0 key=60 name=C4 start=0 dur=480 end=480 pos=1:1:0 end_pos=1:2:0 vel=96 off_vel=64

NOTE fields
-----------
id=t1n0
  Stable note identifier for this read output. t1n0 means track 1, note 0.
  Use this id with SET_NOTE, DELETE_NOTE, or by editing the NOTE line itself.

track=1
  MIDI track index, zero-based.

ch=0
  MIDI channel, zero-based. Valid range is 0..15.

key=60
  MIDI note number. Valid range is 0..127. Middle C is commonly key 60.

name=C4
  Human-friendly note name derived from key. This is informational; midy uses
  key=N as the authority.

start=0
  Start time in MIDI ticks.

dur=480
  Duration in MIDI ticks.

end=480
  End time in ticks. For edited NOTE lines, midy uses start and dur.

pos=1:1:0
  Human-friendly bar:beat:tick position when the MIDI file has metrical timing.
  This is informational.

vel=96
  Note-on velocity. Valid range is 0..127.

off_vel=64
  Note-off velocity. Valid range is 0..127.

Editing workflow styles
-----------------------
You can edit MIDI in two ways.

1. Command file style:

  ADD_NOTE track=0 ch=0 key=64 start=480 dur=480 vel=90
  SET_NOTE id=t0n0 key=62 dur=240
  DELETE_NOTE id=t0n1

2. Modified timeline style:

  midy read song.mid > song.midy.txt

Then edit NOTE lines directly, for example changing:

  NOTE id=t0n0 track=0 ch=0 key=60 name=C4 start=0 dur=480 ...

to:

  NOTE id=t0n0 track=0 ch=0 key=62 name=D4 start=0 dur=240 ...

Then apply the edited timeline:

  midy apply song.mid song.midy.txt -o changed.mid

Or use a pipe instead of passing the timeline as an edit file:

  cat song.midy.txt | midy apply song.mid changed.mid

When applying a full timeline file, midy ignores read-only lines such as
HEADER, SONG, TRACK, META, and EVENT. Edited NOTE lines act like SET_NOTE.

Edit commands
-------------
ADD_NOTE
  Adds a new note.

  ADD_NOTE track=0 ch=0 key=60 start=0 dur=480 vel=96

  Optional:

  off_vel=64

SET_NOTE
  Changes an existing note by id. Only fields you provide are changed.

  SET_NOTE id=t0n0 key=62
  SET_NOTE id=t0n0 start=240 dur=120 vel=80
  SET_NOTE id=t0n0 track=1 ch=2 key=72 off_vel=0

NOTE
  A NOTE line with an id is accepted as an editable SET_NOTE line. This is what
  allows modified read output to be applied directly.

DELETE_NOTE
  Deletes one note by id.

  DELETE_NOTE id=t0n0

DELETE_NOTES
  Deletes all matching notes. With no filter it deletes all notes, so use this
  carefully.

  DELETE_NOTES track=0
  DELETE_NOTES track=0 ch=0 key=60
  DELETE_NOTES start=960 end=1920

TRANSPOSE
  Moves matching note keys by semitones.

  TRANSPOSE semitones=2
  TRANSPOSE semitones=-12 track=1
  TRANSPOSE semitones=7 ch=0 start=0 end=1920

SHIFT
  Moves matching note start times by ticks.

  SHIFT ticks=120
  SHIFT ticks=-240 track=0 start=960 end=1920

SCALE_TIME
  Scales matching note start times and durations. This stretches or compresses
  the timeline.

  SCALE_TIME factor=2/1
  SCALE_TIME factor=1/2
  SCALE_TIME factor=0.5

SCALE_DURATION
  Scales matching note durations without moving their starts.

  SCALE_DURATION factor=1/2
  SCALE_DURATION factor=2 key=60

QUANTIZE
  Snaps starts, durations, or both to a tick grid.

  QUANTIZE grid=120
  QUANTIZE grid=120 mode=start
  QUANTIZE grid=120 mode=duration
  QUANTIZE grid=120 mode=both

Filters
-------
Most whole-file commands accept optional filters:

  track=0
    Match only notes on track 0.

  ch=0 or channel=0
    Match only notes on MIDI channel 0.

  key=60
    Match only MIDI key 60.

  start=480
    Match notes whose start tick is >= 480.

  end=960
    Match notes whose start tick is < 960. The end filter is exclusive.

Examples:

  TRANSPOSE semitones=2 track=1 ch=0
  QUANTIZE grid=120 track=1 start=0 end=1920
  DELETE_NOTES track=2 key=36
  SCALE_DURATION factor=1/2 ch=9

Safety and limits
-----------------
midy currently edits note events. It preserves other MIDI and meta events while
rebuilding note-on and note-off event timing.

If you use the edit-file form, pass -o to choose the output path. If you pipe
edits through stdin, the second positional path is the output MIDI path. If you
pipe edits and omit the output path, midy overwrites the input file after a
successful parse/rewrite.

All times are integer MIDI ticks. midy does not guess musical intent beyond the
explicit commands you provide.

Validation rules:

  - track must exist
  - ch/channel must be 0..15
  - key, vel, and off_vel must be 0..127
  - dur must be greater than zero
  - negative SHIFT cannot move a note before tick 0
  - transpose cannot move keys outside 0..127
  - factor must be greater than zero
  - quantize grid must be greater than zero

Suggested machine workflow
--------------------------
For an ASCII-understanding machine, use this loop:

  1. Run: midy read input.mid
  2. Give the output plus the user's musical instruction to the machine.
  3. Ask the machine to output only valid midy edit lines.
  4. Run: cat edits.txt | midy apply input.mid output.mid
  5. Run: midy read output.mid to inspect the result.

For maximum reliability, tell the machine to prefer explicit edit commands
instead of prose. Good machine output:

  TRANSPOSE semitones=2 track=0
  QUANTIZE grid=120 mode=both
  SET_NOTE id=t0n4 dur=240

Bad machine output:

  Make the melody brighter and tighten the timing.

That sentence is understandable to a human, but midy intentionally applies only
explicit line-based commands.
";

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Help,
    Man,
    Version,
    Schema,
    Read {
        input: PathBuf,
    },
    Apply {
        input: PathBuf,
        edits: EditInput,
        output: PathBuf,
    },
}

#[derive(Debug, Eq, PartialEq)]
enum EditInput {
    File(PathBuf),
    Stdin,
}

/// Runs the command-line interface.
pub fn run(args: Vec<String>) -> Result<()> {
    match parse(args)? {
        Command::Help => {
            print!("{HELP}");
            Ok(())
        }
        Command::Man => {
            print!("{MANUAL}");
            Ok(())
        }
        Command::Version => {
            println!("{} {}", name(), env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Schema => {
            print!("{}", midi::EDIT_FORMAT_HELP);
            Ok(())
        }
        Command::Read { input } => {
            let bytes = std::fs::read(&input)?;
            print!("{}", midi::render_timeline(&bytes)?);
            Ok(())
        }
        Command::Apply {
            input,
            edits,
            output,
        } => {
            let bytes = std::fs::read(&input)?;
            let edit_text = read_edit_text(edits)?;
            let rewritten = midi::apply_edits(&bytes, &edit_text)?;
            std::fs::write(&output, rewritten)?;
            Ok(())
        }
    }
}

fn parse(args: Vec<String>) -> Result<Command> {
    match args.as_slice() {
        [_program] => Ok(Command::Help),
        [_program, flag] if flag == "-h" || flag == "--help" => Ok(Command::Help),
        [_program, flag] if flag == "--man" || flag == "man" => Ok(Command::Man),
        [_program, flag] if flag == "-V" || flag == "--version" => Ok(Command::Version),
        [_program, command] if command == "schema" || command == "format" => Ok(Command::Schema),
        [_program, command, input] if command == "read" || command == "dump" => Ok(Command::Read {
            input: input.into(),
        }),
        [_program, command, rest @ ..] if command == "apply" || command == "write" => {
            parse_apply(rest)
        }
        [_program, unknown, ..] => Err(Error::Usage(format!(
            "unknown argument '{unknown}'. Use 'midy --help' for usage."
        ))),
        [] => Ok(Command::Help),
    }
}

fn parse_apply(rest: &[String]) -> Result<Command> {
    let mut output = None::<PathBuf>;
    let mut positional = Vec::<PathBuf>::new();
    let mut index = 0;

    while index < rest.len() {
        match rest[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                let Some(path) = rest.get(index) else {
                    return Err(Error::Usage(
                        "missing output path after -o/--output".to_owned(),
                    ));
                };
                output = Some(path.into());
            }
            flag if flag != "-" && flag.starts_with('-') => {
                return Err(Error::Usage(format!(
                    "unknown apply option '{flag}'. Use 'midy --help' for usage."
                )));
            }
            value => positional.push(value.into()),
        }
        index += 1;
    }

    match (output, positional.as_slice()) {
        (Some(output), [input]) => Ok(Command::Apply {
            input: input.clone(),
            edits: EditInput::Stdin,
            output,
        }),
        (Some(output), [input, edits]) => Ok(Command::Apply {
            input: input.clone(),
            edits: parse_edit_input(edits),
            output,
        }),
        (Some(_), []) => Err(Error::Usage(
            "apply expects <input.mid> when reading edits from stdin, or <input.mid> <edits.txt> -o <output.mid>".to_owned(),
        )),
        (Some(_), _) => Err(Error::Usage(
            "too many apply arguments; use <input.mid> <edits.txt> -o <output.mid> or pipe edits into <input.mid> [output.mid]".to_owned(),
        )),
        (None, [input]) => Ok(Command::Apply {
            input: input.clone(),
            edits: EditInput::Stdin,
            output: input.clone(),
        }),
        (None, [input, output]) => Ok(Command::Apply {
            input: input.clone(),
            edits: EditInput::Stdin,
            output: output.clone(),
        }),
        (None, []) => Err(Error::Usage(
            "apply expects <input.mid>; pipe edits on stdin or use <input.mid> <edits.txt> -o <output.mid>".to_owned(),
        )),
        (None, _) => Err(Error::Usage(
            "too many apply arguments without -o; pipe edits into <input.mid> [output.mid]".to_owned(),
        )),
    }
}

fn parse_edit_input(path: &PathBuf) -> EditInput {
    if path == std::path::Path::new("-") {
        EditInput::Stdin
    } else {
        EditInput::File(path.clone())
    }
}

fn read_edit_text(input: EditInput) -> Result<String> {
    match input {
        EditInput::File(path) => Ok(std::fs::read_to_string(path)?),
        EditInput::Stdin => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            Ok(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                input: "song.mid".into()
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
            }
        );
    }

    #[test]
    fn rejects_unknown_args() {
        assert!(parse(args(&["midy", "inspect"])).is_err());
        assert!(parse(args(&["midy", "apply", "song.mid", "edits.txt", "out.mid"])).is_err());
    }
}
