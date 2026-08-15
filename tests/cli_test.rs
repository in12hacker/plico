//! Black-box checks for the typed `plico.personal.v2` CLI boundary.

use std::process::{Command, Output};

use tempfile::tempdir;

fn run(root: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aicli"))
        .args(["--embedded", "--root"])
        .arg(root)
        .args(args)
        .env("EMBEDDING_BACKEND", "stub")
        .env("LLM_BACKEND", "stub")
        .env("RUST_LOG", "off")
        .output()
        .expect("run aicli")
}

fn success_json(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "aicli failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("typed JSON response")
}

fn result<'a>(response: &'a serde_json::Value, operation: &str) -> &'a serde_json::Value {
    assert_eq!(response["protocol"], "plico.personal.v2");
    assert_eq!(response["ok"], true);
    assert_eq!(response["data"]["operation"], operation);
    &response["data"]["result"]
}

#[test]
fn catalog_and_object_roundtrip_use_the_typed_surface() {
    let root = tempdir().unwrap();
    let catalog = success_json(run(root.path(), &["capabilities.describe"]));
    let operations = result(&catalog, "capabilities.describe")["operations"]
        .as_array()
        .unwrap();
    assert_eq!(operations.len(), 14);
    assert!(!operations.iter().any(|operation| operation == "agent.register"));

    let put = success_json(run(
        root.path(),
        &["object.put", "--content", "canonical searchable note", "--tag", "note"],
    ));
    let cid = result(&put, "object.put")["cid"].as_str().unwrap().to_string();

    let get = success_json(run(root.path(), &["object.get", "--cid", &cid]));
    assert_eq!(result(&get, "object.get")["cid"], cid);

    let search = success_json(run(
        root.path(),
        &["object.search", "--query", "searchable note", "--limit", "5"],
    ));
    let search_result = result(&search, "object.search");
    assert!(search_result["hits"]
        .as_array()
        .unwrap()
        .iter()
        .any(|hit| hit["cid"] == cid));
    assert!(search_result["retrieval"].is_array());
}

#[test]
fn working_memory_correction_and_forgetting_survive_process_boundaries() {
    let root = tempdir().unwrap();
    let created = success_json(run(
        root.path(),
        &[
            "memory.create",
            "--content",
            "the meeting is Friday",
            "--tag",
            "calendar",
        ],
    ));
    let original = result(&created, "memory.create")["entry"]["entry_id"]
        .as_str()
        .unwrap()
        .to_string();

    let recalled = success_json(run(root.path(), &["memory.recall", "--query", "meeting Friday"]));
    assert!(result(&recalled, "memory.recall")["hits"]
        .as_array()
        .unwrap()
        .iter()
        .any(|hit| hit["entry"]["entry_id"] == original));

    let updated = success_json(run(
        root.path(),
        &[
            "memory.update",
            "--entry-id",
            &original,
            "--content",
            "the meeting is Saturday",
        ],
    ));
    let current = result(&updated, "memory.update")["entry"]["entry_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(current, original);

    let deleted = success_json(run(root.path(), &["memory.delete", "--entry-id", &current]));
    assert_eq!(result(&deleted, "memory.delete")["entry_id"], current);

    let missing = run(root.path(), &["memory.get", "--entry-id", &current]);
    assert_eq!(missing.status.code(), Some(1));
    let response: serde_json::Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(response["error"]["code"], "NOT_FOUND");
}

#[test]
fn cli_rejects_identity_injection_and_legacy_commands_without_state_change() {
    let root = tempdir().unwrap();
    let forged = run(
        root.path(),
        &["memory.create", "--content", "must not persist", "--agent", "forged"],
    );
    assert_eq!(forged.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&forged.stderr).contains("unexpected arguments"));

    let legacy = run(root.path(), &["remember", "--content", "legacy"]);
    assert_eq!(legacy.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&legacy.stderr).contains("unsupported operation"));

    let recalled = success_json(run(root.path(), &["memory.recall", "--query", "persist"]));
    assert!(result(&recalled, "memory.recall")["hits"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn session_boundary_returns_real_typed_watermarks() {
    let root = tempdir().unwrap();
    let started = success_json(run(root.path(), &["session.start"]));
    let start = result(&started, "session.start");
    let session_id = start["session_id"].as_str().unwrap().to_string();
    let watermark = start["watermark"].as_u64().unwrap();

    let ended = success_json(run(root.path(), &["session.end", "--session-id", &session_id]));
    assert!(result(&ended, "session.end")["last_seq"].as_u64().unwrap() >= watermark);
}
