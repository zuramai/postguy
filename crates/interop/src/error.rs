//! The crate's error type: actionable messages, `Result` alias, and no panic paths.
//!
//! Mirrors the `postguy-util` error philosophy (`Error`/`Result` with `From` conversions so
//! `?` composes) but stays local so the cURL module's failure modes are named precisely; a
//! [`From`] impl maps onto `postguy_util::Error` for callers that unify error types.

use std::fmt;

/// Crate result alias.
pub type Result<T, E = CurlError> = std::result::Result<T, E>;

/// Error kind plus what the caller can do about it.
#[derive(Debug)]
#[non_exhaustive]
pub enum CurlError {
    /// The input exceeded a size cap before any parsing happened.
    TooLarge { what: String, size: usize, cap: usize },
    /// A referenced file could not be read (e.g. `-b @cookie-file`).
    Read { what: String, source: postguy_util::Error },
    /// The input contained no `curl` command to import.
    NoCommand,
    /// A construct this importer deliberately does not support (currently: `-b @-`, stdin).
    Unsupported { what: String },
}

impl fmt::Display for CurlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CurlError::TooLarge { what, size, cap } => {
                write!(f, "{what} is {size} bytes, over the {cap}-byte cap; split the input or raise the cap")
            }
            CurlError::Read { what, source } => write!(f, "could not read {what}: {source}"),
            CurlError::NoCommand => write!(f, "no `curl` command found in the input; paste the command line starting with `curl`"),
            CurlError::Unsupported { what } => write!(f, "{what} is not supported by the cURL importer"),
        }
    }
}

impl std::error::Error for CurlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CurlError::Read { source, .. } => source.source(),
            _ => None,
        }
    }
}

impl From<CurlError> for postguy_util::Error {
    fn from(err: CurlError) -> Self {
        match err {
            CurlError::TooLarge { what, size, cap } => postguy_util::Error::TooLarge { what, size, cap },
            CurlError::Read { what: _, source } => source,
            CurlError::NoCommand => postguy_util::Error::Unusable { what: err.to_string() },
            CurlError::Unsupported { what } => postguy_util::Error::Unusable { what },
        }
    }
}
