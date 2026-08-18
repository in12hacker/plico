//! Unit tests for the MCP client layer.
//!
//! No `plico-mcp` binary is spawned here: subprocess cross-validation lives
//! in `tests/mcp_client_test.rs` (integration targets receive Cargo's
//! official `CARGO_BIN_EXE_plico-mcp` location, which honors any
//! `CARGO_TARGET_DIR`). The lifecycle tests below exercise the managed
//! child state machine with fixed literal helper programs only.

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use serde_json::json;

    use crate::api::public::PUBLIC_OPERATIONS;
    use crate::kernel::AIKernel;
    use crate::tool::{ExternalToolProvider, ToolDescriptor, ToolResult};

    /// In-memory `ExternalToolProvider` exposing exactly the public catalog
    /// shape, so the kernel wiring is tested without a real subprocess and
    /// without exposing any new production API.
    struct FakeCatalogProvider;

    impl ExternalToolProvider for FakeCatalogProvider {
        fn provider_name(&self) -> &str {
            "fake-catalog"
        }

        fn discover_tools(&self) -> Vec<ToolDescriptor> {
            PUBLIC_OPERATIONS
                .iter()
                .map(|name| ToolDescriptor {
                    name: (*name).to_string(),
                    description: format!("fake {name}"),
                    schema: json!({ "type": "object" }),
                })
                .collect()
        }

        fn call_tool(&self, name: &str, _params: &serde_json::Value) -> ToolResult {
            ToolResult::ok(json!({ "echo": name }))
        }
    }

    #[test]
    fn kernel_add_tool_provider_uses_typed_names() {
        let provider: Arc<dyn ExternalToolProvider> = Arc::new(FakeCatalogProvider);
        let root = tempfile::TempDir::new().unwrap();
        let kernel = AIKernel::new(root.path().to_path_buf()).unwrap();

        let names = kernel.add_tool_provider(provider, "ext");
        assert_eq!(names.len(), PUBLIC_OPERATIONS.len());
        assert!(names.contains(&"ext.object.put".to_string()));
        assert!(names.contains(&"ext.memory.recall".to_string()));

        let handler = kernel
            .tool_registry
            .get_handler("ext.object.put")
            .expect("typed external handler should exist");
        let result = handler.execute(
            &json!({
                "content": "kernel integration test",
                "tags": ["kernel-test"],
            }),
            "local-cognitive-role",
        );
        assert!(result.success, "handler failed: {:?}", result.error);
    }

    #[test]
    fn poisoned_transport_mutex_is_taken_without_panic() {
        use std::sync::Mutex;

        use crate::mcp::client::take_even_if_poisoned;

        let cell = Mutex::new(Some(7u8));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = cell.lock().unwrap();
            panic!("poison the transport mutex");
        }));
        assert!(cell.is_poisoned());
        assert_eq!(take_even_if_poisoned(&cell), Some(7));
        assert_eq!(take_even_if_poisoned(&cell), None);
    }
}

/// Managed-child lifecycle counterexamples. `/proc` is the reaping oracle: a
/// reaped pid has no `/proc/<pid>` entry, an unreaped one is a zombie ('Z').
///
/// Only two forks happen here, one per state-machine path: every `fork` in
/// the library test binary briefly stalls sibling threads and can trip the
/// pre-existing flock/reopen race in the projection suite (KNOWN_ISSUES D1
/// family). The heavier counterexamples (100x churn, construction-failure
/// loops, drop-only backstop at scale) live in the integration target.
#[cfg(all(test, target_os = "linux"))]
mod managed_child_lifecycle {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use crate::mcp::client::{ManagedChild, ManagedChildOutcome};

    fn spawn_cat() -> std::process::Child {
        let mut command = Command::new("cat");
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
        command.spawn().expect("spawn cat")
    }

    fn spawn_sleep_ignoring_eof() -> std::process::Child {
        let mut command = Command::new("sleep");
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
        command.arg("30");
        command.spawn().expect("spawn sleep")
    }

    fn process_state(pid: u32) -> Option<char> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat.rsplit_once(')')?.1.split_whitespace().next()?.chars().next()
    }

    #[test]
    fn managed_child_exits_gracefully_on_eof_and_is_reaped() {
        let child = spawn_cat();
        let pid = child.id();
        let mut managed = ManagedChild::new(child, Duration::from_secs(3));

        assert_eq!(
            managed.shutdown_and_reap(),
            Some(ManagedChildOutcome::ExitedDuringGrace)
        );
        assert_eq!(managed.shutdown_and_reap(), None, "shutdown is idempotent");
        assert!(process_state(pid).is_none(), "cat must be reaped, not left as a zombie");
    }

    #[test]
    fn managed_child_kills_and_reaps_when_eof_is_ignored() {
        let child = spawn_sleep_ignoring_eof();
        let pid = child.id();
        let mut managed = ManagedChild::new(child, Duration::from_millis(50));
        assert_eq!(
            managed.shutdown_and_reap(),
            Some(ManagedChildOutcome::KilledAfterTimeout)
        );
        assert!(
            process_state(pid).is_none(),
            "killed sleep must be reaped, not left as a zombie"
        );
    }
}
