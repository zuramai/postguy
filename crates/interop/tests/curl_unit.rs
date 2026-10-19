//! Unit tests for the cURL importer/exporter (`src/curl.rs`).

use postguy_interop::curl::{INPUT_CAP, generate_curl, parse_curl};
use postguy_interop::error::CurlError;
use postguy_model::{Body, HttpMethod, HttpRequest, HttpVersion, KeyValue, KeyValueKind, RawLanguage, RequestSettings};

fn parse_one(input: &str) -> HttpRequest {
    parse_curl(input).map(|mut v| v.remove(0).request).expect("parse should succeed")
}

#[test]
fn parses_basic_get_with_url() {
    let r = parse_one("curl https://example.com/path");
    assert_eq!(r.method, HttpMethod::Get);
    assert_eq!(r.url, "https://example.com/path");
    assert!(r.headers.is_empty());
    assert_eq!(r.body, Body::None);
}

#[test]
fn parses_method_headers_and_raw_json_body() {
    let r = parse_one("curl -X POST 'https://api.example.com/v1/users' -H 'Content-Type: application/json' --data-raw '{\"name\": \"ada\"}'");
    assert_eq!(r.method, HttpMethod::Post);
    assert_eq!(r.headers[0].key, "Content-Type");
    assert_eq!(r.headers[0].value, "application/json");
    match &r.body {
        Body::Raw { language, text, .. } => {
            assert_eq!(*language, RawLanguage::Json);
            assert_eq!(text, "{\"name\": \"ada\"}");
        }
        other => panic!("wrong body: {other:?}"),
    }
}

#[test]
fn multiple_data_pieces_concatenate_without_separator() {
    let r = parse_one("curl -d 'a=1' -d 'b=2' https://example.com");
    match &r.body {
        Body::Raw { text, .. } => assert_eq!(text, "a=1b=2"),
        other => panic!("wrong body: {other:?}"),
    }
}

#[test]
fn form_rows_map_text_and_file_kinds() {
    let r = parse_one("curl -F 'name=ada' -F 'photo=@/tmp/pic.png' -F 'doc=@/tmp/doc.pdf;type=application/pdf' https://example.com");
    match &r.body {
        Body::FormData(rows) => {
            assert_eq!(rows[0].key, "name");
            assert_eq!(rows[0].kind, KeyValueKind::Text);
            assert_eq!(rows[1].key, "photo");
            assert_eq!(rows[1].kind, KeyValueKind::File);
            assert_eq!(rows[1].value, "/tmp/pic.png");
            assert_eq!(rows[2].value, "/tmp/doc.pdf");
        }
        other => panic!("wrong body: {other:?}"),
    }
}

#[test]
fn user_flag_becomes_basic_auth() {
    let r = parse_one("curl -u ada:secret123 https://example.com");
    match &r.auth {
        postguy_model::AuthConfig::Basic(basic) => {
            assert_eq!(basic.username, "ada");
            assert_eq!(basic.password, "secret123");
        }
        other => panic!("wrong auth: {other:?}"),
    }
}

#[test]
fn user_agent_referer_become_headers() {
    let r = parse_one("curl -A 'agent/1' -e 'https://ref.example' https://example.com");
    assert_eq!(r.headers[0].key, "User-Agent");
    assert_eq!(r.headers[0].value, "agent/1");
    assert_eq!(r.headers[1].key, "Referer");
    assert_eq!(r.headers[1].value, "https://ref.example");
}

#[test]
fn cookie_pairs_build_one_cookie_header() {
    let r = parse_one("curl -b 'a=1; b=2' -b 'c=3' https://example.com");
    let cookie = r.headers.iter().find(|h| h.key == "Cookie").map(|h| h.value.clone());
    assert_eq!(cookie.as_deref(), Some("a=1; b=2; c=3"));
}

#[test]
fn cookie_stdin_is_unsupported_but_reported() {
    let err = parse_curl("curl -b @- https://example.com").expect_err("stdin cookies must be rejected");
    match err {
        CurlError::Unsupported { .. } => {}
        other => panic!("expected unsupported error, got: {other:?}"),
    }
}

#[test]
fn settings_flags_map_onto_request_settings() {
    let r = parse_one("curl -k -L --http2 --max-redirs 5 -m 2.5 -x http://proxy:8080 https://example.com");
    assert!(!r.settings.verify_ssl);
    assert!(r.settings.follow_redirects);
    assert_eq!(r.settings.http_version, HttpVersion::Http2);
    assert_eq!(r.settings.max_redirects, 5);
    assert_eq!(r.settings.timeout_ms, 2500);
}

#[test]
fn head_flag_sets_method_head() {
    let r = parse_one("curl -I https://example.com");
    assert_eq!(r.method, HttpMethod::Head);
}

#[test]
fn get_mode_moves_data_into_query_string() {
    let r = parse_one("curl -G -d 'q=post man' -d 'page=2' 'https://example.com/search?lang=en'");
    assert_eq!(r.method, HttpMethod::Get);
    assert_eq!(r.body, Body::None);
    assert_eq!(r.url, "https://example.com/search?lang=en&q=post%20manpage=2");
}

#[test]
fn json_shorthand_implies_post_and_content_type() {
    let r = parse_one("curl --json '{\"k\": 1}' https://example.com");
    assert_eq!(r.method, HttpMethod::Post);
    assert!(r.headers.iter().any(|h| h.key == "Content-Type" && h.value == "application/json"));
    match &r.body {
        Body::Raw { text, .. } => assert_eq!(text, "{\"k\": 1}"),
        other => panic!("wrong body: {other:?}"),
    }
}

#[test]
fn unknown_flags_become_warnings_not_errors() {
    let parsed = parse_curl("curl --proto '=https' https://example.com").map(|mut v| v.remove(0)).unwrap();
    assert_eq!(parsed.request.url, "https://example.com");
    assert!(parsed.warnings.iter().any(|w| w.contains("--proto")));
}

#[test]
fn ansi_c_and_double_quoted_and_escaped_strings() {
    let r = parse_one("curl -d $'line1\\nline2' -d \"quoted \\\"inner\\\" value\" https://example.com");
    match &r.body {
        Body::Raw { text, .. } => assert_eq!(text, "line1\nline2quoted \"inner\" value"),
        other => panic!("wrong body: {other:?}"),
    }
}

#[test]
fn backslash_and_caret_continuations_join_lines() {
    let r = parse_one("curl https://example.com \\\n  -H 'A: 1' \\\n  -H 'B: 2'");
    assert_eq!(r.headers.len(), 2);
    let r2 = parse_one("curl https://example.com ^\n  -H 'A: 1'");
    assert_eq!(r2.headers[0].value, "1");
}

#[test]
fn multiple_commands_split_on_line_starting_curl() {
    let parsed = parse_curl("curl https://a.example/x\ncurl -X POST https://b.example/y").expect("parse");
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].request.url, "https://a.example/x");
    assert_eq!(parsed[1].request.method, HttpMethod::Post);
}

#[test]
fn no_curl_command_is_an_error() {
    assert!(matches!(parse_curl("just some text"), Err(CurlError::NoCommand)));
}

#[test]
fn oversized_input_is_rejected() {
    let big = "curl ".to_string() + &"x".repeat(INPUT_CAP + 1);
    assert!(matches!(parse_curl(&big), Err(CurlError::TooLarge { .. })));
}

#[test]
fn attached_short_and_long_values_parse() {
    let r = parse_one("curl -XPUT --header='A: 1' -Hb:2 https://example.com");
    assert_eq!(r.method, HttpMethod::Put);
    assert_eq!(r.headers[0].value, "1");
    assert_eq!(r.headers[1].key, "b");
}

#[test]
fn generate_curl_produces_postman_style_lines() {
    let request = HttpRequest {
        method: HttpMethod::Post,
        url: "https://host/path?x=1".to_string(),
        headers: vec![KeyValue { key: "Content-Type".to_string(), value: "application/json".to_string(), enabled: true, ..KeyValue::default() }],
        body: Body::Raw { language: RawLanguage::Json, text: "{\"a\": 1}".to_string(), extras: postguy_model::Extras::default() },
        ..HttpRequest::default()
    };
    let text = generate_curl(&request, 80);
    assert!(text.starts_with("curl --location --request POST 'https://host/path?x=1' \\"));
    assert!(text.contains("\n  --header 'Content-Type: application/json' \\"));
    assert!(text.contains("\n  --data-raw '{\"a\": 1}'"));
    let single = generate_curl(&request, 0);
    assert!(!single.contains('\n'));
}

#[test]
fn generate_curl_emits_settings_flags() {
    let request = HttpRequest {
        url: "https://example.com".to_string(),
        settings: RequestSettings {
            verify_ssl: false,
            http_version: HttpVersion::Http1,
            timeout_ms: 1500,
            max_redirects: 3,
            ..RequestSettings::default()
        },
        ..HttpRequest::default()
    };
    let text = generate_curl(&request, 80);
    assert!(text.contains("--insecure"));
    assert!(text.contains("--http1.1"));
    assert!(text.contains("--max-time 1.500"));
    assert!(text.contains("--max-redirs 3"));
}

#[test]
fn round_trip_keeps_everything() {
    let request = HttpRequest {
        method: HttpMethod::Patch,
        url: "https://example.com/a?b=c".to_string(),
        headers: vec![
            KeyValue { key: "X-One".to_string(), value: "it's 'quoted'".to_string(), enabled: true, ..KeyValue::default() },
            KeyValue { key: "Cookie".to_string(), value: "a=1; b=2".to_string(), enabled: true, ..KeyValue::default() },
        ],
        body: Body::Raw {
            language: RawLanguage::Text,
            text: "data with spaces \"and quotes\"".to_string(),
            extras: postguy_model::Extras::default(),
        },
        ..HttpRequest::default()
    };
    let generated = generate_curl(&request, 80);
    let parsed = parse_curl(&generated).expect("parse").remove(0);
    assert_eq!(parsed.request.method, request.method);
    assert_eq!(parsed.request.url, request.url);
    assert_eq!(parsed.request.headers, request.headers);
    assert_eq!(parsed.request.body, request.body);
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
}
