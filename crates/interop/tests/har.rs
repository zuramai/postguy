//! HAR 1.2 import from a synthetic recording: requests, bodies, response examples and the
//! hostile-input handling.

use std::path::Path;

use postguy_interop::{import_har, is_har};
use postguy_model::{Body, HttpRequest, Item, KeyValueKind, RawLanguage, Request, RequestKind};
use serde_json::{Value, json};

fn sample() -> Value {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/har/sample.har")).expect("fixture");
    serde_json::from_str(&text).expect("har json")
}

fn requests(collection: &postguy_model::Collection) -> Vec<&Request> {
    collection
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Request(request) => Some(request.as_ref()),
            Item::Folder(_) => None,
        })
        .collect()
}

fn http(request: &Request) -> &HttpRequest {
    match &request.kind {
        RequestKind::Http(http) => http,
        other => panic!("not http: {other:?}"),
    }
}

#[test]
fn entries_become_requests_with_bodies_and_examples() {
    let (collection, report) = import_har(&sample()).expect("import");
    assert_eq!(collection.name, "Corner Shop session");
    let list = requests(&collection);
    assert_eq!(list.len(), 4);
    assert_eq!((report.requests, report.examples, report.collections), (4, 3, 1), "the failed entry (status 0) has no example");

    assert_eq!(list[0].name, "GET /items");
    assert_eq!(http(list[0]).url, "https://shop.test/items?limit=20&sort=name&sort=price", "the URL keeps its query, repeated keys included");
    assert!(http(list[0]).params.is_empty(), "the URL is the source of truth for the query");
    assert_eq!(list[0].examples[0].name, "200 OK");
    assert_eq!(list[0].examples[0].status, Some(200));
    assert_eq!(list[0].examples[0].language, Some(RawLanguage::Json));
    assert_eq!(String::from_utf8_lossy(&list[0].examples[0].body.as_ref().expect("body").bytes), "{\"items\":[{\"id\":1,\"n\":\"a\"}]}");

    let post = http(list[1]);
    let Body::Raw { language, text, .. } = &post.body else { panic!("{:?}", post.body) };
    assert_eq!((*language, text.as_str()), (RawLanguage::Json, "{\"name\":\"widget\"}"));
    assert!(post.headers.iter().any(|row| row.key == "Authorization" && row.value == "Bearer demo-token"));
    assert_eq!(list[1].examples[0].body.as_ref().map(|body| body.bytes.clone()), Some(vec![0, 1, 2, 255]), "base64 bodies decode");

    let multipart = http(list[2]);
    let Body::FormData(rows) = &multipart.body else { panic!("{:?}", multipart.body) };
    assert_eq!((rows[0].key.as_str(), rows[0].value.as_str(), rows[0].kind), ("caption", "front", KeyValueKind::Text));
    assert_eq!((rows[1].key.as_str(), rows[1].kind, rows[1].content_type.as_deref()), ("file", KeyValueKind::File, Some("image/png")));
    assert!(report.warnings.iter().any(|w| w.contains("file part file needs a file chosen")), "{:?}", report.warnings);

    let form = http(list[3]);
    assert!(matches!(&form.body, Body::UrlEncoded(rows) if rows.len() == 2 && rows[1].value == "two words"), "{:?}", form.body);
    assert!(report.warnings.iter().any(|w| w.contains("method TRACE is not supported; imported as GET")), "{:?}", report.warnings);
}

#[test]
fn pseudo_headers_are_skipped_and_reported() {
    let (collection, report) = import_har(&sample()).expect("import");
    let first = requests(&collection)[0];
    let names: Vec<&str> = http(first).headers.iter().map(|row| row.key.as_str()).collect();
    assert_eq!(names, ["Accept"], "pseudo-headers, Host and Content-Length are dropped");
    let response_names: Vec<&str> = first.examples[0].headers.iter().map(|row| row.key.as_str()).collect();
    assert_eq!(response_names, ["Content-Type"], "response pseudo-headers are dropped too");
    assert_eq!(report.warnings.iter().filter(|w| w.contains("pseudo-headers, Content-Length and Host were not imported")).count(), 1);
}

#[test]
fn bad_base64_bodies_are_dropped_with_a_report_entry() {
    let (collection, report) = import_har(&sample()).expect("import");
    let photo = requests(&collection)[2];
    assert_eq!(photo.examples.len(), 1, "the example survives");
    assert!(photo.examples[0].body.is_none(), "`###` is not base64, so the body is dropped");
    assert!(report.dropped.iter().any(|d| d.contains("not valid base64")), "{:?}", report.dropped);
}

#[test]
fn detection_and_hostile_documents_are_handled() {
    assert!(is_har(&sample()));
    assert!(!is_har(&json!({"log": {}})));
    assert!(!is_har(&json!({"openapi": "3.0.0"})));
    assert!(import_har(&json!({"log": {"entries": "nope"}})).is_err());

    // An entry with no URL is dropped with a report entry; a missing title falls back.
    let (collection, report) =
        import_har(&json!({"log": {"entries": [{"request": {"method": "GET"}}, {"request": {"method": "GET", "url": "http://x"}}]}}))
            .expect("import");
    assert_eq!(collection.name, "Imported HAR");
    assert_eq!(requests(&collection).len(), 1);
    assert!(report.dropped.iter().any(|d| d.contains("no request URL")));

    // More entries than the cap are cut off with a warning, and each body is bounded.
    let entries: Vec<Value> =
        (0..postguy_interop::har::MAX_ENTRIES + 5).map(|i| json!({"request": {"method": "GET", "url": format!("http://x/{i}")}})).collect();
    let (collection, report) = import_har(&json!({"log": {"entries": entries}})).expect("import");
    assert_eq!(requests(&collection).len(), postguy_interop::har::MAX_ENTRIES);
    assert!(report.warnings.iter().any(|w| w.contains("more than 10000 entries")));
}
