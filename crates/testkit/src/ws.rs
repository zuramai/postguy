//! A loopback WebSocket echo server. Binds 127.0.0.1:0 on its own current-thread runtime, so
//! sync tests can drive a client from another runtime. Text and binary frames are echoed;
//! ping/pong is handled by tungstenite.
#![allow(clippy::result_large_err)] // tungstenite's handshake error is large by design

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

/// The echo server's URL and control flag.
pub struct WsEchoServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl WsEchoServer {
    pub fn start() -> Result<WsEchoServer, String> {
        Self::start_with_greeting(None)
    }

    /// Start and send `greeting` immediately after every handshake (useful for event tests).
    pub fn start_with_greeting(greeting: Option<String>) -> Result<WsEchoServer, String> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|err| format!("binding the WebSocket echo server: {err}"))?;
        let addr = listener.local_addr().map_err(|err| format!("reading the WebSocket echo address: {err}"))?;
        listener.set_nonblocking(true).map_err(|err| format!("configuring the WebSocket echo server: {err}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        std::thread::Builder::new()
            .name("testkit-ws".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
                runtime.block_on(async move {
                    let Ok(listener) = TcpListener::from_std(listener) else { return };
                    loop {
                        if thread_stop.load(Ordering::SeqCst) {
                            break;
                        }
                        let accept = tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept()).await;
                        let Ok(Ok((stream, _))) = accept else { continue };
                        let greeting = greeting.clone();
                        tokio::spawn(async move {
                            let callback =
                                |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                                 mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                                    if let Some(protocol) = request.headers().get("sec-websocket-protocol") {
                                        response.headers_mut().insert("sec-websocket-protocol", protocol.clone());
                                    }
                                    Ok(response)
                                };
                            let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(stream, callback).await else { return };
                            if let Some(greeting) = greeting {
                                let _ = socket.send(Message::Text(greeting.into())).await;
                            }
                            while let Some(Ok(message)) = socket.next().await {
                                match message {
                                    Message::Text(text) => {
                                        let _ = socket.send(Message::Text(text)).await;
                                    }
                                    Message::Binary(bytes) => {
                                        let _ = socket.send(Message::Binary(bytes)).await;
                                    }
                                    Message::Close(frame) => {
                                        let _ = socket.send(Message::Close(frame)).await;
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                        });
                    }
                });
            })
            .map_err(|err| format!("starting the WebSocket echo thread: {err}"))?;
        Ok(WsEchoServer { addr, stop })
    }

    pub fn url(&self) -> String {
        format!("ws://{}/echo", self.addr)
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for WsEchoServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}
