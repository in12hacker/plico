//! Migration-A.1 executable corpus (A1-R04): MCP-A01..A12 as runnable
//! cases with deterministic oracles. Cases must be fast and green under
//! the clean build; each `mut-*` feature makes at least one case RED.
//! Cases run sequentially in this custom-main runner (fixture mode is
//! process-env selected).
//!
//! Runner: scripts/milestones/v54/harness/run_corpus.py — preflight and
//! formal modes share the same rule set.

use std::time::Duration;

use reference_adapter::{LineBoundedReader, ReferenceAdapter, TwinError, MAX_MCP_MESSAGE_BYTES};

const FIXTURE: &str = env!("CARGO_BIN_EXE_fixture");
const FAST: Duration = Duration::from_millis(600);

fn connect_ok(mode: &str) -> ReferenceAdapter {
    std::env::set_var("FIXTURE_MODE", mode);
    ReferenceAdapter::connect_with_timeouts(FIXTURE, FAST, FAST)
        .unwrap_or_else(|error| panic!("connect({mode}) failed: {error}"))
}

fn connect_err(mode: &str) -> TwinError {
    std::env::set_var("FIXTURE_MODE", mode);
    match ReferenceAdapter::connect_with_timeouts(FIXTURE, FAST, FAST) {
        Ok(_) => panic!("connect({mode}) must fail closed"),
        Err(error) => error,
    }
}

fn pid_gone(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_err()
}

fn a01_wrong_duplicate_unknown_id() {
    // The fixture answers a shifted id: the waiter must never receive it.
    let adapter = connect_ok("wrong-id");
    let pid = adapter.child_pid().expect("pid");
    assert_eq!(
        adapter
            .call_tool("object.put", &serde_json::json!({ "content": "x" }))
            .expect_err("mismatched id must not satisfy the caller"),
        TwinError::Deadline
    );
    assert_eq!(adapter.shutdown(), Some(pid), "wrong-id child must be reaped");
    assert!(pid_gone(pid), "wrong-id child must be gone");
    // dup-id: second copy and unknown-id responses have no waiter either.
    std::env::set_var("FIXTURE_MODE", "dup-id");
    let adapter = ReferenceAdapter::connect_with_timeouts(FIXTURE, FAST, FAST).expect("dup connect");
    let dup_pid = adapter.child_pid().expect("dup pid");
    assert_eq!(
        adapter
            .call_tool("object.put", &serde_json::json!({ "content": "x" }))
            .expect("first response resolves"),
        "ok:dup"
    );
    std::thread::sleep(Duration::from_millis(80));
    assert!(
        adapter
            .call_tool("runtime.readiness", &serde_json::json!({}))
            .expect("fresh call unaffected by duplicate id")
            .starts_with("ok:")
    );
    assert_eq!(adapter.shutdown(), Some(dup_pid));
    assert!(pid_gone(dup_pid));
}

fn a02_notifications_diverted() {
    let adapter = connect_ok("interleave");
    assert_eq!(
        adapter
            .call_tool("object.put", &serde_json::json!({ "content": "x" }))
            .expect("request resolves despite interleaved notification"),
        "ok:interleave"
    );
    assert!(
        adapter.notification_count() >= 1,
        "notification must be counted as diverted, never parsed as a response"
    );
}

fn a03_silent_initialize_rejected() {
    assert_eq!(connect_err("silent"), TwinError::Deadline);
}

fn a05_never_responding_deadline() {
    let adapter = connect_ok("never");
    assert_eq!(
        adapter
            .call_tool("object.put", &serde_json::json!({ "content": "x" }))
            .expect_err("deadline must fire"),
        TwinError::Deadline
    );
}

fn a06_late_response_quarantined() {
    std::env::set_var("FIXTURE_MODE", "late");
    // Deadline 300 ms: the first call times out before its 400 ms stale
    // response; the fresh call (sent at ~300 ms, answered at ~450 ms) is
    // still pending when the stale response arrives at ~400 ms.
    let adapter =
        ReferenceAdapter::connect_with_timeouts(FIXTURE, FAST, Duration::from_millis(300))
            .expect("late connect");
    assert_eq!(
        adapter
            .call_tool("object.put", &serde_json::json!({ "content": "x" }))
            .expect_err("late response must not beat the deadline"),
        TwinError::Deadline
    );
    let fresh = adapter
        .call_tool("runtime.readiness", &serde_json::json!({}))
        .expect("fresh request must resolve on its own response");
    assert_eq!(fresh, "ok:fresh", "stale payload leaked into the fresh call");
}

fn a07_stubborn_child_killed_and_reaped() {
    let adapter = connect_ok("stubborn");
    let pid = adapter.child_pid().expect("pid");
    let reaped = adapter.shutdown();
    assert_eq!(reaped, Some(pid), "shutdown must always reap");
    assert!(pid_gone(pid), "stubborn child must be killed, not left running");
}

fn a09_wire_failures() {
    let adapter = connect_ok("oversized");
    assert_eq!(
        adapter
            .call_tool("object.put", &serde_json::json!({ "content": "x" }))
            .expect_err("oversized line must fail closed before parse"),
        TwinError::WireCap
    );
    adapter.shutdown();
    assert!(
        matches!(connect_err("no-delimiter"), TwinError::Protocol(_)),
        "trailing bytes without delimiter must be a typed protocol failure"
    );
}

fn a09b_bounded_reader_boundaries() {
    let read = |data: &[u8]| {
        let mut reader = LineBoundedReader::new(data);
        reader.read_line_bounded()
    };
    assert!(read(format!("{}\n", "a".repeat(64)).as_bytes()).is_ok());
    assert!(read(format!("{}\n", "b".repeat(MAX_MCP_MESSAGE_BYTES)).as_bytes()).is_ok());
    assert_eq!(
        read(format!("{}\n", "c".repeat(MAX_MCP_MESSAGE_BYTES + 1)).as_bytes()),
        Err(TwinError::WireCap)
    );
    let mut reader = LineBoundedReader::new(b"{\"x\":1}\n".as_slice());
    assert!(reader.read_line_bounded().is_ok());
    assert!(matches!(read(b"{\"unterminated\""), Err(TwinError::Protocol(_))));
}

fn a12_catalog_drift_rejected() {
    // The fixture advertises 16 tools: the frozen catalog assertion must
    // reject the connection outright (this is the mut-loosen-exact14 kill).
    let error = connect_err("wide16");
    assert!(
        matches!(error, TwinError::Catalog(_)),
        "catalog drift must be rejected, got {error:?}"
    );
}

fn a12_exact14_equivalence() {
    let adapter = connect_ok("exact14");
    for name in reference_adapter::FROZEN_EXACT_14 {
        let payload = adapter
            .call_tool(name, &serde_json::json!({ "content": "x" }))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(payload.starts_with("ok:"), "{name}: {payload}");
    }
}

fn a07b_churn_1000_deterministic_reap() {
    for round in 0..1000 {
        std::env::set_var("FIXTURE_MODE", "exact14");
        let adapter =
            ReferenceAdapter::connect_with_timeouts(FIXTURE, FAST, FAST).unwrap_or_else(|e| panic!("round {round}: {e}"));
        let pid = adapter.child_pid().expect("pid");
        let reaped = adapter.shutdown();
        assert_eq!(reaped, Some(pid), "round {round}: reap must be deterministic");
        assert!(pid_gone(pid), "round {round}: zombie leaked");
    }
}

fn main() {
    let filter = std::env::args().nth(1).unwrap_or_default();
    let cases: [(&str, fn()); 11] = [
        ("a01", a01_wrong_duplicate_unknown_id),
        ("a02", a02_notifications_diverted),
        ("a03", a03_silent_initialize_rejected),
        ("a05", a05_never_responding_deadline),
        ("a06", a06_late_response_quarantined),
        ("a07", a07_stubborn_child_killed_and_reaped),
        ("a09", a09_wire_failures),
        ("a09b", a09b_bounded_reader_boundaries),
        ("a12d", a12_catalog_drift_rejected),
        ("a12", a12_exact14_equivalence),
        ("a07b", a07b_churn_1000_deterministic_reap),
    ];
    let mut executed = 0;
    let mut failed = 0;
    let mut skipped: Vec<&str> = Vec::new();
    for (name, case) in cases {
        if !filter.is_empty() && !name.contains(filter.as_str()) {
            skipped.push(name);
            continue;
        }
        executed += 1;
        print!("case {name}: ");
        let started = std::time::Instant::now();
        match std::panic::catch_unwind(case) {
            Ok(()) => println!("pass ({:?})", started.elapsed()),
            Err(_) => {
                failed += 1;
                println!("FAIL ({:?})", started.elapsed());
            }
        }
    }
    println!("summary: executed={executed} failed={failed} not-run={}", skipped.len());
    if failed > 0 {
        std::process::exit(1);
    }
}
