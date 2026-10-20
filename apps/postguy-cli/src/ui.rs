//! `postguy-cli ui`: the client for the app's opt-in control channel (AGENTS.md 3).
//! The channel is loopback-only and authenticated with the owner-only token file the app wrote
//! for this launch. This client never guesses a token and never widens the bind address.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct ControlFile {
    port: u16,
    token: String,
}

pub fn run(args: &[String]) -> i32 {
    match execute(args) {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(error) => {
            eprintln!("ui: {error}");
            2
        }
    }
}

fn execute(args: &[String]) -> Result<String, String> {
    let (control, args) = take_flag(args, "--control");
    let control = control.ok_or("--control FILE is required; launch the app with `postguy --control FILE`")?;
    let (root, args) = take_flag(&args, "--root");
    let (out, args) = take_flag(&args, "--out");
    let action = args.first().map(String::as_str).ok_or("expected inspect, command, set, screenshot, click, key or type")?;
    let file = read_control_file(Path::new(&control))?;
    match action {
        "inspect" => http(&file, "GET", "/inspect", None),
        "command" => {
            let id = args.get(1).ok_or("command ID is required")?;
            http(&file, "POST", "/command", Some(&serde_json::json!({"id": id})))
        }
        "set" => {
            let assignment = args.get(1).ok_or("set needs key=value")?;
            let (key, value) = assignment.split_once('=').ok_or("set needs key=value")?;
            let value = serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()));
            http(&file, "POST", "/set", Some(&serde_json::json!({"key": key, "value": value})))
        }
        "screenshot" => {
            let out = out.ok_or("screenshot needs --out FILE")?;
            let path = resolve_out(&out, root.as_deref())?;
            let encoded = percent_encode(&path.to_string_lossy());
            http(&file, "GET", &format!("/screenshot?path={encoded}"), None)
        }
        "click" => {
            let x = args.get(1).and_then(|value| value.parse::<f32>().ok()).ok_or("click needs X Y")?;
            let y = args.get(2).and_then(|value| value.parse::<f32>().ok()).ok_or("click needs X Y")?;
            http(&file, "POST", "/click", Some(&serde_json::json!({"x": x, "y": y})))
        }
        "key" => {
            let key = args.get(1).ok_or("key needs a key name")?;
            http(&file, "POST", "/key", Some(&serde_json::json!({"key": key})))
        }
        "type" => {
            let text = args.get(1).ok_or("type needs text")?;
            http(&file, "POST", "/type", Some(&serde_json::json!({"text": text})))
        }
        other => Err(format!("unknown ui action `{other}`")),
    }
}

fn resolve_out(given: &str, root: Option<&str>) -> Result<PathBuf, String> {
    match root {
        Some(dir) => {
            std::fs::create_dir_all(dir).map_err(|err| format!("creating --root {dir}: {err}"))?;
            let root = postguy_automation::Root::new(Path::new(dir)).map_err(|err| err.to_string())?;
            root.resolve_existing(given).map_err(|err| err.to_string())
        }
        None => Ok(PathBuf::from(given)),
    }
}

fn read_control_file(path: &Path) -> Result<ControlFile, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let metadata = std::fs::metadata(path).map_err(|err| format!("reading {}: {err}", path.display()))?;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(format!("{} is not owner-only; refusing to read the token", path.display()));
        }
    }
    let bytes = std::fs::read(path).map_err(|err| format!("reading {}: {err}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|err| format!("{} is not a control file: {err}", path.display()))
}

fn http(file: &ControlFile, method: &str, target: &str, body: Option<&Value>) -> Result<String, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", file.port)).map_err(|err| format!("connecting to the control channel: {err}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(30))).ok();
    let bytes = body.map(serde_json::to_vec).transpose().map_err(|err| err.to_string())?.unwrap_or_default();
    let head = format!(
        "{method} {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        file.token,
        bytes.len()
    );
    stream.write_all(head.as_bytes()).map_err(|err| format!("writing the control request: {err}"))?;
    stream.write_all(&bytes).map_err(|err| format!("writing the control request: {err}"))?;
    stream.flush().ok();
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).map_err(|err| format!("reading the control response: {err}"))?;
    let status: u16 = status_line.split_whitespace().nth(1).and_then(|value| value.parse().ok()).ok_or("malformed control response")?;
    let mut length = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|err| format!("reading the control response: {err}"))?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = Vec::new();
    match length {
        Some(length) => {
            body.reserve(length.min(16 << 20));
            reader.take(length as u64).read_to_end(&mut body).map_err(|err| format!("reading the control response: {err}"))?;
        }
        None => {
            reader.read_to_end(&mut body).map_err(|err| format!("reading the control response: {err}"))?;
        }
    }
    let text = String::from_utf8_lossy(&body).into_owned();
    if (200..300).contains(&status) { Ok(text) } else { Err(text) }
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn take_flag(args: &[String], name: &str) -> (Option<String>, Vec<String>) {
    let mut value = None;
    let mut rest = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == name {
            if let Some(next) = args.get(index + 1) {
                value = Some(next.clone());
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }
        rest.push(args[index].clone());
        index += 1;
    }
    (value, rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::mpsc::channel;

    fn fake_server(body: &'static str, expected_path: &'static str, expected_token: &'static str) -> (u16, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test listener");
        let port = listener.local_addr().expect("test address").port();
        let (sender, receiver) = channel();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let mut request_line = String::new();
                reader.read_line(&mut request_line).ok();
                let mut auth = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line.trim_end().is_empty() {
                        break;
                    }
                    if let Some(value) = line.trim_end().strip_prefix("Authorization: ") {
                        auth = value.to_string();
                    }
                }
                let _ = sender.send(format!("{request_line}|{auth}"));
                let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                stream.write_all(response.as_bytes()).ok();
            }
        });
        assert_eq!(expected_path, "/inspect");
        assert_eq!(expected_token, "test-token");
        (port, receiver)
    }

    #[test]
    fn inspect_sends_the_bearer_token_and_prints_the_reply() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("control.json");
        let (port, receiver) = fake_server("{\"tabs\":[]}", "/inspect", "test-token");
        std::fs::write(&path, serde_json::json!({"port": port, "token": "test-token", "pid": 1}).to_string()).expect("control file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("mode");
        }
        let text = execute(&["--control".into(), path.display().to_string(), "inspect".into()]).expect("inspect");
        assert_eq!(text, "{\"tabs\":[]}");
        let request = receiver.recv().expect("request");
        assert!(request.contains("GET /inspect HTTP/1.1") && request.contains("|Bearer test-token"), "{request}");
    }

    #[test]
    fn a_group_readable_control_file_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("control.json");
        std::fs::write(&path, serde_json::json!({"port": 1, "token": "test-token", "pid": 1}).to_string()).expect("control file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("mode");
            let error = execute(&["--control".into(), path.display().to_string(), "inspect".into()]).expect_err("refused");
            assert!(error.contains("owner-only"), "{error}");
        }
    }

    #[test]
    fn screenshot_paths_stay_inside_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inside = resolve_out("shots/ui.png", Some(&dir.path().display().to_string())).expect("inside");
        assert!(inside.starts_with(std::fs::canonicalize(dir.path()).expect("canonical")));
        let outside = resolve_out("../escape.png", Some(&dir.path().display().to_string()));
        assert!(outside.is_err(), "outside paths are refused");
    }
}
