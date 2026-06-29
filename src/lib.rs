//! Library support for the `midy` command line tool.

pub mod cli;
pub mod midi;

mod error;

pub use error::{Error, Result};

/// Returns this crate's display name.
pub fn name() -> &'static str {
    "midy"
}

/// Runs the CLI using an already-collected argument vector.
///
/// Keeping this in the library makes the binary thin and gives future commands
/// an easy place to test their behavior without spawning a process.
pub fn run(args: Vec<String>) -> Result<()> {
    cli::run(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_name() {
        assert_eq!(name(), "midy");
    }
}
