//! An RFC 6265 cookie jar we own: visible, editable, persistent in `.postguy/cookies.json`
//! and bounded (per-domain and total caps so a hostile server cannot grow it
//! without limit). Parsing uses the `cookie` crate; the matching, path defaults, expiry and
//! persistence rules are here.
//!
//! Documented v1 limits (recorded in docs/gaps.md): no public-suffix list (a domain attribute
//! is refused when it is the host itself or has no dot and differs from the host; a full PSL is
//! a later data dependency), SameSite is stored and shown but not enforced beyond the RFC
//! 6265bis defaults, and `__Host-`/`__Secure-` prefixes are
//! not enforced.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Total cookies kept; the oldest are evicted first (RFC suggested bounds, kept generous).
pub const MAX_COOKIES: usize = 3000;
/// Per-domain cookies kept.
pub const MAX_PER_DOMAIN: usize = 180;

/// One stored cookie.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredCookie {
    pub name: String,
    pub value: String,
    /// True when the Set-Cookie had no Domain attribute (host-only cookie).
    pub host_only: bool,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_site: Option<String>,
    /// Unix millis; None = session cookie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_ms: Option<i64>,
    /// Unix millis, for ordering and eviction.
    pub created_ms: i64,
}

/// The jar. Mutation happens through `store_from_headers`; reads through `header_for`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CookieJar {
    #[serde(default)]
    pub cookies: Vec<StoredCookie>,
    /// Domains scripts may read/write through `pm.cookies.jar()`; stored with the jar so
    /// the UI's "Domain allowlist" tab survives restarts.
    #[serde(default)]
    pub domain_allowlist: Vec<String>,
}

impl CookieJar {
    pub fn new() -> CookieJar {
        CookieJar::default()
    }

    /// Parse every `Set-Cookie` header value received for `url`. Expired cookies are removed on
    /// arrival; `Max-Age=0`/past `Expires` delete an existing cookie.
    pub fn store_from_headers<'a>(&mut self, url: &str, set_cookies: impl IntoIterator<Item = &'a str>) {
        self.store_from_headers_at(url, set_cookies, now_ms());
    }

    /// Timestamp-injectable form for tests.
    pub fn store_from_headers_at<'a>(&mut self, url: &str, set_cookies: impl IntoIterator<Item = &'a str>, now: i64) {
        let origin = Origin::parse(url);
        for raw in set_cookies {
            let Ok(parsed) = cookie::Cookie::parse(raw.to_string()) else { continue };
            let Some(domain) = accept_domain(&origin, parsed.domain()) else { continue };
            let path = parsed.path().map(str::to_string).unwrap_or_else(|| default_path(&origin.path));
            let expires_ms = match (parsed.max_age(), parsed.expires()) {
                (Some(max_age), _) => {
                    let seconds = max_age.whole_seconds();
                    if seconds <= 0 {
                        self.remove(&domain, &path, parsed.name());
                        continue;
                    }
                    Some(now.saturating_add(seconds.saturating_mul(1000)))
                }
                (None, Some(cookie::Expiration::DateTime(datetime))) => {
                    let stamp = datetime.unix_timestamp().saturating_mul(1000);
                    if stamp <= now {
                        self.remove(&domain, &path, parsed.name());
                        continue;
                    }
                    Some(stamp)
                }
                (None, Some(cookie::Expiration::Session)) | (None, None) => None,
            };
            if parsed.secure().unwrap_or(false) && origin.scheme != "https" {
                // An insecure origin cannot set a Secure cookie (RFC 6265 5.3).
                continue;
            }
            let stored = StoredCookie {
                name: parsed.name().to_string(),
                value: parsed.value().to_string(),
                host_only: parsed.domain().is_none(),
                domain: domain.clone(),
                path: path.clone(),
                secure: parsed.secure().unwrap_or(false),
                http_only: parsed.http_only().unwrap_or(false),
                same_site: parsed.same_site().map(|value| format!("{value:?}").to_ascii_lowercase()),
                expires_ms,
                created_ms: now,
            };
            self.replace(stored);
        }
        self.expire(now);
        self.enforce_caps();
    }

    /// The Cookie header value for a request to `url` ("" when none), RFC 6265 order: longer
    /// paths first, then earlier creation.
    pub fn header_for(&self, url: &str) -> String {
        self.header_for_at(url, now_ms())
    }

    pub fn header_for_at(&self, url: &str, now: i64) -> String {
        let origin = Origin::parse(url);
        let mut matching: Vec<&StoredCookie> = self
            .cookies
            .iter()
            .filter(|cookie| cookie.expires_ms.is_none_or(|expires| expires > now))
            .filter(|cookie| domain_matches(&origin.host, &cookie.domain, cookie.host_only))
            .filter(|cookie| path_matches(&origin.path, &cookie.path))
            .filter(|cookie| !cookie.secure || origin.scheme == "https")
            .collect();
        matching.sort_by(|a, b| b.path.len().cmp(&a.path.len()).then(a.created_ms.cmp(&b.created_ms)));
        matching.iter().map(|cookie| format!("{}={}", cookie.name, cookie.value)).collect::<Vec<String>>().join("; ")
    }

    /// Delete one cookie by (domain, path, name).
    pub fn remove(&mut self, domain: &str, path: &str, name: &str) {
        self.cookies.retain(|cookie| !(cookie.domain == domain && cookie.path == path && cookie.name == name));
    }

    /// Delete every cookie of a domain (the UI's "delete domain").
    pub fn remove_domain(&mut self, domain: &str) -> usize {
        let before = self.cookies.len();
        self.cookies.retain(|cookie| cookie.domain != domain);
        before - self.cookies.len()
    }

    pub fn clear(&mut self) {
        self.cookies.clear();
    }

    /// Domains present, sorted, with their cookie counts (the manager's left list).
    pub fn domains(&self) -> BTreeMap<&str, usize> {
        let mut map: BTreeMap<&str, usize> = BTreeMap::new();
        for cookie in &self.cookies {
            *map.entry(cookie.domain.as_str()).or_insert(0) += 1;
        }
        map
    }

    pub fn allow_domain(&mut self, domain: &str) {
        let normalized = domain.trim().trim_start_matches('.').to_ascii_lowercase();
        if !normalized.is_empty() && !self.domain_allowlist.contains(&normalized) {
            self.domain_allowlist.push(normalized);
        }
    }

    pub fn disallow_domain(&mut self, domain: &str) {
        let normalized = domain.trim().trim_start_matches('.').to_ascii_lowercase();
        self.domain_allowlist.retain(|allowed| *allowed != normalized);
    }

    pub fn allows(&self, domain: &str) -> bool {
        let host = domain.to_ascii_lowercase();
        self.domain_allowlist.iter().any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
    }

    /// Load from the persisted JSON shape; a corrupt document yields an empty jar (the store
    /// quarantines the file and reports it — never a crash).
    pub fn from_json(value: &serde_json::Value) -> CookieJar {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    fn replace(&mut self, stored: StoredCookie) {
        self.cookies.retain(|cookie| !(cookie.domain == stored.domain && cookie.path == stored.path && cookie.name == stored.name));
        self.cookies.push(stored);
    }

    fn expire(&mut self, now: i64) {
        self.cookies.retain(|cookie| cookie.expires_ms.is_none_or(|expires| expires > now));
    }

    fn enforce_caps(&mut self) {
        // Per-domain cap: drop the oldest of that domain first.
        let mut per_domain: BTreeMap<String, usize> = BTreeMap::new();
        for cookie in &self.cookies {
            *per_domain.entry(cookie.domain.clone()).or_insert(0) += 1;
        }
        for (domain, count) in per_domain {
            if count <= MAX_PER_DOMAIN {
                continue;
            }
            let mut indices: Vec<usize> =
                self.cookies.iter().enumerate().filter(|(_, cookie)| cookie.domain == domain).map(|(index, _)| index).collect();
            indices.sort_by_key(|index| self.cookies.get(*index).map(|cookie| cookie.created_ms).unwrap_or(0));
            let drop_count = count.saturating_sub(MAX_PER_DOMAIN);
            let mut to_drop: Vec<usize> = indices.into_iter().take(drop_count).collect();
            to_drop.sort_unstable_by(|a, b| b.cmp(a));
            for index in to_drop {
                if index < self.cookies.len() {
                    self.cookies.remove(index);
                }
            }
        }
        // Total cap.
        while self.cookies.len() > MAX_COOKIES {
            let Some(oldest) = self.cookies.iter().enumerate().min_by_key(|(_, cookie)| cookie.created_ms).map(|(index, _)| index) else {
                break;
            };
            self.cookies.remove(oldest);
        }
    }
}

/// The parts of a URL that cookie matching needs.
struct Origin {
    scheme: String,
    host: String,
    path: String,
}

impl Origin {
    fn parse(url: &str) -> Origin {
        let parsed = postguy_urlkit::Url::parse(url);
        let scheme = parsed.scheme.clone().unwrap_or_else(|| "http".to_string()).to_ascii_lowercase();
        let raw_host = parsed.authority.clone();
        let host = match raw_host.rsplit_once(':') {
            Some((host, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => host.to_string(),
            _ => raw_host,
        };
        let path = if parsed.path.is_empty() { "/".to_string() } else { parsed.path };
        Origin { scheme, host: host.to_ascii_lowercase(), path }
    }
}

/// RFC 6265 5.3 domain acceptance: host-only when no Domain; otherwise the Domain must
/// domain-match the host and not be a bare public suffix (minimal guard without a PSL).
fn accept_domain(origin: &Origin, domain: Option<&str>) -> Option<String> {
    let host = origin.host.as_str();
    let Some(domain) = domain else { return Some(host.to_string()) };
    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
    if domain.is_empty() || host == domain && !domain.contains('.') {
        // A set-cookie for the bare host (127.0.0.1, localhost) as host-only is fine; a Domain
        // attribute equal to a dotless host would make it a supercookie.
        return None;
    }
    if domain.parse::<std::net::IpAddr>().is_ok() {
        // RFC 6265 5.3 step 6: an IP address in Domain is ignored (the cookie would only be
        // host-only anyway, and a Domain attribute on an IP widens it dangerously).
        return None;
    }
    if host == domain || host.ends_with(&format!(".{domain}")) {
        if !domain.contains('.') {
            return None; // dotless domain attributes are refused (public-suffix guard)
        }
        return Some(domain);
    }
    None
}

fn domain_matches(host: &str, cookie_domain: &str, host_only: bool) -> bool {
    if host == cookie_domain {
        return true;
    }
    if host_only {
        return false;
    }
    host.ends_with(&format!(".{cookie_domain}"))
}

/// RFC 6265 5.1.4 default path: everything up to (not including) the rightmost `/`.
fn default_path(request_path: &str) -> String {
    if request_path.is_empty() || !request_path.starts_with('/') {
        return "/".to_string();
    }
    match request_path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(index) => request_path[..index].to_string(),
    }
}

/// RFC 6265 5.1.4 path-match.
fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if !request_path.starts_with(cookie_path) {
        return false;
    }
    cookie_path.ends_with('/') || request_path.as_bytes().get(cookie_path.len()) == Some(&b'/')
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jar_with(set_cookies: &[&str], url: &str) -> CookieJar {
        let mut jar = CookieJar::new();
        jar.store_from_headers_at(url, set_cookies.iter().copied(), 1_000_000);
        jar
    }

    #[test]
    fn set_cookie_round_trips_into_the_request_header_in_rfc_order() {
        let jar = jar_with(&["a=1; Path=/", "b=2; Path=/deep", "c=3"], "https://example.com/deep/page");
        let header = jar.header_for_at("https://example.com/deep/x", 1_000_001);
        assert_eq!(header, "b=2; c=3; a=1", "longer path first, then creation order");
        // A shallower request drops the deeper cookie.
        assert_eq!(jar.header_for_at("https://example.com/", 1_000_001), "a=1");
    }

    #[test]
    fn host_only_cookies_do_not_leak_to_subdomains() {
        let host_only = jar_with(&["private=1"], "https://api.example.com/");
        assert_eq!(host_only.header_for_at("https://api.example.com/x", 1_000_001), "private=1");
        assert_eq!(host_only.header_for_at("https://other.example.com/x", 1_000_001), "");
        let domain_cookie = jar_with(&["shared=1; Domain=example.com"], "https://api.example.com/");
        assert_eq!(domain_cookie.header_for_at("https://www.example.com/x", 1_000_001), "shared=1");
        assert_eq!(domain_cookie.header_for_at("https://example.com/x", 1_000_001), "shared=1");
    }

    #[test]
    fn expired_max_age_zero_and_past_expires_delete() {
        let mut jar = jar_with(&["keep=1; Max-Age=60", "gone=1"], "https://h.example/");
        assert_eq!(jar.header_for_at("https://h.example/", 1_000_000 + 30_000), "keep=1; gone=1");
        jar.store_from_headers_at("https://h.example/", ["gone=; Max-Age=0"], 1_000_000 + 40_000);
        assert_eq!(jar.header_for_at("https://h.example/", 1_000_000 + 40_000), "keep=1");
        // keep expires at +60s.
        assert_eq!(jar.header_for_at("https://h.example/", 1_000_000 + 60_001), "");
        // A past Expires deletes too.
        let mut jar = jar_with(&["x=1"], "https://h.example/");
        jar.store_from_headers_at("https://h.example/", ["x=; Expires=Thu, 01 Jan 1970 00:00:00 GMT"], 1_000_000);
        assert!(jar.cookies.is_empty());
    }

    #[test]
    fn secure_cookies_need_https_both_ways() {
        let jar = jar_with(&["token=1; Secure"], "https://h.example/");
        assert_eq!(jar.header_for_at("https://h.example/", 1_000_001), "token=1");
        assert_eq!(jar.header_for_at("http://h.example/", 1_000_001), "");
        // An insecure origin cannot set a Secure cookie.
        let mut jar = CookieJar::new();
        jar.store_from_headers_at("http://h.example/", ["bad=1; Secure"], 1_000_000);
        assert!(jar.cookies.is_empty());
    }

    #[test]
    fn foreign_domains_and_bare_dotless_domain_attributes_are_refused() {
        let mut jar = CookieJar::new();
        jar.store_from_headers_at("https://a.example.com/", ["evil=1; Domain=other.com"], 1_000_000);
        assert!(jar.cookies.is_empty(), "foreign domain refused");
        jar.store_from_headers_at("https://a.example.com/", ["super=1; Domain=com"], 1_000_000);
        assert!(jar.cookies.is_empty(), "public-suffix guard");
        jar.store_from_headers_at("http://127.0.0.1:8080/", ["local=1; Domain=127.0.0.1"], 1_000_000);
        assert!(jar.cookies.is_empty(), "IP domain attribute refused (keep it host-only)");
        jar.store_from_headers_at("http://127.0.0.1:8080/", ["local=1"], 1_000_000);
        assert_eq!(jar.header_for_at("http://127.0.0.1:8080/", 1_000_001), "local=1");
    }

    #[test]
    fn persistence_json_round_trips_and_corrupt_json_yields_an_empty_jar() {
        let jar = jar_with(&["a=1; Max-Age=600"], "https://example.com/");
        let json = jar.to_json();
        let back = CookieJar::from_json(&json);
        assert_eq!(back.cookies, jar.cookies);
        assert_eq!(back.header_for_at("https://example.com/", 1_000_001), "a=1");
        assert!(CookieJar::from_json(&serde_json::json!({"cookies": "not-an-array"})).cookies.is_empty());
    }

    #[test]
    fn the_domain_allowlist_matches_subdomains() {
        let mut jar = CookieJar::new();
        jar.allow_domain(".Example.com");
        assert!(jar.allows("api.example.com"));
        assert!(jar.allows("example.com"));
        assert!(!jar.allows("badexample.com"));
        jar.disallow_domain("example.com");
        assert!(!jar.allows("api.example.com"));
    }

    #[test]
    fn caps_evict_the_oldest_cookies() {
        let mut jar = CookieJar::new();
        for index in 0..(MAX_PER_DOMAIN + 20) {
            let set_cookie = format!("c{index}=v");
            jar.store_from_headers_at("https://example.com/", [set_cookie.as_str()], 1_000_000 + index as i64);
        }
        let domain_count = jar.cookies.iter().filter(|cookie| cookie.domain == "example.com").count();
        assert_eq!(domain_count, MAX_PER_DOMAIN);
        // The earliest created are gone, the newest stay.
        assert!(!jar.cookies.iter().any(|cookie| cookie.name == "c0"));
        assert!(jar.cookies.iter().any(|cookie| cookie.name == format!("c{}", MAX_PER_DOMAIN + 19)));
    }

    #[test]
    fn default_path_comes_from_the_request_path() {
        assert_eq!(default_path("/deep/page"), "/deep");
        assert_eq!(default_path("/only"), "/");
        assert_eq!(default_path(""), "/");
        assert!(path_matches("/deep/x", "/deep"));
        assert!(!path_matches("/deepest/x", "/deep"));
    }
}
