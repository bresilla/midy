use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    match midy::run(std::env::args().collect()) {
        Ok(midy::Status::Success) => ExitCode::SUCCESS,
        Ok(midy::Status::Findings) => ExitCode::FAILURE,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "midy: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}
