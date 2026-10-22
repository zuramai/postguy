//! Headless frame-time bench for the frame-time budgets. Layout and painting run
//! through egui's test harness, so the numbers cover our own widget tree without a GPU.
//! `cargo xtask bench` runs this in release and checks the budgets; run it directly to inspect.
//!
//! Metrics: a 10 000-request tree (first frame, average frame, filter keystroke) and a 10 MB
//! JSON body in Raw mode (first frame, average frame). The harness size is 1280x800.

use std::time::{Duration, Instant};

use egui_kittest::Harness;
use postguy_model::{Collection, HttpMethod, HttpRequest, Id, Item, Request, RequestKind};
use postguy_ui_egui::PostguyShell;
use postguy_ui_egui::response::{BodyMode, ResponseData, ResponseViewerState, response_view};
use postguy_ui_egui::sidebar::SidebarData;

fn main() {
    println!("bench: {} {} (release frame-time capture)", std::env::consts::OS, std::env::consts::ARCH);
    tree();
    response();
}

fn tree() {
    let requests: Vec<Item> = (0..10_000)
        .map(|index| {
            Item::Request(Box::new(Request {
                id: Id(format!("request-{index}")),
                name: format!("Request {index}"),
                kind: RequestKind::Http(HttpRequest {
                    method: HttpMethod::Get,
                    url: format!("https://example.test/items/{index}"),
                    ..Default::default()
                }),
                ..Default::default()
            }))
        })
        .collect();
    let data = SidebarData {
        revision: 1,
        collections: vec![Collection { id: Id("bench".into()), name: "Bench".into(), items: requests, ..Default::default() }],
        ..Default::default()
    };
    let mut shell = PostguyShell::new("bench");
    shell.sidebar_data = data;
    let mut harness = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, shell: &mut PostguyShell| {
            shell.ui(ui);
        },
        shell,
    );
    let first = time(|| {
        harness.run();
    });
    let average = average_frame(&mut harness, 20);
    harness.state_mut().sidebar_state.filter = "Request 9999".into();
    let filter = time(|| {
        harness.run();
    });
    println!("tree.rows=10000");
    println!("tree.first_ms={:.2}", millis(first));
    println!("tree.avg_ms={:.2}", millis(average));
    println!("tree.filter_ms={:.2}", millis(filter));
}

fn response() {
    let mut body = String::with_capacity(10 << 20);
    body.push('[');
    let mut index = 0usize;
    while body.len() < (10 << 20) {
        if index > 0 {
            body.push(',');
        }
        body.push_str("{\"id\":123456,\"name\":\"synthetic-item\",\"settled\":true}");
        index += 1;
    }
    body.push(']');
    let data = ResponseData {
        status: 200,
        status_text: "OK".into(),
        http_version: "HTTP/2".into(),
        headers: vec![("content-type".into(), "application/json".into())],
        body: body.into_bytes(),
        revision: 1,
        header_bytes: 64,
        body_encoded_bytes: 10 << 20,
        body_decoded_bytes: 10 << 20,
        content_encoding: None,
        connect_ms: 1,
        ttfb_ms: 2,
        total_ms: 3,
        download_ms: 1,
        redirects: 0,
        proxy: None,
    };
    let mut state = ResponseViewerState::default();
    state.mode = BodyMode::Raw;
    let mut harness = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, (data, state): &mut (ResponseData, ResponseViewerState)| {
            response_view(ui, data, state);
        },
        (data, state),
    );
    let first = time(|| {
        harness.run();
    });
    let average = average_frame(&mut harness, 20);
    println!("response.bytes={}", harness.state().0.body.len());
    println!("response.first_raw_ms={:.2}", millis(first));
    println!("response.avg_raw_ms={:.2}", millis(average));
}

fn time(run: impl FnOnce()) -> Duration {
    let start = Instant::now();
    run();
    start.elapsed()
}

fn average_frame<State>(harness: &mut Harness<'_, State>, frames: u32) -> Duration {
    let mut total = Duration::ZERO;
    for _ in 0..frames {
        total += time(|| {
            harness.run();
        });
    }
    total / frames
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
