//! Settings: the global settings file (PROMPT.md 7, defaults per section) and the per-request
//! settings (PROMPT.md 5.3.2). Unknown keys ride along in `extras`; corrupt files are
//! `store`'s concern.

use crate::extras::Extras;
use serde::{Deserialize, Serialize};

/// Per-request overrides (PROMPT.md 5.3.2 Settings tab). Defaults come from the global
/// `Settings` when a request is created; the stored request holds full values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestSettings {
    /// `auto` | `http1` | `http2` (PROMPT.md: HTTP version Auto, HTTP/1.x, HTTP/2).
    #[serde(default)]
    pub http_version: HttpVersion,
    #[serde(default = "true_value")]
    pub verify_ssl: bool,
    #[serde(default = "true_value")]
    pub follow_redirects: bool,
    /// off: 301/302/303 on POST become GET (PROMPT.md 5.3.2).
    #[serde(default)]
    pub follow_original_method: bool,
    /// off: drop `Authorization` on cross-host redirect.
    #[serde(default)]
    pub follow_authorization_on_redirect: bool,
    #[serde(default)]
    pub remove_referer_on_redirect: bool,
    #[serde(default = "ten_value")]
    pub max_redirects: u32,
    /// 0 = none (PROMPT.md 5.3.2).
    #[serde(default)]
    pub timeout_ms: u64,
    #[serde(default = "true_value")]
    pub encode_url_automatically: bool,
    #[serde(default)]
    pub cookie_jar_disabled: bool,
    /// 0 = unlimited (PROMPT.md 5.3.2).
    #[serde(default = "fifty_value")]
    pub max_response_mb: u64,
    #[serde(default, flatten)]
    pub extras: Extras,
}

impl Default for RequestSettings {
    /// The defaults the brief gives the per-request Settings tab (PROMPT.md 5.3.2).
    fn default() -> Self {
        RequestSettings {
            http_version: HttpVersion::Auto,
            verify_ssl: true,
            follow_redirects: true,
            follow_original_method: false,
            follow_authorization_on_redirect: false,
            remove_referer_on_redirect: false,
            max_redirects: 10,
            timeout_ms: 0,
            encode_url_automatically: true,
            cookie_jar_disabled: false,
            max_response_mb: 50,
            extras: Extras::default(),
        }
    }
}

impl Default for Settings {
    /// The defaults from PROMPT.md 7 (General + Editor + Theme + History).
    fn default() -> Self {
        Settings {
            http_version: HttpVersion::Auto,
            request_timeout_ms: 0,
            max_response_mb: 50,
            verify_ssl: true,
            disable_cookies: false,
            follow_redirects: true,
            send_no_cache_header: true,
            trim_keys_and_values: false,
            response_format_detection: ResponseFormatDetection::Auto,
            working_directory: String::new(), // `<workspace>/files` resolved by store
            allow_files_outside_working_directory: false,
            always_open_in_new_tab: false,
            always_ask_on_close: true,
            show_icons_with_tab_names: true,
            two_pane_layout: false,
            variable_autocomplete: true,
            autosave: false,
            editor_font_size: 12.5,
            editor_indent_json: 2,
            editor_indent_other: 4,
            editor_spaces: true,
            editor_auto_close: true,
            editor_word_wrap: false,
            theme: Theme::System,
            history_cap: 5000,
            save_history_responses: false,
            proxy: ProxySettings::default(),
            extras: Extras::default(),
        }
    }
}

/// The settings file in the OS config dir. Section 7 lists the keys; anything users or
/// NEWER builds store goes to `extras` and survives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    // General
    #[serde(default)]
    pub http_version: HttpVersion,
    #[serde(default)]
    pub request_timeout_ms: u64,
    #[serde(default = "fifty_value")]
    pub max_response_mb: u64,
    #[serde(default = "true_value")]
    pub verify_ssl: bool,
    #[serde(default)]
    pub disable_cookies: bool,
    #[serde(default = "true_value")]
    pub follow_redirects: bool,
    #[serde(default = "true_value")]
    pub send_no_cache_header: bool,
    #[serde(default)]
    pub trim_keys_and_values: bool,
    #[serde(default)]
    pub response_format_detection: ResponseFormatDetection,
    /// Default: `<workspace>/files` (PROMPT.md 7: working directory).
    #[serde(default)]
    pub working_directory: String,
    #[serde(default)]
    pub allow_files_outside_working_directory: bool,
    #[serde(default)]
    pub always_open_in_new_tab: bool,
    #[serde(default = "true_value")]
    pub always_ask_on_close: bool,
    #[serde(default = "true_value")]
    pub show_icons_with_tab_names: bool,
    #[serde(default)]
    pub two_pane_layout: bool,
    #[serde(default = "true_value")]
    pub variable_autocomplete: bool,
    #[serde(default)]
    pub autosave: bool,
    // Editor
    #[serde(default = "editor_font_size")]
    pub editor_font_size: f32,
    #[serde(default = "two_value")]
    pub editor_indent_json: u32,
    #[serde(default = "four_value")]
    pub editor_indent_other: u32,
    #[serde(default = "true_value")]
    pub editor_spaces: bool,
    #[serde(default = "true_value")]
    pub editor_auto_close: bool,
    #[serde(default)]
    pub editor_word_wrap: bool,
    // Theme
    #[serde(default)]
    pub theme: Theme,
    // History
    #[serde(default = "history_cap")]
    pub history_cap: usize,
    /// "Save responses" toggle (PROMPT.md 5.5): bodies cached under `.postguy/responses/`.
    #[serde(default)]
    pub save_history_responses: bool,
    /// Proxy (PROMPT.md 5.5): system env vars by default, or a custom route.
    #[serde(default)]
    pub proxy: ProxySettings,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// Where the proxy comes from: the environment (`HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY`) or the
/// custom block below (which wins, PROMPT.md 5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ProxyMode {
    #[default]
    System,
    Custom,
}

/// The proxy protocol. `Socks5h` resolves the target host at the proxy; `Socks5` resolves it
/// locally (PROMPT.md 5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ProxyProtocol {
    #[default]
    Http,
    Socks5,
    Socks5h,
}

/// Settings > Proxy (PROMPT.md 5.5). Empty `host` with `Custom` means no proxy.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxySettings {
    #[serde(default)]
    pub mode: ProxyMode,
    #[serde(default)]
    pub protocol: ProxyProtocol,
    #[serde(default)]
    pub host: String,
    #[serde(default = "proxy_port")]
    pub port: u16,
    /// Optional HTTP proxy basic auth (never logged; masked in the console).
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "true_value")]
    pub apply_to_http: bool,
    #[serde(default = "true_value")]
    pub apply_to_https: bool,
    /// Comma-separated hosts or patterns (`*.internal.example`, `localhost`).
    #[serde(default)]
    pub bypass: String,
    #[serde(default, flatten)]
    pub extras: Extras,
}

impl std::fmt::Debug for ProxySettings {
    /// Never print the proxy password (PROMPT.md 9: secrets stay out of logs and the console).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxySettings")
            .field("mode", &self.mode)
            .field("protocol", &self.protocol)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &if self.password.is_empty() { "" } else { "<redacted>" })
            .field("apply_to_http", &self.apply_to_http)
            .field("apply_to_https", &self.apply_to_https)
            .field("bypass", &self.bypass)
            .finish()
    }
}

impl Default for ProxySettings {
    fn default() -> Self {
        ProxySettings {
            mode: ProxyMode::System,
            protocol: ProxyProtocol::Http,
            host: String::new(),
            port: 8080,
            username: String::new(),
            password: String::new(),
            apply_to_http: true,
            apply_to_https: true,
            bypass: String::new(),
            extras: Extras::default(),
        }
    }
}

fn proxy_port() -> u16 {
    8080
}

/// Http version preference per request or globally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum HttpVersion {
    #[default]
    Auto,
    Http1,
    Http2,
}

/// Response format detection (PROMPT.md 7: Auto; "force JSON" override).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ResponseFormatDetection {
    #[default]
    Auto,
    ForceJson,
}

/// Light, Dark, System (PROMPT.md 6.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

fn true_value() -> bool {
    true
}

fn ten_value() -> u32 {
    10
}

fn fifty_value() -> u64 {
    50
}

fn two_value() -> u32 {
    2
}

fn four_value() -> u32 {
    4
}

fn editor_font_size() -> f32 {
    12.5
}

fn history_cap() -> usize {
    5000
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_match_the_brief() {
        let settings = Settings::default();
        assert_eq!(settings.max_response_mb, 50);
        assert!(settings.verify_ssl);
        assert!(settings.send_no_cache_header);
        assert!(!settings.autosave);
        assert!(!settings.allow_files_outside_working_directory);
        assert_eq!(settings.editor_font_size, 12.5);
        assert_eq!(settings.history_cap, 5000);
        assert_eq!(settings.theme, Theme::System);
    }

    #[test]
    fn request_settings_default_to_the_table() {
        let r = RequestSettings::default();
        assert_eq!(r.max_redirects, 10);
        assert_eq!(r.timeout_ms, 0, "0 = none");
        assert_eq!(r.max_response_mb, 50);
        assert!(r.verify_ssl);
        assert!(r.encode_url_automatically);
        assert!(!r.follow_original_method);
        assert!(!r.cookie_jar_disabled);
    }

    #[test]
    fn unknown_keys_and_partials_survive() {
        let raw = json!({"maxResponseMb": 120, "vendorPause": 3});
        let settings: Settings = serde_json::from_value(raw).unwrap();
        assert_eq!(settings.max_response_mb, 120);
        assert_eq!(settings.request_timeout_ms, 0);
        assert_eq!(settings.extras.get("vendorPause").and_then(|v| v.as_i64()), Some(3));
        let back = serde_json::to_value(settings).unwrap();
        assert_eq!(back["vendorPause"], json!(3));
    }

    #[test]
    fn per_request_settings_round_trip_with_http_version() {
        let r = RequestSettings { http_version: HttpVersion::Http2, max_redirects: 3, timeout_ms: 12345, ..Default::default() };
        let back = serde_json::to_value(&r).unwrap();
        assert_eq!(back["httpVersion"], json!("http2"));
        assert_eq!(back["maxRedirects"], json!(3));
        assert_eq!(back["timeoutMs"], json!(12345));
        assert_eq!(back["followRedirects"], json!(true));
        let round: RequestSettings = serde_json::from_value(back).unwrap();
        assert_eq!(round, r, "extras-free settings round trip exactly");
    }
}
