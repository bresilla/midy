//! End-to-end behavior of the `midy` binary.

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use common::{band, midi, track};

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("midy-cli-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.path(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_str().unwrap().to_owned()
    }

    fn text(&self, name: &str) -> String {
        self.path(name).to_str().unwrap().to_owned()
    }

    fn midy(&self, args: &[&str]) -> Output {
        self.midy_with_stdin(args, b"")
    }

    fn midy_with_stdin(&self, args: &[&str], stdin: &[u8]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_midy"))
            .args(args)
            .current_dir(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin).unwrap();
        child.wait_with_output().unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn one_note() -> Vec<u8> {
    midi(vec![track().note(0, 96, 0, 60, 64)])
}

mod apply_paths {
    use super::*;

    #[test]
    fn a_text_file_is_never_taken_as_the_output() {
        let sandbox = Sandbox::new("no-clobber");
        let input = sandbox.write("song.mid", &one_note());
        let edits = sandbox.write("edits.txt", b"TRANSPOSE semitones=2\n");

        let output = sandbox.midy(&["apply", &input, &edits]);

        assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
        assert!(stderr(&output).contains("-o <output.mid>"));
        assert_eq!(
            std::fs::read(sandbox.path("edits.txt")).unwrap(),
            b"TRANSPOSE semitones=2\n"
        );
    }

    #[test]
    fn stdin_edits_write_a_named_output() {
        let sandbox = Sandbox::new("stdin");
        let input = sandbox.write("song.mid", &one_note());
        let out = sandbox.text("out.mid");

        let output = sandbox.midy_with_stdin(&["apply", &input, &out], b"TRANSPOSE semitones=2\n");

        assert!(output.status.success(), "{}", stderr(&output));
        let read = sandbox.midy(&["read", &out]);
        assert!(stdout(&read).contains("key=62"));
    }

    #[test]
    fn inline_edits_work_without_stdin() {
        let sandbox = Sandbox::new("inline");
        let input = sandbox.write("song.mid", &one_note());
        let out = sandbox.text("out.mid");

        let output = sandbox.midy(&[
            "apply",
            &input,
            "-e",
            "TRANSPOSE semitones=2",
            "--edit",
            "VELOCITY set=99",
            "-o",
            &out,
        ]);

        assert!(output.status.success(), "{}", stderr(&output));
        let text = stdout(&sandbox.midy(&["read", &out]));
        assert!(text.contains("key=62") && text.contains("vel=99"));
    }

    #[test]
    fn a_file_and_inline_edits_conflict() {
        let sandbox = Sandbox::new("conflict");
        let input = sandbox.write("song.mid", &one_note());
        let edits = sandbox.write("edits.txt", b"");
        let out = sandbox.text("out.mid");

        let output = sandbox.midy(&["apply", &input, &edits, "-e", "X", "-o", &out]);
        assert_eq!(output.status.code(), Some(2));
    }

    #[test]
    fn output_may_not_be_the_edit_file() {
        let sandbox = Sandbox::new("same");
        let input = sandbox.write("song.mid", &one_note());
        let edits = sandbox.write("edits.txt", b"TRANSPOSE semitones=1\n");

        let output = sandbox.midy(&["apply", &input, &edits, "-o", &edits]);

        assert_eq!(output.status.code(), Some(2));
        assert!(stderr(&output).contains("both the edit file and the output"));
    }

    #[test]
    fn empty_input_warns_and_leaves_the_file_alone() {
        let sandbox = Sandbox::new("empty");
        let bytes = one_note();
        let input = sandbox.write("song.mid", &bytes);

        let output = sandbox.midy_with_stdin(&["apply", &input], b"");

        assert!(output.status.success());
        assert!(stderr(&output).contains("no edit commands"));
        assert_eq!(std::fs::read(sandbox.path("song.mid")).unwrap(), bytes);
    }

    #[test]
    fn a_failed_edit_leaves_the_input_alone() {
        let sandbox = Sandbox::new("failed");
        let bytes = one_note();
        let input = sandbox.write("song.mid", &bytes);

        let output = sandbox.midy_with_stdin(&["apply", &input], b"TRANSPOSE semitones=999\n");

        assert_eq!(output.status.code(), Some(1));
        assert!(stderr(&output).contains("edit line 1 (TRANSPOSE)"));
        assert_eq!(std::fs::read(sandbox.path("song.mid")).unwrap(), bytes);
    }

    #[test]
    fn no_temporary_files_are_left_behind() {
        let sandbox = Sandbox::new("temp");
        let input = sandbox.write("song.mid", &one_note());
        let output = sandbox.midy_with_stdin(&["apply", &input], b"TRANSPOSE semitones=2\n");
        assert!(output.status.success());
        let names = std::fs::read_dir(&sandbox.root).unwrap().count();
        assert_eq!(names, 1);
    }

    #[cfg(unix)]
    #[test]
    fn overwriting_keeps_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let sandbox = Sandbox::new("perms");
        let input = sandbox.write("song.mid", &one_note());
        let path = sandbox.path("song.mid");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

        sandbox.midy_with_stdin(&["apply", &input], b"TRANSPOSE semitones=2\n");

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
    }

    #[test]
    fn json_and_csv_on_stdin_are_detected() {
        let sandbox = Sandbox::new("detect");
        let input = sandbox.write("song.mid", &one_note());
        let out = sandbox.text("out.mid");

        let json = stdout(&sandbox.midy(&["read", &input, "--format", "json"]));
        let output = sandbox.midy_with_stdin(
            &["apply", &input, &out],
            json.replace("\"key\": 60", "\"key\": 65").as_bytes(),
        );
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(stdout(&sandbox.midy(&["read", &out])).contains("key=65"));

        let csv = stdout(&sandbox.midy(&["read", &input, "--format", "csv"]));
        let output = sandbox.midy_with_stdin(
            &["apply", &input, &out],
            csv.replace(",60,", ",66,").as_bytes(),
        );
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(stdout(&sandbox.midy(&["read", &out])).contains("key=66"));
    }
}

mod dry_run {
    use super::*;

    #[test]
    fn reports_without_writing() {
        let sandbox = Sandbox::new("dry");
        let bytes = band();
        let input = sandbox.write("song.mid", &bytes);
        let edits = sandbox.write(
            "edits.txt",
            b"TRANSPOSE semitones=2 track=1\nDELETE_NOTES key=C8\n",
        );

        let output = sandbox.midy(&["apply", &input, &edits, "--dry-run"]);

        assert!(output.status.success(), "{}", stderr(&output));
        let text = stdout(&output);
        assert!(
            text.contains("LINE line=1 command=TRANSPOSE matched=6"),
            "{text}"
        );
        assert!(
            text.contains("SUMMARY notes_before=13 notes_after=13"),
            "{text}"
        );
        assert!(
            text.contains("WARN line 2: DELETE_NOTES matched nothing"),
            "{text}"
        );
        assert_eq!(std::fs::read(sandbox.path("song.mid")).unwrap(), bytes);
    }

    #[test]
    fn strict_fails_on_warnings_and_writes_nothing() {
        let sandbox = Sandbox::new("strict");
        let bytes = band();
        let input = sandbox.write("song.mid", &bytes);

        let output =
            sandbox.midy_with_stdin(&["apply", &input, "--strict"], b"DELETE_NOTES key=C8\n");

        assert_eq!(output.status.code(), Some(1));
        assert_eq!(std::fs::read(sandbox.path("song.mid")).unwrap(), bytes);
    }
}

mod options {
    use super::*;

    #[test]
    fn unknown_options_are_rejected_with_a_hint() {
        let sandbox = Sandbox::new("unknown");
        let input = sandbox.write("song.mid", &one_note());
        for (args, hint) in [
            (vec!["read", &input, "--formatt", "json"], "--format"),
            (vec!["chords", &input, "--bar", "2"], "--bars"),
            (vec!["roll", &input, "--gird", "1/8"], "--grid"),
            (vec!["analyze", &input, "--jsno"], "--json"),
        ] {
            let output = sandbox.midy(&args);
            assert_eq!(output.status.code(), Some(2), "{args:?}");
            assert!(
                stderr(&output).contains(hint),
                "{args:?}: {}",
                stderr(&output)
            );
        }
    }

    #[test]
    fn boolean_flags_do_not_swallow_paths() {
        let sandbox = Sandbox::new("flags");
        let input = sandbox.write("song.mid", &one_note());
        let out = sandbox.text("out.mid");

        let output = sandbox.midy(&["analyze", "--json", &input]);
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(stdout(&output).starts_with('{'));

        let output = sandbox.midy(&["fix", "--remove-empty-tracks", &input, &out]);
        assert!(output.status.success(), "{}", stderr(&output));
    }

    #[test]
    fn long_options_accept_either_separator() {
        let sandbox = Sandbox::new("spelling");
        let input = sandbox.write("song.mid", &one_note());
        assert!(
            sandbox
                .midy(&["read", &input, "--events=none"])
                .status
                .success()
        );
        assert!(
            sandbox
                .midy(&["suggest", "bass", &input, "--out_track", "1"])
                .status
                .success()
        );
    }

    #[test]
    fn note_names_work_for_read_only_commands() {
        let sandbox = Sandbox::new("names");
        let input = sandbox.write("song.mid", &one_note());
        let output = sandbox.midy(&["chords", &input, "--key", "C4"]);
        assert!(output.status.success(), "{}", stderr(&output));
    }

    #[test]
    fn read_is_scoped() {
        let sandbox = Sandbox::new("scope");
        let input = sandbox.write("song.mid", &band());
        let text = stdout(&sandbox.midy(&["read", &input, "--track", "2", "--compact"]));
        assert!(text.contains("SCOPE track=2"));
        assert_eq!(
            text.lines()
                .filter(|line| line.starts_with("NOTE "))
                .count(),
            2
        );
    }

    #[test]
    fn wrong_argument_counts_name_the_command() {
        let sandbox = Sandbox::new("arity");
        let output = sandbox.midy(&["tracks", "a.mid", "b.mid"]);
        assert_eq!(output.status.code(), Some(2));
        assert!(
            stderr(&output).contains("tracks expects"),
            "{}",
            stderr(&output)
        );
    }

    #[test]
    fn unknown_commands_get_a_hint() {
        let sandbox = Sandbox::new("command");
        let output = sandbox.midy(&["raed", "a.mid"]);
        assert_eq!(output.status.code(), Some(2));
        assert!(stderr(&output).contains("did you mean 'read'"));
    }
}

mod errors {
    use super::*;

    #[test]
    fn io_errors_name_the_path() {
        let sandbox = Sandbox::new("io");
        let output = sandbox.midy(&["read", "missing.mid"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(
            stderr(&output).contains("missing.mid"),
            "{}",
            stderr(&output)
        );
    }

    #[test]
    fn invalid_midi_fails_cleanly() {
        let sandbox = Sandbox::new("invalid");
        let input = sandbox.write("bad.mid", b"not midi");
        let output = sandbox.midy(&["read", &input]);
        assert_eq!(output.status.code(), Some(1));
    }
}

mod exit_codes {
    use super::*;

    #[test]
    fn lint_reports_findings_only_when_asked() {
        let sandbox = Sandbox::new("lint");
        let dirty = sandbox.write("dirty.mid", &midi(vec![track().on(0, 0, 60, 80)]));
        let clean = sandbox.write("clean.mid", &one_note());

        assert!(sandbox.midy(&["lint", &dirty]).status.success());
        assert_eq!(
            sandbox.midy(&["lint", &dirty, "--exit-code"]).status.code(),
            Some(1)
        );
        assert!(
            sandbox
                .midy(&["lint", &clean, "--exit-code"])
                .status
                .success()
        );
    }

    #[test]
    fn diff_reports_changes_only_when_asked() {
        let sandbox = Sandbox::new("diff");
        let first = sandbox.write("a.mid", &one_note());
        let second = sandbox.write("b.mid", &midi(vec![track().note(0, 96, 0, 62, 64)]));

        assert!(sandbox.midy(&["diff", &first, &second]).status.success());
        assert_eq!(
            sandbox
                .midy(&["diff", &first, &second, "--exit-code"])
                .status
                .code(),
            Some(1)
        );
        assert!(
            sandbox
                .midy(&["diff", &first, &first, "--exit-code"])
                .status
                .success()
        );
    }
}

#[cfg(unix)]
#[test]
fn a_closed_pipe_is_not_a_failure() {
    let sandbox = Sandbox::new("pipe");
    let mut part = track();
    for index in 0..6000 {
        part = part.note(index * 96, 48, 0, 60, 80);
    }
    let input = sandbox.write("big.mid", &midi(vec![part]));

    let script = format!(
        "{} read {input} | head -1 > /dev/null; echo \"${{PIPESTATUS[0]}}\"",
        env!("CARGO_BIN_EXE_midy")
    );
    let output = Command::new("bash").arg("-c").arg(script).output().unwrap();

    assert_eq!(stdout(&output).trim(), "0", "{}", stderr(&output));
}

#[cfg(unix)]
mod render {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fake_synth(sandbox: &Sandbox, body: &str) -> String {
        let script = sandbox.write("synth.sh", format!("#!/bin/sh\n{body}\n").as_bytes());
        std::fs::set_permissions(
            sandbox.path("synth.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        script
    }

    fn render(sandbox: &Sandbox, synth: &str, soundfont: &Path) -> Output {
        let input = sandbox.write("song.mid", &one_note());
        sandbox.midy(&[
            "render",
            &input,
            "song.flac",
            "--soundfont",
            soundfont.to_str().unwrap(),
            "--synth",
            synth,
            "--sample-rate",
            "48000",
        ])
    }

    #[test]
    fn passes_the_expected_arguments_to_the_synth() {
        let sandbox = Sandbox::new("render");
        let synth = fake_synth(&sandbox, "echo \"$@\" > args.txt");
        let soundfont = sandbox.write("gm.sf2", b"sf");

        let output = render(&sandbox, &synth, Path::new(&soundfont));

        assert!(output.status.success(), "{}", stderr(&output));
        let args = std::fs::read_to_string(sandbox.path("args.txt")).unwrap();
        assert!(args.contains("-ni -F song.flac -T flac -r 48000"), "{args}");
    }

    #[test]
    fn a_failing_synth_is_reported() {
        let sandbox = Sandbox::new("render-fail");
        let synth = fake_synth(&sandbox, "exit 3");
        let soundfont = sandbox.write("gm.sf2", b"sf");

        let output = render(&sandbox, &synth, Path::new(&soundfont));

        assert_eq!(output.status.code(), Some(1));
        assert!(stderr(&output).contains("failed while rendering"));
    }

    #[test]
    fn a_missing_synth_is_named() {
        let sandbox = Sandbox::new("render-missing");
        let soundfont = sandbox.write("gm.sf2", b"sf");

        let output = render(&sandbox, "no-such-synth", Path::new(&soundfont));

        assert!(
            stderr(&output).contains("no-such-synth"),
            "{}",
            stderr(&output)
        );
    }

    #[test]
    fn a_missing_soundfont_is_named_before_running_the_synth() {
        let sandbox = Sandbox::new("render-sf");
        let synth = fake_synth(&sandbox, "touch ran.txt");

        let output = render(&sandbox, &synth, Path::new("missing.sf2"));

        assert!(
            stderr(&output).contains("missing.sf2"),
            "{}",
            stderr(&output)
        );
        assert!(!sandbox.path("ran.txt").exists());
    }
}
