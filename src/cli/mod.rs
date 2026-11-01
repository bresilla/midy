//! Command-line entry point: running parsed commands.

mod args;

#[cfg(test)]
mod tests;

use std::io::{IsTerminal, Read, Write};
use std::path::Path;

use crate::{Error, Result, midi, name};

use args::{Command, EditInput, ReadFormat};

const HELP: &str = include_str!("help.txt");
const MANUAL: &str = include_str!("manual.txt");

/// How a successful run ended.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Status {
    /// Nothing to report.
    Success,
    /// `--exit-code` was given and the command found something.
    Findings,
}

/// Runs the command-line interface.
pub fn run(args: Vec<String>) -> Result<Status> {
    match args::parse(args)? {
        Command::Help => print_out(HELP)?,
        Command::Man => print_out(MANUAL)?,
        Command::Version => print_out(&format!("{} {}\n", name(), env!("CARGO_PKG_VERSION")))?,
        Command::Schema => print_out(midi::EDIT_FORMAT_HELP)?,
        Command::Read {
            input,
            format,
            options,
        } => {
            let bytes = read_file(&input)?;
            let text = match format {
                ReadFormat::Midy => midi::render_timeline_with(&bytes, &options)?,
                ReadFormat::Json => midi::render_timeline_json_with(&bytes, &options)?,
                ReadFormat::Csv => midi::render_timeline_csv_with(&bytes, &options)?,
            };
            print_out(&text)?;
        }
        Command::Apply {
            input,
            edits,
            output,
            format,
            options,
            dry_run,
        } => {
            let bytes = read_file(&input)?;
            let text = read_edits(&edits)?;
            let format = match (format, &edits) {
                (midi::EditFormat::Auto, EditInput::File(path)) => format_from_path(path),
                (format, _) => format,
            };
            let applied = midi::apply_edits_with(&bytes, &text, format, options)?;
            if dry_run {
                print_out(&midi::render_edit_report(&applied.report))?;
                return Ok(Status::Success);
            }
            warn_all(&applied.report.warnings);
            if !(applied.report.commands.is_empty() && output == input) {
                write_file(&output, &applied.bytes)?;
            }
        }
        Command::SuggestReduceChords { input, options } => {
            print_out(&midi::suggest_reduce_chords(&read_file(&input)?, &options)?)?;
        }
        Command::SuggestBass { input, options } => {
            print_out(&midi::suggest_bassline(&read_file(&input)?, &options)?)?;
        }
        Command::Chords { input, options } => {
            print_out(&midi::render_chords(&read_file(&input)?, &options)?)?;
        }
        Command::Roll { input, options } => {
            print_out(&midi::render_roll(&read_file(&input)?, &options)?)?;
        }
        Command::Analyze { input, json } => {
            let bytes = read_file(&input)?;
            let text = if json {
                midi::render_analysis_json(&bytes, input.to_str())?
            } else {
                midi::render_analysis(&bytes, input.to_str())?
            };
            print_out(&text)?;
        }
        Command::Tracks { input } => print_out(&midi::render_tracks(&read_file(&input)?)?)?,
        Command::Lint { input, exit_code } => {
            let (report, warnings) = midi::lint_report(&read_file(&input)?)?;
            print_out(&report)?;
            if exit_code && warnings > 0 {
                return Ok(Status::Findings);
            }
        }
        Command::Fix {
            input,
            output,
            options,
        } => {
            let rewritten = midi::fix_midi_with_options(&read_file(&input)?, options)?;
            write_file(&output, &rewritten)?;
        }
        Command::Humanize {
            input,
            output,
            edit,
        }
        | Command::Dehumanize {
            input,
            output,
            edit,
        }
        | Command::Swing {
            input,
            output,
            edit,
        }
        | Command::Chordize {
            input,
            output,
            edit,
        }
        | Command::Arpeggiate {
            input,
            output,
            edit,
        } => {
            let applied = midi::apply_edits_with(
                &read_file(&input)?,
                &edit,
                midi::EditFormat::Text,
                midi::ApplyOptions::default(),
            )?;
            warn_all(&applied.report.warnings);
            write_file(&output, &applied.bytes)?;
        }
        Command::Extract {
            input,
            output,
            selector,
        } => write_file(
            &output,
            &midi::extract_selection(&read_file(&input)?, &selector)?,
        )?,
        Command::Mute {
            input,
            output,
            selector,
        } => write_file(
            &output,
            &midi::mute_selection(&read_file(&input)?, &selector)?,
        )?,
        Command::Solo {
            input,
            output,
            selector,
        } => write_file(
            &output,
            &midi::solo_selection(&read_file(&input)?, &selector)?,
        )?,
        Command::Split {
            input,
            out_dir,
            mode,
        } => {
            let outputs = midi::split_selection(&read_file(&input)?, mode)?;
            std::fs::create_dir_all(&out_dir).map_err(|source| path_error(&out_dir, source))?;
            for (name, output) in outputs {
                write_file(&out_dir.join(name), &output)?;
            }
        }
        Command::Merge { inputs, output } => {
            let bytes = inputs
                .iter()
                .map(|path| read_file(path))
                .collect::<Result<Vec<_>>>()?;
            let input_refs = bytes.iter().map(Vec::as_slice).collect::<Vec<_>>();
            let (rewritten, warnings) = midi::merge_midi_report(&input_refs)?;
            warn_all(&warnings);
            write_file(&output, &rewritten)?;
        }
        Command::Diff {
            before,
            after,
            exit_code,
        } => {
            let diff = midi::render_diff(&read_file(&before)?, &read_file(&after)?)?;
            print_out(&diff)?;
            if exit_code && !diff.is_empty() {
                return Ok(Status::Findings);
            }
        }
        Command::Render {
            input,
            output,
            soundfont,
            synth,
            sample_rate,
        } => render_audio(&input, &output, &soundfont, &synth, sample_rate)?,
    }
    Ok(Status::Success)
}

fn path_error(path: &Path, source: std::io::Error) -> Error {
    Error::Path {
        path: path.to_owned(),
        source,
    }
}

fn read_file(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|source| path_error(path, source))
}

/// Writes through a temporary file so a failure leaves any existing file intact.
fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let file_name = target
        .file_name()
        .ok_or_else(|| Error::Usage(format!("'{}' is not a file path", path.display())))?;
    let mut temporary_name = std::ffi::OsString::from(".");
    temporary_name.push(file_name);
    temporary_name.push(format!(".midy-{}", std::process::id()));
    let temporary = target.with_file_name(temporary_name);

    let result = (|| {
        std::fs::write(&temporary, bytes)?;
        if let Ok(metadata) = std::fs::metadata(&target) {
            std::fs::set_permissions(&temporary, metadata.permissions())?;
        }
        std::fs::rename(&temporary, &target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|source| path_error(path, source))
}

/// Writes to stdout; a reader that closed early is not an error.
fn print_out(text: &str) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Err(error) if error.kind() != std::io::ErrorKind::BrokenPipe => Err(error.into()),
        _ => Ok(()),
    }
}

fn warn_all(warnings: &[String]) {
    let mut stderr = std::io::stderr().lock();
    for warning in warnings {
        let _ = writeln!(stderr, "midy: warning: {warning}");
    }
}

fn read_edits(input: &EditInput) -> Result<String> {
    match input {
        EditInput::File(path) => {
            std::fs::read_to_string(path).map_err(|source| path_error(path, source))
        }
        EditInput::Inline(lines) => Ok(lines.join("\n")),
        EditInput::Stdin => {
            let mut stdin = std::io::stdin().lock();
            if stdin.is_terminal() {
                return Err(Error::Usage(
                    "apply is waiting for edits on stdin, but stdin is a terminal; pipe edits in, or use: midy apply <input.mid> <edits.txt> -o <output.mid>"
                        .to_owned(),
                ));
            }
            let mut text = String::new();
            stdin.read_to_string(&mut text)?;
            Ok(text)
        }
    }
}

fn format_from_path(path: &Path) -> midi::EditFormat {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json") => midi::EditFormat::Json,
        Some("csv") => midi::EditFormat::Csv,
        Some(_) => midi::EditFormat::Text,
        None => midi::EditFormat::Auto,
    }
}

fn render_audio(
    input: &Path,
    output: &Path,
    soundfont: &Path,
    synth: &str,
    sample_rate: u32,
) -> Result<()> {
    let file_type = audio_file_type(output)?;
    for path in [input, soundfont] {
        std::fs::metadata(path).map_err(|source| path_error(path, source))?;
    }
    let status = std::process::Command::new(synth)
        .arg("-ni")
        .arg("-F")
        .arg(output)
        .arg("-T")
        .arg(file_type)
        .arg("-r")
        .arg(sample_rate.to_string())
        .arg(soundfont)
        .arg(input)
        .status()
        .map_err(|error| Error::External(format!("could not run '{synth}': {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::External(format!(
            "{synth} failed while rendering '{}' to '{}' with status {status}",
            input.display(),
            output.display(),
        )))
    }
}

fn audio_file_type(output: &Path) -> Result<&'static str> {
    match output
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("wav") => Ok("wav"),
        Some("flac") => Ok("flac"),
        Some("oga") | Some("ogg") => Ok("oga"),
        Some("aiff") | Some("aif") => Ok("aiff"),
        Some(value) => Err(Error::Usage(format!(
            "unsupported render output extension '.{value}'; use .wav, .flac, .oga, or .aiff"
        ))),
        None => Err(Error::Usage(
            "render output needs an audio extension such as .wav or .flac".to_owned(),
        )),
    }
}

fn has_midi_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("mid") || extension.eq_ignore_ascii_case("midi")
        })
}

fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (std::fs::canonicalize(left), std::fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left == right
        )
}

/// Checks that an edit file and output path cannot clobber each other.
fn check_edit_paths(edits: &Path, output: &Path) -> Result<()> {
    if same_path(edits, output) {
        return Err(Error::Usage(format!(
            "'{}' is both the edit file and the output",
            output.display()
        )));
    }
    Ok(())
}
