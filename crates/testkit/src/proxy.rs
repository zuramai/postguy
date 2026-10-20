//! Proxy fixtures for the HTTP engine's tests (test support only): a forwarding HTTP proxy that
//! accepts absolute-form requests, tunnels CONNECT, records what it saw and can require basic
//! auth; and a SOCKS5 proxy that tunnels and records whether the client sent a host name
//! (`socks5h`) or an IP (`socks5`).
//!
//! Everything binds `127.0.0.1:0`; nothing reaches the internet.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt as _, Full};
use hyper::body::Incoming;
use hyper::header::{HeaderValue, PROXY_AUTHORIZATION};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode, Uri};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

/// One request the proxy saw (what the tests assert on).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxySeen {
    pub method: String,
    /// The request target as written: absolute-form for plain HTTP, `host:port` for CONNECT.
    pub target: String,
    pub proxy_authorization: Option<String>,
}

struct ProxyState {
    require_auth: Option<String>,
    seen: Mutex<Vec<ProxySeen>>,
    connects: AtomicUsize,
}

/// A running forwarding HTTP proxy. Drop stops it.
pub struct ProxyServer {
    addr: SocketAddr,
    state: Arc<ProxyState>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ProxyServer {
    /// Start a proxy that accepts every request.
    #[allow(clippy::panic)]
    pub fn start() -> ProxyServer {
        ProxyServer::spawn(None).unwrap_or_else(|err| panic!("testkit proxy failed to start: {err}"))
    }

    /// Start a proxy that answers 407 unless `Proxy-Authorization: Basic <base64(user:pass)>`
    /// is present.
    #[allow(clippy::panic)]
    pub fn start_with_auth(username: &str, password: &str) -> ProxyServer {
        let expected = format!("Basic {}", base64_encode(format!("{username}:{password}").as_bytes()));
        ProxyServer::spawn(Some(expected)).unwrap_or_else(|err| panic!("testkit proxy failed to start: {err}"))
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://127.0.0.1:PORT` (what a client puts in its proxy settings).
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn seen(&self) -> Vec<ProxySeen> {
        self.state.seen.lock().map(|seen| seen.clone()).unwrap_or_default()
    }

    pub fn connect_count(&self) -> usize {
        self.state.connects.load(Ordering::SeqCst)
    }

    fn spawn(require_auth: Option<String>) -> Result<ProxyServer, String> {
        let state = Arc::new(ProxyState { require_auth, seen: Mutex::new(Vec::new()), connects: AtomicUsize::new(0) });
        let (addr_tx, addr_rx) = mpsc::channel::<SocketAddr>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let thread_state = state.clone();
        let thread = std::thread::Builder::new()
            .name("postguy-testkit-proxy".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ = ready_tx.send(Err(format!("runtime: {err}")));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", 0)).await {
                        Ok(listener) => listener,
                        Err(err) => {
                            let _ = ready_tx.send(Err(format!("bind: {err}")));
                            return;
                        }
                    };
                    let address = match listener.local_addr() {
                        Ok(address) => address,
                        Err(err) => {
                            let _ = ready_tx.send(Err(format!("local_addr: {err}")));
                            return;
                        }
                    };
                    let _ = addr_tx.send(address);
                    let _ = ready_tx.send(Ok(()));
                    let mut shutdown_rx = shutdown_rx;
                    loop {
                        tokio::select! {
                            _ = &mut shutdown_rx => break,
                            accepted = listener.accept() => {
                                let Ok((stream, _peer)) = accepted else { continue };
                                let state = thread_state.clone();
                                tokio::spawn(async move {
                                    serve_connection(stream, state).await;
                                });
                            }
                        }
                    }
                });
            })
            .map_err(|err| format!("thread: {err}"))?;
        let addr = addr_rx.recv_timeout(Duration::from_secs(5)).map_err(|err| format!("startup: {err}"))?;
        ready_rx.recv_timeout(Duration::from_secs(5)).map_err(|err| format!("startup: {err}"))??;
        Ok(ProxyServer { addr, state, shutdown: Some(shutdown_tx), thread: Some(thread) })
    }
}

impl Drop for ProxyServer {
    fn drop(&mut self) {
        if let Some(sender) = self.shutdown.take() {
            let _ = sender.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn serve_connection(stream: TcpStream, state: Arc<ProxyState>) {
    let io = TokioIo::new(stream);
    let service = service_fn(move |request: Request<Incoming>| {
        let state = state.clone();
        async move { handle(request, state).await }
    });
    let _ = hyper::server::conn::http1::Builder::new().serve_connection(io, service).with_upgrades().await;
}

async fn handle(request: Request<Incoming>, state: Arc<ProxyState>) -> Result<Response<Full<Bytes>>, Infallible> {
    let method = request.method().clone();
    let target = request.uri().to_string();
    let proxy_authorization = request.headers().get(PROXY_AUTHORIZATION).and_then(|value| value.to_str().ok()).map(str::to_string);
    if let Ok(mut seen) = state.seen.lock() {
        seen.push(ProxySeen { method: method.as_str().to_string(), target: target.clone(), proxy_authorization: proxy_authorization.clone() });
    }
    if let Some(expected) = &state.require_auth
        && proxy_authorization.as_deref() != Some(expected.as_str())
    {
        let mut response = Response::new(Full::new(Bytes::from_static(b"proxy authentication required")));
        *response.status_mut() = StatusCode::PROXY_AUTHENTICATION_REQUIRED;
        response.headers_mut().insert("proxy-authenticate", HeaderValue::from_static("Basic realm=\"testkit\""));
        return Ok(response);
    }

    if method == Method::CONNECT {
        state.connects.fetch_add(1, Ordering::SeqCst);
        let Some(authority) = request.uri().authority().map(|authority| authority.to_string()) else {
            return Ok(bad_request("CONNECT without an authority"));
        };
        let upgrade = hyper::upgrade::on(request);
        tokio::spawn(async move {
            let Ok(upgraded) = upgrade.await else { return };
            let Ok(upstream) = TcpStream::connect(&authority).await else { return };
            let mut downstream = TokioIo::new(upgraded);
            let mut upstream = upstream;
            let _ = tokio::io::copy_bidirectional(&mut downstream, &mut upstream).await;
        });
        return Ok(Response::new(Full::new(Bytes::new())));
    }

    let Some(uri) = absolute_target(request.uri()) else {
        return Ok(bad_request("plain requests must use absolute-form through the proxy"));
    };
    let Some(authority) = uri.authority().map(|authority| authority.to_string()) else {
        return Ok(bad_request("absolute-form target without an authority"));
    };
    let Some(path_and_query) = uri.path_and_query().map(|path| path.as_str().to_string()) else {
        return Ok(bad_request("absolute-form target without a path"));
    };
    let origin_uri = match path_and_query.parse::<Uri>() {
        Ok(uri) => uri,
        Err(_) => return Ok(bad_request("bad request target")),
    };
    let Ok(tcp) = TcpStream::connect(&authority).await else {
        return Ok(bad_gateway("the proxy could not reach the origin"));
    };
    let (mut sender, connection) = match hyper::client::conn::http1::handshake(TokioIo::new(tcp)).await {
        Ok(pair) => pair,
        Err(_) => return Ok(bad_gateway("the proxy could not speak HTTP/1.1 to the origin")),
    };
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let (mut parts, body) = request.into_parts();
    parts.uri = origin_uri;
    let forwarded = Request::from_parts(parts, body);
    let response = match sender.send_request(forwarded).await {
        Ok(response) => response,
        Err(_) => return Ok(bad_gateway("the origin request failed")),
    };
    let (parts, body) = response.into_parts();
    let bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return Ok(bad_gateway("reading the origin response failed")),
    };
    if bytes.len() > 8 << 20 {
        return Ok(bad_gateway("the fixture refuses bodies over 8 MiB"));
    }
    Ok(Response::from_parts(parts, Full::new(bytes)))
}

/// Turn an absolute-form request target into a URI (some hyper versions strip the scheme but
/// keep the authority; both shapes are accepted).
fn absolute_target(uri: &Uri) -> Option<Uri> {
    if uri.scheme().is_some() {
        return Some(uri.clone());
    }
    if uri.authority().is_some() {
        let path = uri.path_and_query().map(|path| path.as_str()).unwrap_or("/");
        return format!("http://{}{}", uri.authority()?, path).parse().ok();
    }
    None
}

fn bad_request(message: &str) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(message.to_string())));
    *response.status_mut() = StatusCode::BAD_REQUEST;
    response
}

fn bad_gateway(message: &str) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(message.to_string())));
    *response.status_mut() = StatusCode::BAD_GATEWAY;
    response
}

/// One address the SOCKS5 fixture was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocksSeen {
    /// `domain`, `ipv4` or `ipv6`.
    pub address_type: &'static str,
    pub host: String,
    pub port: u16,
}

struct SocksState {
    seen: Mutex<Vec<SocksSeen>>,
}

/// A running SOCKS5 proxy (no auth). Drop stops it.
pub struct Socks5Server {
    addr: SocketAddr,
    state: Arc<SocksState>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Socks5Server {
    #[allow(clippy::panic)]
    pub fn start() -> Socks5Server {
        Socks5Server::spawn().unwrap_or_else(|err| panic!("testkit SOCKS5 failed to start: {err}"))
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn seen(&self) -> Vec<SocksSeen> {
        self.state.seen.lock().map(|seen| seen.clone()).unwrap_or_default()
    }

    fn spawn() -> Result<Socks5Server, String> {
        let state = Arc::new(SocksState { seen: Mutex::new(Vec::new()) });
        let (addr_tx, addr_rx) = mpsc::channel::<SocketAddr>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let thread_state = state.clone();
        let thread = std::thread::Builder::new()
            .name("postguy-testkit-socks".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ = ready_tx.send(Err(format!("runtime: {err}")));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", 0)).await {
                        Ok(listener) => listener,
                        Err(err) => {
                            let _ = ready_tx.send(Err(format!("bind: {err}")));
                            return;
                        }
                    };
                    let address = match listener.local_addr() {
                        Ok(address) => address,
                        Err(err) => {
                            let _ = ready_tx.send(Err(format!("local_addr: {err}")));
                            return;
                        }
                    };
                    let _ = addr_tx.send(address);
                    let _ = ready_tx.send(Ok(()));
                    let mut shutdown_rx = shutdown_rx;
                    loop {
                        tokio::select! {
                            _ = &mut shutdown_rx => break,
                            accepted = listener.accept() => {
                                let Ok((stream, _peer)) = accepted else { continue };
                                let state = thread_state.clone();
                                tokio::spawn(async move {
                                    let _ = serve_socks(stream, state).await;
                                });
                            }
                        }
                    }
                });
            })
            .map_err(|err| format!("thread: {err}"))?;
        let addr = addr_rx.recv_timeout(Duration::from_secs(5)).map_err(|err| format!("startup: {err}"))?;
        ready_rx.recv_timeout(Duration::from_secs(5)).map_err(|err| format!("startup: {err}"))??;
        Ok(Socks5Server { addr, state, shutdown: Some(shutdown_tx), thread: Some(thread) })
    }
}

impl Drop for Socks5Server {
    fn drop(&mut self) {
        if let Some(sender) = self.shutdown.take() {
            let _ = sender.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn serve_socks(mut stream: TcpStream, state: Arc<SocksState>) -> Result<(), std::io::Error> {
    let mut version_and_count = [0u8; 2];
    stream.read_exact(&mut version_and_count).await?;
    if version_and_count[0] != 0x05 {
        return Ok(());
    }
    let mut methods = vec![0u8; usize::from(version_and_count[1])];
    stream.read_exact(&mut methods).await?;
    if !methods.contains(&0x00) {
        stream.write_all(&[0x05, 0xff]).await?;
        return Ok(());
    }
    stream.write_all(&[0x05, 0x00]).await?;

    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await?;
    if head[0] != 0x05 || head[1] != 0x01 {
        stream.write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await?;
        return Ok(());
    }
    let (address_type, host, port) = match head[3] {
        0x01 => {
            let mut octets = [0u8; 4];
            stream.read_exact(&mut octets).await?;
            let mut port_bytes = [0u8; 2];
            stream.read_exact(&mut port_bytes).await?;
            ("ipv4", format!("{}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3]), u16::from_be_bytes(port_bytes))
        }
        0x04 => {
            let mut octets = [0u8; 16];
            stream.read_exact(&mut octets).await?;
            let mut port_bytes = [0u8; 2];
            stream.read_exact(&mut port_bytes).await?;
            let address = std::net::Ipv6Addr::from(octets);
            ("ipv6", address.to_string(), u16::from_be_bytes(port_bytes))
        }
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await?;
            let mut host = vec![0u8; usize::from(len[0])];
            stream.read_exact(&mut host).await?;
            let mut port_bytes = [0u8; 2];
            stream.read_exact(&mut port_bytes).await?;
            ("domain", String::from_utf8_lossy(&host).into_owned(), u16::from_be_bytes(port_bytes))
        }
        _ => {
            stream.write_all(&[0x05, 0x08, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await?;
            return Ok(());
        }
    };
    if let Ok(mut seen) = state.seen.lock() {
        seen.push(SocksSeen { address_type, host: host.clone(), port });
    }
    let target = match tokio::net::lookup_host((host.as_str(), port)).await {
        Ok(mut addresses) => addresses.next(),
        Err(_) => None,
    };
    let Some(target) = target else {
        stream.write_all(&[0x05, 0x04, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await?;
        return Ok(());
    };
    let mut upstream = match TcpStream::connect(target).await {
        Ok(stream) => stream,
        Err(_) => {
            stream.write_all(&[0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await?;
            return Ok(());
        }
    };
    // Success, bound address 0.0.0.0:0.
    stream.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await?;
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
    Ok(())
}

/// Minimal standard base64 for the fixture's expected header (test code only).
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk.first().copied().unwrap_or(0));
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        for shift in [18, 12, 6, 0] {
            out.push(TABLE.get(((triple >> shift) & 0x3f) as usize).copied().unwrap_or(b'A') as char);
        }
        let padding = 3usize.saturating_sub(chunk.len());
        for _ in 0..padding {
            out.pop();
            out.push('=');
        }
    }
    out
}
