//! Send-overhead measurement: loopback latency of the full engine
//! pipeline versus a bare `postguy_http::Client` request, with a 100 KiB JSON body. Run it
//! explicitly and record the numbers:
//!
//! ```text
//! cargo test -p postguy-engine --release --test overhead -- --ignored --nocapture
//! ```
//!
//! The UI's "last byte to rendered Pretty view" budget is measured separately; this test isolates the engine's own per-send cost (resolve, build, send, decode,
//! history, console, jar persistence) on the same loopback server.

use std::time::{Duration, Instant};

use postguy_engine::{Engine, SendOptions, SendTarget};
use postguy_http::{Cancel, Client, ClientConfig, RequestSpec};
use postguy_model::{HttpMethod, HttpRequest};
use postguy_store::StorePaths;
use postguy_testkit::Server;

const ITERATIONS: usize = 30;

fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted.get(index).copied().unwrap_or(Duration::ZERO)
}

#[test]
#[ignore = "measurement harness; run with --ignored --nocapture in release"]
fn send_overhead_on_loopback() {
    let server = Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = Engine::open(StorePaths::new(dir.path().join("workspace"), dir.path().join("config"))).expect("engine");
    let url = format!("{}/json?kb=100", server.base_url());
    let request = HttpRequest { method: HttpMethod::Get, url: url.clone(), ..HttpRequest::default() };

    // Warm-up: JIT-free but the first send builds history/jar files and TLS-free paths.
    for _ in 0..3 {
        let outcome = engine.send(SendTarget::Inline { request: Box::new(request.clone()), name: "warmup".into() }, SendOptions::default());
        assert!(outcome.is_ok(), "{outcome:?}");
    }

    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    let bare = Client::new(ClientConfig::default()).expect("client");
    let mut bare_times = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let spec = RequestSpec { max_body_bytes: 4 << 20, ..RequestSpec::get(url.clone()) };
        let started = Instant::now();
        let response = runtime.block_on(bare.send(spec, &Cancel::new())).expect("bare send");
        bare_times.push(started.elapsed());
        assert!(response.body.len() >= 100_000, "bare body {} status {}", response.body.len(), response.status);
    }

    let mut engine_times = Vec::with_capacity(ITERATIONS);
    let mut prepare = Vec::with_capacity(ITERATIONS);
    let mut record = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let started = Instant::now();
        let outcome = engine
            .send(SendTarget::Inline { request: Box::new(request.clone()), name: "bench".into() }, SendOptions::default())
            .expect("engine send");
        engine_times.push(started.elapsed());
        prepare.push(Duration::from_millis(outcome.timings.prepare_ms));
        record.push(Duration::from_millis(outcome.timings.record_ms));
        assert!(outcome.response.body.len() >= 100_000);
    }

    bare_times.sort();
    engine_times.sort();
    let bare_median = percentile(&bare_times, 0.5);
    let engine_median = percentile(&engine_times, 0.5);
    let overhead = engine_median.saturating_sub(bare_median);
    println!("send overhead: bare client median {bare_median:?}, p95 {:?}", percentile(&bare_times, 0.95));
    println!("send overhead: engine median {engine_median:?}, p95 {:?}", percentile(&engine_times, 0.95));
    println!("send overhead: engine-only median {overhead:?} (100 KiB JSON, loopback, release)");
    println!(
        "send overhead: prepare median {:?} (budget <= 5 ms), record median {:?} (model budget <= 16 ms)",
        percentile(&prepare, 0.5),
        percentile(&record, 0.5)
    );
    println!("send overhead: history entries {}", engine.history(usize::MAX).map(|entries| entries.len()).unwrap_or(0));

    // A generous sanity ceiling: this catches an accidental O(n) per-send blow-up without
    // turning a busy machine into a red build. The recorded number is the budget's evidence.
    assert!(engine_median < Duration::from_millis(250), "engine median {engine_median:?} looks pathological");
}
