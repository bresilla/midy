use std::fmt;
use std::path::PathBuf;

/// Project-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can be returned by `midy`.
#[derive(Debug)]
pub enum Error {
    /// Command-line input was not valid.
    Usage(String),
    /// Edit input was not valid.
    Edit(String),
    /// MIDI data cannot be used for the requested operation.
    Input(String),
    /// An external program failed.
    External(String),
    /// File or stream I/O failed.
    Io(std::io::Error),
    /// File I/O failed on a known path.
    Path {
        /// The file or directory involved.
        path: PathBuf,
        /// The underlying failure.
        source: std::io::Error,
    },
    /// MIDI parsing failed.
    Midi(midly::Error),
}

impl Error {
    /// Returns the process exit code for this error.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) => 2,
            _ => 1,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message)
            | Self::Edit(message)
            | Self::Input(message)
            | Self::External(message) => formatter.write_str(message),
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Path { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Midi(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Usage(_) | Self::Edit(_) | Self::Input(_) | Self::External(_) => None,
            Self::Io(error) => Some(error),
            Self::Path { source, .. } => Some(source),
            Self::Midi(error) => Some(error),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<midly::Error> for Error {
    fn from(error: midly::Error) -> Self {
        Self::Midi(error)
    }
}
