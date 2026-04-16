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

pub mod cas;
pub mod memory;
pub mod scheduler;
pub mod fs;
pub mod kernel;
pub mod api;
pub mod temporal;

// Permission re-exports for ergonomic access
pub use api::permission::{PermissionGuard, PermissionContext, PermissionAction, PermissionGrant};

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
