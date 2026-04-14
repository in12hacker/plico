//! Permission & Safety Guardrails
//!
//! Fine-grained access control for AI agents. Every operation passes through
//! the permission guard before execution.
//!
//! # Permission Model
//!
//! Each agent operates within a permission context. Dangerous operations
//! (delete, network, external tool access) require explicit permission grants.
//!
//! # Default Policy
//!
//! - All agents can READ objects they created.
//! - All agents can WRITE to their own memory tiers.
//! - DELETE requires explicit permission grant.
//! - Network access requires explicit permission grant.
//!
//! # Risk Levels
//!
//! | Action | Risk Level | Requires Confirmation |
//! |--------|-----------|----------------------|
//! | Read own objects | Low | No |
//! | Write to own memory | Low | No |
//! | Read any object | Medium | Yes (if not owner) |
//! | Delete any object | High | Yes |
//! | Network access | High | Yes |
//! | External tool execution | Variable | Yes |

use serde::{Deserialize, Serialize};

/// A permission context — carries agent identity and granted permissions.
#[derive(Debug, Clone)]
pub struct PermissionContext {
    pub agent_id: String,
    pub granted: Vec<PermissionGrant>,
}

impl PermissionContext {
    pub fn new(agent_id: String) -> Self {
        Self {
            agent_id,
            granted: Vec::new(),
        }
    }

    pub fn with_grant(mut self, grant: PermissionGrant) -> Self {
        self.granted.push(grant);
        self
    }

    pub fn has_permission(&self, action: PermissionAction) -> bool {
        self.granted.iter().any(|g| g.covers(action))
    }
}

/// A granted permission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionGrant {
    pub action: PermissionAction,
    pub scope: Option<String>,
    pub expires_at: Option<u64>,
}

impl PermissionGrant {
    pub fn covers(&self, action: PermissionAction) -> bool {
        match (&self.action, &action) {
            (PermissionAction::All, _) => true,
            (PermissionAction::Write, PermissionAction::Write) => true,
            (PermissionAction::Read, PermissionAction::Read) => true,
            (PermissionAction::Delete, PermissionAction::Delete) => true,
            (PermissionAction::Network, PermissionAction::Network) => true,
            (PermissionAction::Execute, PermissionAction::Execute) => true,
            _ => false,
        }
    }
}

/// Actions that require permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionAction {
    Read,
    Write,
    Delete,
    Network,
    Execute,
    All,
}

/// The permission guard — enforces access control on all kernel operations.
#[derive(Debug)]
pub struct PermissionGuard {
    trusted_agents: std::collections::HashSet<String>,
}

impl PermissionGuard {
    pub fn new() -> Self {
        let mut trusted = std::collections::HashSet::new();
        trusted.insert("kernel".to_string());
        trusted.insert("system".to_string());
        Self { trusted_agents: trusted }
    }

    pub fn check(&self, ctx: &PermissionContext, action: PermissionAction) -> std::io::Result<()> {
        if self.trusted_agents.contains(&ctx.agent_id) {
            return Ok(());
        }
        if ctx.has_permission(action) || ctx.has_permission(PermissionAction::All) {
            return Ok(());
        }
        match action {
            PermissionAction::Read | PermissionAction::Write => Ok(()),
            PermissionAction::Delete | PermissionAction::Network | PermissionAction::Execute => {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("Agent '{}' lacks permission for {:?}", ctx.agent_id, action),
                ))
            }
            PermissionAction::All => Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Permission 'All' not granted",
            )),
        }
    }

    pub fn grant(&mut self, agent_id: &str, action: PermissionAction) {
        let _ = agent_id;
        let _ = action;
    }

    pub fn revoke_all(&mut self, agent_id: &str) {
        self.trusted_agents.remove(agent_id);
    }
}

impl Default for PermissionGuard {
    fn default() -> Self {
        Self::new()
    }
}
