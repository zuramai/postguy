//! Error vocabulary: every network failure keeps its PHASE so the response viewer can say
//! whether DNS, connect, TLS, writing or the server failed (PROMPT.md 5.4 timings hover).

/// Result alias.
pub type Result<T, E = HttpError> = std::result::Result<T, E>;

#[derive(Debug)]
#[non_exhaustive]
pub enum HttpError {
    /// The URL could not be understood (scheme/host missing).
    BadUrl { url: String, reason: String },
    /// DNS resolution failed or produced no usable address.
    Dns { host: String, source: std::io::Error },
    /// TCP connect failed or timed out.
    Connect { address: String, source: std::io::Error, timeout: bool },
    /// TLS handshake failed (bad certificate, hostname mismatch, protocol error).
    Tls { host: String, reason: String },
    /// The request could not be written or the response could not be read.
    Transport { source: hyper::Error },
    /// The client layer failed around the connection (hyper-util's legacy error type).
    Client { message: String },
    /// The whole request exceeded the configured timeout.
    Timeout { millis: u64 },
    /// The caller cancelled the request.
    Cancelled,
    /// The server sent a status the caller asked to treat as an error? Never implicit: this
    /// variant is for protocol violations (e.g. an ALPN mismatch).
    Protocol { reason: String },
    /// The response body exceeded the configured cap; the partial bytes are dropped (the UI
    /// shows the banner and offers Save to file; streaming arrives with M2.4).
    BodyTooLarge { cap: usize },
    /// A configuration problem (bad CA PEM, unusable client config).
    Config { reason: String },
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::BadUrl { url, reason } => write!(f, "cannot use `{url}`: {reason}"),
            HttpError::Dns { host, source } => write!(f, "resolving {host}: {source}"),
            HttpError::Connect { address, source, timeout } => {
                if *timeout {
                    write!(f, "connecting to {address} timed out")
                } else {
                    write!(f, "connecting to {address}: {source}")
                }
            }
            HttpError::Tls { host, reason } => write!(f, "TLS with {host}: {reason}"),
            HttpError::Transport { source } => write!(f, "transport: {source}"),
            HttpError::Client { message } => write!(f, "client: {message}"),
            HttpError::Timeout { millis } => write!(f, "request timed out after {millis} ms"),
            HttpError::Cancelled => f.write_str("request cancelled"),
            HttpError::Protocol { reason } => write!(f, "protocol: {reason}"),
            HttpError::BodyTooLarge { cap } => write!(f, "response body exceeds the {cap}-byte cap"),
            HttpError::Config { reason } => write!(f, "configuration: {reason}"),
        }
    }
}

impl std::error::Error for HttpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HttpError::Dns { source, .. } | HttpError::Connect { source, .. } => Some(source),
            HttpError::Transport { source } => Some(source),
            _ => None,
        }
    }
}

impl HttpError {
    /// The timing phase this failure belongs to, for the timeline and console.
    pub fn phase(&self) -> &'static str {
        match self {
            HttpError::BadUrl { .. } | HttpError::Config { .. } => "prepare",
            HttpError::Dns { .. } => "dns",
            HttpError::Connect { .. } => "connect",
            HttpError::Tls { .. } => "tls",
            HttpError::Transport { .. } | HttpError::Client { .. } | HttpError::Protocol { .. } => "request",
            HttpError::Timeout { .. } | HttpError::Cancelled => "request",
            HttpError::BodyTooLarge { .. } => "download",
        }
    }
}
