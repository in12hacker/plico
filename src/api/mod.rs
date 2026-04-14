//! API layer — permission and semantic interfaces

pub mod semantic;
pub mod permission;

pub use permission::{PermissionGuard, PermissionContext, PermissionAction, PermissionGrant};
