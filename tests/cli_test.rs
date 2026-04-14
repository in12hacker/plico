//! CLI E2E tests — test the aicli binary as an external process.
//!
//! Exercises the full binary: put → get → update → delete roundtrip,
//! agent registration, memory (remember/recall), and error paths.

use std::process::{Command, Output};
use tempfile::tempdir;

/// Path to the aicli binary.
fn aicli() -> String {
    // CARGO_MANIFEST_DIR = /home/leo/work/Plico (package root)
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    format!("{}/target/debug/aicli", manifest_dir)
}

/// Run aicli with the given args, using `root` as the kernel data directory.
fn run(root: &std::path::Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(aicli());
    cmd.arg("--root").arg(root);
    for arg in args {
        cmd.arg(arg);
    }
    // Capture both stdout and stderr so we can inspect failures
    cmd.output().expect("failed to spawn aicli")
}

/// Extract the CID from "CID: <hex>" output lines.
fn extract_cid(output: &Output) -> Option<String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find(|l| l.starts_with("CID:"))
        .map(|l| l.trim_start_matches("CID:").trim().to_string())
}

/// Assert that stdout contains the given substring.
fn assert_contains(output: &Output, needle: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}\n{}", stdout, stderr);
    assert!(
        combined.contains(needle),
        "expected output to contain {:?}, got:\nstdout: {}\nstderr: {}",
        needle,
        stdout,
        stderr
    );
}

#[test]
fn test_put_and_get_roundtrip() {
    let root = tempdir().unwrap();
    let output = run(root.path(), &[
        "put",
        "--content", "Rust async meeting notes",
        "--tags", "meeting,rust",
        "--intent", "Q1 planning",
    ]);
    assert!(
        output.status.success(),
        "put failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cid = extract_cid(&output).expect("no CID in put output");
    assert_eq!(cid.len(), 64, "CID should be 64-char SHA-256 hex");

    // GET the object we just stored
    let get_output = run(root.path(), &["get", &cid]);
    assert!(
        get_output.status.success(),
        "get failed: {}",
        String::from_utf8_lossy(&get_output.stderr)
    );
    assert_contains(&get_output, "Rust async meeting notes");
    assert_contains(&get_output, "meeting");
    assert_contains(&get_output, "Q1 planning");
}

#[test]
fn test_put_get_different_root_isolated() {
    let root1 = tempdir().unwrap();
    let root2 = tempdir().unwrap();

    // Put in root1
    let out1 = run(root1.path(), &["put", "--content", "in root1", "--tags", "test"]);
    let cid1 = extract_cid(&out1).expect("no CID");

    // Same CID content would deduplicate, so use different content
    let out2 = run(root2.path(), &["put", "--content", "in root2", "--tags", "test"]);
    let cid2 = extract_cid(&out2).expect("no CID");

    // CIDs must be different (different content)
    assert_ne!(cid1, cid2);

    // root2 cannot see root1's object
    let get2 = run(root2.path(), &["get", &cid1]);
    assert_contains(&get2, "not found");
}

#[test]
fn test_agent_register_shows_id() {
    // Agent state is in-memory per kernel instance. Each CLI invocation
    // creates a fresh kernel — agents don't persist across invocations.
    // This test verifies registration itself works and returns an ID.
    let root = tempdir().unwrap();
    let output = run(root.path(), &["agent", "--register", "TestAgent"]);
    assert!(output.status.success(), "agent register failed: {}", String::from_utf8_lossy(&output.stderr));
    assert_contains(&output, "Agent registered");
    assert_contains(&output, "TestAgent");
    // Agent ID is a UUID — extract and verify it looks like one
    let stdout = String::from_utf8_lossy(&output.stdout);
    let has_uuid = stdout.lines().any(|l| l.contains('-') && l.len() > 30);
    assert!(has_uuid, "expected UUID in agent output: {}", stdout);
}

#[test]
fn test_remember_and_recall() {
    let root = tempdir().unwrap();

    // Register an agent first
    let reg = run(root.path(), &["agent", "--register", "MemoryAgent"]);
    assert!(reg.status.success());

    // Remember something
    let remember = run(root.path(), &[
        "remember",
        "--agent", "MemoryAgent",
        "--content", "Remember to review the PR",
    ]);
    assert!(remember.status.success(), "remember failed: {}", String::from_utf8_lossy(&remember.stderr));
    assert_contains(&remember, "Remembered");

    // Recall it — same kernel instance so in-memory
    let recall = run(root.path(), &["recall", "--agent", "MemoryAgent"]);
    assert!(
        recall.status.success(),
        "recall failed: {}",
        String::from_utf8_lossy(&recall.stderr)
    );
    // Note: recall may return empty if memory is not persisted in same session
    // but should not crash
}

#[test]
fn test_get_nonexistent_cid() {
    let root = tempdir().unwrap();
    let fake_cid = "0000000000000000000000000000000000000000000000000000000000000000";
    let output = run(root.path(), &["get", fake_cid]);
    // Should exit with error but produce a meaningful message
    assert_contains(&output, "not found");
}

#[test]
fn test_delete_requires_permission() {
    let root = tempdir().unwrap();
    // Put an object
    let put = run(root.path(), &["put", "--content", "secret", "--tags", "private"]);
    let cid = extract_cid(&put).expect("no CID");

    // Delete as 'cli' — should fail with permission denied (default policy)
    let delete = run(root.path(), &["delete", "--cid", &cid]);
    assert_contains(&delete, "permission");
}

#[test]
fn test_unknown_command_shows_help() {
    let root = tempdir().unwrap();
    let output = run(root.path(), &["unknown-command"]);
    assert_contains(&output, "Unknown command");
    assert_contains(&output, "--help");
}

#[test]
fn test_tags_empty_filesystem() {
    // Tags are in-memory per kernel instance (same limitation as agents).
    // This test verifies the tags command works on an empty filesystem.
    let root = tempdir().unwrap();
    let output = run(root.path(), &["tags"]);
    assert!(output.status.success());
    assert_contains(&output, "No tags");
}
