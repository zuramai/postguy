//! App updates: ask the release feed for the newest published release, compare it with this
//! build's version, and download one release asset with its SHA-256 checked against the
//! release's `SHA256SUMS.txt`.
//!
//! The network is touched only when a caller asks (the user's "Check for updates", or the
//! automatic check the user can turn off in Settings). Installing the downloaded package is the
//! platform layer's job (`postguy-platform`); nothing here replaces files outside `dir`.
//!
//! Every value from the feed is hostile: the JSON is parsed with a nesting cap, the release
//! notes and asset list are capped, asset names must be plain file names before they touch a
//! path, redirects are capped and may never leave HTTPS, and every body has a size cap.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use postguy_http::{Cancel, Client, ClientConfig, RequestSpec, TlsMode};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use crate::Engine;
use crate::error::{EngineError, Result};

/// The release feed: GitHub's "latest release" endpoint, which skips drafts and pre-releases.
pub const RELEASES_LATEST_URL: &str = "https://api.github.com/repos/zuramai/postguy/releases/latest";
/// The newest release's page for people (opened in the browser, never fetched).
pub const RELEASES_PAGE_URL: &str = "https://github.com/zuramai/postguy/releases/latest";
/// The release asset that lists `<sha256>  <file name>` for every other asset.
pub const CHECKSUMS_ASSET: &str = "SHA256SUMS.txt";
/// The largest release asset a download accepts.
pub const ASSET_CAP: usize = 512 << 20;

const RELEASE_JSON_CAP: usize = 2 << 20;
const RELEASE_JSON_DEPTH: usize = 32;
const CHECKSUMS_CAP: usize = 256 << 10;
const NOTES_CAP: usize = 16 << 10;
const ASSETS_CAP: usize = 256;
const ASSET_NAME_CAP: usize = 200;
const REDIRECT_CAP: usize = 5;
const CHECK_TIMEOUT_MS: u64 = 30_000;
/// The download's per-read idle timeout (the client restarts it for every body frame).
const DOWNLOAD_IDLE_TIMEOUT_MS: u64 = 60_000;

/// This build's version (the workspace version every crate shares).
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// A semantic version (`MAJOR.MINOR.PATCH[-PRE][+BUILD]`, an optional leading `v`), ordered by
/// SemVer 2.0 precedence: build metadata is ignored and a pre-release sorts before its release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<String>,
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text.split_once('+').map_or(text, |(core, _)| core);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut numbers = core.split('.');
        let mut number = || -> Option<u64> {
            let part = numbers.next()?;
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) || (part.len() > 1 && part.starts_with('0')) {
                return None;
            }
            part.parse().ok()
        };
        let (major, minor, patch) = (number()?, number()?, number()?);
        if numbers.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => {
                let identifiers: Vec<String> = pre.split('.').map(str::to_string).collect();
                if identifiers.iter().any(|id| id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')) {
                    return None;
                }
                identifiers
            }
        };
        Some(Version { major, minor, patch, pre })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let core = (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch));
        if core != Ordering::Equal {
            return core;
        }
        match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            (false, false) => {}
        }
        for (left, right) in self.pre.iter().zip(&other.pre) {
            let order = match (left.parse::<u64>(), right.parse::<u64>()) {
                (Ok(left), Ok(right)) => left.cmp(&right),
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => left.cmp(right),
            };
            if order != Ordering::Equal {
                return order;
            }
        }
        self.pre.len().cmp(&other.pre.len())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// One downloadable file of a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// A published release as the feed describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    /// The version without the tag's leading `v`.
    pub version: String,
    pub tag: String,
    /// The release page for people (notes, every package).
    pub page_url: String,
    /// The release notes (Markdown), capped at 16 KiB.
    pub notes: String,
    pub published_at: String,
    pub prerelease: bool,
    pub assets: Vec<ReleaseAsset>,
}

impl Release {
    pub fn asset(&self, name: &str) -> Option<&ReleaseAsset> {
        self.assets.iter().find(|asset| asset.name == name)
    }
}

/// The outcome of a check: this build, the newest release, and whether it is an update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    pub current: String,
    pub latest: Release,
    /// The latest release is a newer, non-pre-release version than this build.
    pub available: bool,
}

/// A downloaded asset whose SHA-256 matched the release's checksum list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadedAsset {
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Deserialize)]
struct FeedRelease {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<FeedAsset>,
}

#[derive(Deserialize)]
struct FeedAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

fn feed_error(message: impl Into<String>) -> EngineError {
    EngineError::Update { message: message.into() }
}

/// Parse the feed's latest-release JSON. Drafts and tags that are not versions are refused;
/// assets with names that are not plain file names are dropped.
pub fn parse_release(body: &[u8]) -> Result<Release> {
    let value = postguy_util::json::parse_json(body, &postguy_util::json::Caps { max_depth: RELEASE_JSON_DEPTH })
        .map_err(|err| feed_error(format!("the release feed is not valid JSON: {err}")))?;
    let feed: FeedRelease = serde_json::from_value(value).map_err(|err| feed_error(format!("the release feed is missing release fields: {err}")))?;
    if feed.draft {
        return Err(feed_error("the release feed returned a draft release"));
    }
    let version = Version::parse(&feed.tag_name).ok_or_else(|| feed_error(format!("the latest release tag `{}` is not a version", feed.tag_name)))?;
    let version_text = feed.tag_name.trim().trim_start_matches('v').to_string();
    let assets = feed
        .assets
        .into_iter()
        .filter(|asset| is_plain_file_name(&asset.name))
        .take(ASSETS_CAP)
        .map(|asset| ReleaseAsset { name: asset.name, url: asset.browser_download_url, size: asset.size })
        .collect();
    Ok(Release {
        version: version_text,
        tag: feed.tag_name,
        page_url: feed.html_url,
        notes: truncate_chars(feed.body.unwrap_or_default(), NOTES_CAP),
        published_at: feed.published_at.unwrap_or_default(),
        prerelease: feed.prerelease || version.is_prerelease(),
        assets,
    })
}

/// Compare a release with a running version.
pub fn compare(current: &str, latest: Release) -> Result<UpdateCheck> {
    let running = Version::parse(current).ok_or_else(|| feed_error(format!("this build's version `{current}` is not a version")))?;
    let newest = Version::parse(&latest.version).ok_or_else(|| feed_error(format!("`{}` is not a version", latest.version)))?;
    let available = !latest.prerelease && newest > running;
    Ok(UpdateCheck { current: current.to_string(), latest, available })
}

/// The `sha256sum` lines (`<hex>  <name>` or `<hex> *<name>`) as (name, lowercase hex) pairs.
/// Lines that are not a 64-digit hash and a file name are skipped.
pub fn parse_checksums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (hash, name) = line.trim().split_once(char::is_whitespace)?;
            let name = name.trim_start();
            let name = name.strip_prefix('*').unwrap_or(name);
            (hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) && !name.is_empty())
                .then(|| (name.to_string(), hash.to_ascii_lowercase()))
        })
        .collect()
}

/// A release asset name that is safe to join to a directory: ASCII letters, digits and
/// `.-_+`, no leading dot, at most 200 bytes.
pub fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= ASSET_NAME_CAP
        && !name.starts_with('.')
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+'))
}

fn truncate_chars(mut text: String, cap: usize) -> String {
    if text.len() > cap {
        let mut end = cap;
        while !text.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        text.truncate(end);
    }
    text
}

fn scheme_of(url: &str) -> String {
    postguy_urlkit::Url::parse(url).scheme.unwrap_or_default().to_ascii_lowercase()
}

/// The next URL for a 3xx answer, or `None` for any other status. Never leaves HTTPS.
fn next_hop(current: &str, status: u16, headers: &[(String, String)]) -> Result<Option<String>> {
    if !matches!(status, 301 | 302 | 303 | 307 | 308) {
        return Ok(None);
    }
    let location = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("location"))
        .map(|(_, value)| value.as_str())
        .ok_or_else(|| feed_error(format!("{current} redirected without a Location")))?;
    let next = postguy_http::resolve_location(current, location)?;
    if scheme_of(current) == "https" && scheme_of(&next) != "https" {
        return Err(feed_error(format!("{current} redirected to {next}, which is not HTTPS")));
    }
    Ok(Some(next))
}

fn get_spec(url: &str, accept: &str, cap: usize, timeout_ms: u64) -> RequestSpec {
    let mut spec = RequestSpec::get(url);
    spec.headers = vec![("Accept".into(), accept.into()), ("User-Agent".into(), format!("Postguy/{}", current_version()))];
    spec.max_body_bytes = cap;
    spec.timeout_ms = timeout_ms;
    spec
}

/// GET into memory, following up to five redirects.
async fn get_bytes(client: &Client, url: &str, accept: &str, cap: usize, cancel: &Cancel) -> Result<Vec<u8>> {
    let mut url = url.to_string();
    for _ in 0..=REDIRECT_CAP {
        let response = client.send(get_spec(&url, accept, cap, CHECK_TIMEOUT_MS), cancel).await?;
        let status = response.status.as_u16();
        if let Some(next) = next_hop(&url, status, &response.headers)? {
            url = next;
            continue;
        }
        if status != 200 {
            return Err(feed_error(format!("{url} answered HTTP {status}")));
        }
        return Ok(response.body);
    }
    Err(feed_error(format!("{url} redirected more than {REDIRECT_CAP} times")))
}

/// GET straight into `file`, following up to five redirects.
async fn get_to_file(client: &Client, url: &str, file: &Path, cancel: &Cancel) -> Result<()> {
    let mut url = url.to_string();
    for _ in 0..=REDIRECT_CAP {
        let response = client.send_to_file(get_spec(&url, "application/octet-stream", ASSET_CAP, DOWNLOAD_IDLE_TIMEOUT_MS), cancel, file).await?;
        let status = response.status.as_u16();
        if let Some(next) = next_hop(&url, status, &response.headers)? {
            url = next;
            continue;
        }
        if status != 200 {
            return Err(feed_error(format!("downloading {url} answered HTTP {status}")));
        }
        return Ok(());
    }
    Err(feed_error(format!("{url} redirected more than {REDIRECT_CAP} times")))
}

fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0u8; 64 << 10];
    let mut total: u64 = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(buffer.get(..read).unwrap_or_default());
        total = total.saturating_add(read as u64);
    }
    Ok((hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect(), total))
}

impl Engine {
    /// The client for the release feed: certificate verification always on, the user's CA
    /// bundle and proxy honoured, no client certificate.
    fn update_client(&self) -> Result<Client> {
        let settings = self.settings();
        let ca_pem_path = (!settings.ca_bundle_path.trim().is_empty()).then(|| settings.ca_bundle_path.clone());
        Client::new(ClientConfig { tls: TlsMode::Verify { ca_pem_path }, proxy: settings.proxy, ..ClientConfig::default() })
            .map_err(EngineError::Http)
    }

    /// Check [`RELEASES_LATEST_URL`] for a newer release (blocking; call it off the UI thread).
    pub fn check_for_update(&self) -> Result<UpdateCheck> {
        self.check_for_update_at(RELEASES_LATEST_URL)
    }

    /// The same against another feed with the GitHub latest-release shape (a mirror or a fork).
    pub fn check_for_update_at(&self, feed_url: &str) -> Result<UpdateCheck> {
        let client = self.update_client()?;
        let cancel = Cancel::new();
        let body = self.inner.runtime.block_on(get_bytes(&client, feed_url, "application/vnd.github+json", RELEASE_JSON_CAP, &cancel))?;
        let mut release = parse_release(&body)?;
        // A feed fetched over HTTPS may not send a download over plain HTTP.
        if scheme_of(feed_url) == "https" {
            release.assets.retain(|asset| scheme_of(&asset.url) == "https");
        }
        compare(current_version(), release)
    }

    /// Download one asset of `release` into `dir` and check its SHA-256 against the release's
    /// [`CHECKSUMS_ASSET`]. The file is written as `<name>.part` and renamed only after the hash
    /// matched; a mismatch, a missing checksum or a cancel leaves nothing behind.
    pub fn download_update_asset(&self, release: &Release, name: &str, dir: &Path, cancel: &Cancel) -> Result<DownloadedAsset> {
        if !is_plain_file_name(name) {
            return Err(feed_error(format!("`{name}` is not a release asset file name")));
        }
        let asset = release.asset(name).ok_or_else(|| {
            let names: Vec<&str> = release.assets.iter().map(|asset| asset.name.as_str()).collect();
            feed_error(format!("release {} has no asset `{name}`; it has: {}", release.tag, names.join(", ")))
        })?;
        let sums = release
            .asset(CHECKSUMS_ASSET)
            .ok_or_else(|| feed_error(format!("release {} has no {CHECKSUMS_ASSET}, so its downloads cannot be verified", release.tag)))?;
        let client = self.update_client()?;
        let sums_body = self.inner.runtime.block_on(get_bytes(&client, &sums.url, "text/plain", CHECKSUMS_CAP, cancel))?;
        let expected = parse_checksums(&String::from_utf8_lossy(&sums_body))
            .into_iter()
            .find(|(listed, _)| listed == name)
            .map(|(_, hash)| hash)
            .ok_or_else(|| feed_error(format!("{CHECKSUMS_ASSET} of {} does not list `{name}`", release.tag)))?;

        std::fs::create_dir_all(dir).map_err(|err| feed_error(format!("creating {}: {err}", dir.display())))?;
        let part = dir.join(format!("{name}.part"));
        let target = dir.join(name);
        let downloaded = self.inner.runtime.block_on(get_to_file(&client, &asset.url, &part, cancel));
        let verified = downloaded.and_then(|()| {
            let (actual, bytes) = sha256_file(&part).map_err(|err| feed_error(format!("reading {}: {err}", part.display())))?;
            if actual != expected {
                return Err(feed_error(format!(
                    "`{name}` failed its checksum: {CHECKSUMS_ASSET} says {expected}, the download is {actual}; nothing was installed"
                )));
            }
            std::fs::rename(&part, &target).map_err(|err| feed_error(format!("moving the download to {}: {err}", target.display())))?;
            Ok(DownloadedAsset { path: target.clone(), sha256: actual, bytes })
        });
        if verified.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        verified
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|| panic!("{text} parses"))
    }

    #[test]
    fn versions_order_by_semver_precedence() {
        let ordered = [
            "0.9.9",
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.10.0",
            "2.0.0",
        ];
        for pair in ordered.windows(2) {
            let [lower, higher] = pair else { continue };
            assert!(version(lower) < version(higher), "{lower} < {higher}");
        }
        assert_eq!(version("v1.2.3+build.5"), version("1.2.3"), "a leading v and build metadata do not count");
    }

    #[test]
    fn malformed_versions_are_refused() {
        for text in ["", "1", "1.2", "1.2.3.4", "01.2.3", "1.2.x", "1.2.3-", "1.2.3-a..b", "1.2.3-ü", "99999999999999999999.0.0"] {
            assert!(Version::parse(text).is_none(), "{text:?}");
        }
    }

    #[test]
    fn an_update_is_a_newer_non_prerelease() {
        let release = |version: &str, prerelease: bool| Release {
            version: version.into(),
            tag: format!("v{version}"),
            page_url: String::new(),
            notes: String::new(),
            published_at: String::new(),
            prerelease,
            assets: Vec::new(),
        };
        assert!(compare("0.1.0", release("0.2.0", false)).map(|check| check.available).unwrap_or(false));
        assert!(!compare("0.2.0", release("0.2.0", false)).map(|check| check.available).unwrap_or(true));
        assert!(!compare("0.3.0", release("0.2.0", false)).map(|check| check.available).unwrap_or(true), "never offers a downgrade");
        assert!(!compare("0.1.0", release("0.2.0", true)).map(|check| check.available).unwrap_or(true), "pre-releases are not offered");
        assert!(compare("0.2.0-rc.1", release("0.2.0", false)).map(|check| check.available).unwrap_or(false), "a release replaces its rc");
    }

    #[test]
    fn the_feed_drops_unsafe_asset_names_and_caps_notes() {
        let notes = "é".repeat(NOTES_CAP);
        let body = serde_json::json!({
            "tag_name": "v1.2.0",
            "html_url": "https://example.test/r",
            "body": notes,
            "assets": [
                {"name": "postguy-1.2.0-linux-x86_64.AppImage", "browser_download_url": "https://example.test/a", "size": 3},
                {"name": "../../evil", "browser_download_url": "https://example.test/b"},
                {"name": "..\\evil.exe", "browser_download_url": "https://example.test/c"},
                {"name": ".hidden", "browser_download_url": "https://example.test/d"},
                {"name": "", "browser_download_url": "https://example.test/e"}
            ]
        });
        let release = parse_release(body.to_string().as_bytes()).expect("release");
        assert_eq!(release.version, "1.2.0");
        let names: Vec<&str> = release.assets.iter().map(|asset| asset.name.as_str()).collect();
        assert_eq!(names, ["postguy-1.2.0-linux-x86_64.AppImage"]);
        assert!(release.notes.len() <= NOTES_CAP && release.notes.chars().all(|ch| ch == 'é'), "notes are cut on a char boundary");
    }

    #[test]
    fn hostile_feeds_are_errors() {
        let deep = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
        for body in [deep.as_str(), "{}", r#"{"tag_name":"latest"}"#, r#"{"tag_name":"v1.0.0","draft":true}"#, "not json"] {
            assert!(parse_release(body.as_bytes()).is_err(), "{body:.40}");
        }
    }

    #[test]
    fn checksum_lines_parse_in_both_sha256sum_modes() {
        let hash = "A".repeat(64);
        let text = format!("{hash}  postguy.zip\n{} *postguy.msi\nnot a line\n{}  short.bin\n", "b".repeat(64), "c".repeat(10));
        assert_eq!(parse_checksums(&text), [("postguy.zip".to_string(), "a".repeat(64)), ("postguy.msi".to_string(), "b".repeat(64))]);
    }

    #[test]
    fn redirects_never_leave_https() {
        let headers = |location: &str| vec![("location".to_string(), location.to_string())];
        assert_eq!(next_hop("https://a.test/x", 302, &headers("https://b.test/y")).ok().flatten().as_deref(), Some("https://b.test/y"));
        assert!(next_hop("https://a.test/x", 302, &headers("http://b.test/y")).is_err());
        assert_eq!(next_hop("http://127.0.0.1/x", 302, &headers("/y")).ok().flatten().as_deref(), Some("http://127.0.0.1/y"));
        assert!(next_hop("https://a.test/x", 200, &[]).ok().flatten().is_none());
    }
}
