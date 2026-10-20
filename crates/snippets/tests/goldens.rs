//! Golden tests: one canonical POST request rendered in every M4.2 language. The files are
//! written by the project (not exported from another client) and reviewed as sendable snippets.

use postguy_model::{Body, HttpMethod, HttpRequest, KeyValue, RawLanguage};
use postguy_snippets::{Language, Options, generate};

fn request() -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Post,
        url: "https://api.example.com/v1/items?limit=2".into(),
        headers: vec![
            KeyValue { key: "Accept".into(), value: "application/json".into(), ..Default::default() },
            KeyValue { key: "X-Trace".into(), value: "abc".into(), ..Default::default() },
        ],
        body: Body::Raw { language: RawLanguage::Json, text: "{\"name\":\"widget\",\"price\":9.5}".into(), extras: Default::default() },
        ..Default::default()
    }
}

#[test]
fn every_language_matches_its_golden() {
    for (language, file) in [
        (Language::Curl, include_str!("golden/curl.txt")),
        (Language::RawHttp, include_str!("golden/raw-http.txt")),
        (Language::Fetch, include_str!("golden/fetch.txt")),
        (Language::PythonRequests, include_str!("golden/python.txt")),
        (Language::Go, include_str!("golden/go.txt")),
        (Language::Rust, include_str!("golden/rust.txt")),
        (Language::PowerShell, include_str!("golden/powershell.txt")),
        (Language::NodeAxios, include_str!("golden/node-axios.txt")),
    ] {
        assert_eq!(generate(language, &request(), Options::default()), file, "{} drifted", language.id());
    }
}
