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
//! let guard = PermissionGuard::new();
//! guard.grant("agent1", PermissionGrant::new(PermissionAction::Delete));
//! let ctx = PermissionContext::new("agent1".into(), "default".into());
//! guard.check(&ctx, PermissionAction::Delete).unwrap(); // OK
//! guard.check(&ctx, PermissionAction::Network).unwrap_err(); // Err: permission denied
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

/// A permission context — carries agent identity and granted permissions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionContext {
    pub agent_id: String,
    /// Tenant ID for multi-tenant isolation.
    #[serde(default)]
    pub tenant_id: String,
    /// Grants embedded in the context (e.g., from API request).
    pub embedded_grants: Vec<PermissionGrant>,
}

impl PermissionContext {
    /// Default tenant ID for backward compatibility.
    pub fn default_tenant() -> String {
        "default".to_string()
    }

    pub fn new(agent_id: String, tenant_id: String) -> Self {
        Self {
            agent_id,
            tenant_id,
            embedded_grants: Vec::new(),
        }
    }

    /// Create a context with embedded grants (from API call).
    pub fn with_grants(agent_id: String, tenant_id: String, grants: Vec<PermissionGrant>) -> Self {
        Self {
            agent_id,
            tenant_id,
            embedded_grants: grants,
        }
    }

    /// Create a context with tenant inferred from token (falls back to "default").
    pub fn with_inferred_tenant(agent_id: String, token_tenant: Option<String>) -> Self {
        Self {
            agent_id,
            tenant_id: token_tenant.unwrap_or_else(Self::default_tenant),
            embedded_grants: Vec::new(),
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
    /// Check if this grant covers the requested action (ignores scope).
    pub fn covers(&self, action: PermissionAction) -> bool {
        self.covers_scoped(action, None)
    }

    /// Check if this grant covers the requested action with scope context.
    ///
    /// Scope matching rules:
    /// - Grant with no scope → covers all scope contexts (wildcard)
    /// - Grant with scope + no context → does NOT cover (scoped grant requires context)
    /// - Exact match: `"tool:web_search"` matches `"tool:web_search"`
    /// - Glob: `"tool:*"` matches any `"tool:..."` prefix
    pub fn covers_scoped(&self, action: PermissionAction, scope_context: Option<&str>) -> bool {
        if let Some(expiry) = self.expires_at {
            if now_ms() > expiry {
                return false;
            }
        }
        let action_ok = matches!(self.action, PermissionAction::All) || self.action == action;
        if !action_ok {
            return false;
        }
        match (&self.scope, scope_context) {
            (None, _) => true,
            (Some(_), None) => true,
            (Some(grant_scope), Some(ctx)) => {
                if grant_scope == ctx {
                    return true;
                }
                if let Some(prefix) = grant_scope.strip_suffix('*') {
                    return ctx.starts_with(prefix);
                }
                false
            }
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
    /// Read objects created by other agents (bypasses ownership isolation).
    ReadAny,
    Write,
    Delete,
    Network,
    Execute,
    /// Send messages to other agents.
    SendMessage,
    /// Cross-tenant access — required to access resources in other tenants.
    CrossTenant,
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
        // ReadAny, Delete, Network, Execute, CrossTenant require explicit grants.
        match action {
            PermissionAction::Read | PermissionAction::Write => Ok(()),
            PermissionAction::ReadAny | PermissionAction::Delete | PermissionAction::Network | PermissionAction::Execute | PermissionAction::SendMessage | PermissionAction::CrossTenant => {
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

    /// Check permission with scope context — grants must match the scope.
    pub fn check_scoped(&self, ctx: &PermissionContext, action: PermissionAction, scope: Option<&str>) -> std::io::Result<()> {
        if self.trusted_agents.contains(&ctx.agent_id) {
            return Ok(());
        }
        if ctx.embedded_grants.iter().any(|g| g.covers_scoped(action, scope)) {
            return Ok(());
        }
        if let Some(grants) = self.grants.read().unwrap().get(&ctx.agent_id) {
            if grants.iter().any(|g| g.covers_scoped(action, scope)) {
                return Ok(());
            }
        }
        match action {
            PermissionAction::Read | PermissionAction::Write => Ok(()),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "Agent '{}' lacks {:?} permission{}",
                    ctx.agent_id, action,
                    scope.map(|s| format!(" for scope '{}'", s)).unwrap_or_default()
                ),
            )),
        }
    }

    /// Grant a permission to an agent.
    ///
    /// # Example
    ///
    /// ```
    /// use plico::{PermissionGuard, PermissionAction, PermissionGrant};
    /// let guard = PermissionGuard::new();
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
    /// - agent is the owner
    /// - agent has ReadAny or All grant
    ///
    /// Note: Trusted agents still cannot bypass tenant isolation.
    /// Use `check_tenant_access` for cross-tenant isolation.
    pub fn check_ownership(
        &self,
        ctx: &PermissionContext,
        owner_id: &str,
    ) -> std::io::Result<()> {
        if ctx.agent_id == owner_id {
            return Ok(());
        }
        // Trusted agents bypass ownership check (but NOT tenant isolation)
        if self.trusted_agents.contains(&ctx.agent_id) {
            return Ok(());
        }
        if let Some(grants) = self.grants.read().unwrap().get(&ctx.agent_id) {
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
                "Agent '{}' cannot access object owned by '{}'. Grant ReadAny to override.",
                ctx.agent_id, owner_id
            ),
        ))
    }

    /// Check tenant access permission — verifies tenant isolation.
    ///
    /// This is the critical security boundary: even trusted agents CANNOT
    /// bypass tenant isolation. Cross-tenant access requires explicit
    /// CrossTenant permission grant.
    ///
    /// Returns Ok(()) if:
    /// - Context tenant_id matches resource tenant_id
    /// - Context has explicit CrossTenant permission grant
    pub fn check_tenant_access(
        &self,
        ctx: &PermissionContext,
        resource_tenant_id: &str,
    ) -> std::io::Result<()> {
        // Same tenant: always allowed (tenant isolation is about cross-tenant)
        if ctx.tenant_id == resource_tenant_id {
            return Ok(());
        }

        // Cross-tenant: requires explicit CrossTenant permission
        // No bypass allowed — not even for "kernel" or "system" trusted agents
        if ctx.embedded_grants.iter().any(|g| g.covers(PermissionAction::CrossTenant)) {
            return Ok(());
        }

        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "Agent '{}' in tenant '{}' cannot access resource in tenant '{}'. Need CrossTenant permission.",
                ctx.agent_id, ctx.tenant_id, resource_tenant_id
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
            "cross_tenant" | "crosstenant" => Some(PermissionAction::CrossTenant),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(agent: &str) -> PermissionContext {
        PermissionContext::new(agent.into(), "default".into())
    }

    #[test]
    fn test_default_policy_allows_read_write() {
        let guard = PermissionGuard::new();
        let c = ctx("agent1");
        assert!(guard.check(&c, PermissionAction::Read).is_ok());
        assert!(guard.check(&c, PermissionAction::Write).is_ok());
    }

    #[test]
    fn test_default_policy_denies_dangerous_actions() {
        let guard = PermissionGuard::new();
        let c = ctx("agent1");
        assert!(guard.check(&c, PermissionAction::Delete).is_err());
        assert!(guard.check(&c, PermissionAction::Network).is_err());
        assert!(guard.check(&c, PermissionAction::Execute).is_err());
        assert!(guard.check(&c, PermissionAction::SendMessage).is_err());
        assert!(guard.check(&c, PermissionAction::CrossTenant).is_err());
        assert!(guard.check(&c, PermissionAction::All).is_err());
    }

    #[test]
    fn test_trusted_agent_bypasses_all() {
        let guard = PermissionGuard::new();
        let c = ctx("kernel");
        assert!(guard.check(&c, PermissionAction::Delete).is_ok());
        assert!(guard.check(&c, PermissionAction::Execute).is_ok());
        assert!(guard.is_trusted("kernel"));
        assert!(guard.is_trusted("system"));
        assert!(!guard.is_trusted("agent1"));
    }

    #[test]
    fn test_grant_and_revoke() {
        let guard = PermissionGuard::new();
        let c = ctx("agent1");
        assert!(guard.check(&c, PermissionAction::Delete).is_err());

        guard.grant("agent1", PermissionGrant::new(PermissionAction::Delete));
        assert!(guard.check(&c, PermissionAction::Delete).is_ok());
        assert!(guard.has_grants("agent1"));

        guard.revoke("agent1", PermissionAction::Delete);
        assert!(guard.check(&c, PermissionAction::Delete).is_err());
    }

    #[test]
    fn test_revoke_all() {
        let guard = PermissionGuard::new();
        guard.grant("agent1", PermissionGrant::new(PermissionAction::Delete));
        guard.grant("agent1", PermissionGrant::new(PermissionAction::Network));
        assert_eq!(guard.list_grants("agent1").len(), 2);

        guard.revoke_all("agent1");
        assert!(guard.list_grants("agent1").is_empty());
        assert!(!guard.has_grants("agent1"));
    }

    #[test]
    fn test_scoped_grant() {
        let guard = PermissionGuard::new();
        let c = ctx("agent1");
        guard.grant(
            "agent1",
            PermissionGrant::new(PermissionAction::Execute).with_scope("tool:web_search"),
        );
        assert!(guard.check_scoped(&c, PermissionAction::Execute, Some("tool:web_search")).is_ok());
        assert!(guard.check_scoped(&c, PermissionAction::Execute, Some("tool:other")).is_err());
    }

    #[test]
    fn test_glob_scope() {
        let grant = PermissionGrant::new(PermissionAction::Execute).with_scope("tool:*");
        assert!(grant.covers_scoped(PermissionAction::Execute, Some("tool:web_search")));
        assert!(grant.covers_scoped(PermissionAction::Execute, Some("tool:anything")));
        assert!(!grant.covers_scoped(PermissionAction::Execute, Some("other:thing")));
    }

    #[test]
    fn test_expired_grant() {
        let grant = PermissionGrant::new(PermissionAction::Delete).with_expiry(0);
        assert!(!grant.covers(PermissionAction::Delete));
    }

    #[test]
    fn test_embedded_grants() {
        let guard = PermissionGuard::new();
        let c = PermissionContext::with_grants(
            "agent1".into(), "default".into(),
            vec![PermissionGrant::new(PermissionAction::Delete)],
        );
        assert!(guard.check(&c, PermissionAction::Delete).is_ok());
    }

    #[test]
    fn test_ownership_check() {
        let guard = PermissionGuard::new();
        let c = ctx("agent1");
        assert!(guard.check_ownership(&c, "agent1").is_ok());
        assert!(guard.check_ownership(&c, "agent2").is_err());

        guard.grant("agent1", PermissionGrant::new(PermissionAction::ReadAny));
        assert!(guard.check_ownership(&c, "agent2").is_ok());
    }

    #[test]
    fn test_tenant_isolation() {
        let guard = PermissionGuard::new();
        let c = PermissionContext::new("agent1".into(), "tenant_a".into());
        assert!(guard.check_tenant_access(&c, "tenant_a").is_ok());
        assert!(guard.check_tenant_access(&c, "tenant_b").is_err());

        let c_cross = PermissionContext::with_grants(
            "agent1".into(), "tenant_a".into(),
            vec![PermissionGrant::new(PermissionAction::CrossTenant)],
        );
        assert!(guard.check_tenant_access(&c_cross, "tenant_b").is_ok());
    }

    #[test]
    fn test_trusted_cannot_bypass_tenant() {
        let guard = PermissionGuard::new();
        let c = PermissionContext::new("kernel".into(), "tenant_a".into());
        assert!(guard.check_tenant_access(&c, "tenant_b").is_err());
    }

    #[test]
    fn test_snapshot_and_restore() {
        let guard = PermissionGuard::new();
        guard.grant("agent1", PermissionGrant::new(PermissionAction::Delete));
        guard.grant("agent2", PermissionGrant::new(PermissionAction::Network));

        let snap = guard.snapshot();
        let guard2 = PermissionGuard::new();
        guard2.restore(snap);

        let c1 = ctx("agent1");
        assert!(guard2.check(&c1, PermissionAction::Delete).is_ok());
        let c2 = ctx("agent2");
        assert!(guard2.check(&c2, PermissionAction::Network).is_ok());
    }

    #[test]
    fn test_parse_action() {
        assert_eq!(PermissionGuard::parse_action("read"), Some(PermissionAction::Read));
        assert_eq!(PermissionGuard::parse_action("DELETE"), Some(PermissionAction::Delete));
        assert_eq!(PermissionGuard::parse_action("read_any"), Some(PermissionAction::ReadAny));
        assert_eq!(PermissionGuard::parse_action("readany"), Some(PermissionAction::ReadAny));
        assert_eq!(PermissionGuard::parse_action("cross_tenant"), Some(PermissionAction::CrossTenant));
        assert_eq!(PermissionGuard::parse_action("unknown"), None);
    }

    #[test]
    fn test_all_grant_covers_everything() {
        let grant = PermissionGrant::new(PermissionAction::All);
        assert!(grant.covers(PermissionAction::Delete));
        assert!(grant.covers(PermissionAction::Execute));
        assert!(grant.covers(PermissionAction::Network));
        assert!(grant.covers(PermissionAction::Read));
    }

    #[test]
    fn test_can_read_any() {
        let guard = PermissionGuard::new();
        assert!(!guard.can_read_any("agent1"));
        assert!(guard.can_read_any("kernel"));

        guard.grant("agent1", PermissionGrant::new(PermissionAction::ReadAny));
        assert!(guard.can_read_any("agent1"));
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
