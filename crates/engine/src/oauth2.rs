//! OAuth 2.0 token acquisition (M5.2): protocol pieces come from `postguy-auth`; this module
//! performs the token endpoint request over the engine's HTTP client (no history/console entry,
//! no redirects), returns the parsed token and can persist it in the workspace secrets store.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use postguy_auth::oauth2 as protocol;
use postguy_auth::oauth2::{TokenRequest, TokenResponse, parse_token_response};
use postguy_http::{Cancel, RequestSpec};
use postguy_model::RequestSettings;
use postguy_model::auth::OAuth2Auth;
use serde::{Deserialize, Serialize};

use crate::Engine;
use crate::error::{EngineError, Result};

/// A token saved in the secrets store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StoredToken {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub scope: String,
    pub expires_at_ms: i64,
}

impl StoredToken {
    pub fn from_response(response: &TokenResponse) -> StoredToken {
        let expires_at_ms = response
            .expires_in
            .map(|seconds| {
                let now = postguy_util::time::now_unix_ms();
                now.saturating_add(i64::try_from(seconds.saturating_mul(1000)).unwrap_or(i64::MAX))
            })
            .unwrap_or(0);
        StoredToken {
            access_token: response.access_token.clone(),
            refresh_token: response.refresh_token.clone(),
            token_type: response.token_type.clone(),
            scope: response.scope.clone(),
            expires_at_ms,
        }
    }

    pub fn expired(&self, now_ms: i64) -> bool {
        self.expires_at_ms > 0 && self.expires_at_ms <= now_ms
    }
}

impl Engine {
    /// Run one token request (client credentials, password, code exchange or refresh).
    pub async fn oauth2_request(&self, request: TokenRequest) -> Result<TokenResponse> {
        let form = request.form.iter().map(|(key, value)| format!("{}={}", form_encode(key), form_encode(value))).collect::<Vec<_>>().join("&");
        let mut headers = vec![
            ("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string()),
            ("Accept".to_string(), "application/json".to_string()),
            ("User-Agent".to_string(), format!("Postguy/{}", env!("CARGO_PKG_VERSION"))),
        ];
        if let Some(authorization) = request.authorization {
            headers.push(("Authorization".to_string(), authorization));
        }
        let spec = RequestSpec {
            method: hyper::Method::POST,
            url: request.url,
            headers,
            body: form.into_bytes(),
            timeout_ms: 30_000,
            max_body_bytes: 4 << 20,
            decode_compression: true,
        };
        let client = self.client_for(&RequestSettings::default(), &self.settings())?;
        let cancel = Cancel::new();
        let response = client.send(spec, &cancel).await?;
        let token =
            parse_token_response(response.status.as_u16(), &response.body).map_err(|err| EngineError::BadRequest { reason: err.to_string() })?;
        if token.is_error() {
            return Err(EngineError::BadRequest { reason: format!("OAuth 2.0: {}", token.message()) });
        }
        Ok(token)
    }

    /// Blocking façade for the UI/CLI.
    pub fn oauth2_blocking(&self, request: TokenRequest) -> Result<TokenResponse> {
        self.inner.runtime.block_on(self.oauth2_request(request))
    }

    /// Exchange an authorization code, with the PKCE verifier when the grant needs it.
    pub fn oauth2_exchange_code(&self, auth: &OAuth2Auth, code: &str, verifier: Option<&str>) -> Result<TokenResponse> {
        let request = protocol::token_request(auth, Some(code), verifier).map_err(|err| EngineError::BadRequest { reason: err.to_string() })?;
        self.oauth2_blocking(request)
    }

    /// Password or client-credentials grant.
    pub fn oauth2_grant(&self, auth: &OAuth2Auth) -> Result<TokenResponse> {
        let request = protocol::token_request(auth, None, None).map_err(|err| EngineError::BadRequest { reason: err.to_string() })?;
        self.oauth2_blocking(request)
    }

    /// Refresh an access token.
    pub fn oauth2_refresh(&self, auth: &OAuth2Auth, refresh_token: &str) -> Result<TokenResponse> {
        let request = protocol::refresh_request(auth, refresh_token).map_err(|err| EngineError::BadRequest { reason: err.to_string() })?;
        self.oauth2_blocking(request)
    }

    /// Persist OAuth 2.0 tokens in the same secret keys the collection saver uses.
    pub fn oauth2_store_token(&self, owner: &str, token: &StoredToken) -> Result<()> {
        self.workspace().secrets.set(owner, "oauth2.access", (!token.access_token.is_empty()).then_some(token.access_token.as_str()))?;
        self.workspace().secrets.set(owner, "oauth2.refresh", (!token.refresh_token.is_empty()).then_some(token.refresh_token.as_str()))?;
        Ok(())
    }

    /// Read the stored token, if any.
    pub fn oauth2_stored_token(&self, owner: &str) -> Result<Option<StoredToken>> {
        let values = self.workspace().secrets.values_for(owner)?;
        let access_token = values.get("oauth2.access").cloned().unwrap_or_default();
        let refresh_token = values.get("oauth2.refresh").cloned().unwrap_or_default();
        if access_token.is_empty() && refresh_token.is_empty() {
            return Ok(None);
        }
        Ok(Some(StoredToken { access_token, refresh_token, token_type: "Bearer".into(), scope: String::new(), expires_at_ms: 0 }))
    }

    /// Delete a stored token.
    pub fn oauth2_forget_token(&self, owner: &str) -> Result<()> {
        self.workspace().secrets.set(owner, "oauth2.access", None)?;
        self.workspace().secrets.set(owner, "oauth2.refresh", None)?;
        Ok(())
    }
}

/// A one-shot loopback listener for the authorization-code redirect. The app opens the browser
/// on demand; the listener waits up to five minutes, validates `state`, returns the code and
/// closes. Binding is loopback-only on an ephemeral port.
pub struct OAuth2Callback {
    pub redirect_uri: String,
    pub state: String,
    pub verifier: String,
    receiver: Receiver<std::result::Result<String, String>>,
    stop: Arc<AtomicBool>,
}

impl Drop for OAuth2Callback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl OAuth2Callback {
    pub fn start(pkce_method: postguy_model::auth::PkceMethod) -> Result<OAuth2Callback> {
        let listener =
            TcpListener::bind(("127.0.0.1", 0)).map_err(|err| EngineError::BadRequest { reason: format!("binding the OAuth callback: {err}") })?;
        let port = listener.local_addr().map_err(|err| EngineError::BadRequest { reason: err.to_string() })?.port();
        let state = postguy_auth::random_nonce();
        let (verifier, _challenge) = protocol::pkce(pkce_method);
        let expected_state = state.clone();
        let (sender, receiver) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        std::thread::Builder::new()
            .name("oauth2-callback".into())
            .spawn(move || {
                let result = wait_for_callback(listener, &expected_state, &thread_stop);
                let _ = sender.send(result);
            })
            .map_err(|err| EngineError::BadRequest { reason: format!("starting the OAuth callback thread: {err}") })?;
        Ok(OAuth2Callback { redirect_uri: format!("http://127.0.0.1:{port}/callback"), state, verifier, receiver, stop })
    }

    /// Non-blocking poll for the authorization code.
    pub fn try_code(&self) -> Option<std::result::Result<String, String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err("the OAuth callback listener stopped without a result".into())),
        }
    }
}

fn wait_for_callback(listener: TcpListener, expected_state: &str, stop: &AtomicBool) -> std::result::Result<String, String> {
    listener.set_nonblocking(true).ok();
    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    while std::time::Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            Err(err) => return Err(format!("OAuth callback connection: {err}")),
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let mut reader = BufReader::new(match stream.try_clone() {
            Ok(clone) => clone,
            Err(err) => return Err(format!("OAuth callback stream: {err}")),
        });
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_err() {
            continue;
        }
        let target = request_line.split_whitespace().nth(1).unwrap_or_default();
        if !target.starts_with("/callback") {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            continue;
        }
        let query = target.split_once('?').map(|(_, query)| query).unwrap_or_default();
        let mut code = String::new();
        let mut state = String::new();
        let mut error = String::new();
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let key = postguy_urlkit::decode_percent(key);
            let value = postguy_urlkit::decode_percent(value);
            match key.as_str() {
                "code" => code = value,
                "state" => state = value,
                "error" => error = value,
                "error_description" if error.is_empty() => error = value,
                _ => {}
            }
        }
        let body = if !error.is_empty() {
            "OAuth authorization failed; you can close this window."
        } else if state != expected_state {
            "OAuth state mismatch; you can close this window."
        } else {
            "Authorization complete; you can close this window."
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        if !error.is_empty() {
            return Err(error);
        }
        if state != expected_state {
            return Err("the OAuth callback state did not match; retry the authorization".into());
        }
        if code.is_empty() {
            return Err("the OAuth callback did not contain a code".into());
        }
        return Ok(code);
    }
    if stop.load(Ordering::SeqCst) {
        Err("the OAuth callback was cancelled".into())
    } else {
        Err("the OAuth callback timed out after five minutes".into())
    }
}

/// Form-encode one value (`application/x-www-form-urlencoded`); spaces become `%20`.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The standard introspection-free helper stays in `postguy-http`; this module only exchanges.
#[cfg(test)]
mod tests {
    use super::*;
    use postguy_model::auth::{OAuth2Auth, OAuth2ClientAuthentication, OAuth2Grant};
    use postguy_store::StorePaths;

    fn fixture() -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = Engine::open(StorePaths::new(dir.path().join("ws"), dir.path().join("cfg"))).expect("engine");
        (dir, engine)
    }

    #[test]
    fn client_credentials_token_exchanges_against_the_testkit() {
        let server = postguy_testkit::Server::start();
        let (_dir, engine) = fixture();
        let auth = OAuth2Auth {
            grant_type: OAuth2Grant::ClientCredentials,
            token_url: format!("{}/token", server.base_url()),
            client_id: "client".into(),
            client_secret: "secret".into(),
            client_authentication: OAuth2ClientAuthentication::Body,
            ..OAuth2Auth::default()
        };
        let token = engine.oauth2_grant(&auth).expect("token");
        assert_eq!(token.access_token, "postguy-access-token");
        assert_eq!(token.token_type, "Bearer");
        let stored = StoredToken::from_response(&token);
        assert!(!stored.access_token.is_empty());
        assert_eq!(stored.refresh_token, "postguy-refresh-token");
    }

    #[test]
    fn tokens_round_trip_through_the_secret_store() {
        let (_dir, engine) = fixture();
        assert!(engine.oauth2_stored_token("req1").expect("empty").is_none());
        let token =
            StoredToken { access_token: "a".into(), refresh_token: "r".into(), token_type: "Bearer".into(), scope: "s".into(), expires_at_ms: 0 };
        engine.oauth2_store_token("req1", &token).expect("store");
        let loaded = engine.oauth2_stored_token("req1").expect("load").expect("token");
        assert_eq!(loaded.access_token, "a");
        assert_eq!(loaded.refresh_token, "r");
        engine.oauth2_forget_token("req1").expect("forget");
        assert!(engine.oauth2_stored_token("req1").expect("empty").is_none());
    }

    #[test]
    fn callback_parses_code_and_state() {
        use std::io::Write as _;
        let callback = OAuth2Callback::start(postguy_model::auth::PkceMethod::S256).expect("listener");
        let address = callback.redirect_uri.trim_start_matches("http://").trim_end_matches("/callback");
        let mut stream = std::net::TcpStream::connect(address).expect("connect");
        let request = format!("GET /callback?code=abc&state={} HTTP/1.1\r\nHost: localhost\r\n\r\n", callback.state);
        stream.write_all(request.as_bytes()).expect("write");
        deadline_loop(&callback);
    }

    fn deadline_loop(callback: &OAuth2Callback) {
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            if let Some(result) = callback.try_code() {
                assert_eq!(result.expect("code"), "abc");
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("OAuth callback did not arrive");
    }
}
