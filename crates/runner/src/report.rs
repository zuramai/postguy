//! Reporters for a finished [`RunResult`]: a Newman-like CLI text, JSON, JUnit XML and one
//! self-contained HTML file. Every user string is escaped; none of them can panic.

use std::fmt::Write as _;

use postguy_model::TestStatus;

use crate::result::{RequestRun, RunResult};

/// The CLI report: per-iteration lines, then a summary table.
pub fn cli(result: &RunResult) -> String {
    let mut out = String::new();
    let total = result.iterations.len();
    let _ =
        writeln!(out, "Run of {}{}", result.collection_name, result.environment_name.as_ref().map(|name| format!(" ({name})")).unwrap_or_default());
    for iteration in &result.iterations {
        let _ = writeln!(out, "\nIteration {}/{}", iteration.index.saturating_add(1), total);
        for run in &iteration.requests {
            write_request_lines(&mut out, run);
        }
        for warning in &iteration.warnings {
            let _ = writeln!(out, "  ! {warning}");
        }
    }
    let _ = writeln!(out);
    out.push_str(&summary_table(result));
    let _ = writeln!(out, "total run duration: {} ms", result.duration_ms());
    if let Some(reason) = &result.stopped {
        let _ = writeln!(out, "stopped: {reason}");
    }
    out
}

fn write_request_lines(out: &mut String, run: &RequestRun) {
    let _ = writeln!(out, "→ {}", run.name);
    if let Some(reason) = &run.skipped {
        let _ = writeln!(out, "  ○ skipped: {reason}");
        return;
    }
    let status = run.status.map(|status| status.to_string()).unwrap_or_else(|| "no response".to_string());
    let _ = writeln!(out, "  {} {} [{status}, {} B, {} ms]", run.method, run.url, run.size_bytes, run.duration_ms);
    if let Some(error) = &run.error {
        let _ = writeln!(out, "  ✗ {error}");
    }
    for failure in &run.failures {
        let line = failure.line.map(|line| format!(" at line {line}")).unwrap_or_default();
        let _ = writeln!(out, "  ✗ {} script in {} failed{line}: {}", failure.phase.label(), failure.origin, failure.message);
    }
    for test in &run.tests {
        match test.status {
            TestStatus::Passed => {
                let _ = writeln!(out, "  ✓ {}", test.name);
            }
            TestStatus::Failed => {
                let _ = writeln!(out, "  ✗ {}: {}", test.name, test.message.as_deref().unwrap_or("failed"));
            }
            TestStatus::Skipped => {
                let _ = writeln!(out, "  ○ {} (skipped)", test.name);
            }
        }
    }
}

fn summary_table(result: &RunResult) -> String {
    let failed_iterations = result.iterations.iter().filter(|iteration| iteration.requests.iter().any(RequestRun::failed)).count();
    let scripted: Vec<&RequestRun> = result
        .iterations
        .iter()
        .flat_map(|iteration| &iteration.requests)
        .filter(|run| run.skipped.is_none() && (!run.tests.is_empty() || !run.failures.is_empty()))
        .collect();
    let failed_scripts = scripted.iter().filter(|run| run.failed()).count();
    let totals = &result.totals;
    let rows: [(&str, usize, usize); 4] = [
        ("iterations", result.iterations.len(), failed_iterations),
        ("requests", totals.requests, totals.failed_requests),
        ("test-scripts", scripted.len(), failed_scripts),
        ("assertions", totals.tests_passed.saturating_add(totals.tests_failed), totals.tests_failed),
    ];
    let label_width = rows.iter().map(|(label, _, _)| label.len()).max().unwrap_or(0);
    let number_width = |column: usize, header: &str| {
        rows.iter().map(|row| if column == 0 { row.1 } else { row.2 }.to_string().len()).max().unwrap_or(1).max(header.len())
    };
    let executed = number_width(0, "executed");
    let failed = number_width(1, "failed");
    let rule = |left: &str, mid: &str, right: &str| {
        format!("{left}{}{mid}{}{mid}{}{right}\n", "─".repeat(label_width + 2), "─".repeat(executed + 2), "─".repeat(failed + 2))
    };
    let mut out = rule("┌", "┬", "┐");
    let _ = writeln!(out, "│ {:label_width$} │ {:>executed$} │ {:>failed$} │", "", "executed", "failed");
    out.push_str(&rule("├", "┼", "┤"));
    for (label, run, bad) in rows {
        let _ = writeln!(out, "│ {label:>label_width$} │ {run:>executed$} │ {bad:>failed$} │");
    }
    out.push_str(&rule("└", "┴", "┘"));
    out
}

/// The result as JSON (the same shape `run_get` returns).
pub fn json(result: &RunResult) -> serde_json::Value {
    serde_json::to_value(result).unwrap_or_else(|err| serde_json::json!({ "error": err.to_string() }))
}

/// JUnit XML: one `testsuite` per request run, one `testcase` per test.
pub fn junit(result: &RunResult) -> String {
    let totals = &result.totals;
    let tests = totals.tests_passed.saturating_add(totals.tests_failed).saturating_add(totals.tests_skipped);
    let failures = totals.tests_failed.saturating_add(totals.script_failures);
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        out,
        "<testsuites name=\"{}\" tests=\"{tests}\" failures=\"{failures}\" time=\"{}\">",
        xml(&result.collection_name),
        seconds(result.duration_ms())
    );
    for iteration in &result.iterations {
        for run in &iteration.requests {
            let mut path = run.path.join("/");
            if !path.is_empty() {
                path.push('/');
            }
            let suite = format!("{}/{path}{}", iteration.index.saturating_add(1), run.name);
            let count = run.tests.len().max(usize::from(run.error.is_some() || run.skipped.is_some()));
            let bad = run.tests.iter().filter(|test| test.status == TestStatus::Failed).count().saturating_add(run.failures.len());
            let _ =
                writeln!(out, "  <testsuite name=\"{}\" tests=\"{count}\" failures=\"{bad}\" time=\"{}\">", xml(&suite), seconds(run.duration_ms));
            if let Some(error) = &run.error {
                let _ = writeln!(out, "    <testcase name=\"request\"><error message=\"{}\"/></testcase>", xml(error));
            }
            if let Some(reason) = &run.skipped {
                let _ = writeln!(out, "    <testcase name=\"request\"><skipped message=\"{}\"/></testcase>", xml(reason));
            }
            for failure in &run.failures {
                let _ = writeln!(
                    out,
                    "    <testcase name=\"{} script\"><failure message=\"{}\"/></testcase>",
                    failure.phase.label(),
                    xml(&failure.message)
                );
            }
            for test in &run.tests {
                match test.status {
                    TestStatus::Passed => {
                        let _ = writeln!(out, "    <testcase name=\"{}\"/>", xml(&test.name));
                    }
                    TestStatus::Failed => {
                        let _ = writeln!(
                            out,
                            "    <testcase name=\"{}\"><failure message=\"{}\"/></testcase>",
                            xml(&test.name),
                            xml(test.message.as_deref().unwrap_or("failed"))
                        );
                    }
                    TestStatus::Skipped => {
                        let _ = writeln!(out, "    <testcase name=\"{}\"><skipped/></testcase>", xml(&test.name));
                    }
                }
            }
            out.push_str("  </testsuite>\n");
        }
    }
    out.push_str("</testsuites>\n");
    out
}

fn seconds(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

/// Escape for XML/HTML text and attribute values; drops control characters XML forbids.
fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

const STYLE: &str = "body{font:14px/1.5 system-ui,sans-serif;margin:24px;color:#1f2937}h1{font-size:20px}\
details{border:1px solid #d1d5db;border-radius:6px;margin:6px 0;padding:6px 10px}summary{cursor:pointer}\
.pass{color:#2e7d32}.fail{color:#c62828}.muted{color:#6b7280}code{font-family:ui-monospace,monospace}\
table{border-collapse:collapse}td,th{padding:2px 12px;text-align:right;border-bottom:1px solid #e5e7eb}";

/// One self-contained HTML file: inline style, no script, no external URL.
pub fn html(result: &RunResult) -> String {
    let totals = &result.totals;
    let verdict = if result.passed() { "<span class=\"pass\">passed</span>" } else { "<span class=\"fail\">failed</span>" };
    let mut out = String::from("<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">");
    let _ = write!(out, "<title>{} - run report</title><style>{STYLE}</style></head><body>", xml(&result.collection_name));
    let _ = writeln!(out, "<h1>{} - {verdict}</h1>", xml(&result.collection_name));
    let _ =
        writeln!(out, "<p class=\"muted\">{} iteration(s), {} request(s), {} ms</p>", result.iterations.len(), totals.requests, result.duration_ms());
    let _ = writeln!(
        out,
        "<table><tr><th></th><th>passed</th><th>failed</th><th>skipped</th></tr><tr><th>tests</th><td>{}</td><td>{}</td><td>{}</td></tr><tr><th>requests</th><td>{}</td><td>{}</td><td>{}</td></tr></table>",
        totals.tests_passed,
        totals.tests_failed,
        totals.tests_skipped,
        totals.requests.saturating_sub(totals.failed_requests),
        totals.failed_requests,
        totals.skipped_requests
    );
    if let Some(reason) = &result.stopped {
        let _ = writeln!(out, "<p class=\"fail\">Stopped: {}</p>", xml(reason));
    }
    for iteration in &result.iterations {
        let _ = writeln!(out, "<h2>Iteration {}</h2>", iteration.index.saturating_add(1));
        for run in &iteration.requests {
            let class = if run.failed() { "fail" } else { "pass" };
            let status = run.status.map(|status| status.to_string()).unwrap_or_else(|| "-".to_string());
            let _ = writeln!(
                out,
                "<details><summary class=\"{class}\">{} <span class=\"muted\">{} {} - {status} - {} ms</span></summary>",
                xml(&run.name),
                xml(&run.method),
                xml(&run.url),
                run.duration_ms
            );
            if let Some(reason) = &run.skipped {
                let _ = writeln!(out, "<p class=\"muted\">skipped: {}</p>", xml(reason));
            }
            if let Some(error) = &run.error {
                let _ = writeln!(out, "<p class=\"fail\">{}</p>", xml(error));
            }
            for failure in &run.failures {
                let _ =
                    writeln!(out, "<p class=\"fail\">{} script in {}: {}</p>", failure.phase.label(), xml(&failure.origin), xml(&failure.message));
            }
            out.push_str("<ul>\n");
            for test in &run.tests {
                let (class, mark) = match test.status {
                    TestStatus::Passed => ("pass", "PASS"),
                    TestStatus::Failed => ("fail", "FAIL"),
                    TestStatus::Skipped => ("muted", "SKIP"),
                };
                let message = test.message.as_ref().map(|message| format!(" - {}", xml(message))).unwrap_or_default();
                let _ = writeln!(out, "<li class=\"{class}\">{mark} {}{message}</li>", xml(&test.name));
            }
            out.push_str("</ul></details>\n");
        }
    }
    out.push_str("</body></html>\n");
    out
}
