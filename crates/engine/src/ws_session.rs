//! WebSocket sessions (M6.2): one background task per connection, an event log for tools/UI and
//! commands for sending text/binary/ping and closing. Reconnection/UI live above this module.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use postguy_model::Id;
use postguy_ws::{WsConnection, WsEvent, WsOptions};
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::Engine;
use crate::error::{EngineError, Result};

/// A capped event row (the UI timeline / tools).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WsEventView {
    pub seq: u64,
    pub time_ms: i64,
    /// `sent`, `received` or `network`.
    pub direction: String,
    /// `text`, `binary`, `ping`, `pong`, `open`, `close`, `error`.
    pub kind: String,
    pub text: Option<String>,
    pub bytes: usize,
    pub detail: Option<String>,
}

struct WsState {
    events: Mutex<VecDeque<WsEventView>>,
    connected: AtomicBool,
    next_seq: Mutex<u64>,
    total_bytes: Mutex<usize>,
}

/// The handle stored by the engine; commands go to the socket task.
pub struct WsHandle {
    sender: UnboundedSender<WsCommand>,
    state: Arc<WsState>,
}

impl WsHandle {
    pub fn events(&self, since: u64) -> Vec<WsEventView> {
        self.state.events.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).iter().filter(|event| event.seq > since).cloned().collect()
    }

    pub fn connected(&self) -> bool {
        self.state.connected.load(Ordering::SeqCst)
    }

    pub fn send_text(&self, text: String) -> Result<()> {
        self.sender.send(WsCommand::Text(text)).map_err(|_| EngineError::BadRequest { reason: "the WebSocket is closed".into() })
    }

    pub fn send_binary(&self, bytes: Vec<u8>) -> Result<()> {
        self.sender.send(WsCommand::Binary(bytes)).map_err(|_| EngineError::BadRequest { reason: "the WebSocket is closed".into() })
    }

    pub fn ping(&self, bytes: Vec<u8>) -> Result<()> {
        self.sender.send(WsCommand::Ping(bytes)).map_err(|_| EngineError::BadRequest { reason: "the WebSocket is closed".into() })
    }

    pub fn close(&self, code: u16, reason: String) {
        let _ = self.sender.send(WsCommand::Close(code, reason));
    }
}

enum WsCommand {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Close(u16, String),
}

impl Engine {
    /// Open a WebSocket and return its session id.
    pub fn ws_connect(&self, options: WsOptions) -> Result<String> {
        let id = Id::generate().0;
        let state = Arc::new(WsState {
            events: Mutex::new(VecDeque::new()),
            connected: AtomicBool::new(false),
            next_seq: Mutex::new(0),
            total_bytes: Mutex::new(0),
        });
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let handle = WsHandle { sender, state: state.clone() };
        let task_state = state.clone();
        self.inner.runtime.spawn(async move {
            let push = |direction: &str, kind: &str, text: Option<String>, bytes: usize, detail: Option<String>| {
                let mut seq = task_state.next_seq.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                *seq = seq.saturating_add(1);
                let event = WsEventView {
                    seq: *seq,
                    time_ms: postguy_util::time::now_unix_ms(),
                    direction: direction.into(),
                    kind: kind.into(),
                    text,
                    bytes,
                    detail,
                };
                drop(seq);
                let mut events = task_state.events.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                events.push_back(event);
                while events.len() > 10_000 {
                    events.pop_front();
                }
                let mut total = task_state.total_bytes.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                *total = total.saturating_add(bytes);
                while *total > 64 << 20 && events.len() > 1 {
                    if let Some(removed) = events.pop_front() {
                        *total = total.saturating_sub(removed.bytes);
                    }
                }
            };
            let mut connection = match WsConnection::connect(&options).await {
                Ok(connection) => connection,
                Err(err) => {
                    push("network", "error", None, 0, Some(err));
                    return;
                }
            };
            task_state.connected.store(true, Ordering::SeqCst);
            push("network", "open", None, 0, Some(connection.negotiated_protocol.clone().unwrap_or_else(|| "connected".into())));
            loop {
                tokio::select! {
                    command = receiver.recv() => {
                        match command {
                            Some(WsCommand::Text(text)) => {
                                if let Err(err) = connection.send_text(&text).await {
                                    push("network", "error", None, 0, Some(err));
                                } else {
                                    push("sent", "text", Some(text.clone()), text.len(), None);
                                }
                            }
                            Some(WsCommand::Binary(bytes)) => {
                                if let Err(err) = connection.send_binary(bytes.clone()).await {
                                    push("network", "error", None, 0, Some(err));
                                } else {
                                    push("sent", "binary", None, bytes.len(), None);
                                }
                            }
                            Some(WsCommand::Ping(bytes)) => {
                                let _ = connection.ping(bytes.clone()).await;
                                push("sent", "ping", None, bytes.len(), None);
                            }
                            Some(WsCommand::Close(code, reason)) => {
                                let _ = connection.close(code, &reason).await;
                                task_state.connected.store(false, Ordering::SeqCst);
                                push("network", "close", None, 0, Some(format!("{code} {reason}")));
                                break;
                            }
                            None => break,
                        }
                    }
                    event = connection.next() => {
                        match event {
                            Some(WsEvent::Text(text)) => push("received", "text", Some(text.clone()), text.len(), None),
                            Some(WsEvent::Binary(bytes)) => push("received", "binary", None, bytes.len(), None),
                            Some(WsEvent::Ping(bytes)) => push("received", "ping", None, bytes.len(), None),
                            Some(WsEvent::Pong(bytes)) => push("received", "pong", None, bytes.len(), None),
                            Some(WsEvent::Closed { code, reason }) => {
                                task_state.connected.store(false, Ordering::SeqCst);
                                push("network", "close", None, 0, Some(format!("{} {}", code.unwrap_or(1005), reason)));
                                break;
                            }
                            Some(WsEvent::Error(err)) => {
                                task_state.connected.store(false, Ordering::SeqCst);
                                push("network", "error", None, 0, Some(err));
                                break;
                            }
                            None => {
                                task_state.connected.store(false, Ordering::SeqCst);
                                push("network", "close", None, 0, Some("stream ended".into()));
                                break;
                            }
                        }
                    }
                }
            }
            task_state.connected.store(false, Ordering::SeqCst);
        });
        if let Ok(mut sessions) = self.inner.ws_sessions.lock() {
            sessions.insert(id.clone(), handle);
        }
        Ok(id)
    }

    /// Session events after `since` (0 = all).
    pub fn ws_events(&self, session: &str, since: u64) -> Result<Vec<WsEventView>> {
        let sessions = self.inner.ws_sessions.lock().map_err(|_| EngineError::BadRequest { reason: "WebSocket session lock poisoned".into() })?;
        let handle = sessions.get(session).ok_or_else(|| EngineError::NotFound { what: "WebSocket session".into(), id: session.into() })?;
        Ok(handle.events(since))
    }

    pub fn ws_send_text(&self, session: &str, text: String) -> Result<()> {
        self.with_ws(session, |handle| handle.send_text(text))
    }

    pub fn ws_send_binary(&self, session: &str, bytes: Vec<u8>) -> Result<()> {
        self.with_ws(session, |handle| handle.send_binary(bytes))
    }

    pub fn ws_ping(&self, session: &str, bytes: Vec<u8>) -> Result<()> {
        self.with_ws(session, |handle| handle.ping(bytes))
    }

    pub fn ws_disconnect(&self, session: &str, code: u16, reason: String) -> Result<()> {
        self.with_ws(session, |handle| {
            handle.close(code, reason);
            Ok(())
        })
    }

    pub fn ws_connected(&self, session: &str) -> Result<bool> {
        let sessions = self.inner.ws_sessions.lock().map_err(|_| EngineError::BadRequest { reason: "WebSocket session lock poisoned".into() })?;
        sessions
            .get(session)
            .map(|handle| handle.connected())
            .ok_or_else(|| EngineError::NotFound { what: "WebSocket session".into(), id: session.into() })
    }

    fn with_ws<T>(&self, session: &str, work: impl FnOnce(&WsHandle) -> Result<T>) -> Result<T> {
        let sessions = self.inner.ws_sessions.lock().map_err(|_| EngineError::BadRequest { reason: "WebSocket session lock poisoned".into() })?;
        let handle = sessions.get(session).ok_or_else(|| EngineError::NotFound { what: "WebSocket session".into(), id: session.into() })?;
        work(handle)
    }
}

impl Engine {
    /// Build connection options from a stored WebSocket request, resolving URL/header/param
    /// variables against the active environment and the workspace settings.
    pub fn ws_options_for(&self, collection: &Id, request: &Id) -> Result<WsOptions> {
        let collection_model = self.collection(collection)?;
        let request = self.request(collection, request)?;
        let postguy_model::RequestKind::WebSocket(ws) = &request.kind else {
            return Err(EngineError::BadRequest { reason: "the request is not a WebSocket request".into() });
        };
        let mut url = self.resolve_variables_with(&ws.url, Some(collection), None)?.text;
        let params: Vec<(String, String)> = ws
            .params
            .iter()
            .filter(|row| row.enabled && !row.key.is_empty())
            .map(|row| {
                Ok((
                    self.resolve_variables_with(&row.key, Some(collection), None)?.text,
                    self.resolve_variables_with(&row.value, Some(collection), None)?.text,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        if !params.is_empty() {
            let query = params
                .iter()
                .map(|(key, value)| format!("{}={}", postguy_urlkit::encode_query(key), postguy_urlkit::encode_query(value)))
                .collect::<Vec<_>>()
                .join("&");
            url.push(if url.contains('?') { '&' } else { '?' });
            url.push_str(&query);
        }
        let headers = ws
            .headers
            .iter()
            .filter(|row| row.enabled && !row.key.is_empty())
            .map(|row| Ok((row.key.clone(), self.resolve_variables_with(&row.value, Some(collection), None)?.text)))
            .collect::<Result<Vec<_>>>()?;
        let global = self.settings();
        let tls = if ws.settings.verify_ssl {
            postguy_http::TlsMode::Verify { ca_pem_path: (!global.ca_bundle_path.trim().is_empty()).then(|| global.ca_bundle_path.clone()) }
        } else {
            postguy_http::TlsMode::Insecure
        };
        let client_identity = if !global.client_cert_path.trim().is_empty() && !global.client_key_path.trim().is_empty() {
            Some(postguy_http::ClientIdentity { cert_path: global.client_cert_path.clone(), key_path: global.client_key_path.clone() })
        } else {
            None
        };
        let max_message_bytes = usize::try_from(ws.settings.max_message_mb.saturating_mul(1024 * 1024)).unwrap_or(usize::MAX);
        let _ = collection_model;
        Ok(WsOptions {
            url,
            headers,
            protocols: ws.protocols.clone(),
            handshake_timeout_ms: if ws.settings.handshake_timeout_ms == 0 { 30_000 } else { ws.settings.handshake_timeout_ms },
            max_message_bytes,
            tls,
            client_identity,
        })
    }

    /// Connect a stored WebSocket request and return the session id.
    pub fn ws_connect_saved(&self, collection: &Id, request: &Id) -> Result<String> {
        self.ws_connect(self.ws_options_for(collection, request)?)
    }
}
