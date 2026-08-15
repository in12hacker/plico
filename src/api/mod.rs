//! API Layer — Permission Guardrails + Semantic JSON Protocol
//!
//! Provides the AI-facing interface: fine-grained permission checks
//! and a structured JSON request/response protocol over TCP or local CLI.

pub mod agent_auth;
pub mod dto;
#[cfg(feature = "offline-migration")]
pub mod offline_credentials;
pub mod permission;
pub mod public;
pub mod semantic;
pub mod version;

pub use agent_auth::{AgentAuthMode, AgentKeyStore, AgentToken};
#[cfg(feature = "offline-migration")]
pub use offline_credentials::{OfflineCredentialError, OfflineCredentialSet};
pub use permission::{PermissionAction, PermissionContext, PermissionGrant, PermissionGuard};
pub use public::*;
pub use version::ApiVersion;
