//! Auth errors are user-presentable: what was missing or malformed, never a secret value.

pub type Result<T, E = AuthError> = std::result::Result<T, E>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// A required field is empty.
    Missing { kind: &'static str, field: &'static str },
    /// The configuration is understood but not supported (unknown algorithm, bad key shape).
    Unsupported { kind: &'static str, reason: String },
    /// Malformed input (challenge, JSON claims, key, URL).
    Invalid { kind: &'static str, reason: String },
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::Missing { kind, field } => write!(f, "{kind} auth: `{field}` is required"),
            AuthError::Unsupported { kind, reason } => write!(f, "{kind} auth: {reason}"),
            AuthError::Invalid { kind, reason } => write!(f, "{kind} auth: {reason}"),
        }
    }
}

impl std::error::Error for AuthError {}

pub(crate) fn missing(kind: &'static str, field: &'static str) -> AuthError {
    AuthError::Missing { kind, field }
}

pub(crate) fn invalid(kind: &'static str, reason: impl Into<String>) -> AuthError {
    AuthError::Invalid { kind, reason: reason.into() }
}
