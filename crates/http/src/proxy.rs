//! Proxy resolution: the environment (`HTTP_PROXY`, `HTTPS_PROXY`,
//! `ALL_PROXY`, `NO_PROXY`, case-insensitive) or the custom Settings > Proxy block, which wins.
//! This module decides the route for one URL; the connector performs HTTP CONNECT tunnels and
//! SOCKS5 handshakes itself (no extra dependency).
//!
//! OS proxy discovery (macOS/Windows system settings) is `platform`'s job (L7) and is not
//! wired yet; environment variables are the system path today (documented gap).

pub use postguy_model::ProxyProtocol;
use postguy_model::{ProxyMode, ProxySettings};
use postguy_urlkit::Url;

/// One resolved proxy route.
#[derive(Clone, PartialEq, Eq)]
pub struct ProxyRoute {
    pub protocol: ProxyProtocol,
    pub host: String,
    pub port: u16,
    /// Basic auth for the proxy itself (`Proxy-Authorization: Basic …`), never logged.
    pub username: String,
    pub password: String,
    /// True when the route came from the environment rather than the custom settings.
    pub from_environment: bool,
}

impl std::fmt::Debug for ProxyRoute {
    /// Credentials never reach a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyRoute")
            .field("protocol", &self.protocol)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &if self.password.is_empty() { "" } else { "<redacted>" })
            .field("from_environment", &self.from_environment)
            .finish()
    }
}

impl ProxyRoute {
    /// `Basic …` header value, when credentials are set.
    pub fn basic_auth_value(&self) -> Option<String> {
        if self.username.is_empty() && self.password.is_empty() {
            return None;
        }
        Some(format!("Basic {}", base64_encode(format!("{}:{}", self.username, self.password).as_bytes())))
    }

    /// The proxy address as `host:port` (for errors and the console, never credentials).
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// True when this route tunnels with HTTP CONNECT (HTTP proxy + HTTPS target).
    pub fn is_http_tunnel(&self, target_scheme: &str) -> bool {
        self.protocol == ProxyProtocol::Http && target_scheme.eq_ignore_ascii_case("https")
    }
}

/// The environment snapshot (read once at client construction; the app does not poll the env).
/// Debug reports presence only: a proxy URL may carry a password.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ProxyEnvironment {
    pub http_proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub all_proxy: Option<String>,
    pub no_proxy: Option<String>,
}

impl std::fmt::Debug for ProxyEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyEnvironment")
            .field("http_proxy", &self.http_proxy.is_some())
            .field("https_proxy", &self.https_proxy.is_some())
            .field("all_proxy", &self.all_proxy.is_some())
            .field("no_proxy", &self.no_proxy.is_some())
            .finish()
    }
}

impl ProxyEnvironment {
    pub fn from_process() -> ProxyEnvironment {
        let lookup = |name: &str| std::env::var(name).ok().filter(|value| !value.trim().is_empty());
        ProxyEnvironment {
            http_proxy: lookup("HTTP_PROXY").or_else(|| lookup("http_proxy")),
            https_proxy: lookup("HTTPS_PROXY").or_else(|| lookup("https_proxy")),
            all_proxy: lookup("ALL_PROXY").or_else(|| lookup("all_proxy")),
            no_proxy: lookup("NO_PROXY").or_else(|| lookup("no_proxy")),
        }
    }
}

/// Resolves a route per URL. Cheap to clone; built once per client.
#[derive(Debug, Clone)]
pub struct ProxyPolicy {
    settings: ProxySettings,
    env: ProxyEnvironment,
}

impl ProxyPolicy {
    pub fn new(settings: ProxySettings) -> ProxyPolicy {
        ProxyPolicy { settings, env: ProxyEnvironment::from_process() }
    }

    /// Explicit environment (tests, and a future "reload environment" action).
    pub fn with_environment(settings: ProxySettings, env: ProxyEnvironment) -> ProxyPolicy {
        ProxyPolicy { settings, env }
    }

    pub fn settings(&self) -> &ProxySettings {
        &self.settings
    }

    /// The route for `url`, or None when it goes direct. Only http/https URLs are proxied.
    pub fn route_for(&self, url: &str) -> Option<ProxyRoute> {
        let parsed = Url::parse(url);
        let scheme = parsed.scheme.as_deref()?.to_ascii_lowercase();
        if scheme != "http" && scheme != "https" {
            return None;
        }
        let host = parsed.authority.split(':').next().unwrap_or_default().to_ascii_lowercase();
        let port =
            parsed.authority.rsplit_once(':').and_then(|(_, port)| port.parse::<u16>().ok()).unwrap_or(if scheme == "https" { 443 } else { 80 });
        if host.is_empty() {
            return None;
        }
        match self.settings.mode {
            ProxyMode::Custom => {
                let applies = if scheme == "https" { self.settings.apply_to_https } else { self.settings.apply_to_http };
                if !applies || self.settings.host.trim().is_empty() || bypass_matches(&self.settings.bypass, &host, port) {
                    return None;
                }
                Some(ProxyRoute {
                    protocol: self.settings.protocol,
                    host: self.settings.host.trim().to_string(),
                    port: self.settings.port,
                    username: self.settings.username.clone(),
                    password: self.settings.password.clone(),
                    from_environment: false,
                })
            }
            ProxyMode::System => {
                if let Some(no_proxy) = &self.env.no_proxy
                    && bypass_matches(no_proxy, &host, port)
                {
                    return None;
                }
                let raw = if scheme == "https" {
                    self.env.https_proxy.clone().or_else(|| self.env.all_proxy.clone())
                } else {
                    self.env.http_proxy.clone().or_else(|| self.env.all_proxy.clone())
                }?;
                parse_proxy_url(&raw).map(|mut route| {
                    route.from_environment = true;
                    route
                })
            }
        }
    }
}

/// Parse a proxy URL as the environment allows: `http://host:port`, `socks5://…`,
/// `socks5h://…`, or a bare `host:port` (HTTP). Missing port defaults per protocol.
fn parse_proxy_url(raw: &str) -> Option<ProxyRoute> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (scheme, rest) = match trimmed.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => ("http".to_string(), trimmed),
    };
    let protocol = match scheme.as_str() {
        "http" | "https" => ProxyProtocol::Http,
        "socks5" => ProxyProtocol::Socks5,
        "socks5h" => ProxyProtocol::Socks5h,
        _ => return None,
    };
    let (userinfo, authority) = match rest.rsplit_once('@') {
        Some((userinfo, authority)) => (Some(userinfo), authority),
        None => (None, rest),
    };
    let authority = authority.trim_end_matches('/');
    // IPv6 literal: [::1]:8080.
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port =
            tail.strip_prefix(':').and_then(|port| port.parse::<u16>().ok()).unwrap_or(if protocol == ProxyProtocol::Http { 8080 } else { 1080 });
        (host.to_string(), port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if port.parse::<u16>().is_ok() => (host.to_string(), port.parse().ok()?),
            _ => (authority.to_string(), if protocol == ProxyProtocol::Http { 8080 } else { 1080 }),
        }
    };
    if host.is_empty() {
        return None;
    }
    let (username, password) = match userinfo {
        Some(userinfo) => {
            let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
            (percent_decode(user), percent_decode(password))
        }
        None => (String::new(), String::new()),
    };
    Some(ProxyRoute { protocol, host, port, username, password, from_environment: false })
}

/// NO_PROXY semantics: `*` disables
/// proxying entirely; `host`, `host:port`, `.suffix`, `*.suffix` and glob patterns match.
pub fn bypass_matches(list: &str, host: &str, port: u16) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let host_with_port = format!("{host}:{port}");
    for pattern in list.split(',') {
        let pattern = pattern.trim().trim_end_matches('.').to_ascii_lowercase();
        if pattern.is_empty() {
            continue;
        }
        if pattern == "*" {
            return true;
        }
        if pattern == host || pattern == host_with_port {
            return true;
        }
        if let Some(suffix) = pattern.strip_prefix('.') {
            if host == suffix || host.ends_with(&format!(".{suffix}")) {
                return true;
            }
            continue;
        }
        if let Some(suffix) = pattern.strip_prefix("*.") {
            if host == suffix || host.ends_with(&format!(".{suffix}")) {
                return true;
            }
            continue;
        }
        if pattern.contains('*') && glob_matches(&pattern, &host) {
            return true;
        }
    }
    false
}

fn glob_matches(pattern: &str, text: &str) -> bool {
    let (pattern, text) = (pattern.as_bytes(), text.as_bytes());
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star, mut resume) = (None, 0usize);
    while t < text.len() {
        match pattern.get(p) {
            Some(b'*') => {
                star = Some(p);
                resume = t;
                p += 1;
            }
            Some(byte) if *byte == text.get(t).copied().unwrap_or_default() => {
                p += 1;
                t += 1;
            }
            _ => {
                let Some(star_index) = star else { return false };
                p = star_index + 1;
                resume += 1;
                t = resume;
            }
        }
    }
    while pattern.get(p) == Some(&b'*') {
        p += 1;
    }
    p == pattern.len()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes.get(index) == Some(&b'%')
            && let (Some(high), Some(low)) = (bytes.get(index + 1), bytes.get(index + 2))
            && let (Some(high), Some(low)) = (hex_digit(*high), hex_digit(*low))
        {
            out.push(high * 16 + low);
            index += 3;
            continue;
        }
        out.push(bytes.get(index).copied().unwrap_or_default());
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Minimal standard base64 (the proxy header only; avoids a dependency for 20 lines).
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk.first().copied().unwrap_or(0) as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE.get(((triple >> 18) & 0x3f) as usize).copied().unwrap_or(b'A') as char);
        out.push(TABLE.get(((triple >> 12) & 0x3f) as usize).copied().unwrap_or(b'A') as char);
        out.push(if chunk.len() > 1 { TABLE.get(((triple >> 6) & 0x3f) as usize).copied().unwrap_or(b'A') as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE.get((triple & 0x3f) as usize).copied().unwrap_or(b'A') as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use postguy_model::{Extras, ProxySettings};

    fn http_route(settings: ProxySettings) -> ProxyRoute {
        ProxyPolicy::with_environment(settings, ProxyEnvironment::default()).route_for("http://example.com/x").expect("route")
    }

    #[test]
    fn custom_http_proxy_applies_to_the_right_schemes_and_honours_bypass() {
        let settings = ProxySettings {
            mode: ProxyMode::Custom,
            host: "proxy.local".into(),
            port: 3128,
            apply_to_https: false,
            bypass: "*.internal, localhost, 127.0.0.1".into(),
            ..ProxySettings { extras: Extras::default(), ..ProxySettings::default() }
        };
        let route = http_route(settings.clone());
        assert_eq!((route.protocol, route.host.as_str(), route.port), (ProxyProtocol::Http, "proxy.local", 3128));
        let policy = ProxyPolicy::with_environment(settings, ProxyEnvironment::default());
        assert!(policy.route_for("https://example.com/").is_none(), "https is switched off");
        assert!(policy.route_for("http://api.internal/").is_none(), "wildcard bypass");
        assert!(policy.route_for("http://localhost:3000/").is_none(), "host bypass");
        assert!(policy.route_for("http://example.com/").is_some());
    }

    #[test]
    fn environment_proxies_are_case_insensitive_and_respect_no_proxy() {
        let env = ProxyEnvironment {
            http_proxy: Some("http://proxy.env:8888".into()),
            https_proxy: Some("socks5h://user:p%40ss@localhost:1080".into()),
            all_proxy: None,
            no_proxy: Some(".corp.example, 10.0.0.5".into()),
        };
        let policy = ProxyPolicy::with_environment(ProxySettings::default(), env);
        let plain = policy.route_for("http://example.com/").expect("http route");
        assert_eq!((plain.host.as_str(), plain.port), ("proxy.env", 8888));
        assert!(plain.from_environment);
        let tls = policy.route_for("https://example.com/").expect("https route");
        assert_eq!(tls.protocol, ProxyProtocol::Socks5h);
        assert_eq!((tls.username.as_str(), tls.password.as_str()), ("user", "p@ss"));
        assert!(policy.route_for("http://api.corp.example/").is_none(), "NO_PROXY suffix");
        assert!(policy.route_for("http://10.0.0.5:9000/").is_none(), "NO_PROXY host anywhere");
    }

    #[test]
    fn a_star_no_proxy_disables_everything_and_empty_settings_go_direct() {
        let env = ProxyEnvironment { http_proxy: Some("http://proxy:8080".into()), https_proxy: None, all_proxy: None, no_proxy: Some("*".into()) };
        assert!(ProxyPolicy::with_environment(ProxySettings::default(), env).route_for("http://example.com/").is_none());
        assert!(ProxyPolicy::with_environment(ProxySettings::default(), ProxyEnvironment::default()).route_for("http://example.com/").is_none());
    }

    #[test]
    fn default_ports_follow_the_protocol_and_credentials_are_optional() {
        let env = ProxyEnvironment { http_proxy: Some("socks5://socks.local".into()), https_proxy: None, all_proxy: None, no_proxy: None };
        let route = ProxyPolicy::with_environment(ProxySettings::default(), env).route_for("http://example.com/").expect("route");
        assert_eq!(route.port, 1080);
        assert!(route.basic_auth_value().is_none());
        let bare = parse_proxy_url("plainproxy:3129").expect("bare authority");
        assert_eq!((bare.protocol, bare.host.as_str(), bare.port), (ProxyProtocol::Http, "plainproxy", 3129));
        assert_eq!(parse_proxy_url("ftp://nope:21"), None);
    }

    #[test]
    fn base64_matches_rfc_4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode("user:p@ss".as_bytes()), "dXNlcjpwQHNz");
    }

    #[test]
    fn glob_bypass_examples() {
        assert!(bypass_matches("*.example.com", "api.example.com", 443));
        assert!(!bypass_matches("*.example.com", "example.com.evil", 443));
        assert!(bypass_matches("*", "anything", 80));
        assert!(bypass_matches("10.*", "10.1.2.3", 80));
    }
}
