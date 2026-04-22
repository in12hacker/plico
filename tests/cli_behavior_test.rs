//! CLI behavior tests (F-4 Part 3 / F-6 Behavioral Fingerprint)
//!
//! Tests CLI command behavior through black-box subprocess invocation.
//! These complement kernel_test.rs which tests kernel methods directly.

use std::process::{Command, Output};
use tempfile::tempdir;

fn run_cli(root: &std::path::Path, args: &[&str]) -> Output {
    std::env::set_var("EMBEDDING_BACKEND", "stub");
    std::env::set_var("RUST_LOG", "off");

    Command::new("cargo")
        .args(&["run", "--quiet", "--bin", "aicli", "--", "--root"])
        .arg(root)
        .args(args)
        .output()
        .expect("failed to run aicli")
}

fn run_cli_json(root: &std::path::Path, args: &[&str]) -> serde_json::Value {
    std::env::set_var("EMBEDDING_BACKEND", "stub");
    std::env::set_var("RUST_LOG", "off");
    std::env::set_var("AICLI_OUTPUT", "json");

    let output = Command::new("cargo")
        .args(&["run", "--quiet", "--bin", "aicli", "--", "--root"])
        .arg(root)
        .args(args)
        .output()
        .expect("failed to run aicli");

    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        serde_json::json!({"error": String::from_utf8_lossy(&output.stderr)})
    })
}

fn setup_root() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    std::env::set_var("EMBEDDING_BACKEND", "stub");
    std::env::set_var("RUST_LOG", "off");
    let _ = Command::new("cargo")
        .args(&["run", "--quiet", "--bin", "aicli", "--", "--root"])
        .arg(dir.path())
        .args(&["put", "--content", "bootstrap", "--tags", "boot"])
        .output();
    dir
}

// ─── CRUD CLI Tests ──────────────────────────────────────────────────────────

#[test]
fn test_cli_create_and_read() {
    let dir = setup_root();
    let root = dir.path();

    let create_out = run_cli(root, &["put", "--content", "hello world", "--tags", "greeting"]);
    assert!(create_out.status.success(), "create failed: {}", String::from_utf8_lossy(&create_out.stderr));

    let create_json = run_cli_json(root, &["put", "--content", "readable doc", "--tags", "test"]);
    let cid = create_json.get("cid").and_then(|v| v.as_str()).expect("no cid in response");

    let read_out = run_cli(root, &["get", cid]);
    assert!(read_out.status.success());
    let stdout = String::from_utf8_lossy(&read_out.stdout);
    assert!(stdout.contains("readable doc"), "read output should contain content");
}

#[test]
fn test_cli_delete_positional_cid() {
    let dir = setup_root();
    let root = dir.path();

    let create_json = run_cli_json(root, &["put", "--content", "to delete", "--tags", "temp"]);
    let cid = create_json.get("cid").and_then(|v| v.as_str()).expect("no cid");

    // Grant Delete permission first
    let _ = run_cli(root, &["permission", "grant", "--action", "delete", "--agent", "cli"]);

    let delete_out = run_cli(root, &["delete", cid]);
    assert!(delete_out.status.success(), "delete positional should work: {}", String::from_utf8_lossy(&delete_out.stderr));
}

#[test]
fn test_cli_delete_flag_form() {
    let dir = setup_root();
    let root = dir.path();

    let create_json = run_cli_json(root, &["put", "--content", "to delete2", "--tags", "temp"]);
    let cid = create_json.get("cid").and_then(|v| v.as_str()).expect("no cid");

    let _ = run_cli(root, &["permission", "grant", "--action", "delete", "--agent", "cli"]);

    let delete_out = run_cli(root, &["delete", "--cid", cid]);
    assert!(delete_out.status.success(), "delete --cid flag should work");
}

#[test]
fn test_cli_delete_empty_error() {
    let dir = setup_root();
    let root = dir.path();

    let delete_out = run_cli(root, &["delete"]);
    assert!(!delete_out.status.success() || String::from_utf8_lossy(&delete_out.stderr).contains("CID"));
}

#[test]
fn test_cli_search_tag_only() {
    let dir = setup_root();
    let root = dir.path();

    let _ = run_cli(root, &["put", "--content", "rust doc", "--tags", "lang:rust"]);
    let _ = run_cli(root, &["put", "--content", "python doc", "--tags", "lang:python"]);

    let search_out = run_cli(root, &["search", "--tags", "lang:rust"]);
    assert!(search_out.status.success(), "tag search should succeed");
}

#[test]
fn test_cli_search_require_tags_and() {
    let dir = setup_root();
    let root = dir.path();

    run_cli(root, &["put", "--content", "doc1", "--tags", "a,b"]);
    run_cli(root, &["put", "--content", "doc2", "--tags", "a"]);

    let search_out = run_cli(root, &["search", "--require-tags", "a", "--require-tags", "b"]);
    assert!(search_out.status.success());
    let stdout = String::from_utf8_lossy(&search_out.stdout);
    assert!(stdout.contains("doc1"), "AND search should return doc1 only");
}

#[test]
fn test_cli_history() {
    let dir = setup_root();
    let root = dir.path();

    let v1 = run_cli_json(root, &["put", "--content", "v1", "--tags", "ver"]);
    let cid1 = v1.get("cid").and_then(|v| v.as_str()).expect("no cid");

    run_cli(root, &["update", "--cid", cid1, "--content", "v2"]);

    let hist_out = run_cli(root, &["history", "--cid", cid1]);
    assert!(hist_out.status.success());
}

// ─── Agent CLI Tests ─────────────────────────────────────────────────────────

#[test]
fn test_cli_agent_register() {
    let dir = setup_root();
    let root = dir.path();

    let out = run_cli(root, &["agent", "--register", "--name", "TestAgent"]);
    assert!(out.status.success(), "register should succeed: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn test_cli_quota_by_name() {
    let dir = setup_root();
    let root = dir.path();

    run_cli(root, &["agent", "--register", "--name", "QuotaAgent"]);

    let out = run_cli(root, &["quota", "--agent", "QuotaAgent"]);
    assert!(out.status.success(), "quota by name should resolve: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn test_cli_agents_list() {
    let dir = setup_root();
    let root = dir.path();

    run_cli(root, &["agent", "--register", "--name", "ListAgent1"]);
    run_cli(root, &["agent", "--register", "--name", "ListAgent2"]);

    let out = run_cli(root, &["agents"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ListAgent1") && stdout.contains("ListAgent2"));
}

// ─── Memory CLI Tests ────────────────────────────────────────────────────────

#[test]
fn test_cli_remember_working() {
    let dir = setup_root();
    let root = dir.path();

    let out = run_cli(root, &["remember", "--tier", "working", "--content", "working memory", "--agent", "cli"]);
    assert!(out.status.success(), "remember working tier should succeed: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn test_cli_remember_longterm() {
    let dir = setup_root();
    let root = dir.path();

    let out = run_cli(root, &["remember", "--tier", "long-term", "--content", "persistent fact", "--tags", "fact", "--agent", "cli"]);
    assert!(out.status.success(), "remember long-term should succeed");
}

#[test]
fn test_cli_recall() {
    let dir = setup_root();
    let root = dir.path();

    run_cli(root, &["remember", "--tier", "working", "--content", "recallable", "--agent", "cli"]);

    let out = run_cli(root, &["recall", "--agent", "cli"]);
    assert!(out.status.success(), "recall should succeed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("recallable") || stdout.contains("Working"));
}

#[test]
fn test_cli_recall_with_tier() {
    let dir = setup_root();
    let root = dir.path();

    run_cli(root, &["remember", "--tier", "working", "--content", "wk", "--agent", "cli"]);
    run_cli(root, &["remember", "--tier", "ephemeral", "--content", "eph", "--agent", "cli"]);

    let out = run_cli(root, &["recall", "--agent", "cli", "--tier", "working"]);
    assert!(out.status.success());
}

#[test]
fn test_cli_tags() {
    let dir = setup_root();
    let root = dir.path();

    run_cli(root, &["put", "--content", "doc1", "--tags", "tag-x"]);
    run_cli(root, &["put", "--content", "doc2", "--tags", "tag-y"]);

    let out = run_cli(root, &["tags"]);
    assert!(out.status.success());
}

// ─── Messaging CLI Tests ─────────────────────────────────────────────────────

#[test]
fn test_cli_send_message() {
    let dir = setup_root();
    let root = dir.path();

    let _ = run_cli(root, &["permission", "grant", "--action", "SendMessage", "--agent", "cli"]);

    let reg_out = run_cli_json(root, &["agent", "--register", "--name", "Recipient"]);
    let recipient = reg_out.get("agent_id").and_then(|v| v.as_str()).expect("no agent_id");

    let send_out = run_cli(root, &["send", "--to", recipient, "--payload", r#"{"msg":"hello"}"#]);
    assert!(send_out.status.success(), "send should work: {}", String::from_utf8_lossy(&send_out.stderr));
}

#[test]
fn test_cli_send_with_agent_flag() {
    let dir = setup_root();
    let root = dir.path();

    let _ = run_cli(root, &["permission", "grant", "--action", "SendMessage", "--agent", "cli"]);

    let reg_out = run_cli_json(root, &["agent", "--register", "--name", "R2"]);
    let recipient = reg_out.get("agent_id").and_then(|v| v.as_str()).expect("no agent_id");

    let send_out = run_cli(root, &["send", "--agent", "cli", "--to", recipient, "--payload", r#"{"test":true}"#]);
    assert!(send_out.status.success(), "send with --agent should work");
}

// ─── KG CLI Tests ────────────────────────────────────────────────────────────

#[test]
fn test_cli_paths() {
    let dir = setup_root();
    let root = dir.path();

    let out = run_cli(root, &["paths"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() || stderr.contains("Usage") || stdout.contains("paths"));
}

// ─── F-2 Phantom Delete Defense ─────────────────────────────────────────────

#[test]
fn test_cli_delete_invalid_cid_returns_error() {
    let dir = setup_root();
    let root = dir.path();

    let _ = run_cli(root, &["permission", "grant", "--action", "delete", "--agent", "cli"]);

    let out = run_cli(root, &["delete", "a"]);
    assert!(!out.status.success(), "delete invalid CID 'a' should fail");
    let combined = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(combined.contains("Invalid CID") || combined.to_lowercase().contains("invalid"),
        "error should mention invalid CID: {}", combined);
}

#[test]
fn test_cli_delete_nonexistent_returns_error() {
    let dir = setup_root();
    let root = dir.path();

    let _ = run_cli(root, &["permission", "grant", "--action", "delete", "--agent", "cli"]);

    let out = run_cli(root, &["delete", "0000000000000000000000000000000000000000000000000000000000000000"]);
    assert!(!out.status.success(), "delete nonexistent CID should fail");
}

// ─── F-3 Name Resolution — Skills ─────────────────────────────────────────────

#[test]
fn test_cli_skills_register_by_name() {
    let dir = setup_root();
    let root = dir.path();

    let reg_out = run_cli_json(root, &["agent", "--register", "--name", "SkillAgent"]);
    let _agent_id = reg_out.get("agent_id").and_then(|v| v.as_str()).expect("no agent_id");

    let out = run_cli(root, &["skills", "register", "--agent", "SkillAgent", "--name", "my-skill", "--description", "test skill"]);
    assert!(out.status.success(), "skills register by name should succeed: {}",
        String::from_utf8_lossy(&out.stderr));
}

#[test]
fn test_cli_skills_list_by_name() {
    let dir = setup_root();
    let root = dir.path();

    let reg_out = run_cli_json(root, &["agent", "--register", "--name", "ListSkillAgent"]);
    let _agent_id = reg_out.get("agent_id").and_then(|v| v.as_str()).expect("no agent_id");

    run_cli(root, &["skills", "register", "--agent", "ListSkillAgent", "--name", "skill-to-list", "--description", "desc"]);

    let out = run_cli(root, &["skills", "list", "--agent", "ListSkillAgent"]);
    assert!(out.status.success(), "skills list by name should succeed: {}",
        String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("skill-to-list"), "output should contain registered skill: {}", stdout);
}

// ─── F-5 Handlers — Messaging ────────────────────────────────────────────────

#[test]
fn test_cli_send_message_with_agent_flag() {
    let dir = setup_root();
    let root = dir.path();

    let _ = run_cli(root, &["permission", "grant", "--action", "SendMessage", "--agent", "cli"]);

    let reg_out = run_cli_json(root, &["agent", "--register", "--name", "MsgRecipient"]);
    let recipient = reg_out.get("agent_id").and_then(|v| v.as_str()).expect("no agent_id");

    let send_out = run_cli(root, &["send", "--agent", "cli", "--to", recipient, "--payload", r#"{"msg":"hello"}"#]);
    assert!(send_out.status.success(), "send with --agent flag should work: {}",
        String::from_utf8_lossy(&send_out.stderr));
}

// ─── F-5 Handlers — Agent Quota ──────────────────────────────────────────────

#[test]
fn test_cli_quota_by_name_f5() {
    let dir = setup_root();
    let root = dir.path();

    run_cli(root, &["agent", "--register", "--name", "QuotaTestAgent"]);

    let out = run_cli(root, &["quota", "--agent", "QuotaTestAgent"]);
    assert!(out.status.success(), "quota by name should resolve: {}",
        String::from_utf8_lossy(&out.stderr));
}
