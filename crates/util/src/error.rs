//! The crate's error type: actionable messages, `From` conversions so `?` composes, and no
//! panicking paths. Callers get to know what went wrong without a backtrace dance.

use std::fmt;

/// Crate result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Error kind plus what the caller can do about it.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// Filesystem or OS error, carrying the attempt being made.
    Io { operation: String, source: std::io::Error },
    /// The input exceeded a size cap; nothing was (or all of it was not) loaded.
    TooLarge { what: String, size: usize, cap: usize },
    /// The input was not valid UTF-8 where a string was required.
    Encoding { what: String },
    /// JSON nesting exceeded the cap.
    DepthExceeded { what: String, cap: usize },
    /// JSON could not be parsed; `source` carries serde's position info.
    JsonSyntax { what: String, source: serde_json::Error },
    /// A buffered structure was used beyond its bounds (programmer error, still an error).
    Unusable { what: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { operation, source } => write!(f, "{operation}: {source}"),
            Error::TooLarge { what, size, cap } => {
                write!(f, "{what} is {size} bytes, over the {cap}-byte cap; raise the cap or save the tail separately")
            }
            Error::Encoding { what } => write!(f, "{what} is not valid UTF-8"),
            Error::DepthExceeded { what, cap } => write!(f, "{what} nests deeper than the {cap}-level cap"),
            Error::JsonSyntax { what, source } => write!(f, "{what} is not valid JSON: {source}"),
            Error::Unusable { what } => write!(f, "{what}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            Error::JsonSyntax { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Error::Io { operation: "filesystem operation".to_string(), source }
    }
}

impl From<serde_json::Error> for Error {
    fn from(source: serde_json::Error) -> Self {
        Error::JsonSyntax { what: "JSON input".to_string(), source }
    }
}

/// Marker the depth-checked JSON parser emits through serde so the caller can distinguish
/// "too deep" from "invalid syntax" without string sniffing at the call site.
pub const DEPTH_MARKER: &str = "postguy-nesting-cap-exceeded";

impl Error {
    /// True when `err` was produced by the nesting cap rather than malformed syntax.
    pub fn is_depth_exceeded(err: &serde_json::Error) -> bool {
        err.to_string().contains(DEPTH_MARKER)
    }

    /// Attach concrete `what` context to an error for better messages at the boundary.
    pub fn with_context(self, what: &str) -> Error {
        match self {
            Error::JsonSyntax { source, .. } => Error::JsonSyntax { what: what.to_string(), source },
            Error::DepthExceeded { cap, .. } => Error::DepthExceeded { what: what.to_string(), cap },
            Error::TooLarge { size, cap, .. } => Error::TooLarge { what: what.to_string(), size, cap },
            Error::Encoding { .. } => Error::Encoding { what: what.to_string() },
            other => other,
        }
    }
}
