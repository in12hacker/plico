//! API Layer — Permission Guardrails + Semantic JSON Protocol
//!
//! Provides the AI-facing interface: fine-grained permission checks
//! and a structured JSON request/response protocol over TCP or local CLI.

pub mod agent_auth;
pub mod semantic;
pub mod permission;

pub use agent_auth::{AgentKeyStore, AgentToken, AgentAuthMode};
pub use permission::{PermissionGuard, PermissionContext, PermissionAction, PermissionGrant};
pub use semantic::{
    SystemStatus,
    ApiVersion,
    DeprecationNotice,
    VersionFeatures,
    version_supports as check_version_feature,
    get_deprecation_notice as notice_for_request,
};
