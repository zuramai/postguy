//! The client: one request, phase timings, cancellation and the body cap. Redirects (M2.5),
//! compression decode (M2.4) and cookies (M2.6) are deliberately NOT here yet; the caller
//! sees exactly what the server sent.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt as _, Full};
use hyper::body::Incoming;
use hyper::header::{HeaderName, HeaderValue};
use hyper::{Method, Request, StatusCode, Version};
use hyper_util::client::legacy::Client as LegacyClient;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tokio::sync::watch;

use crate::connector::{ConnectTimings, ConnectorConfig, TimedConnector};
use crate::error::{HttpError, Result};
use crate::tls::TlsMode;

/// HTTP version preference (PROMPT.md 5.3.2: Auto, HTTP/1.x, HTTP/2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HttpVersionPreference {
    #[default]
    Auto,
    Http1,
    Http2,
}

/// Cancellation handle: clone it and call [`Cancel::cancel`] from anywhere (Esc on a send).
#[derive(Debug, Clone)]
pub struct Cancel {
    sender: Arc<watch::Sender<bool>>,
}

impl Cancel {
    pub fn new() -> Cancel {
        let (sender, _receiver) = watch::channel(false);
        Cancel { sender: Arc::new(sender) }
    }

    /// Request cancellation; idempotent.
    pub fn cancel(&self) {
        let _ = self.sender.send(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }

    async fn cancelled(&self) {
        let mut receiver = self.sender.subscribe();
        if *receiver.borrow() {
            return;
        }
        while receiver.changed().await.is_ok() {
            if *receiver.borrow() {
                return;
            }
        }
        // Sender gone: never resolves (the request proceeds); callers get their own timeouts.
        std::future::pending::<()>().await;
    }
}

impl Default for Cancel {
    fn default() -> Self {
        Cancel::new()
    }
}

/// Everything the engine hands the client after variable resolution and body building.
#[derive(Debug, Clone)]
pub struct RequestSpec {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// 0 = no whole-request timeout (PROMPT.md 5.3.2).
    pub timeout_ms: u64,
    /// Response body cap; exceeding it is an error with the partial body dropped.
    pub max_body_bytes: usize,
}

impl RequestSpec {
    pub fn get(url: impl Into<String>) -> RequestSpec {
        RequestSpec { method: Method::GET, url: url.into(), headers: Vec::new(), body: Vec::new(), timeout_ms: 0, max_body_bytes: 50 << 20 }
    }
}

/// Client-wide settings (per-request settings arrive with the spec/engine later).
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub tls: TlsMode,
    pub version: HttpVersionPreference,
    pub connect_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig { tls: TlsMode::default(), version: HttpVersionPreference::Auto, connect_timeout: Duration::from_secs(30) }
    }
}

/// What the server actually sent, raw bytes included, plus the phase story.
#[derive(Debug)]
pub struct ResponseData {
    pub status: StatusCode,
    pub version: Version,
    /// Received order, name as the server sent it (lowercased by HTTP/1 parsing rules; the
    /// original case is not preserved by hyper's http1 codec — noted in the Console).
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timings: ResponseTimings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseTimings {
    /// Connection phases from the connector (DNS/TCP/TLS).
    pub connect: ConnectTimings,
    /// First byte of the response line/headers received.
    pub ttfb_ms: u64,
    /// Whole response read (headers + body) since request start.
    pub total_ms: u64,
    /// Time spent reading the body after TTFB.
    pub download_ms: u64,
}

/// The client: one hyper legacy client over the timed connector.
#[derive(Clone)]
pub struct Client {
    inner: LegacyClient<TimedConnector, Full<Bytes>>,
    connector: TimedConnector,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("timings", &self.connector.last_timings()).finish_non_exhaustive()
    }
}

impl Client {
    /// Build a client for the config. One client is cheap to clone and shares its pool.
    pub fn new(config: ClientConfig) -> Result<Client> {
        let alpn = match config.version {
            HttpVersionPreference::Auto => vec![b"h2".to_vec(), b"http/1.1".to_vec()],
            HttpVersionPreference::Http1 => vec![b"http/1.1".to_vec()],
            HttpVersionPreference::Http2 => vec![b"h2".to_vec()],
        };
        let connector = TimedConnector::new(ConnectorConfig { tls: config.tls.clone(), connect_timeout: config.connect_timeout, alpn });
        let mut builder = LegacyClient::builder(TokioExecutor::new());
        builder
            .pool_timer(hyper_util::rt::TokioTimer::new())
            .pool_idle_timeout(Duration::from_secs(30))
            .http2_only(config.version == HttpVersionPreference::Http2)
            .http1_title_case_headers(false)
            .http1_preserve_header_case(false);
        let inner = builder.build::<_, Full<Bytes>>(connector.clone());
        Ok(Client { inner, connector })
    }

    /// The last connection's phase timings (single-flight contract; the engine serialises).
    pub fn last_connect_timings(&self) -> Option<ConnectTimings> {
        self.connector.last_timings()
    }

    /// Send ONE request: no redirects, no decompression, no cookies. Cancellation and the
    /// whole-request timeout abort cleanly; the body cap errors instead of allocating past it.
    pub async fn send(&self, spec: RequestSpec, cancel: &Cancel) -> Result<ResponseData> {
        if cancel.is_cancelled() {
            return Err(HttpError::Cancelled);
        }
        let uri: hyper::Uri = spec.url.parse().map_err(|err| HttpError::BadUrl { url: spec.url.clone(), reason: format!("{err}") })?;
        let mut builder = Request::builder().method(spec.method.clone()).uri(uri);
        for (name, value) in &spec.headers {
            let header_name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|err| HttpError::BadUrl { url: spec.url.clone(), reason: format!("bad header `{name}`: {err}") })?;
            let header_value = HeaderValue::from_str(value)
                .map_err(|err| HttpError::BadUrl { url: spec.url.clone(), reason: format!("bad header value for `{name}`: {err}") })?;
            builder = builder.header(header_name, header_value);
        }
        let request =
            builder.body(Full::new(Bytes::from(spec.body))).map_err(|err| HttpError::BadUrl { url: spec.url.clone(), reason: err.to_string() })?;

        let started = Instant::now();
        let response = {
            let send = self.inner.request(request);
            if spec.timeout_ms > 0 {
                tokio::select! {
                    outcome = send => outcome.map_err(classify_client_error)?,
                    _ = cancel.cancelled() => return Err(HttpError::Cancelled),
                    _ = tokio::time::sleep(Duration::from_millis(spec.timeout_ms)) => {
                        return Err(HttpError::Timeout { millis: spec.timeout_ms });
                    }
                }
            } else {
                tokio::select! {
                    outcome = send => outcome.map_err(classify_client_error)?,
                    _ = cancel.cancelled() => return Err(HttpError::Cancelled),
                }
            }
        };

        let ttfb_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let status = response.status();
        let version = response.version();
        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(name, value)| (name.as_str().to_string(), String::from_utf8_lossy(value.as_bytes()).into_owned()))
            .collect();

        // Read the body with the cap enforced by a running count (never preallocate the cap).
        let mut body: Vec<u8> = Vec::new();
        let mut incoming: Incoming = response.into_body();
        loop {
            let fence = tokio::select! {
                frame = incoming.frame() => frame,
                _ = cancel.cancelled() => return Err(HttpError::Cancelled),
                _ = async {
                    if spec.timeout_ms > 0 { tokio::time::sleep(Duration::from_millis(spec.timeout_ms)).await } else { std::future::pending::<()>().await }
                } => return Err(HttpError::Timeout { millis: spec.timeout_ms }),
            };
            let Some(frame) = fence else { break };
            let frame = frame.map_err(|source| HttpError::Transport { source })?;
            if let Ok(data) = frame.into_data() {
                if body.len().saturating_add(data.len()) > spec.max_body_bytes {
                    return Err(HttpError::BodyTooLarge { cap: spec.max_body_bytes });
                }
                body.extend_from_slice(&data);
            }
        }
        let total_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        Ok(ResponseData {
            status,
            version,
            headers,
            body,
            timings: ResponseTimings {
                connect: self.connector.last_timings().unwrap_or_default(),
                ttfb_ms,
                total_ms,
                download_ms: total_ms.saturating_sub(ttfb_ms),
            },
        })
    }
}

/// The legacy client wraps connector errors; walk the source chain to recover OUR error so
/// the response viewer keeps the phase (DNS/TCP/TLS) instead of "client error (Connect)".
fn classify_client_error(source: hyper_util::client::legacy::Error) -> HttpError {
    let mut current: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(&source);
    while let Some(err) = current {
        if let Some(http) = err.downcast_ref::<HttpError>() {
            return clone_error(http);
        }
        current = err.source();
    }
    HttpError::Client { message: source.to_string() }
}

/// Boxed errors cannot be moved out of a `&dyn`; our variants are small and Clone-able by
/// reconstruction (the io errors are rebuilt from their kinds/messages).
fn clone_error(err: &HttpError) -> HttpError {
    match err {
        HttpError::BadUrl { url, reason } => HttpError::BadUrl { url: url.clone(), reason: reason.clone() },
        HttpError::Dns { host, source } => HttpError::Dns { host: host.clone(), source: std::io::Error::new(source.kind(), source.to_string()) },
        HttpError::Connect { address, source, timeout } => {
            HttpError::Connect { address: address.clone(), source: std::io::Error::new(source.kind(), source.to_string()), timeout: *timeout }
        }
        HttpError::Tls { host, reason } => HttpError::Tls { host: host.clone(), reason: reason.clone() },
        HttpError::Timeout { millis } => HttpError::Timeout { millis: *millis },
        HttpError::Cancelled => HttpError::Cancelled,
        HttpError::Protocol { reason } => HttpError::Protocol { reason: reason.clone() },
        HttpError::BodyTooLarge { cap } => HttpError::BodyTooLarge { cap: *cap },
        HttpError::Config { reason } => HttpError::Config { reason: reason.clone() },
        HttpError::Client { message } => HttpError::Client { message: message.clone() },
        other => HttpError::Client { message: other.to_string() },
    }
}

/// A convenience connector for plain-HTTP-only callers (nothing else links it today).
pub type PlainConnector = HttpConnector;
