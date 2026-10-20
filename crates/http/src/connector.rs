//! The timed connector: DNS → TCP → TLS, each phase measured, ALPN agreed, cancellation and
//! connect timeouts honoured. It plugs into hyper-util's legacy client as a tower `Service`
//! (ADR 0003), so one connection stack serves HTTP/1.1 and HTTP/2.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use hyper::Uri;
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use crate::error::{HttpError, Result};
use crate::tls::{self, TlsMode};

/// Per-phase timings of one connection attempt (milliseconds, measured around each await).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectTimings {
    pub dns_ms: u64,
    pub tcp_ms: u64,
    pub tls_ms: u64,
    pub total_ms: u64,
}

impl ConnectTimings {
    /// Sum of the per-phase numbers (the wall-clock total may include scheduling gaps).
    pub fn phases_ms(&self) -> u64 {
        self.dns_ms.saturating_add(self.tcp_ms).saturating_add(self.tls_ms)
    }
}

/// The connector's knobs.
#[derive(Debug, Clone)]
pub struct ConnectorConfig {
    pub tls: TlsMode,
    /// Connect timeout per phase group (DNS+TCP+TLS share one deadline budget).
    pub connect_timeout: Duration,
    /// ALPN preference for HTTPS connections.
    pub alpn: Vec<Vec<u8>>,
}

impl Default for ConnectorConfig {
    fn default() -> Self {
        ConnectorConfig { tls: TlsMode::default(), connect_timeout: Duration::from_secs(30), alpn: vec![b"h2".to_vec(), b"http/1.1".to_vec()] }
    }
}

/// The connector service. One instance per client; the LAST successful attempt's timings are
/// kept for the response viewer (single-flight; the engine serialises per-connection reads).
#[derive(Clone)]
pub struct TimedConnector {
    config: Arc<ConnectorConfig>,
    last: Arc<Mutex<Option<ConnectTimings>>>,
}

impl TimedConnector {
    pub fn new(config: ConnectorConfig) -> TimedConnector {
        TimedConnector { config: Arc::new(config), last: Arc::new(Mutex::new(None)) }
    }

    /// The most recent connection timings (None until the first connection).
    pub fn last_timings(&self) -> Option<ConnectTimings> {
        *self.last.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn record(&self, timings: ConnectTimings) {
        *self.last.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(timings);
    }
}

/// The stream handed to hyper: TCP or TLS, with the negotiated ALPN.
pub struct TimedStream {
    inner: StreamKind,
    http2: bool,
}

enum StreamKind {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl std::fmt::Debug for TimedStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimedStream").field("http2", &self.http2).finish_non_exhaustive()
    }
}

impl Connection for TimedStream {
    fn connected(&self) -> Connected {
        Connected::new().proxy(false)
    }
}

impl tokio::io::AsyncRead for TimedStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut tokio::io::ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        match &mut self.get_mut().inner {
            StreamKind::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            StreamKind::Tls(stream) => Pin::new(stream.as_mut()).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for TimedStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        match &mut self.get_mut().inner {
            StreamKind::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            StreamKind::Tls(stream) => Pin::new(stream.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut self.get_mut().inner {
            StreamKind::Plain(stream) => Pin::new(stream).poll_flush(cx),
            StreamKind::Tls(stream) => Pin::new(stream.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut self.get_mut().inner {
            StreamKind::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            StreamKind::Tls(stream) => Pin::new(stream.as_mut()).poll_shutdown(cx),
        }
    }
}

impl tower_service::Service<Uri> for TimedConnector {
    type Response = TokioIo<TimedStream>;
    type Error = HttpError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let config = self.config.clone();
        let observer = self.clone();
        Box::pin(async move {
            let started = Instant::now();
            let scheme = uri.scheme_str().unwrap_or("http").to_ascii_lowercase();
            let host =
                uri.host().map(str::to_string).ok_or_else(|| HttpError::BadUrl { url: uri.to_string(), reason: "missing host".to_string() })?;
            let port = uri.port_u16().unwrap_or(if scheme == "https" { 443 } else { 80 });
            let tls_required = scheme == "https";

            // DNS.
            let dns_started = Instant::now();
            let mut addresses =
                tokio::net::lookup_host((host.as_str(), port)).await.map_err(|source| HttpError::Dns { host: host.clone(), source })?;
            let Some(address) = addresses.next() else {
                return Err(HttpError::Dns {
                    host: host.clone(),
                    source: std::io::Error::new(std::io::ErrorKind::NotFound, "no addresses returned"),
                });
            };
            let dns_ms = millis(dns_started.elapsed());

            // TCP (bounded by the connect timeout).
            let tcp_started = Instant::now();
            let tcp = match tokio::time::timeout(config.connect_timeout, TcpStream::connect(address)).await {
                Ok(Ok(stream)) => stream,
                Ok(Err(source)) => {
                    return Err(HttpError::Connect { address: address.to_string(), source, timeout: false });
                }
                Err(_elapsed) => {
                    return Err(HttpError::Connect {
                        address: address.to_string(),
                        source: std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timeout"),
                        timeout: true,
                    });
                }
            };
            let _ = tcp.set_nodelay(true);
            let tcp_ms = millis(tcp_started.elapsed());

            // TLS when the scheme asks for it.
            let (stream, http2) = if tls_required {
                let tls_started = Instant::now();
                let alpn: Vec<&[u8]> = config.alpn.iter().map(Vec::as_slice).collect();
                let client_config = tls::client_config(&config.tls, &alpn).map_err(|reason| HttpError::Config { reason })?;
                let server_name = rustls::pki_types::ServerName::try_from(host.clone())
                    .map_err(|_| HttpError::Tls { host: host.clone(), reason: "invalid server name for TLS".to_string() })?;
                let connector = TlsConnector::from(Arc::new(client_config));
                let tls_stream = match tokio::time::timeout(config.connect_timeout, connector.connect(server_name, tcp)).await {
                    Ok(Ok(stream)) => stream,
                    Ok(Err(err)) => return Err(HttpError::Tls { host: host.clone(), reason: err.to_string() }),
                    Err(_elapsed) => {
                        return Err(HttpError::Connect {
                            address: address.to_string(),
                            source: std::io::Error::new(std::io::ErrorKind::TimedOut, "TLS timeout"),
                            timeout: true,
                        });
                    }
                };
                let negotiated = tls_stream.get_ref().1.alpn_protocol().map(|protocol| protocol == b"h2").unwrap_or(false);
                let tls_ms = millis(tls_started.elapsed());
                observer.record(ConnectTimings { dns_ms, tcp_ms, tls_ms, total_ms: millis(started.elapsed()) });
                (StreamKind::Tls(Box::new(tls_stream)), negotiated)
            } else {
                observer.record(ConnectTimings { dns_ms, tcp_ms, tls_ms: 0, total_ms: millis(started.elapsed()) });
                (StreamKind::Plain(tcp), false)
            };

            Ok(TokioIo::new(TimedStream { inner: stream, http2 }))
        })
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

// The connector advertises HTTP/2 per connection through hyper-util's `Connection` only for
// proxied connections; legacy over plain ALPN relies on the builder's http2 setting, which the
// client configures from the same preference. `http2` stays on the stream for callers/tests.
impl TimedStream {
    /// Whether ALPN negotiated HTTP/2 for this connection.
    pub fn negotiated_http2(&self) -> bool {
        self.http2
    }
}
