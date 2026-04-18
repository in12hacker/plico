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
//! use plico::{PermissionGuard, PermissionContext, PermissionAction, PermissionGrant};
//! let mut guard = PermissionGuard::new();
//! guard.grant("agent1", PermissionGrant::new(PermissionAction::Delete));
//! let ctx = PermissionContext::new("agent1".into());
//! guard.check(&ctx, PermissionAction::Delete).unwrap(); // OK
//! guard.check(&ctx, PermissionAction::Network).unwrap_err(); // Err: permission denied
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

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
        matches!(self.action, PermissionAction::All) || self.action == action
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
    /// Read objects created by other agents (bypasses ownership isolation).
    ReadAny,
    Write,
    Delete,
    Network,
    Execute,
    /// Send messages to other agents.
    SendMessage,
    All,
}

/// The permission guard — global access control registry.
#[derive(Debug)]
pub struct PermissionGuard {
    /// System-level trusted agents — bypass all permission checks.
    trusted_agents: std::collections::HashSet<String>,
    /// Persistent grants per agent (interior mutability for Arc sharing).
    grants: RwLock<HashMap<String, Vec<PermissionGrant>>>,
}

impl PermissionGuard {
    pub fn new() -> Self {
        let mut trusted = std::collections::HashSet::new();
        trusted.insert("kernel".to_string());
        trusted.insert("system".to_string());
        Self {
            trusted_agents: trusted,
            grants: RwLock::new(HashMap::new()),
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
        if let Some(grants) = self.grants.read().unwrap().get(&ctx.agent_id) {
            if grants.iter().any(|g| g.covers(action)) {
                return Ok(());
            }
        }

        // Default policy: Read and Write are allowed by default.
        // ReadAny, Delete, Network, Execute require explicit grants.
        match action {
            PermissionAction::Read | PermissionAction::Write => Ok(()),
            PermissionAction::ReadAny | PermissionAction::Delete | PermissionAction::Network | PermissionAction::Execute | PermissionAction::SendMessage => {
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
    /// use plico::{PermissionGuard, PermissionAction, PermissionGrant};
    /// let mut guard = PermissionGuard::new();
    /// guard.grant("agent1", PermissionGrant::new(PermissionAction::Delete));
    /// guard.grant("agent2", PermissionGrant::new(PermissionAction::Execute).with_scope("tool:web_search"));
    /// ```
    pub fn grant(&self, agent_id: &str, grant: PermissionGrant) {
        self.grants
            .write()
            .unwrap()
            .entry(agent_id.to_string())
            .or_default()
            .push(grant);
    }

    /// Grant a simple action permission (no scope, no expiry).
    pub fn grant_action(&self, agent_id: &str, action: PermissionAction) {
        self.grant(agent_id, PermissionGrant::new(action));
    }

    /// Revoke all grants for an agent.
    pub fn revoke_all(&self, agent_id: &str) {
        self.grants.write().unwrap().remove(agent_id);
    }

    /// Revoke grants for a specific action from an agent.
    pub fn revoke(&self, agent_id: &str, action: PermissionAction) {
        let mut grants = self.grants.write().unwrap();
        if let Some(agent_grants) = grants.get_mut(agent_id) {
            agent_grants.retain(|g| g.action != action);
            if agent_grants.is_empty() {
                grants.remove(agent_id);
            }
        }
    }

    /// List all grants for an agent.
    pub fn list_grants(&self, agent_id: &str) -> Vec<PermissionGrant> {
        self.grants.read().unwrap().get(agent_id).cloned().unwrap_or_default()
    }

    /// Check if an agent has any grants.
    pub fn has_grants(&self, agent_id: &str) -> bool {
        self.grants.read().unwrap().contains_key(agent_id)
    }

    /// Check if agent is trusted (bypasses all checks including isolation).
    pub fn is_trusted(&self, agent_id: &str) -> bool {
        self.trusted_agents.contains(agent_id)
    }

    /// Check if an agent can read objects from all agents (trusted or has ReadAny/All grant).
    pub fn can_read_any(&self, agent_id: &str) -> bool {
        if self.trusted_agents.contains(agent_id) {
            return true;
        }
        if let Some(grants) = self.grants.read().unwrap().get(agent_id) {
            return grants.iter().any(|g|
                g.covers(PermissionAction::ReadAny) || g.covers(PermissionAction::All)
            );
        }
        false
    }

    /// Check if an agent can read an object owned by `owner_id`.
    ///
    /// Returns Ok(()) if:
    /// - agent is trusted
    /// - agent is the owner
    /// - agent has ReadAny or All grant
    pub fn check_ownership(
        &self,
        agent_id: &str,
        owner_id: &str,
    ) -> std::io::Result<()> {
        if self.trusted_agents.contains(agent_id) {
            return Ok(());
        }
        if agent_id == owner_id {
            return Ok(());
        }
        let ctx = PermissionContext::new(agent_id.to_string());
        if let Some(grants) = self.grants.read().unwrap().get(agent_id) {
            if grants.iter().any(|g| g.covers(PermissionAction::ReadAny) || g.covers(PermissionAction::All)) {
                return Ok(());
            }
        }
        if ctx.has_permission(PermissionAction::ReadAny) {
            return Ok(());
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "Agent '{}' cannot read objects owned by '{}'. Grant ReadAny to override.",
                agent_id, owner_id
            ),
        ))
    }
    /// Snapshot all grants for serialization/persistence.
    pub fn snapshot(&self) -> HashMap<String, Vec<PermissionGrant>> {
        self.grants.read().unwrap().clone()
    }

    /// Restore grants from a persisted snapshot (bulk-load).
    pub fn restore(&self, grants: HashMap<String, Vec<PermissionGrant>>) {
        let mut guard = self.grants.write().unwrap();
        for (agent_id, agent_grants) in grants {
            guard.entry(agent_id).or_default().extend(agent_grants);
        }
    }

    /// Parse a permission action from string.
    pub fn parse_action(s: &str) -> Option<PermissionAction> {
        match s.to_lowercase().as_str() {
            "read" => Some(PermissionAction::Read),
            "read_any" | "readany" => Some(PermissionAction::ReadAny),
            "write" => Some(PermissionAction::Write),
            "delete" => Some(PermissionAction::Delete),
            "network" => Some(PermissionAction::Network),
            "execute" => Some(PermissionAction::Execute),
            "send_message" | "sendmessage" => Some(PermissionAction::SendMessage),
            "all" => Some(PermissionAction::All),
            _ => None,
        }
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
