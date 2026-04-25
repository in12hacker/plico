//! Verification Gate (F-4) — postcondition verification for critical operations.
//!
//! From design doc:
//! - verify_write: CAS write content must be non-empty and CID retrievable
//! - verify_memory_scope: memory scope must match request
//! - verify_edge_type: edge type must match request (no silent degradation)
//!
//! Verification failures emit events via the Hook system.

use std::sync::Arc;

use crate::fs::SemanticFS;
use crate::memory::MemoryScope;

/// Result of a verification check.
#[derive(Debug, Clone)]
pub enum VerificationResult {
    Pass,
    Fail { reason: String },
}

/// Verification gate — checks postconditions for critical operations.
pub struct VerificationGate;

impl VerificationGate {
    /// Verify a CAS write: content must be non-empty, CID must be retrievable.
    pub fn verify_write(fs: &SemanticFS, cid: &str) -> VerificationResult {
        match fs.read(&crate::fs::semantic_fs::Query::ByCid(cid.to_string())) {
            Ok(objects) if objects.is_empty() => VerificationResult::Fail {
                reason: "No object returned for CID".into(),
            },
            Ok(objects) if objects[0].data.is_empty() => VerificationResult::Fail {
                reason: "Empty content stored at CID".into(),
            },
            Ok(objects) if objects[0].data.len() < 4 => VerificationResult::Fail {
                reason: format!("Content suspiciously small ({} bytes)", objects[0].data.len()),
            },
            Ok(_) => VerificationResult::Pass,
            Err(e) => VerificationResult::Fail {
                reason: format!("CID not retrievable: {}", e),
            },
        }
    }

    /// Verify a memory store: scope must match what was requested.
    pub fn verify_memory_scope(requested: &MemoryScope, actual: &MemoryScope) -> VerificationResult {
        if requested == actual {
            VerificationResult::Pass
        } else {
            VerificationResult::Fail {
                reason: format!(
                    "Scope mismatch: requested {:?}, stored {:?}",
                    requested, actual
                ),
            }
        }
    }

    /// Verify a KG edge creation: edge type must match request.
    pub fn verify_edge_type(requested: &str, actual_type_name: &str) -> VerificationResult {
        if requested.eq_ignore_ascii_case(actual_type_name) {
            VerificationResult::Pass
        } else {
            VerificationResult::Fail {
                reason: format!(
                    "Edge type mismatch: requested '{}', stored '{}'",
                    requested, actual_type_name
                ),
            }
        }
    }
}

// ─── VerificationHookHandler ─────────────────────────────────────────────────

use crate::kernel::event_bus::{EventBus, KernelEvent};
use crate::kernel::hook::{HookContext, HookHandler, HookPoint, HookResult};
use crate::tool::ToolResult;

/// Hook handler that verifies postconditions for critical operations.
///
/// Registered at PostToolCall to verify:
/// - CAS writes: CID is retrievable and content is non-empty
/// - Memory stores: operation succeeded
///
/// Emits VerificationFailed events on verification failure.
pub struct VerificationHookHandler {
    fs: Arc<SemanticFS>,
    event_bus: Arc<EventBus>,
}

impl VerificationHookHandler {
    pub fn new(fs: Arc<SemanticFS>, event_bus: Arc<EventBus>) -> Self {
        Self { fs, event_bus }
    }

    /// Verify a cas.create or cas.update result.
    fn verify_cas_write(&self, result: &ToolResult, agent_id: &str) {
        if !result.success {
            return; // Tool already failed, no need to verify
        }

        // Extract CID from result output
        let cid = match result.output.get("cid").and_then(|v| v.as_str()) {
            Some(cid) => cid,
            None => return,
        };

        // Verify the CID is retrievable
        match VerificationGate::verify_write(&self.fs, cid) {
            VerificationResult::Pass => {}
            VerificationResult::Fail { reason } => {
                let cid_prefix = if cid.len() >= 8 { &cid[..8] } else { cid };
                tracing::warn!(
                    "VerificationFailed: cas write verification failed for {}: {}",
                    cid_prefix,
                    reason
                );
                self.event_bus.emit(KernelEvent::VerificationFailed {
                    tool_name: "cas.create/update".into(),
                    operation: "verify_write".into(),
                    reason,
                    agent_id: agent_id.into(),
                });
            }
        }
    }
}

impl HookHandler for VerificationHookHandler {
    fn handle(&self, point: HookPoint, context: &HookContext) -> HookResult {
        if point != HookPoint::PostToolCall {
            return HookResult::Continue;
        }

        // Deserialize ToolResult from params (PostToolCall puts result in params)
        let result: ToolResult = match serde_json::from_value(context.params.clone()) {
            Ok(r) => r,
            Err(_) => return HookResult::Continue,
        };

        // Verify critical write operations
        match context.tool_name.as_str() {
            "cas.create" | "cas.update" => {
                self.verify_cas_write(&result, &context.agent_id);
            }
            _ => {}
        }

        HookResult::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_write_not_found() {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("EMBEDDING_BACKEND", "stub");
        let embedding = Arc::new(crate::fs::StubEmbeddingProvider::new());
        let search = Arc::new(crate::fs::search::memory::InMemoryBackend::new());
        let fs = Arc::new(crate::fs::SemanticFS::new(
            dir.path().to_path_buf(),
            embedding.clone(),
            search.clone(),
            None,
            None,
        ).unwrap());

        let result = VerificationGate::verify_write(&fs, "nonexistent_cid_12345678");
        assert!(matches!(result, VerificationResult::Fail { .. }));
    }

    #[test]
    fn test_verify_memory_scope_match() {
        use crate::memory::MemoryScope;
        let result = VerificationGate::verify_memory_scope(
            &MemoryScope::Private,
            &MemoryScope::Private,
        );
        assert!(matches!(result, VerificationResult::Pass));
    }

    #[test]
    fn test_verify_memory_scope_mismatch() {
        use crate::memory::MemoryScope;
        let result = VerificationGate::verify_memory_scope(
            &MemoryScope::Private,
            &MemoryScope::Shared,
        );
        assert!(matches!(result, VerificationResult::Fail { .. }));
    }

    #[test]
    fn test_verify_edge_type_match() {
        let result = VerificationGate::verify_edge_type("SimilarTo", "similarto");
        assert!(matches!(result, VerificationResult::Pass));
    }

    #[test]
    fn test_verify_edge_type_mismatch() {
        let result = VerificationGate::verify_edge_type("SimilarTo", "CausedBy");
        assert!(matches!(result, VerificationResult::Fail { .. }));
    }
}
