//! Plico — AI-Native Operating System
//!
//! A complete OS designed from AI perspective. No human CLI/GUI.
//! All data operations via semantic APIs for AI agents.
//!
//! # Architecture
//!
//! - [`cas`] — Content-Addressed Storage (SHA-256 object store)
//! - [`memory`] — Layered memory management (4-tier cognitive hierarchy)
//! - [`scheduler`] — Agent lifecycle scheduler (priority-based dispatch)
//! - [`fs`] — Semantic filesystem (CRUD, vector index, knowledge graph)
//! - [`kernel`] — AI Kernel (orchestrates all subsystems)
//! - [`api`] — AI-friendly semantic API (permission + semantic protocol)
//! - [`temporal`] — Temporal reasoning (natural language time → time ranges)

pub mod api;
pub mod cas;
pub mod client;
pub mod config;
pub mod fs;
pub mod intent;
pub mod kernel;
pub mod llm;
pub mod mcp;
pub mod memory;
pub mod prompt;
pub mod scheduler;
pub mod temporal;
pub mod tool;
pub mod util;

/// Default tenant ID used when no tenant is specified.
pub const DEFAULT_TENANT: &str = "default";

/// Stable trusted role identity of the owner of the personal vault.
pub const PERSONAL_OWNER_ROLE_ID: &str = "personal-owner";

/// Serializes only scoped tracing-capture tests because tracing callsite
/// interest caches are process-global. Poison is explicitly recoverable so a
/// failing canary cannot cascade into unrelated tests.
#[cfg(test)]
pub(crate) static TRACE_CAPTURE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// Permission re-exports for ergonomic access
pub use api::permission::{PermissionAction, PermissionContext, PermissionGrant, PermissionGuard};

pub use cas::object::{AIObject, AIObjectMeta};
pub use cas::storage::CASStorage;
pub use kernel::AIKernel;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PlicoError {
    #[error("CAS error: {0}")]
    CAS(#[from] cas::CASError),

    #[error("Memory error: {0}")]
    Memory(#[from] memory::MemoryError),

    #[error("Scheduler error: {0}")]
    Scheduler(#[from] scheduler::SchedulerError),

    #[error("Filesystem error: {0}")]
    Filesystem(#[from] fs::FSError),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),
}

/// Result type for Plico operations
pub type Result<T> = std::result::Result<T, PlicoError>;
