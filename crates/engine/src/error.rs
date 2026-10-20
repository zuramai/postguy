//! Engine errors: everything a send or a session operation can fail with, phrased for a person
//! or an agent to act on (AGENTS.md 3). HTTP phase errors keep their own vocabulary.

/// Result alias for the engine.
pub type Result<T, E = EngineError> = std::result::Result<T, E>;

#[derive(Debug)]
#[non_exhaustive]
pub enum EngineError {
    /// A workspace read/write failed.
    Store(postguy_util::Error),
    /// The HTTP layer failed (DNS, TLS, proxy, timeout, cancellation, body cap).
    Http(postguy_http::HttpError),
    /// The requested collection/request/environment is not in the workspace.
    NotFound { what: String, id: String },
    /// The request cannot be built as asked (bad URL, missing file, empty auth field, ...).
    BadRequest { reason: String },
    /// The feature exists in the model but not in this build yet.
    Unsupported { what: String },
    /// A panic escaped the pipeline; it became this error instead of taking the app down.
    Panicked { message: String },
    /// An import could not be parsed or written; the message is user-presentable.
    Import { message: String },
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Store(err) => write!(f, "{err}"),
            EngineError::Http(err) => write!(f, "{err}"),
            EngineError::NotFound { what, id } => write!(f, "{what} `{id}` is not in this workspace"),
            EngineError::BadRequest { reason } => write!(f, "cannot send: {reason}"),
            EngineError::Unsupported { what } => write!(f, "not supported yet: {what}"),
            EngineError::Panicked { message } => write!(f, "the send failed unexpectedly: {message}"),
            EngineError::Import { message } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EngineError::Store(err) => Some(err),
            EngineError::Http(err) => Some(err),
            _ => None,
        }
    }
}

impl From<postguy_util::Error> for EngineError {
    fn from(err: postguy_util::Error) -> EngineError {
        EngineError::Store(err)
    }
}

impl From<postguy_http::HttpError> for EngineError {
    fn from(err: postguy_http::HttpError) -> EngineError {
        EngineError::Http(err)
    }
}

impl From<EngineError> for String {
    fn from(err: EngineError) -> String {
        err.to_string()
    }
}
