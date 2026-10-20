//! Response-viewer tests: strip chips/tabs, Pretty/Raw/Hex, Headers and Cookies, the save/clear
//! menus and the size/time popovers.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_ui_egui::response::{ResponseAction, ResponseData, ResponseViewerState, response_view};

struct State {
    data: ResponseData,
    viewer: ResponseViewerState,
    action: Option<ResponseAction>,
    last: Option<ResponseAction>,
}

fn data() -> ResponseData {
    ResponseData {
        status: 200,
        status_text: "OK".into(),
        http_version: "HTTP/1.1".into(),
        headers: vec![("content-type".into(), "application/json".into()), ("set-cookie".into(), "sid=abc; Path=/; HttpOnly".into())],
        body: b"{\"b\":2,\"a\":1}".to_vec(),
        revision: 1,
        header_bytes: 120,
        body_encoded_bytes: 13,
        body_decoded_bytes: 13,
        content_encoding: None,
        connect_ms: 2,
        ttfb_ms: 10,
        total_ms: 14,
        download_ms: 4,
        redirects: 0,
        proxy: None,
    }
}

fn harness() -> Harness<'static, State> {
    Harness::new_ui_state(
        |ui, state: &mut State| {
            state.action = response_view(ui, &mut state.data, &mut state.viewer);
            if state.action.is_some() {
                state.last = state.action.clone();
            }
        },
        State { data: data(), viewer: ResponseViewerState::default(), action: None, last: None },
    )
}

#[test]
fn the_strip_shows_status_time_size_and_the_json_pretty_view() {
    let mut harness = harness();
    harness.run();
    assert!(harness.query_by_label_contains("200 OK").is_some());
    assert!(harness.query_by_label_contains("14 ms").is_some());
    assert!(harness.query_by_label_contains("13 B").is_some());
    assert!(harness.query_by_label_contains("\"a\": 1").is_some(), "Pretty orders and indents JSON");
}

#[test]
fn raw_hex_headers_and_cookies_are_reachable() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("Raw").click();
    harness.run();
    assert!(harness.query_by_label_contains("{\"b\":2,\"a\":1}").is_some());
    harness.get_by_label("Hex").click();
    harness.run();
    assert!(harness.query_by_label_contains("00000000").is_some());
    harness.get_by_label_contains("Headers 2").click();
    harness.run();
    assert!(harness.query_by_label_contains("content-type").is_some());
    harness.get_by_label_contains("Cookies 1").click();
    harness.run();
    assert!(harness.query_by_label_contains("sid").is_some());
    assert!(harness.query_by_label_contains("HttpOnly").is_some());
}

#[test]
fn the_menu_saves_and_clears_and_the_popovers_explain_the_numbers() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("...").click();
    harness.run();
    harness.get_by_label("Save body to file").click();
    harness.run();
    assert_eq!(harness.state().last, Some(ResponseAction::SaveBody));

    harness.get_by_label("...").click();
    harness.run();
    harness.get_by_label("Clear response").click();
    harness.run();
    assert_eq!(harness.state().last, Some(ResponseAction::Clear));

    harness.get_by_label_contains("14 ms").hover();
    harness.run();
    assert!(harness.query_by_label_contains("Download: 4").is_some());
    harness.get_by_label_contains("13 B").hover();
    harness.run();
    assert!(harness.query_by_label_contains("Headers: 120 B").is_some());
}

#[test]
fn search_counts_matches() {
    let mut harness = harness();
    harness.state_mut().viewer.search = "a".into();
    harness.run();
    assert!(harness.query_by_label_contains("match(es)").is_some(), "a match counter is visible");
}
