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
//! - All agents can READ and WRITE by default (low risk).
//! - DELETE, NETWORK, EXECUTE require explicit permission grant.
//! - Trusted agents ("kernel", "system") bypass all checks.
//!
//! # Usage
//!
//! ```
//! let mut guard = PermissionGuard::new();
//! guard.grant("agent1", PermissionAction::Delete);
//! let ctx = PermissionContext::new("agent1".into());
//! guard.check(&ctx, PermissionAction::Delete)?; // OK
//! guard.check(&ctx, PermissionAction::Network)?; // Err: not granted
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A permission context — carries agent identity and granted permissions.
#[derive(Debug, Clone)]
pub struct PermissionContext {
    pub agent_id: String,
    /// Grants embedded in the context (e.g., from API request).
    pub embedded_grants: Vec<PermissionGrant>,
}

impl PermissionContext {
    pub fn new(agent_id: String) -> Self {
        Self {
            agent_id,
            embedded_grants: Vec::new(),
        }
    }

    /// Create a context with embedded grants (from API call).
    pub fn with_grants(agent_id: String, grants: Vec<PermissionGrant>) -> Self {
        Self {
            agent_id,
            embedded_grants: grants,
        }
    }

    /// Check if this context has permission for the given action.
    pub fn has_permission(&self, action: PermissionAction) -> bool {
        self.embedded_grants.iter().any(|g| g.covers(action))
    }
}

/// A granted permission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionGrant {
    pub action: PermissionAction,
    /// Optional scope restriction (e.g., specific CID, tag pattern).
    pub scope: Option<String>,
    /// Expiration timestamp (ms), None = never expires.
    pub expires_at: Option<u64>,
}

impl PermissionGrant {
    /// Check if this grant covers the requested action.
    pub fn covers(&self, action: PermissionAction) -> bool {
        if let Some(expiry) = self.expires_at {
            if now_ms() > expiry {
                return false; // Grant expired
            }
        }
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

    /// Create a new grant for an action with no expiry.
    pub fn new(action: PermissionAction) -> Self {
        Self {
            action,
            scope: None,
            expires_at: None,
        }
    }

    /// Create a grant with a scope restriction.
    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    /// Create a grant that expires at the given timestamp (ms).
    pub fn with_expiry(mut self, expiry_ms: u64) -> Self {
        self.expires_at = Some(expiry_ms);
        self
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

/// The permission guard — global access control registry.
#[derive(Debug)]
pub struct PermissionGuard {
    /// System-level trusted agents — bypass all permission checks.
    trusted_agents: std::collections::HashSet<String>,
    /// Persistent grants per agent.
    grants: HashMap<String, Vec<PermissionGrant>>,
}

impl PermissionGuard {
    pub fn new() -> Self {
        let mut trusted = std::collections::HashSet::new();
        trusted.insert("kernel".to_string());
        trusted.insert("system".to_string());
        Self {
            trusted_agents: trusted,
            grants: HashMap::new(),
        }
    }

    /// Check if the agent has permission for the action.
    ///
    /// Checks in order:
    /// 1. Trusted agent bypass
    /// 2. Embedded grants in context
    /// 3. Global grants in guard registry
    ///
    /// Returns `Ok(())` if allowed, `Err` if denied.
    pub fn check(&self, ctx: &PermissionContext, action: PermissionAction) -> std::io::Result<()> {
        // 1. Trusted agents bypass all checks
        if self.trusted_agents.contains(&ctx.agent_id) {
            return Ok(());
        }

        // 2. Check embedded grants in context
        if ctx.has_permission(action) {
            return Ok(());
        }

        // 3. Check global grants registry
        if let Some(grants) = self.grants.get(&ctx.agent_id) {
            if grants.iter().any(|g| g.covers(action)) {
                return Ok(());
            }
        }

        // Default policy: Read and Write are allowed by default
        match action {
            PermissionAction::Read | PermissionAction::Write => Ok(()),
            PermissionAction::Delete | PermissionAction::Network | PermissionAction::Execute => {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "Agent '{}' lacks permission for {:?}. Grant it first: grant(..., {:?})",
                        ctx.agent_id, action, action
                    ),
                ))
            }
            PermissionAction::All => Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Permission 'All' not granted",
            )),
        }
    }

    /// Grant a permission to an agent.
    ///
    /// # Example
    ///
    /// ```
    /// guard.grant("agent1", PermissionAction::Delete);
    /// guard.grant("agent2", PermissionAction::Execute.with_scope("tool:web_search"));
    /// ```
    pub fn grant(&mut self, agent_id: &str, grant: PermissionGrant) {
        self.grants
            .entry(agent_id.to_string())
            .or_default()
            .push(grant);
    }

    /// Grant a simple action permission (no scope, no expiry).
    pub fn grant_action(&mut self, agent_id: &str, action: PermissionAction) {
        self.grant(agent_id, PermissionGrant::new(action));
    }

    /// Revoke all grants for an agent.
    pub fn revoke_all(&mut self, agent_id: &str) {
        self.grants.remove(agent_id);
    }

    /// List all grants for an agent.
    pub fn list_grants(&self, agent_id: &str) -> Vec<PermissionGrant> {
        self.grants.get(agent_id).cloned().unwrap_or_default()
    }

    /// Check if an agent has any grants.
    pub fn has_grants(&self, agent_id: &str) -> bool {
        self.grants.contains_key(agent_id)
    }
}

impl Default for PermissionGuard {
    fn default() -> Self {
        Self::new()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
