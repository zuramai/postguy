//! The opt-in UI control channel (PROMPT.md 6.2, AGENTS.md 3): only with `--control FILE`, bound
//! to loopback, a fresh random token per launch written to an owner-only file and never logged.
//! `postguy-cli ui` speaks this JSON protocol. Supported operations are state inspection,
//! commands, panel/tab setting and screenshots (via egui's viewport screenshot request).
//! Synthetic pointer clicks/typing are intentionally not emulated; use commands/set instead.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

/// One control request forwarded to the UI thread.
pub type ControlRequest = (ControlCommand, Sender<Result<Value, String>>);

/// The operations in the protocol.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlCommand {
    Inspect,
    Command(String),
    Set(String, Value),
    Screenshot(PathBuf),
    Click(f32, f32),
    Key(String),
    Type(String),
}

/// The token file contents.
#[derive(Debug, Clone, Serialize)]
pub struct ControlFile {
    pub port: u16,
    pub token: String,
    pub pid: u32,
}

/// Raised once per request queue; the app drains it each frame.
pub type ControlQueue = Arc<Mutex<VecDeque<ControlRequest>>>;

/// A running control server. Dropping it stops the listener and removes the token file.
pub struct ControlServer {
    token: String,
    queue: ControlQueue,
    wake: Arc<Mutex<Option<egui::Context>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    file: PathBuf,
}

impl ControlServer {
    /// Start on `127.0.0.1:0` and write the owner-only token file.
    pub fn start(file: PathBuf) -> Result<(ControlServer, ControlQueue), String> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|err| format!("binding the control channel: {err}"))?;
        listener.set_nonblocking(true).map_err(|err| format!("configuring the control channel: {err}"))?;
        let addr = listener.local_addr().map_err(|err| format!("reading the control address: {err}"))?;
        let token = random_token();
        let queue: ControlQueue = Arc::new(Mutex::new(VecDeque::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let wake: Arc<Mutex<Option<egui::Context>>> = Arc::new(Mutex::new(None));
        write_control_file(&file, &token, addr)?;
        let thread_queue = queue.clone();
        let thread_token = token.clone();
        let thread_stop = stop.clone();
        let thread_wake = wake.clone();
        let thread = std::thread::Builder::new()
            .name("postguy-control".to_string())
            .spawn(move || serve(listener, thread_token, thread_queue, thread_wake, thread_stop))
            .map_err(|err| format!("starting the control thread: {err}"))?;
        Ok((ControlServer { token, queue: queue.clone(), wake, stop, thread: Some(thread), file }, queue))
    }

    pub fn queue(&self) -> ControlQueue {
        self.queue.clone()
    }

    /// The UI calls this once with its context so a request can wake an idle (no-repaint) app.
    pub fn set_context(&self, context: egui::Context) {
        if let Ok(mut wake) = self.wake.lock() {
            *wake = Some(context);
        }
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        remove_own_file(&self.file, &self.token);
    }
}

fn random_token() -> String {
    format!("{:016x}{:016x}{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..), fastrand::u64(..), fastrand::u64(..))
}

fn write_control_file(path: &std::path::Path, token: &str, addr: SocketAddr) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|err| format!("creating {}: {err}", parent.display()))?;
    }
    // Remove a stale file first (never follow a symlink someone planted) and create fresh 0600.
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("replacing {}: {err}", path.display())),
    }
    let file = ControlFile { port: addr.port(), token: token.to_string(), pid: std::process::id() };
    let bytes = serde_json::to_vec(&file).map_err(|err| err.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut handle = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(|err| format!("creating {}: {err}", path.display()))?;
        handle.write_all(&bytes).map_err(|err| format!("writing {}: {err}", path.display()))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .and_then(|mut handle| handle.write_all(&bytes))
            .map_err(|err| format!("writing {}: {err}", path.display()))
    }
}

/// Remove the token file only when it still contains this launch's token.
fn remove_own_file(path: &std::path::Path, token: &str) {
    let Ok(bytes) = std::fs::read(path) else { return };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return };
    if value.get("token").and_then(serde_json::Value::as_str) == Some(token) {
        let _ = std::fs::remove_file(path);
    }
}

fn serve(listener: TcpListener, token: String, queue: ControlQueue, wake: Arc<Mutex<Option<egui::Context>>>, stop: Arc<AtomicBool>) {
    const MAX_HANDLERS: usize = 16;
    let active = Arc::new(AtomicUsize::new(0));
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut stream, _peer)) => {
                if active.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| (count < MAX_HANDLERS).then_some(count + 1)).is_err() {
                    let _ = respond(&mut stream, 429, &serde_json::json!({"error": "too many concurrent control connections"}));
                    continue;
                }
                let token = token.clone();
                let queue = queue.clone();
                let wake = wake.clone();
                let active = active.clone();
                std::thread::spawn(move || {
                    let _ = handle(stream, &token, queue, &wake);
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break,
        }
    }
}

fn handle(mut stream: TcpStream, token: &str, queue: ControlQueue, wake: &Mutex<Option<egui::Context>>) -> Result<(), String> {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut reader = BufReader::new(stream.try_clone().map_err(|err| err.to_string())?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).map_err(|err| err.to_string())?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let mut content_length = 0usize;
    let mut authorized = false;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|err| err.to_string())?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0).min(1 << 20);
            }
            if name == "authorization" && value == format!("Bearer {token}") {
                authorized = true;
            }
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).map_err(|err| err.to_string())?;
    }
    if !authorized {
        let _ = respond(&mut stream, 401, &serde_json::json!({"error": "missing or invalid control token"}));
        return Ok(());
    }
    let command = parse_target(&method, &target, &body);
    let command = match command {
        Ok(command) => command,
        Err(error) => {
            let _ = respond(&mut stream, 400, &serde_json::json!({"error": error}));
            return Ok(());
        }
    };
    let (sender, receiver) = channel();
    if let Ok(mut queue) = queue.lock() {
        queue.push_back((command, sender));
        drop(queue);
        if let Ok(wake) = wake.lock()
            && let Some(context) = wake.as_ref()
        {
            context.request_repaint();
        }
    } else {
        let _ = respond(&mut stream, 503, &serde_json::json!({"error": "the UI is not available"}));
        return Ok(());
    }
    match receiver.recv_timeout(Duration::from_secs(20)) {
        Ok(Ok(value)) => {
            let _ = respond(&mut stream, 200, &value);
        }
        Ok(Err(error)) => {
            let _ = respond(&mut stream, 400, &serde_json::json!({"error": error}));
        }
        Err(_) => {
            let _ = respond(&mut stream, 504, &serde_json::json!({"error": "the UI did not answer in time"}));
        }
    }
    Ok(())
}

fn parse_target(method: &str, target: &str, body: &[u8]) -> Result<ControlCommand, String> {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    match (method, path) {
        ("GET", "/inspect") => Ok(ControlCommand::Inspect),
        ("POST", "/command") => {
            let value: Value = serde_json::from_slice(body).map_err(|err| format!("body is not JSON: {err}"))?;
            value.get("id").and_then(Value::as_str).map(|id| ControlCommand::Command(id.to_string())).ok_or_else(|| "`id` is required".into())
        }
        ("POST", "/set") => {
            let value: Value = serde_json::from_slice(body).map_err(|err| format!("body is not JSON: {err}"))?;
            let key = value.get("key").and_then(Value::as_str).ok_or("`key` is required")?.to_string();
            let value = value.get("value").cloned().unwrap_or(Value::Null);
            Ok(ControlCommand::Set(key, value))
        }
        ("GET", "/screenshot") => {
            let path = query
                .split('&')
                .find_map(|pair| pair.strip_prefix("path="))
                .map(percent_decode)
                .filter(|path| !path.is_empty())
                .ok_or_else(|| "`path` is required".to_string())?;
            Ok(ControlCommand::Screenshot(PathBuf::from(path)))
        }
        ("POST", "/click") => {
            let value: Value = serde_json::from_slice(body).map_err(|err| format!("body is not JSON: {err}"))?;
            let x = value.get("x").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let y = value.get("y").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            Ok(ControlCommand::Click(x, y))
        }
        ("POST", "/key") => {
            let value: Value = serde_json::from_slice(body).map_err(|err| format!("body is not JSON: {err}"))?;
            Ok(ControlCommand::Key(value.get("key").and_then(Value::as_str).unwrap_or_default().to_string()))
        }
        ("POST", "/type") => {
            let value: Value = serde_json::from_slice(body).map_err(|err| format!("body is not JSON: {err}"))?;
            Ok(ControlCommand::Type(value.get("text").and_then(Value::as_str).unwrap_or_default().to_string()))
        }
        _ => Err(format!("unknown control endpoint {method} {path}")),
    }
}

fn respond(stream: &mut TcpStream, status: u16, body: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        429 => "Too Many Requests",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    };
    let head =
        format!("HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len());
    stream.write_all(head.as_bytes())?;
    stream.write_all(&bytes)?;
    stream.flush()
}

/// Minimal percent decoding for the screenshot path query.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes.get(index) == Some(&b'%')
            && let (Some(high), Some(low)) = (bytes.get(index + 1).and_then(|b| hex(*b)), bytes.get(index + 2).and_then(|b| hex(*b)))
        {
            out.push(high * 16 + low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
