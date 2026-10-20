//! Manual redirect handling (PROMPT.md 5.3.2 step 8): our own loop, never hyper's, because the
//! product needs per-hop records, method rewrite rules, cross-host credential dropping, a hop
//! cap and loop detection.
//!
//! Rules implemented exactly:
//! - 303 switches to GET (HEAD stays HEAD); 301/302 switch POST to GET unless
//!   `follow_original_method` is on; 307/308 always keep method and body.
//! - A cross-host hop (scheme or host or port changed) drops `Authorization` (when
//!   `follow_authorization_on_redirect` is off, the default), `Cookie`, and the body headers.
//! - `remove_referer_on_redirect` removes a user-set Referer after the first hop.
//! - More than `max_redirects` hops, or seeing the same (method, url) twice, is an error with
//!   the hop list preserved for the timeline.

use std::sync::Mutex;

use crate::client::{Cancel, Client, RequestSpec, ResponseData};
use crate::error::{HttpError, Result};
use crate::jar::CookieJar;

/// The per-request redirect settings (PROMPT.md 5.3.2 Settings tab defaults).
#[derive(Debug, Clone, Copy)]
pub struct RedirectPolicy {
    pub follow: bool,
    /// off: 301/302/303 on POST become GET (the default).
    pub follow_original_method: bool,
    /// off: drop Authorization on cross-host hops (the default).
    pub follow_authorization_on_redirect: bool,
    pub remove_referer_on_redirect: bool,
    pub max_redirects: u32,
}

impl Default for RedirectPolicy {
    fn default() -> Self {
        RedirectPolicy {
            follow: true,
            follow_original_method: false,
            follow_authorization_on_redirect: false,
            remove_referer_on_redirect: false,
            max_redirects: 10,
        }
    }
}

/// One hop as the Console and the response timeline list it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectHop {
    pub url: String,
    pub status: u16,
    pub location: Option<String>,
}

/// The final response plus every hop it took to get there.
#[derive(Debug)]
pub struct RedirectOutcome {
    pub response: ResponseData,
    pub hops: Vec<RedirectHop>,
}

impl Client {
    /// Send, following redirects per the policy. Without `follow`, this is exactly one hop.
    pub async fn send_following_redirects(&self, spec: RequestSpec, policy: RedirectPolicy, cancel: &Cancel) -> Result<RedirectOutcome> {
        self.follow(spec, policy, None, cancel).await
    }

    /// Send with a cookie jar: `Set-Cookie` is captured on EVERY hop (including redirects) and
    /// stored cookies ride every request; `jar_disabled` means neither sending nor storing
    /// (the per-request "Disable cookie jar" setting). A user-set Cookie header merges with
    /// the jar's cookies (jar first, then the manual pairs).
    pub async fn send_with_cookies(
        &self,
        spec: RequestSpec,
        policy: RedirectPolicy,
        jar: &Mutex<CookieJar>,
        jar_disabled: bool,
        cancel: &Cancel,
    ) -> Result<RedirectOutcome> {
        self.follow(spec, policy, if jar_disabled { None } else { Some(jar) }, cancel).await
    }

    async fn follow(
        &self,
        spec: RequestSpec,
        policy: RedirectPolicy,
        cookies: Option<&Mutex<CookieJar>>,
        cancel: &Cancel,
    ) -> Result<RedirectOutcome> {
        let mut current = spec;
        let mut hops: Vec<RedirectHop> = Vec::new();
        let mut seen: Vec<(String, String)> = Vec::new();
        let mut crossed_hosts = false;
        loop {
            let method_key = current.method.as_str().to_string();
            if !seen.iter().any(|(method, url)| *method == method_key && *url == current.url) {
                seen.push((method_key.clone(), current.url.clone()));
            } else {
                return Err(HttpError::Protocol { reason: format!("redirect loop at {} ({} hops)", current.url, hops.len()) });
            }

            // Cookie exchange for this hop: jar cookies merge with a manual Cookie header.
            let mut hop_spec = current.clone();
            if let Some(jar) = cookies {
                let jar_header = jar.lock().ok().map(|jar| jar.header_for(&hop_spec.url)).unwrap_or_default();
                hop_spec.headers.retain(|(name, _)| !name.eq_ignore_ascii_case("cookie"));
                let manual = current.headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("cookie")).map(|(_, value)| value.clone());
                let merged = match (jar_header.is_empty(), manual) {
                    (true, None) => None,
                    (true, Some(manual)) => Some(manual),
                    (false, None) => Some(jar_header),
                    (false, Some(manual)) => Some(format!("{jar_header}; {manual}")),
                };
                if let Some(value) = merged {
                    hop_spec.headers.push(("Cookie".to_string(), value));
                }
            }
            let response = self.send(hop_spec, cancel).await?;
            if let Some(jar) = cookies {
                let set_cookies: Vec<String> =
                    response.headers.iter().filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie")).map(|(_, value)| value.clone()).collect();
                if !set_cookies.is_empty()
                    && let Ok(mut jar) = jar.lock()
                {
                    jar.store_from_headers(&current.url, set_cookies.iter().map(String::as_str));
                }
            }
            let status = response.status.as_u16();
            let location = response.headers.iter().find(|(name, _)| name == "location").map(|(_, value)| value.clone());
            let redirect = matches!(status, 301 | 302 | 303 | 307 | 308);
            if !policy.follow || !redirect {
                return Ok(RedirectOutcome { response, hops });
            }
            let Some(location) = location else {
                // A 3xx without Location is final (the viewer shows it as-is).
                return Ok(RedirectOutcome { response, hops });
            };
            hops.push(RedirectHop { url: current.url.clone(), status, location: Some(location.clone()) });
            if hops.len() as u32 > policy.max_redirects {
                return Err(HttpError::Protocol {
                    reason: format!("stopped after {} redirects (maximum {})", policy.max_redirects, policy.max_redirects),
                });
            }

            let next_url = resolve_location(&current.url, &location)?;
            let cross_host = origins_differ(&current.url, &next_url);
            if cross_host {
                crossed_hosts = true;
            }

            // Method/body rewrite rules.
            let mut next = RequestSpec { url: next_url.clone(), ..current.clone() };
            let switch_to_get = (status == 303 && current.method != hyper::Method::HEAD)
                || ((status == 301 || status == 302) && current.method == hyper::Method::POST && !policy.follow_original_method);
            if switch_to_get {
                next.method = hyper::Method::GET;
                next.body.clear();
                strip_body_headers(&mut next.headers);
            }
            if status == 307 || status == 308 {
                // Method and body stay; nothing else to do.
            }

            // Cross-hop header hygiene.
            if crossed_hosts {
                if !policy.follow_authorization_on_redirect {
                    next.headers.retain(|(name, _)| !name.eq_ignore_ascii_case("authorization"));
                }
                next.headers.retain(|(name, _)| !name.eq_ignore_ascii_case("cookie"));
                next.headers.retain(|(name, _)| !is_body_header(name));
            }
            if policy.remove_referer_on_redirect {
                next.headers.retain(|(name, _)| !name.eq_ignore_ascii_case("referer"));
            }
            current = next;
        }
    }
}

fn is_body_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("content-length")
        || name.eq_ignore_ascii_case("content-type")
        || name.eq_ignore_ascii_case("content-encoding")
        || name.eq_ignore_ascii_case("content-language")
        || name.eq_ignore_ascii_case("transfer-encoding")
}

fn strip_body_headers(headers: &mut Vec<(String, String)>) {
    headers.retain(|(name, _)| !is_body_header(name));
}

/// Whether two URLs differ in scheme, host or effective port.
pub fn origins_differ(a: &str, b: &str) -> bool {
    let (sa, ha, pa) = split_origin(a);
    let (sb, hb, pb) = split_origin(b);
    sa != sb || !ha.eq_ignore_ascii_case(&hb) || pa != pb
}

fn split_origin(url: &str) -> (String, String, u16) {
    let parsed = postguy_urlkit::Url::parse(url);
    let scheme = parsed.scheme.clone().unwrap_or_else(|| "http".to_string()).to_ascii_lowercase();
    let raw_host = parsed.authority.clone();
    let (host, port) = match raw_host.rsplit_once(':') {
        Some((host, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => {
            (host.to_string(), port.parse::<u16>().unwrap_or(if scheme == "https" { 443 } else { 80 }))
        }
        _ => (raw_host, if scheme == "https" { 443 } else { 80 }),
    };
    (scheme, host, port)
}

/// RFC 3986 reference resolution, relative to `base` (the redirect `Location` may be
/// absolute, scheme-relative, root-relative or path-relative; fragments are dropped).
pub fn resolve_location(base: &str, location: &str) -> Result<String> {
    let base_url = postguy_urlkit::Url::parse(base);
    let scheme = base_url.scheme.clone().unwrap_or_else(|| "http".to_string());
    let authority = base_url.authority.clone();
    if location.contains("://") {
        return Ok(strip_fragment(location));
    }
    if let Some(rest) = location.strip_prefix("//") {
        return Ok(strip_fragment(&format!("{scheme}://{rest}")));
    }
    if location.starts_with('/') {
        return Ok(strip_fragment(&format!("{scheme}://{authority}{location}")));
    }
    if location.is_empty() || location.starts_with('#') {
        return Ok(strip_fragment(base));
    }
    if location.starts_with('?') {
        // Query-only reference: the base PATH stays, the query is replaced.
        let base_path = if base_url.path.is_empty() { "/" } else { base_url.path.as_str() };
        return Ok(strip_fragment(&format!("{scheme}://{authority}{base_path}{location}")));
    }
    // Path-relative: replace the last segment of the base path with `location`.
    let path = &base_url.path;
    let directory = match path.rfind('/') {
        Some(index) => &path[..index + 1],
        None => "/",
    };
    let merged = normalize_path(&format!("{directory}{location}"));
    if authority.is_empty() {
        return Err(HttpError::BadUrl { url: base.to_string(), reason: "cannot resolve a relative redirect without a host".to_string() });
    }
    Ok(strip_fragment(&format!("{scheme}://{authority}{merged}")))
}

fn strip_fragment(url: &str) -> String {
    match url.split_once('#') {
        Some((head, _)) => head.to_string(),
        None => url.to_string(),
    }
}

/// Collapse `.`/`..` segments in an absolute path (the URL crate is not in the tree; this is
/// the small, total version of what RFC 3986 section 5.2.4 asks for).
fn normalize_path(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    let trailing = path.ends_with('/');
    let mut out = String::from("/");
    out.push_str(&segments.join("/"));
    if trailing && !out.ends_with('/') {
        out.push('/');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_root_relative_and_scheme_relative_locations_resolve() {
        let base = "https://api.example.com/v1/users?limit=1";
        assert_eq!(resolve_location(base, "/login?a=1").expect("root"), "https://api.example.com/login?a=1");
        assert_eq!(resolve_location(base, "https://other.example/x").expect("absolute"), "https://other.example/x");
        assert_eq!(resolve_location(base, "//cdn.example/y").expect("scheme-relative"), "https://cdn.example/y");
        assert_eq!(resolve_location(base, "posts").expect("relative"), "https://api.example.com/v1/posts");
        assert_eq!(resolve_location(base, "../health").expect("relative up"), "https://api.example.com/health");
        assert_eq!(resolve_location(base, "?page=2").expect("query-relative"), "https://api.example.com/v1/users?page=2");
    }

    #[test]
    fn fragments_are_dropped() {
        assert_eq!(resolve_location("http://h/a", "/b#frag").expect("fragment"), "http://h/b");
    }

    #[test]
    fn origins_compare_scheme_host_and_effective_port() {
        assert!(!origins_differ("http://a/x", "http://a:80/y"), "explicit default port is the same origin");
        assert!(origins_differ("http://a/x", "https://a/x"));
        assert!(origins_differ("http://a/x", "http://b/x"));
        assert!(origins_differ("http://a:8080/x", "http://a/x"));
        assert!(!origins_differ("https://API.example/x", "https://api.example/y"));
    }

    #[test]
    fn body_headers_are_recognised_case_insensitively() {
        assert!(is_body_header("Content-Type"));
        assert!(is_body_header("transfer-encoding"));
        assert!(!is_body_header("authorization"));
    }
}
