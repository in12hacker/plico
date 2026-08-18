//! Shared support for MCP integration tests.
//!
//! The typed-client side spawns the real `plico-mcp` binary through the
//! production `McpClient` with a fresh `TempDir` as `PLICO_ROOT` and stub
//! embedding/LLM backends, so no test reaches the network or a real model.
//! The binary location comes from Cargo's official `CARGO_BIN_EXE_plico-mcp`
//! and therefore resolves correctly under any `CARGO_TARGET_DIR`.
//!
//! The raw JSON-RPC parity harness stays in `tests/mcp_test.rs` unchanged;
//! it owns its process lifecycle (explicit kill+wait on drop) deliberately
//! so the protocol-parity file keeps a self-contained, review-frozen diff.

use std::path::Path;

/// Compile-time absolute path of the `plico-mcp` binary. Build-machine
/// constant provided by Cargo for integration-test targets — never user
/// input.
pub const PLICO_MCP_BIN: &str = env!("CARGO_BIN_EXE_plico-mcp");

/// Spawns the typed `McpClient` against the real binary with the stubbed
/// environment. Dropping the client runs the production managed lifecycle:
/// transport closes (stdin EOF), a bounded grace period follows, then kill
/// if needed, and the child is always reaped.
pub fn typed_client(root: &Path) -> plico::mcp::McpClient {
    plico::mcp::McpClient::new(
        PLICO_MCP_BIN,
        &[],
        &[
            ("PLICO_ROOT", root.to_str().expect("utf-8 PLICO_ROOT path")),
            ("EMBEDDING_BACKEND", "stub"),
            ("LLM_BACKEND", "stub"),
        ],
    )
    .expect("typed MCP client over the real plico-mcp binary")
}

/// Counts zombie children of this test process with the given comm name.
/// A reaped child has no `/proc/<pid>` entry; an unreaped one persists as a
/// zombie (`Z` state).
#[cfg(target_os = "linux")]
pub fn count_zombie_children_named(name: &str) -> usize {
    let our_pid = std::process::id().to_string();
    let mut zombies = 0;
    for entry in std::fs::read_dir("/proc").expect("read /proc").flatten() {
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((before, rest)) = stat.rsplit_once(')') else {
            continue;
        };
        let Some(comm) = before.rsplit('(').next() else {
            continue;
        };
        let mut fields = rest.split_whitespace();
        let (Some(state), Some(ppid)) = (fields.next(), fields.next()) else {
            continue;
        };
        if comm == name && ppid == our_pid && state == "Z" {
            zombies += 1;
        }
    }
    zombies
}

/// Waits until no zombie child named `name` remains under this process.
/// Transient exit windows are tolerated by requiring three consecutive
/// clean scans 100 ms apart.
#[cfg(target_os = "linux")]
pub fn wait_for_zero_zombie_children(name: &str, deadline: std::time::Instant) {
    let mut clean_streak = 0;
    while clean_streak < 3 {
        assert!(
            std::time::Instant::now() < deadline,
            "zombie children named {name} persist past the deadline"
        );
        if count_zombie_children_named(name) == 0 {
            clean_streak += 1;
        } else {
            clean_streak = 0;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
