//! Property tests for the cURL importer/exporter: generate→parse round trips and
//! panic-freedom on hostile flag soup (a fuzzing precursor).

use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

use postguy_interop::curl::{generate_curl, parse_curl};
use postguy_interop::error::CurlError;
use postguy_model::{Body, HttpMethod, HttpRequest, KeyValue, RawLanguage};

/// Printable ASCII without newlines (the generator's quoting handles the rest, but a quoted
/// value spanning a physical line is out of scope for the line-based splitter).
fn value_text() -> impl Strategy<Value = String> {
    prop::collection::vec(proptest::char::range(' ', '~'), 0..40).prop_map(|chars| chars.into_iter().filter(|&c| c != '\\').collect())
}

fn http_method() -> impl Strategy<Value = HttpMethod> {
    prop::sample::select(vec![
        HttpMethod::Get,
        HttpMethod::Post,
        HttpMethod::Put,
        HttpMethod::Patch,
        HttpMethod::Delete,
        HttpMethod::Head,
        HttpMethod::Options,
    ])
}

fn header() -> impl Strategy<Value = KeyValue> {
    ("[A-Za-z][A-Za-z0-9-]{0,15}", value_text()).prop_map(|(key, value)| {
        // Header values keep their inner whitespace; leading/trailing whitespace is lost in
        // the HTTP header format itself, so the generator round trip trims.
        let value = value.trim().to_string();
        KeyValue { key, value, enabled: true, ..KeyValue::default() }
    })
}

fn key_value_pair() -> impl Strategy<Value = (String, String)> {
    ("[a-z]{1,8}", value_text())
}

fn body() -> impl Strategy<Value = Body> {
    prop::sample::select(vec![0u8, 1, 2, 3]).prop_flat_map(|kind| match kind {
        0 => Just(Body::None).boxed(),
        1 => value_text().prop_map(|text| Body::Raw { language: RawLanguage::Text, text, extras: postguy_model::Extras::default() }).boxed(),
        2 => prop::collection::vec(
            key_value_pair().prop_map(|(k, v)| {
                // A `-F` text value starting with `@` is indistinguishable from a file reference
                // (cURL semantics); the property only covers values that survive the cURL encoding.
                (k, v.trim_start_matches('@').to_string())
            }),
            1..4,
        )
        .prop_map(|rows| Body::FormData(rows.into_iter().map(|(k, v)| KeyValue { key: k, value: v, enabled: true, ..KeyValue::default() }).collect()))
        .boxed(),
        _ => prop::collection::vec(key_value_pair(), 1..4)
            .prop_map(|rows| {
                Body::UrlEncoded(rows.into_iter().map(|(k, v)| KeyValue { key: k, value: v, enabled: true, ..KeyValue::default() }).collect())
            })
            .boxed(),
    })
}

fn url() -> impl Strategy<Value = String> {
    ("https?://example[.]com/[a-z0-9/-]{0,12}", prop::option::of(("[a-z]{1,4}", value_text()).prop_map(|(k, v)| format!("{k}={v}")))).prop_map(
        |(base, q)| match q {
            Some(q) => format!("{base}?{q}"),
            None => base,
        },
    )
}

fn request() -> impl Strategy<Value = HttpRequest> {
    (http_method(), url(), prop::collection::vec(header(), 0..4), body()).prop_map(|(method, url, headers, body)| {
        // cURL semantics: a repeated `-H` header replaces the earlier row, so the property
        // covers duplicate-free header lists only.
        let mut seen: Vec<String> = Vec::new();
        let headers = headers
            .into_iter()
            .filter(|h| {
                let lower = h.key.to_ascii_lowercase();
                if seen.contains(&lower) {
                    return false;
                }
                seen.push(lower);
                true
            })
            .collect();
        HttpRequest { method, url, headers, body, ..HttpRequest::default() }
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn generate_then_parse_keeps_method_url_headers_body(req in request()) {
        let generated = generate_curl(&req, 80);
        let parsed = match parse_curl(&generated) {
            Ok(mut v) => v.remove(0),
            Err(e) => return Err(TestCaseError::fail(format!("parse failed on {generated:?}: {e}"))),
        };
        prop_assert!(parsed.warnings.is_empty(), "unexpected warnings on generated text: {:?}", parsed.warnings);
        let got = parsed.request;
        prop_assert_eq!(got.method, req.method);
        prop_assert_eq!(got.url, req.url);
        prop_assert_eq!(got.headers, req.headers);
        match (&req.body, &got.body) {
            (Body::UrlEncoded(rows), Body::Raw { text, .. }) => {
                // The generator emits urlencoded rows as one `&`-joined `--data-raw`, which the
                // parser stores as a raw body.
                let joined = rows.iter().filter(|r| r.enabled && !r.key.is_empty()).map(|r| format!("{}={}", r.key, r.value)).collect::<Vec<_>>().join("&");
                prop_assert_eq!(text, &joined);
            }
            (a, b) => prop_assert_eq!(a, b),
        }
    }

    #[test]
    fn parser_never_panics_on_flag_soup(input in prop::collection::vec(prop::sample::select(vec![
        "curl", "-X", "POST", "-H", "'x: y'", "-d", "\"abc\"", "-F", "'f=@p'", "-u", "a:b", "-b", "'c=d'",
        "--json", "--compressed", "-k", "-L", "-I", "-G", "--http2", "--max-redirs", "-m", "3", "-x", "'p'",
        "--nonsense-flag", "'", "\"", "$'", "\\", "^", "-", "--", "&", "|", ";", "https://example.com", "",
    ]), 0..30)) {
        let joined = input.join(" ");
        // Must not panic; any outcome (Ok or Err) is acceptable.
        let _ = parse_curl(&joined);
    }

    #[test]
    fn malformed_quoting_degrades_gracefully(input in prop::collection::vec(
        prop::sample::select(vec!["curl", "https://example.com", "'a", "\"b", "$'c", "'"]),
        2..12,
    )) {
        let joined = format!("curl {}", input.join(" "));
        // Quoting mistakes pair up or leave dangling quotes; either way the parse must not
        // error out, and odd dangling quotes surface as warnings.
        match parse_curl(&joined) {
            Ok(parsed) => prop_assert!(!parsed.is_empty()),
            Err(e) => prop_assert!(matches!(e, CurlError::Unsupported { .. } | CurlError::Read { .. }), "unexpected error: {e}"),
        }
    }
}
