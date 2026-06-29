use std::fmt;

/// Project-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can be returned by `midy`.
#[derive(Debug)]
pub enum Error {
    /// Command-line input was not valid.
    Usage(String),
    /// Edit input was not valid.
    Edit(String),
    /// File or stream I/O failed.
    Io(std::io::Error),
    /// MIDI parsing failed.
    Midi(midly::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => formatter.write_str(message),
            Self::Edit(message) => formatter.write_str(message),
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Midi(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Usage(_) => None,
            Self::Edit(_) => None,
            Self::Io(error) => Some(error),
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
