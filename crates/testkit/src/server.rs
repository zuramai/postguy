//! The server: hyper on a private multi-thread runtime in a background thread, plain HTTP or
//! TLS, clean shutdown on Drop, and REAL chunked streaming so pacing tests mean something.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt as _, Full};
use hyper::body::{Frame, Incoming as HyperIncoming};
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};

use crate::routes::{self, Incoming, Reply};

/// The boxed response body both fixed and streaming replies use.
type ReplyBody = BoxBody<Bytes, std::convert::Infallible>;

/// A running loopback server. Dropping it stops the thread and closes the listener.
pub struct Server {
    addr: SocketAddr,
    scheme: &'static str,
    ca_pem: Option<String>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    /// Start a plain-HTTP server on an ephemeral loopback port.
    ///
    /// TEST SUPPORT ONLY: this crate is a dev-dependency and never ships in the app, so the
    /// `panic!` here is a test-failure convenience, not a product crash path (AGENTS.md 4
    /// applies to non-test code; the no-panic lint allowance is scoped to this function).
    #[allow(clippy::panic)]
    pub fn start() -> Server {
        Server::spawn(false).unwrap_or_else(|err| panic!("testkit server failed to start: {err}"))
    }

    /// Start an HTTPS server with a fresh self-signed certificate (test support; see `start`).
    #[allow(clippy::panic)]
    pub fn start_tls() -> Server {
        Server::spawn(true).unwrap_or_else(|err| panic!("testkit TLS server failed to start: {err}"))
    }

    /// `http://127.0.0.1:PORT` or `https://127.0.0.1:PORT`.
    pub fn base_url(&self) -> String {
        format!("{}://{}", self.scheme, self.addr)
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The PEM the client must trust for the TLS variant (None for plain HTTP).
    pub fn ca_pem(&self) -> Option<&str> {
        self.ca_pem.as_deref()
    }

    /// Stop serving and join the thread.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(sender) = self.shutdown.take() {
            let _ = sender.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    fn spawn(tls: bool) -> Result<Server, String> {
        let identity = if tls { Some(crate::tls::generate()?) } else { None };
        let tls_config = match &identity {
            Some(identity) => Some(Arc::new(crate::tls::server_config(identity)?)),
            None => None,
        };
        let (addr_tx, addr_rx) = mpsc::channel::<SocketAddr>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let thread = std::thread::Builder::new()
            .name("postguy-testkit".to_string())
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
                                let tls_config = tls_config.clone();
                                tokio::spawn(async move {
                                    serve_connection(stream, tls_config).await;
                                });
                            }
                        }
                    }
                });
            })
            .map_err(|err| format!("spawning server thread: {err}"))?;

        let addr = addr_rx.recv_timeout(Duration::from_secs(5)).map_err(|err| format!("server did not report its address: {err}"))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {}
            Ok(Err(err)) => return Err(err),
            Err(err) => return Err(format!("server did not become ready: {err}")),
        }

        Ok(Server {
            addr,
            scheme: if tls { "https" } else { "http" },
            ca_pem: identity.map(|identity| identity.ca_pem),
            shutdown: Some(shutdown_tx),
            thread: Some(thread),
        })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn serve_connection(stream: tokio::net::TcpStream, tls_config: Option<Arc<rustls::ServerConfig>>) {
    match tls_config {
        None => serve_hyper(stream).await,
        Some(config) => {
            let acceptor = tokio_rustls::TlsAcceptor::from(config);
            if let Ok(tls_stream) = acceptor.accept(stream).await {
                serve_hyper(tls_stream).await;
            }
        }
    }
}

async fn serve_hyper<S>(stream: S)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let service = service_fn(|request: Request<HyperIncoming>| async move { Ok::<_, std::convert::Infallible>(handle(request).await) });
    let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
    let _ = builder.serve_connection_with_upgrades(TokioIo::new(stream), service).await;
}

async fn handle(request: Request<HyperIncoming>) -> Response<ReplyBody> {
    let (parts, body) = request.into_parts();
    let body = match body.collect().await {
        Ok(collected) => collected.to_bytes().to_vec(),
        Err(_) => Vec::new(),
    };
    let headers =
        parts.headers.iter().map(|(name, value)| (name.as_str().to_string(), String::from_utf8_lossy(value.as_bytes()).into_owned())).collect();
    let incoming = Incoming {
        method: parts.method,
        path: parts.uri.path().to_string(),
        query: parts.uri.query().unwrap_or_default().to_string(),
        headers,
        body,
    };
    build_reply(routes::route(incoming)).await
}

async fn build_reply(reply: Reply) -> Response<ReplyBody> {
    if reply.delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(reply.delay_ms)).await;
    }
    let streaming = !reply.chunks.is_empty();
    let body: ReplyBody = if streaming {
        BoxBody::new(StreamBody { chunks: reply.chunks, delay: Duration::from_millis(5) })
    } else {
        BoxBody::new(Full::new(Bytes::from(reply.body)))
    };
    let mut response = Response::new(body);
    *response.status_mut() = reply.status;
    for (name, value) in reply.headers {
        if let (Ok(name), Ok(value)) = (hyper::header::HeaderName::from_bytes(name.as_bytes()), hyper::header::HeaderValue::from_str(&value)) {
            response.headers_mut().append(name, value);
        }
    }
    response
}

/// A body that yields its chunks one by one with a small delay, so a client can observe the
/// stream (and a cancel test can abort mid-stream).
struct StreamBody {
    chunks: Vec<Vec<u8>>,
    delay: Duration,
}

impl hyper::body::Body for StreamBody {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        let Some(chunk) = this.chunks.pop() else {
            return std::task::Poll::Ready(None);
        };
        // Still a REAL stream from the client's point of view: each poll yields one chunk, and
        // the small sleep lets a reader observe progress between frames.
        if !this.delay.is_zero() {
            std::thread::sleep(this.delay);
        }
        std::task::Poll::Ready(Some(Ok(Frame::data(Bytes::from(chunk)))))
    }
}
