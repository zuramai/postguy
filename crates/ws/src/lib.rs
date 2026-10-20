//! WebSocket client (M6.1): handshake headers/subprotocols, TLS via the same rustls policy as
//! HTTP, ping/pong, close codes, message size caps and a typed event stream. Reconnection policy
//! lives in the engine/session; this crate is one connection.
//!
//! Not supported in v1: permessage-deflate, HTTP proxies and handshake redirects (they return an
//! actionable error rather than silently falling back).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use postguy_http::tls::client_config;
use postguy_http::{ClientIdentity, TlsMode};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Bytes, Message};
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

/// Connection options from a stored WebSocket request.
#[derive(Debug, Clone)]
pub struct WsOptions {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub protocols: Vec<String>,
    pub handshake_timeout_ms: u64,
    pub max_message_bytes: usize,
    pub tls: TlsMode,
    pub client_identity: Option<ClientIdentity>,
}

impl Default for WsOptions {
    fn default() -> Self {
        WsOptions {
            url: String::new(),
            headers: Vec::new(),
            protocols: Vec::new(),
            handshake_timeout_ms: 30_000,
            max_message_bytes: 0,
            tls: TlsMode::default(),
            client_identity: None,
        }
    }
}

/// One event from the socket (all messages are capped by the handshake config).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsEvent {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Closed { code: Option<u16>, reason: String },
    Error(String),
}

/// An open WebSocket. `next`/`send` are async; the caller drives them on its runtime.
#[derive(Debug)]
pub struct WsConnection {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    pub negotiated_protocol: Option<String>,
}

impl WsConnection {
    /// Connect, complete the handshake and return the socket.
    pub async fn connect(options: &WsOptions) -> Result<WsConnection, String> {
        if options.url.trim().is_empty() {
            return Err("the WebSocket URL is empty".into());
        }
        if !(options.url.starts_with("ws://") || options.url.starts_with("wss://")) {
            return Err("the WebSocket URL must start with ws:// or wss://".into());
        }
        let mut request = options.url.as_str().into_client_request().map_err(|err| format!("WebSocket URL: {err}"))?;
        for (name, value) in &options.headers {
            let name = name.parse::<http::HeaderName>().map_err(|err| format!("WebSocket header `{name}`: {err}"))?;
            let value = value.parse::<http::HeaderValue>().map_err(|err| format!("WebSocket header value for `{name}`: {err}"))?;
            request.headers_mut().append(name, value);
        }
        if !options.protocols.is_empty() {
            let value = options.protocols.join(", ");
            let value = value.parse::<http::HeaderValue>().map_err(|err| format!("subprotocols: {err}"))?;
            request.headers_mut().insert("sec-websocket-protocol", value);
        }
        let tls = client_config(&options.tls, &[], options.client_identity.as_ref())?;
        let connector = Connector::Rustls(Arc::new(tls));
        let config = WebSocketConfig::default()
            .max_message_size((options.max_message_bytes > 0).then_some(options.max_message_bytes))
            .max_frame_size((options.max_message_bytes > 0).then_some(options.max_message_bytes));
        let timeout = Duration::from_millis(options.handshake_timeout_ms.max(1));
        let (stream, response) =
            tokio::time::timeout(timeout, tokio_tungstenite::connect_async_tls_with_config(request, Some(config), false, Some(connector)))
                .await
                .map_err(|_| format!("WebSocket handshake timed out after {} ms", options.handshake_timeout_ms))?
                .map_err(|err| format!("WebSocket handshake: {err}"))?;
        let negotiated_protocol = response.headers().get("sec-websocket-protocol").and_then(|value| value.to_str().ok()).map(str::to_string);
        Ok(WsConnection { stream, negotiated_protocol })
    }

    /// Next event; `None` when the stream ended cleanly.
    pub async fn next(&mut self) -> Option<WsEvent> {
        match self.stream.next().await {
            None => None,
            Some(Ok(message)) => Some(match message {
                Message::Text(text) => WsEvent::Text(text.to_string()),
                Message::Binary(bytes) => WsEvent::Binary(bytes.to_vec()),
                Message::Ping(bytes) => WsEvent::Ping(bytes.to_vec()),
                Message::Pong(bytes) => WsEvent::Pong(bytes.to_vec()),
                Message::Close(frame) => WsEvent::Closed {
                    code: frame.as_ref().map(|frame| u16::from(frame.code)),
                    reason: frame.as_ref().map(|frame| frame.reason.to_string()).unwrap_or_default(),
                },
                Message::Frame(_) => WsEvent::Error("unexpected raw frame".into()),
            }),
            Some(Err(err)) => Some(WsEvent::Error(err.to_string())),
        }
    }

    pub async fn send_text(&mut self, text: &str) -> Result<(), String> {
        self.stream.send(Message::Text(text.into())).await.map_err(|err| err.to_string())
    }

    pub async fn send_binary(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        self.stream.send(Message::Binary(Bytes::from(bytes))).await.map_err(|err| err.to_string())
    }

    pub async fn ping(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        self.stream.send(Message::Ping(Bytes::from(bytes))).await.map_err(|err| err.to_string())
    }

    pub async fn close(&mut self, code: u16, reason: &str) -> Result<(), String> {
        let code = tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::from(code);
        self.stream
            .send(Message::Close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame { code, reason: reason.into() })))
            .await
            .map_err(|err| err.to_string())
    }
}
