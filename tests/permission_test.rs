//! Permission guard unit tests
//!
//! Tests cover: default policy, grant/revoke, trust bypass,
//! expiry, and scope restrictions.

use plico::api::permission::{
    PermissionAction, PermissionContext, PermissionGrant, PermissionGuard,
};

fn check_ok(guard: &PermissionGuard, agent: &str, action: PermissionAction) {
    let ctx = PermissionContext::new(agent.to_string());
    assert!(
        guard.check(&ctx, action).is_ok(),
        "Agent '{}' should have {:?} permission",
        agent,
        action
    );
}

fn check_denied(guard: &PermissionGuard, agent: &str, action: PermissionAction) {
    let ctx = PermissionContext::new(agent.to_string());
    assert!(
        guard.check(&ctx, action).is_err(),
        "Agent '{}' should NOT have {:?} permission without grant",
        agent,
        action
    );
}

#[test]
fn test_default_policy_read_write_allowed() {
    let guard = PermissionGuard::new();

    // Read and Write are allowed by default for any agent
    check_ok(&guard, "random_agent", PermissionAction::Read);
    check_ok(&guard, "random_agent", PermissionAction::Write);
}

#[test]
fn test_default_policy_delete_network_execute_denied() {
    let guard = PermissionGuard::new();

    // Delete, Network, Execute require explicit grant
    check_denied(&guard, "random_agent", PermissionAction::Delete);
    check_denied(&guard, "random_agent", PermissionAction::Network);
    check_denied(&guard, "random_agent", PermissionAction::Execute);
}

#[test]
fn test_trusted_agents_bypass_all() {
    let guard = PermissionGuard::new();

    // Trusted agents bypass all permission checks
    for action in [
        PermissionAction::Read,
        PermissionAction::Write,
        PermissionAction::Delete,
        PermissionAction::Network,
        PermissionAction::Execute,
    ] {
        check_ok(&guard, "kernel", action);
        check_ok(&guard, "system", action);
    }

    // Even untrusted agents with no grants should pass Read/Write
    check_ok(&guard, "kernel", PermissionAction::Read);
}

#[test]
fn test_grant_permission() {
    let mut guard = PermissionGuard::new();

    // Before grant: denied
    check_denied(&mut guard, "agent1", PermissionAction::Delete);

    // Grant Delete to agent1
    guard.grant_action("agent1", PermissionAction::Delete);

    // After grant: allowed
    check_ok(&guard, "agent1", PermissionAction::Delete);

    // But Network is still denied
    check_denied(&guard, "agent1", PermissionAction::Network);
}

#[test]
fn test_grant_with_scope() {
    let mut guard = PermissionGuard::new();

    // Grant Delete with scope (only for specific CID)
    guard.grant(
        "agent1",
        PermissionGrant::new(PermissionAction::Delete).with_scope("cid:abc123"),
    );

    // Basic check still passes (scope is metadata, not enforced by guard)
    check_ok(&guard, "agent1", PermissionAction::Delete);
}

#[test]
fn test_grant_with_expiry() {
    let mut guard = PermissionGuard::new();

    // Grant that expired in the past (1 hour ago)
    let past = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        - 3600_000;

    guard.grant(
        "agent1",
        PermissionGrant::new(PermissionAction::Delete).with_expiry(past),
    );

    // Expired grant should not allow
    check_denied(&guard, "agent1", PermissionAction::Delete);
}

#[test]
fn test_grant_all_covers_everything() {
    let mut guard = PermissionGuard::new();

    // Grant "All" to agent1
    guard.grant_action("agent1", PermissionAction::All);

    // All actions should now be allowed
    for action in [
        PermissionAction::Read,
        PermissionAction::Write,
        PermissionAction::Delete,
        PermissionAction::Network,
        PermissionAction::Execute,
    ] {
        check_ok(&guard, "agent1", action);
    }
}

#[test]
fn test_revoke_all() {
    let mut guard = PermissionGuard::new();

    guard.grant_action("agent1", PermissionAction::Delete);
    guard.grant_action("agent1", PermissionAction::Network);

    // Before revoke: allowed
    check_ok(&guard, "agent1", PermissionAction::Delete);
    check_ok(&guard, "agent1", PermissionAction::Network);

    // Revoke all
    guard.revoke_all("agent1");

    // After revoke: denied again
    check_denied(&guard, "agent1", PermissionAction::Delete);
    check_denied(&guard, "agent1", PermissionAction::Network);
}

#[test]
fn test_list_grants() {
    let mut guard = PermissionGuard::new();

    guard.grant_action("agent1", PermissionAction::Delete);
    guard.grant_action("agent1", PermissionAction::Network);

    let grants = guard.list_grants("agent1");
    assert_eq!(grants.len(), 2);
    assert!(grants.iter().any(|g| matches!(g.action, PermissionAction::Delete)));
    assert!(grants.iter().any(|g| matches!(g.action, PermissionAction::Network)));

    // agent2 has no grants
    assert!(guard.list_grants("agent2").is_empty());
}

#[test]
fn test_has_grants() {
    let mut guard = PermissionGuard::new();

    assert!(!guard.has_grants("agent1"));
    guard.grant_action("agent1", PermissionAction::Delete);
    assert!(guard.has_grants("agent1"));

    guard.revoke_all("agent1");
    assert!(!guard.has_grants("agent1"));
}

#[test]
fn test_permission_context_with_embedded_grants() {
    let guard = PermissionGuard::new();

    // Create context with embedded grant
    let ctx = PermissionContext::with_grants(
        "embedded_agent".to_string(),
        vec![PermissionGrant::new(PermissionAction::Execute)],
    );

    // Even though global grants is empty, embedded grants work
    assert!(guard.check(&ctx, PermissionAction::Execute).is_ok());
    // But Delete still denied (not in embedded grants)
    assert!(guard.check(&ctx, PermissionAction::Delete).is_err());
}

#[test]
fn test_permission_grant_new_factory() {
    let grant = PermissionGrant::new(PermissionAction::Delete);
    assert!(matches!(grant.action, PermissionAction::Delete));
    assert!(grant.scope.is_none());
    assert!(grant.expires_at.is_none());
}

#[test]
fn test_permission_grant_builder() {
    let grant = PermissionGrant::new(PermissionAction::Network)
        .with_scope("tool:web_search")
        .with_expiry(9999999999999u64);

    assert!(matches!(grant.action, PermissionAction::Network));
    assert_eq!(grant.scope, Some("tool:web_search".to_string()));
    assert!(grant.expires_at.is_some());
}

#[test]
fn test_multiple_agents_isolated() {
    let mut guard = PermissionGuard::new();

    guard.grant_action("agent1", PermissionAction::Delete);
    guard.grant_action("agent2", PermissionAction::Execute);

    check_ok(&guard, "agent1", PermissionAction::Delete);
    check_denied(&guard, "agent1", PermissionAction::Execute);

    check_ok(&guard, "agent2", PermissionAction::Execute);
    check_denied(&guard, "agent2", PermissionAction::Delete);
}
