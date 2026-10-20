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
use crate::proxy::{ProxyPolicy, ProxyProtocol, ProxyRoute};
use crate::tls::{self, TlsMode};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// Per-phase timings of one connection attempt (milliseconds, measured around each await).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
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
    /// Proxy resolution (environment or custom; PROMPT.md 5.5).
    pub proxy: ProxyPolicy,
}

impl Default for ConnectorConfig {
    fn default() -> Self {
        ConnectorConfig {
            tls: TlsMode::default(),
            connect_timeout: Duration::from_secs(30),
            alpn: vec![b"h2".to_vec(), b"http/1.1".to_vec()],
            proxy: ProxyPolicy::new(postguy_model::ProxySettings::default()),
        }
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
    /// The connection is to an HTTP proxy in absolute-form mode (plain HTTP through a proxy):
    /// hyper-util then writes absolute-form request targets (PROMPT.md 5.5).
    proxied: bool,
    /// The proxy route used for this connection, if any (the Console logs it).
    proxy: Option<ProxyRoute>,
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
        Connected::new().proxy(self.proxied)
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

            // Proxy route for this target (None = direct). The route is decided per URI, so
            // one client can send some hosts through the proxy and some direct.
            let route = config.proxy.route_for(&uri.to_string());
            let (connect_host, connect_port) = match &route {
                Some(route) => (route.host.clone(), route.port),
                None => (host.clone(), port),
            };

            // DNS (the proxy's host when one is used; SOCKS5h resolves the target remotely).
            let dns_started = Instant::now();
            let mut addresses = tokio::net::lookup_host((connect_host.as_str(), connect_port))
                .await
                .map_err(|source| HttpError::Dns { host: connect_host.clone(), source })?;
            let Some(address) = addresses.next() else {
                return Err(HttpError::Dns {
                    host: connect_host.clone(),
                    source: std::io::Error::new(std::io::ErrorKind::NotFound, "no addresses returned"),
                });
            };
            let dns_ms = millis(dns_started.elapsed());

            // TCP (bounded by the connect timeout).
            let tcp_started = Instant::now();
            let mut tcp = match tokio::time::timeout(config.connect_timeout, TcpStream::connect(address)).await {
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

            // Proxy handshakes: an HTTP CONNECT tunnel for HTTPS, a SOCKS5 greeting for both
            // schemes. Plain HTTP through an HTTP proxy needs no handshake; hyper writes the
            // absolute-form request target once the stream is marked proxied.
            let mut proxied = false;
            if let Some(route) = &route {
                match route.protocol {
                    ProxyProtocol::Http => {
                        if route.is_http_tunnel(&scheme) {
                            tokio::time::timeout(config.connect_timeout, connect_tunnel(&mut tcp, &host, port, route))
                                .await
                                .map_err(|_| HttpError::Proxy { address: route.address(), reason: "CONNECT timed out".into() })??;
                        } else {
                            proxied = true;
                        }
                    }
                    ProxyProtocol::Socks5 | ProxyProtocol::Socks5h => {
                        let remote_dns = route.protocol == ProxyProtocol::Socks5h;
                        tcp = tokio::time::timeout(config.connect_timeout, socks5_connect(tcp, &host, port, route, remote_dns))
                            .await
                            .map_err(|_| HttpError::Proxy { address: route.address(), reason: "SOCKS5 handshake timed out".into() })??;
                    }
                }
            }

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

            Ok(TokioIo::new(TimedStream { inner: stream, http2, proxied, proxy: route }))
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

    /// The proxy route this connection used, if any (the Console logs it).
    pub fn proxy_route(&self) -> Option<&ProxyRoute> {
        self.proxy.as_ref()
    }
}

/// HTTP CONNECT (PROMPT.md 5.5): establish a tunnel through an HTTP proxy before TLS. Reads the
/// proxy's response one byte at a time so no tunneled bytes are consumed, and caps the header.
async fn connect_tunnel(tcp: &mut TcpStream, host: &str, port: u16, route: &ProxyRoute) -> Result<()> {
    let authority = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(auth) = route.basic_auth_value() {
        request.push_str(&format!("Proxy-Authorization: {auth}\r\n"));
    }
    request.push_str("\r\n");
    tcp.write_all(request.as_bytes())
        .await
        .map_err(|err| HttpError::Proxy { address: route.address(), reason: format!("writing CONNECT: {err}") })?;
    tcp.flush().await.map_err(|err| HttpError::Proxy { address: route.address(), reason: format!("writing CONNECT: {err}") })?;

    let mut head: Vec<u8> = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        if head.len() >= 16 * 1024 {
            return Err(HttpError::Proxy { address: route.address(), reason: "CONNECT response headers exceed 16 KiB".into() });
        }
        let read = tcp
            .read(&mut byte)
            .await
            .map_err(|err| HttpError::Proxy { address: route.address(), reason: format!("reading CONNECT response: {err}") })?;
        if read == 0 {
            return Err(HttpError::Proxy { address: route.address(), reason: "the proxy closed the connection during CONNECT".into() });
        }
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&head);
    let status_line = text.lines().next().unwrap_or_default();
    let status = status_line.split_whitespace().nth(1).and_then(|code| code.parse::<u16>().ok()).unwrap_or(0);
    if status != 200 {
        return Err(HttpError::Proxy {
            address: route.address(),
            reason: format!("CONNECT was refused ({status_line}); check the proxy address and credentials"),
        });
    }
    Ok(())
}

/// SOCKS5 (RFC 1928) with optional username/password auth (RFC 1929). `remote_dns` sends the
/// host name to the proxy (`socks5h`); otherwise it is resolved locally (`socks5`).
async fn socks5_connect(mut tcp: TcpStream, host: &str, port: u16, route: &ProxyRoute, remote_dns: bool) -> Result<TcpStream> {
    let fail = |reason: String| HttpError::Proxy { address: route.address(), reason };
    let has_credentials = !route.username.is_empty() || !route.password.is_empty();
    let greeting: &[u8] = if has_credentials { &[0x05, 0x02, 0x00, 0x02] } else { &[0x05, 0x01, 0x00] };
    tcp.write_all(greeting).await.map_err(|err| fail(format!("writing greeting: {err}")))?;
    let mut choice = [0u8; 2];
    tcp.read_exact(&mut choice).await.map_err(|err| fail(format!("reading method choice: {err}")))?;
    if choice.first() != Some(&0x05) {
        return Err(fail(format!("not a SOCKS5 proxy (version {})", choice.first().copied().unwrap_or(0))));
    }
    match choice.get(1).copied() {
        Some(0x00) => {}
        Some(0x02) if has_credentials => {
            let user = route.username.as_bytes();
            let password = route.password.as_bytes();
            if user.len() > 255 || password.len() > 255 {
                return Err(fail("SOCKS5 username/password are limited to 255 bytes".into()));
            }
            let mut auth = vec![0x01, user.len() as u8];
            auth.extend_from_slice(user);
            auth.push(password.len() as u8);
            auth.extend_from_slice(password);
            tcp.write_all(&auth).await.map_err(|err| fail(format!("writing credentials: {err}")))?;
            let mut reply = [0u8; 2];
            tcp.read_exact(&mut reply).await.map_err(|err| fail(format!("reading auth reply: {err}")))?;
            if reply.get(1).copied() != Some(0x00) {
                return Err(fail("SOCKS5 authentication was rejected".into()));
            }
        }
        Some(0x02) => return Err(fail("the SOCKS5 proxy asked for credentials but none are configured".into())),
        Some(method) => return Err(fail(format!("the SOCKS5 proxy accepted no usable method (offered {method:#x})"))),
        None => return Err(fail("malformed SOCKS5 method choice".into())),
    }

    // Address: domain (remote DNS) or a locally resolved socket address.
    let mut request = vec![0x05, 0x01, 0x00];
    if remote_dns {
        if host.len() > 255 {
            return Err(fail("host name is longer than 255 bytes".into()));
        }
        request.push(0x03);
        request.push(host.len() as u8);
        request.extend_from_slice(host.as_bytes());
    } else {
        let mut addresses = tokio::net::lookup_host((host, port)).await.map_err(|err| fail(format!("resolving {host} for SOCKS5: {err}")))?;
        let Some(address) = addresses.next() else {
            return Err(fail(format!("no addresses for {host}")));
        };
        match address {
            std::net::SocketAddr::V4(v4) => {
                request.push(0x01);
                request.extend_from_slice(&v4.ip().octets());
            }
            std::net::SocketAddr::V6(v6) => {
                request.push(0x04);
                request.extend_from_slice(&v6.ip().octets());
            }
        }
    }
    request.extend_from_slice(&port.to_be_bytes());
    tcp.write_all(&request).await.map_err(|err| fail(format!("writing connect request: {err}")))?;

    let mut reply = [0u8; 4];
    tcp.read_exact(&mut reply).await.map_err(|err| fail(format!("reading connect reply: {err}")))?;
    if reply.first() != Some(&0x05) {
        return Err(fail("malformed SOCKS5 connect reply".into()));
    }
    match reply.get(1).copied() {
        Some(0x00) => {}
        Some(code) => {
            let reason = match code {
                0x01 => "general SOCKS server failure",
                0x02 => "connection not allowed by ruleset",
                0x03 => "network unreachable",
                0x04 => "host unreachable",
                0x05 => "connection refused",
                0x06 => "TTL expired",
                0x07 => "command not supported",
                0x08 => "address type not supported",
                _ => "unknown SOCKS5 error",
            };
            return Err(fail(format!("SOCKS5 connect failed: {reason}")));
        }
        None => return Err(fail("truncated SOCKS5 reply".into())),
    }
    // Drain the bound address so the tunnel starts clean.
    let address_len = match reply.get(3).copied() {
        Some(0x01) => 4,
        Some(0x04) => 16,
        Some(0x03) => {
            let mut len = [0u8; 1];
            tcp.read_exact(&mut len).await.map_err(|err| fail(format!("reading bound address: {err}")))?;
            usize::from(len[0])
        }
        _ => return Err(fail("unsupported address type in SOCKS5 reply".into())),
    };
    let mut discard = vec![0u8; address_len + 2];
    tcp.read_exact(&mut discard).await.map_err(|err| fail(format!("reading bound address: {err}")))?;
    Ok(tcp)
}
