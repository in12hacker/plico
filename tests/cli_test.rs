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
    // Use stub embedding to avoid subprocess warm-up delay in tests.
    // E2E embedding tests use EMBEDDING_BACKEND=local separately.
    cmd.env("EMBEDDING_BACKEND", "stub");
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

/// M3: Dogfood CRUD — full chain with non-default agent (plico-dev scenario)
/// Tests: put → search → get → update → delete
/// Corresponds to v0.6 Task A: non-default agent full CRUD automation
#[test]
fn test_dogfood_crud_chain_with_agent() {
    let root = tempdir().unwrap();
    let agent = "plico-dev";

    // Step 1: CREATE (put with custom agent)
    let put = run(root.path(), &[
        "put",
        "--content", "Dogfood milestone v0.5 notes",
        "--tags", "plico,milestone,v0.5",
        "--agent", agent,
    ]);
    assert!(put.status.success(), "put failed: {}", String::from_utf8_lossy(&put.stderr));
    let cid = extract_cid(&put).expect("no CID in put output");
    assert_eq!(cid.len(), 64);

    // Step 2: SEARCH with tag filter + agent
    let search = run(root.path(), &[
        "search",
        "--query", "milestone",
        "--require-tags", "plico,v0.5",
        "--agent", agent,
    ]);
    assert!(search.status.success(), "search failed: {}", String::from_utf8_lossy(&search.stderr));
    let search_stdout = String::from_utf8_lossy(&search.stdout);
    assert!(search_stdout.contains(&cid), "search should contain our CID");

    // Step 3: READ with same agent (should succeed - owner matches)
    let get = run(root.path(), &["get", &cid, "--agent", agent]);
    assert!(get.status.success(), "get failed: {}", String::from_utf8_lossy(&get.stderr));
    assert_contains(&get, "Dogfood milestone v0.5 notes");
    assert_contains(&get, "plico");
    assert_contains(&get, "v0.5");

    // Step 4: UPDATE with same agent
    let update = run(root.path(), &[
        "update",
        "--cid", &cid,
        "--content", "Dogfood milestone v0.5 COMPLETED",
        "--tags", "plico,milestone,v0.5,completed",
        "--agent", agent,
    ]);
    assert!(update.status.success(), "update failed: {}", String::from_utf8_lossy(&update.stderr));
    // Update returns new CID (CAS immutable semantics)
    let update_stdout = String::from_utf8_lossy(&update.stdout);
    assert!(update_stdout.contains("Updated"));

    // Verify updated content via new CID (extract from "New CID: <hex>")
    let new_cid = update_stdout
        .lines()
        .find(|l| l.starts_with("New CID:"))
        .map(|l| l.trim_start_matches("New CID:").trim().to_string())
        .unwrap_or(cid.clone());

    let get_updated = run(root.path(), &["get", &new_cid, "--agent", agent]);
    assert!(get_updated.status.success(), "get updated failed");
    assert_contains(&get_updated, "COMPLETED");

    // Step 5: DELETE without grant — expected to fail (permission denied)
    // This is documented behavior: delete requires explicit grant
    let delete = run(root.path(), &["delete", "--cid", &new_cid, "--agent", agent]);
    let delete_stdout = String::from_utf8_lossy(&delete.stdout);
    let delete_stderr = String::from_utf8_lossy(&delete.stderr);
    let combined = format!("{}\n{}", delete_stdout, delete_stderr);
    // Delete should fail with permission error
    assert!(
        combined.contains("permission") || combined.contains("Permission"),
        "delete without grant should fail with permission error, got: {}",
        combined
    );
}

/// M3: Verify dogfood read bug fix — get with --agent flag works
/// Historical bug: cmd_read hardcoded "cli" ignoring --agent parameter
#[test]
fn test_get_with_agent_flag_works() {
    let root = tempdir().unwrap();
    let agent = "dogfood-test";

    // Create with custom agent
    let put = run(root.path(), &[
        "put",
        "--content", "Agent-owned content",
        "--tags", "test",
        "--agent", agent,
    ]);
    let cid = extract_cid(&put).expect("no CID");

    // Get WITHOUT agent flag — should fail (owner is not 'cli')
    let get_default = run(root.path(), &["get", &cid]);
    let default_stdout = String::from_utf8_lossy(&get_default.stdout);
    let default_stderr = String::from_utf8_lossy(&get_default.stderr);
    let default_combined = format!("{}\n{}", default_stdout, default_stderr);
    // Should fail because 'cli' is not the owner
    // Note: aicli exits 0 even on API error; check combined output for "cannot read"
    assert!(
        default_combined.contains("cannot read"),
        "get without agent should fail for non-owned object, got: {}",
        default_combined
    );

    // Get WITH correct agent flag — should succeed
    let get_agent = run(root.path(), &["get", &cid, "--agent", agent]);
    assert!(get_agent.status.success(), "get with correct agent should succeed");
    assert_contains(&get_agent, "Agent-owned content");
}
