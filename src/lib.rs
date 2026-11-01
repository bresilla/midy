//! Library support for the `midy` command line tool.

pub mod cli;
pub mod midi;

mod error;
mod text;

pub use cli::Status;
pub use error::{Error, Result};

/// Returns this crate's display name.
pub fn name() -> &'static str {
    "midy"
}

/// Runs the CLI using an already-collected argument vector.
pub fn run(args: Vec<String>) -> Result<Status> {
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
