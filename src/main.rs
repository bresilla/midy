use std::process::ExitCode;

fn main() -> ExitCode {
    match midy::run(std::env::args().collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("midy: {error}");
            ExitCode::FAILURE
        }
    }
}
