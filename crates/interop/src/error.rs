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

/// The Postman importer/exporter's error vocabulary: actionable, no panic paths. The cURL
/// module keeps its [`CurlError`]; the Postman module gets its own so failure modes are named
/// precisely.
#[derive(Debug)]
#[non_exhaustive]
pub enum PostmanError {
    /// The top-level value was not a Postman collection/environment/globals document.
    NotPostman { what: String },
    /// The document is a v1 Collection Format (or another generation we cannot convert).
    UnsupportedFormat { format: &'static str, instructions: String },
    /// A required Postman key was missing (or of the wrong shape).
    MissingField { field: String },
    /// A JSON file could not be read (I/O or size cap; from the capped readers).
    File { what: String, source: postguy_util::Error },
    /// The input is not valid JSON (or blew the nesting cap) before Postman shapes were seen.
    InvalidJson { what: String, source: postguy_util::Error },
}

impl fmt::Display for PostmanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PostmanError::NotPostman { what } => {
                write!(f, "this does not look like a Postman collection/environment/globals file: {what}")
            }
            PostmanError::UnsupportedFormat { format, instructions } => {
                write!(f, "{format} is not supported: {instructions}")
            }
            PostmanError::MissingField { field } => {
                write!(f, "the Postman document is missing `{field}`; this does not look like a complete Postman file")
            }
            PostmanError::File { what, source } => write!(f, "could not read {what}: {source}"),
            PostmanError::InvalidJson { what, source } => write!(f, "{what} is not valid JSON: {source}"),
        }
    }
}

impl std::error::Error for PostmanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PostmanError::File { source, .. } | PostmanError::InvalidJson { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<PostmanError> for postguy_util::Error {
    fn from(err: PostmanError) -> Self {
        match err {
            PostmanError::File { source, .. } | PostmanError::InvalidJson { source, .. } => source,
            PostmanError::NotPostman { what } | PostmanError::MissingField { field: what } => postguy_util::Error::Unusable { what },
            PostmanError::UnsupportedFormat { format, instructions } => postguy_util::Error::Unusable { what: format!("{format}: {instructions}") },
        }
    }
}

/// The OpenAPI/Swagger and HAR importers' error vocabulary: actionable, no panic paths.
#[derive(Debug)]
#[non_exhaustive]
pub enum SpecError {
    /// The input exceeded a size cap before any parsing happened.
    TooLarge { what: &'static str, size: usize, cap: usize },
    /// The YAML could not be parsed, or it blew a nesting/alias budget.
    Yaml { message: String },
    /// The document parsed but is not what this importer reads.
    Invalid { what: &'static str, detail: String },
}

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpecError::TooLarge { what, size, cap } => write!(f, "{what} is {size} bytes, over the {cap}-byte cap"),
            SpecError::Yaml { message } => write!(f, "YAML: {message}"),
            SpecError::Invalid { what, detail } => write!(f, "{what}: {detail}"),
        }
    }
}

impl std::error::Error for SpecError {}

impl From<SpecError> for postguy_util::Error {
    fn from(err: SpecError) -> Self {
        match err {
            SpecError::TooLarge { what, size, cap } => postguy_util::Error::TooLarge { what: what.to_string(), size, cap },
            other => postguy_util::Error::Unusable { what: other.to_string() },
        }
    }
}
