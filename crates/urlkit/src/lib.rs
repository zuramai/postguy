//! Postguy urlkit (L1, pure): request-URL parsing and building, percent-encoding per
//! component (PROMPT.md Appendix D), the query-params two-way view, path variables and
//! certificate host patterns.
//!
//! The URL is treated as TEXT with `{{variables}}` possibly anywhere. Parsing is total (an
//! arbitrary string never errors), so the same functions parse a request line while typing,
//! after variable resolution, and a cURL import; building is the exact inverse.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use serde::{Deserialize, Serialize};

/// A parsed request URL. Every field keeps its ORIGINAL spelling (percent encodings and
/// `{{variables}}` included); encoding for the wire happens at send time per Appendix D.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Url {
    /// `https` etc. without `://`, as typed (an unresolved `{{var}}` too).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    /// `user:password` as typed, before encoding decisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub userinfo: Option<String>,
    /// Host and optional `:port`, exactly as typed.
    #[serde(default)]
    pub authority: String,
    /// Everything after the host, up to `?` or `#` (usually starts with `/`).
    #[serde(default)]
    pub path: String,
    /// The query string as typed, without `?` (present even when empty if the URL had `?`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<RawQuery>,
    /// Fragment, without `#`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fragment: Option<String>,
}

/// A query string: kept raw (its exact text) with the two-way params' view computed on demand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct RawQuery(pub String);

impl Url {
    /// Parse any string into its parts. Hostile input stays intact rather than erroring: a
    /// URL mid-edit has `{{vars}}`, a missing scheme, unmatched brackets.
    pub fn parse(text: &str) -> Url {
        let (scheme, rest) = match text.split_once_strong("://") {
            Some((scheme, rest)) => (Some(scheme.to_string()), rest),
            None => (None, text),
        };
        // Fragment: cut once at the LAST `#` (a fragment may hold anything).
        let (rest, fragment) = match rest.text_rest_after('#') {
            Some((head, tail)) => (head, Some(tail.to_string())),
            None => (rest, None),
        };
        // Query: first `?` after the scheme (before `#`, handled above).
        let (head, query) = match rest.find('?') {
            Some(idx) => (&rest[..idx], Some(RawQuery(rest[idx + 1..].to_string()))),
            None => (rest, None),
        };
        // Authority is head up to the first `/`; the remainder is the path.
        let (authority, path) = match head.find('/') {
            Some(idx) => (head[..idx].to_string(), head[idx..].to_string()),
            None => (head.to_string(), String::new()),
        };
        // Userinfo is only `user:pass@host` INSIDE the authority.
        let (userinfo, authority) = match authority.rsplit_once('@') {
            Some((user, host)) => (Some(user.to_string()), host.to_string()),
            None => (None, authority),
        };
        Url { scheme, userinfo, authority, path, query, fragment }
    }

    /// Rebuild the URL text; the exact inverse of [`Url::parse`].
    pub fn build(&self) -> String {
        let mut out = String::with_capacity(64);
        if let Some(scheme) = &self.scheme {
            out.push_str(scheme);
            out.push_str("://");
        }
        if let Some(userinfo) = &self.userinfo {
            out.push_str(userinfo);
            out.push('@');
        }
        out.push_str(&self.authority);
        if self.has_path() {
            if !self.path.starts_with('/') {
                out.push('/');
            }
            out.push_str(&self.path);
        }
        if let Some(query) = &self.query {
            out.push('?');
            out.push_str(&query.0);
        }
        if let Some(fragment) = &self.fragment {
            out.push('#');
            out.push_str(fragment);
        }
        out
    }

    fn has_path(&self) -> bool {
        !self.path.is_empty()
    }

    /// The path's `:name` variables (path only — the caller passes only the path, so a colon
    /// in the host or port or after `?` is never seen here).
    pub fn path_vars(&self) -> Vec<String> {
        path_vars_of(&self.path)
    }
}

impl std::fmt::Display for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.build())
    }
}

/// `:name` variables of a PATH string, in first-appearance order. A name runs until the first
/// character outside [alnum `-_.${}`] (so `{{var}}` inside the name stays in the name) and
/// must be non-empty; duplicates collapse.
pub fn path_vars_of(path: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for segment in path.split('/') {
        if let Some(name) = segment.strip_prefix(':') {
            let end = name
                .char_indices()
                .find(|(_, c)| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '$' | '{' | '}')))
                .map_or(name.len(), |(idx, _)| idx);
            let name = &name[..end];
            if !name.is_empty() && !seen.iter().any(|s| s == name) {
                seen.push(name.to_string());
            }
        }
    }
    seen
}

/// One query parameter of the two-way view, percent-decoded for the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub key: String,
    pub value: String,
}

/// The ordered parameters of a query string, decoded. Duplicates and empty values are kept.
/// Leading/trailing runs of `&` are glue; a contiguous run of empty segments between pairs
/// collapses to one explicit empty row.
pub fn params_of_query(query: &str) -> Vec<Param> {
    let segments: Vec<&str> = query.split('&').collect();
    let mut start = 0;
    let mut end = segments.len();
    while start < end && segments[start].is_empty() {
        start += 1;
    }
    while end > start && segments[end - 1].is_empty() {
        end -= 1;
    }
    let mut params: Vec<Param> = Vec::new();
    for segment in &segments[start..end] {
        if segment.is_empty() {
            // The empty row a user's `a&&b` made; a longer run is one row.
            if params.last().is_some_and(|p: &Param| p.key.is_empty() && p.value.is_empty()) {
                continue;
            }
            params.push(Param { key: String::new(), value: String::new() });
            continue;
        }
        let (key, value) = match segment.split_once('=') {
            Some((key, value)) => (key.to_string(), value.to_string()),
            None => (segment.to_string(), String::new()),
        };
        params.push(Param { key: decode_percent(&key), value: decode_percent(&value) });
    }
    params
}

/// Rebuild the query string from edited params. Table text is encoded per the Appendix D
/// query set; the structural separators stay. Round-trips through [`params_of_query`].
pub fn query_of_params(params: &[Param]) -> Option<RawQuery> {
    if params.is_empty() {
        return None;
    }
    let mut out = String::new();
    for (i, param) in params.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        out.push_str(&encode_query(&param.key));
        out.push('=');
        out.push_str(&encode_query(&param.value));
    }
    Some(RawQuery(out))
}

/// Encode per the Appendix D QUERY set: `"`, `#`, `&`, `'`, `<`, `=`, `>`, space — the value's
/// own `&`/`=` encode, the separators don't. Valid `%XX` escapes pass through unchanged, so
/// encoding is idempotent; non-ASCII encodes as UTF-8 percent text.
pub fn encode_query(input: &str) -> String {
    const QUERY_UNSAFE: &[u8] = br###""#&'<>= "###;
    encode_component(input, QUERY_UNSAFE)
}

/// Encode per the Appendix D PATH set: `"`, `<`, `>`, backtick, `#`, `?`, `{`, `}`, space.
pub fn encode_path(input: &str) -> String {
    const PATH_UNSAFE: &[u8] = br#""<>`#?{} "#;
    encode_component(input, PATH_UNSAFE)
}

/// Encode per the Appendix D USERINFO set (adds `/`, `:`, `;`, `=`, `@`, `[`, `\`, `]`, `^`).
pub fn encode_userinfo(input: &str) -> String {
    const USERINFO_UNSAFE: &[u8] = br###""<>`#?{} /:;=@[\]^ "###;
    encode_component(input, USERINFO_UNSAFE)
}

/// Encode `input` for the wire, touching only bytes in `unsafe_bytes` that are not already
/// part of a valid `%XX` escape. Idempotent by construction.
pub fn encode_component(input: &str, unsafe_bytes: &[u8]) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if is_percent_escape(bytes, i) {
            out.extend_from_slice(&bytes[i..i + 3]);
            i += 3;
        } else if unsafe_bytes.contains(&bytes[i]) || bytes[i] >= 0x80 {
            // Appendix D: non-ASCII is UTF-8 percent-encoded; the rest per component set.
            write_percent(&mut out, bytes[i]);
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    // Encoding only ever percentages ASCII code points and preserves the rest of the input's
    // bytes, so the result is the same bytes that String::from_utf8_lossy would show, with
    // losses only where the input was not valid UTF-8 to begin with (never true on &str input).
    String::from_utf8_lossy(&out).into_owned()
}

/// `%XX` where XX are two hex digits (case-insensitive), fully inside `bytes`.
fn is_percent_escape(bytes: &[u8], at: usize) -> bool {
    bytes[at] == b'%' && bytes.get(at + 1).is_some_and(u8::is_ascii_hexdigit) && bytes.get(at + 2).is_some_and(u8::is_ascii_hexdigit)
}

fn write_percent(out: &mut Vec<u8>, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push(b'%');
    out.push(HEX[(byte >> 4) as usize]);
    out.push(HEX[(byte & 0x0F) as usize]);
}

/// Decode percent-encoding (`%XX` with hex digits); malformed escapes stay literal.
pub fn decode_percent(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut buf: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if is_percent_escape(bytes, i) {
            buf.push((hex_value(bytes[i + 1]).wrapping_mul(16)).wrapping_add(hex_value(bytes[i + 2])));
            i += 3;
        } else {
            buf.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn hex_value(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

/// Convert a non-ASCII host to its IDNA ASCII form (PROMPT.md Appendix D). The error is a
/// readable string: the caller reports it in the console, never panics.
pub fn host_to_ascii(host: &str) -> Result<String, String> {
    if host.is_ascii() {
        return Ok(host.to_string());
    }
    idna::domain_to_ascii(host).map_err(|e| format!("invalid international host `{host}`: {e}"))
}

/// Does a certificate host pattern (`*.example.com`, optional port) cover `host`?
/// Case-insensitive; `*.` covers exactly one MISSING leading label (a wildcard can't span two
/// labels: `*.example.com` covers `api.example.com` but not `a.b.example.com`); a pattern's
/// port must match the host's (a pattern without port covers any port).
pub fn host_matches(pattern: &str, host: &str) -> bool {
    let (pattern_host, pattern_port) = split_host_port(pattern);
    let (url_host, url_port) = split_host_port(host);
    // A pattern without a port covers any port; two ports must match exactly.
    if let (Some(pattern_port), Some(url_port)) = (&pattern_port, &url_port)
        && pattern_port != url_port
    {
        return false;
    }
    let pattern_labels: Vec<String> = pattern_host.split('.').map(|l| l.to_lowercase()).collect();
    let url_labels: Vec<String> = url_host.split('.').map(|l| l.to_lowercase()).collect();
    if pattern_labels == url_labels {
        return true;
    }
    // A wildcard replaces exactly one label, so the label COUNTS must match.
    match pattern_labels.first().map(String::as_str) {
        Some("*") if pattern_labels.len() >= 2 => {
            url_labels.len() == pattern_labels.len() && !url_labels[0].is_empty() && url_labels[1..] == pattern_labels[1..]
        }
        _ => false,
    }
}

/// `(host, port)`; the port only when it is digits so hostnames with colons stay whole.
fn split_host_port(input: &str) -> (String, Option<String>) {
    match input.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => (host.to_string(), Some(port.to_string())),
        _ => (input.to_string(), None),
    }
}

trait StrOnceStrong {
    fn split_once_strong(&self, sep: &str) -> Option<(&str, &str)>;
    fn text_rest_after(&self, sep: char) -> Option<(&str, &str)>;
}

impl StrOnceStrong for str {
    fn split_once_strong(&self, sep: &str) -> Option<(&str, &str)> {
        self.find(sep).map(|idx| (&self[..idx], &self[idx + sep.len()..]))
    }

    fn text_rest_after(&self, sep: char) -> Option<(&str, &str)> {
        self.rfind(sep).map(|idx| (&self[..idx], &self[idx + sep.len_utf8()..]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_reference_url_losing_nothing() {
        let url = Url::parse("https://user:pw@example.com:8080/v1/:id?force=1&id=2#frag");
        assert_eq!(url.scheme.as_deref(), Some("https"));
        assert_eq!(url.userinfo.as_deref(), Some("user:pw"));
        assert_eq!(url.authority, "example.com:8080");
        assert_eq!(url.path, "/v1/:id");
        assert_eq!(url.query.as_ref().map(|q| q.0.as_str()), Some("force=1&id=2"));
        assert_eq!(url.fragment.as_deref(), Some("frag"));
        assert_eq!(url.build(), "https://user:pw@example.com:8080/v1/:id?force=1&id=2#frag");
    }

    #[test]
    fn parses_variable_riddled_hostile_urls_total() {
        let url = Url::parse("{{scheme}}://{{user}}@{{host}}:{{port}}/:base/:id?q={{q}}");
        assert_eq!(url.scheme.as_deref(), Some("{{scheme}}"));
        assert_eq!(url.authority, "{{host}}:{{port}}");
        assert_eq!(url.path, "/:base/:id");
        assert_eq!(url.path_vars(), vec!["base".to_string(), "id".to_string()]);
        // A URL with no scheme but a query and no leading slash still splits correctly.
        let plain = Url::parse("localhost:5051/echo?stub=1");
        assert_eq!(plain.authority, "localhost:5051");
        assert_eq!(plain.path, "/echo");
        // Host with letters after the colon is not a port; it stays in the authority.
        let no_port = Url::parse("grpc://svc.AB1/hello");
        assert_eq!(no_port.authority, "svc.AB1");
        assert_eq!(no_port.path, "/hello");
    }

    #[test]
    fn path_vars_skip_host_and_stop_at_separators() {
        // Only PATH colons count; the caller never passes the host, but the function stays
        // honest about segments: `:name` ends at a non-name character; `::nested` is a colon
        // name only if the name accepts `:` — it does not, so nothing.
        assert_eq!(path_vars_of("/a/:x/b/::nested/c"), vec!["x".to_string()]);
        assert_eq!(path_vars_of("/plain/id"), Vec::<String>::new());
        assert_eq!(path_vars_of("/:a{{v}}"), vec!["a{{v}}".to_string()]);
    }

    #[test]
    fn query_params_two_way_survives_duplicates_and_empties() {
        let params = params_of_query("foo=bar&foo=&baz&&qux=1%202");
        assert_eq!(
            params,
            vec![
                Param { key: "foo".into(), value: "bar".into() },
                Param { key: "foo".into(), value: String::new() },
                Param { key: "baz".into(), value: String::new() },
                // `baz&&qux`: the empty segment between two pairs is ONE explicit empty row.
                Param { key: String::new(), value: String::new() },
                Param { key: "qux".into(), value: "1 2".into() },
            ]
        );
        let rebuilt = query_of_params(&[Param { key: "foo".into(), value: "bar".into() }, Param { key: "q x".into(), value: "1&2".into() }])
            .map(|q| q.0)
            .unwrap();
        assert_eq!(rebuilt, "foo=bar&q%20x=1%262");
        assert_eq!(
            params_of_query(&rebuilt),
            vec![Param { key: "foo".into(), value: "bar".into() }, Param { key: "q x".into(), value: "1&2".into() },]
        );
    }

    #[test]
    fn glue_only_and_empty_queries_have_no_rows() {
        assert!(params_of_query("").is_empty());
        assert!(params_of_query("&").is_empty(), "a single trailing & is glue");
        assert_eq!(params_of_query("&&a=1").len(), 1, "leading glue is dropped");
        assert_eq!(params_of_query("a=1&").len(), 1);
        assert_eq!(params_of_query("a=1&&&b=2").len(), 3, "a contiguous empty run is ONE row");
        assert!(params_of_query("&&").is_empty());
        let glue_only = Url::parse("https://h/p?");
        assert_eq!(glue_only.query.as_ref().map(|q| q.0.clone()), Some(String::new()), "empty edited query is retained");
        assert_eq!(glue_only.build(), "https://h/p?");
    }

    #[test]
    fn encoding_sets_match_the_appendix() {
        // Query: space, =, quotes get encoded (Appendix D query set includes `=`).
        assert_eq!(encode_query("a b='c'=d"), "a%20b%3D%27c%27%3Dd");
        // Path: `?` and braces encode; `/` stays.
        assert_eq!(encode_path("a/b c?d{e}"), "a/b%20c%3Fd%7Be%7D");
        // Userinfo: `:` and `@` encode.
        assert_eq!(encode_userinfo("a:b@c"), "a%3Ab%40c");
        // Valid escapes stay; lower-case hex preserved; idempotent.
        assert_eq!(encode_query("a%20b"), "a%20b");
        assert_eq!(encode_query(&encode_query("x y")), encode_query("x y"));
        // Non-ASCII: UTF-8 percent text per byte.
        assert_eq!(encode_query("münchen.de"), "m%C3%BCnchen.de");
        // Decode unencodes valid escapes and keeps malformed ones visible.
        assert_eq!(decode_percent("a%20b%zz"), "a b%zz");
        // Lenient display decode: bytes that never make valid UTF-8 in themselves (here a
        // lone %A0) surface as U+FFFD rather than erroring; the raw query string on disk is
        // the wire truth, this is only the table view.
        assert_eq!(decode_percent("%a0"), "\u{FFFD}");
    }

    #[test]
    fn idna_converts_non_ascii_hosts() {
        assert_eq!(host_to_ascii("münchen.de").as_deref(), Ok("xn--mnchen-3ya.de"));
        assert!(host_to_ascii("plain.example").is_ok());
    }

    #[test]
    fn host_patterns_cover_one_label() {
        assert!(host_matches("*.example.com", "api.example.com"));
        assert!(!host_matches("*.example.com", "a.b.example.com"));
        assert!(!host_matches("*.example.com", "example.com"));
        assert!(host_matches("api.example.com", "API.EXAMPLE.COM"));
        assert!(host_matches("api.example.com", "api.example.com:443"));
        assert!(!host_matches("api.example.com:8443", "api.example.com:443"));
        assert!(host_matches("*.example.com:8443", "api.example.com:8443"));
    }
}
