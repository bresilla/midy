use std::path::PathBuf;

use crate::{Error, Result, midi, name};

const HELP: &str = "\
midy

Usage:
  midy read <input.mid>
  midy apply <input.mid> <edits.txt> -o <output.mid>
  midy schema
  midy [OPTIONS]

Options:
  -h, --help       Show this help message
  -V, --version    Show version information

Commands:
  read     Print a deterministic ASCII timeline for a MIDI file
  apply    Apply ASCII edit commands and write a new MIDI file
  write    Alias for apply
  schema   Print the ASCII edit format accepted by apply/write
";

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Help,
    Version,
    Schema,
    Read {
        input: PathBuf,
    },
    Apply {
        input: PathBuf,
        edits: PathBuf,
        output: PathBuf,
    },
}

/// Runs the command-line interface.
pub fn run(args: Vec<String>) -> Result<()> {
    match parse(args)? {
        Command::Help => {
            print!("{HELP}");
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
            let edit_text = std::fs::read_to_string(&edits)?;
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
            flag if flag.starts_with('-') => {
                return Err(Error::Usage(format!(
                    "unknown apply option '{flag}'. Use 'midy --help' for usage."
                )));
            }
            value => positional.push(value.into()),
        }
        index += 1;
    }

    if positional.len() != 2 {
        return Err(Error::Usage(
            "apply expects <input.mid> <edits.txt> -o <output.mid>".to_owned(),
        ));
    }

    let output = output.ok_or_else(|| {
        Error::Usage("apply requires -o <output.mid> so the input is not overwritten".to_owned())
    })?;

    Ok(Command::Apply {
        input: positional.remove(0),
        edits: positional.remove(0),
        output,
    })
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
                edits: "edits.txt".into(),
                output: "out.mid".into(),
            }
        );
    }

    #[test]
    fn rejects_unknown_args() {
        assert!(parse(args(&["midy", "inspect"])).is_err());
        assert!(parse(args(&["midy", "apply", "song.mid", "edits.txt"])).is_err());
    }
}
