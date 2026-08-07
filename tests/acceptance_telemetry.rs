//! Acceptance criteria for terminal-mcp's own telemetry (mcp-core#40).
//!
//! Each test is named after the criterion it holds, so a failing run names the
//! unmet requirement rather than a line number.

mod support;

use mcp_core::telemetry::metrics::{self, Label};
use serde_json::json;
use tracing::Level;

use support::capture_dispatch;

/// The argument value the level-contract test hunts for. It has the shape of
/// the thing that must never leak: a command line and a working directory a
/// caller supplied.
const SENTINEL: &str = "MARKER-terminal-secret-9f3d1c2a";

/// AC (epic D10 / mcp-core#40): no command line, argument, or working
/// directory reaches a span field or an INFO-or-louder line.
///
/// The same run proves the positive half too: a `terminal_execute` call opens
/// terminal-mcp's own `terminal.execute` span, so this test cannot pass simply
/// because nothing was instrumented.
#[test]
fn tool_call_records_no_arguments() {
    let dir = tempfile::tempdir().expect("tempdir for a sentinel-bearing cwd");
    let cwd_marker = dir.path().join(format!("cwd-{SENTINEL}"));
    std::fs::create_dir(&cwd_marker).expect("create sentinel-bearing cwd");
    let cwd = cwd_marker
        .to_str()
        .expect("tempdir path is utf8")
        .to_string();

    let recorded = capture_dispatch(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "terminal_execute",
                "arguments": {
                    "command": format!("echo {SENTINEL} 1>&2; echo {SENTINEL}"),
                    "cwd": cwd,
                    "args": [SENTINEL],
                },
            },
        }),
    ]);

    assert!(
        recorded.spans.iter().any(|s| s.name == "terminal.execute"),
        "a terminal_execute call must open a terminal.execute span; spans were {:?}",
        recorded.span_summary()
    );

    for span in &recorded.spans {
        for (key, value) in &span.fields {
            assert!(
                !value.contains(SENTINEL),
                "the sentinel leaked into span {:?} field {key:?}: {value:?}; all spans were {:?}",
                span.name,
                recorded.span_summary()
            );
        }
    }

    for event in &recorded.events {
        // DEBUG/TRACE may legitimately carry tool arguments (D10) -- that is
        // mcp-core's own dispatch layer, inherited rather than added here.
        // Only INFO and louder are checked.
        if event.level > Level::INFO {
            continue;
        }
        for (key, value) in &event.fields {
            assert!(
                !value.contains(SENTINEL),
                "the sentinel leaked into an INFO-or-louder event field {key:?}: {value:?}; \
                 all events were {:?}",
                recorded.event_summary()
            );
        }
    }
}

/// AC (mcp-core#40): a successful execution increments `terminal.execute`
/// labelled `outcome=ok`, and records its latency.
#[test]
fn terminal_execute_records_ok_outcome_metric() {
    let ok_labels = [Label::new("outcome", "ok")];
    let calls_before = counter_total("terminal.execute", &ok_labels);
    let duration_before = histogram_count("terminal.execute.duration", &[]);

    capture_dispatch(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "terminal_execute", "arguments": {"command": "true"}},
        }),
    ]);

    assert_eq!(
        counter_total("terminal.execute", &ok_labels),
        calls_before + 1,
        "a successful command must increment terminal.execute labelled outcome=ok"
    );
    assert!(
        histogram_count("terminal.execute.duration", &[]) > duration_before,
        "a completed execution must record its latency into terminal.execute.duration"
    );
}

/// AC (mcp-core#40): a nonzero exit is counted under its own bounded outcome
/// label, distinct from a genuine execution failure -- a shell exit status is
/// domain information mcp-core's own protocol-level outcome cannot see (a
/// nonzero exit is still a successful JSON-RPC call).
#[test]
fn terminal_execute_records_nonzero_exit_outcome_metric() {
    let labels = [Label::new("outcome", "nonzero_exit")];
    let before = counter_total("terminal.execute", &labels);

    capture_dispatch(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "terminal_execute", "arguments": {"command": "false"}},
        }),
    ]);

    assert_eq!(
        counter_total("terminal.execute", &labels),
        before + 1,
        "a nonzero exit must be counted under its own bounded outcome label"
    );
}

/// AC (mcp-core#40): a command that times out is counted under a `timeout`
/// outcome, distinct from both success and a nonzero exit.
#[test]
fn terminal_execute_records_timeout_outcome_metric() {
    let labels = [Label::new("outcome", "timeout")];
    let before = counter_total("terminal.execute", &labels);

    capture_dispatch(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "terminal_execute",
                "arguments": {"command": "sleep 10", "timeout_secs": 1},
            },
        }),
    ]);

    assert_eq!(
        counter_total("terminal.execute", &labels),
        before + 1,
        "a timed-out command must be counted under its own bounded outcome label"
    );
}

/// The lifetime total of one counter series, or zero when it has never been
/// recorded. The registry is process-wide, so every assertion here is a delta.
fn counter_total(name: &str, labels: &[Label]) -> u64 {
    metrics::global()
        .snapshot()
        .counters
        .iter()
        .find(|counter| counter.name == name && same_labels(&counter.labels, labels))
        .map_or(0, |counter| counter.total)
}

/// The lifetime measurement count of one histogram series.
fn histogram_count(name: &str, labels: &[Label]) -> u64 {
    metrics::global()
        .snapshot()
        .histograms
        .iter()
        .find(|histogram| histogram.name == name && same_labels(&histogram.labels, labels))
        .map_or(0, |histogram| histogram.total.count)
}

fn same_labels(recorded: &[Label], wanted: &[Label]) -> bool {
    recorded.len() == wanted.len()
        && wanted.iter().all(|want| {
            recorded
                .iter()
                .any(|have| have.key() == want.key() && have.value() == want.value())
        })
}
