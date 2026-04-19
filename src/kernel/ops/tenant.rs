//! Tenant Management Operations
//!
//! Handles tenant lifecycle: create, list, and cross-tenant resource sharing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    pub id: String,
    pub admin_agent_id: String,
    pub created_at_ms: u64,
}

impl Tenant {
    pub fn new(id: String, admin_agent_id: String) -> Self {
        Self {
            id,
            admin_agent_id,
            created_at_ms: now_ms(),
        }
    }
}

/// Tenant registry — manages all tenants in the system.
///
/// The registry is global (not per-tenant) and is protected by RwLock
/// for concurrent access from multiple API request handlers.
#[derive(Debug)]
pub struct TenantStore {
    /// Map from tenant_id -> Tenant metadata.
    tenants: RwLock<HashMap<String, Tenant>>,
    /// Agents that are "tenant admins" — they can create tenants.
    /// TODO: move this to a proper admin role system.
    admins: RwLock<HashMap<String, Vec<String>>>, // tenant_id -> vec<agent_id>
}

impl TenantStore {
    pub fn new() -> Self {
        Self {
            tenants: RwLock::new(HashMap::new()),
            admins: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new tenant. Fails if tenant already exists or if tenant_id is empty.
    pub fn create(&self, tenant_id: &str, admin_agent_id: &str) -> std::io::Result<Tenant> {
        if tenant_id.trim().is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "tenant_id cannot be empty",
            ));
        }

        let mut tenants = self.tenants.write().unwrap();
        if tenants.contains_key(tenant_id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("tenant '{}' already exists", tenant_id),
            ));
        }

        let tenant = Tenant::new(tenant_id.to_string(), admin_agent_id.to_string());
        tenants.insert(tenant_id.to_string(), tenant.clone());

        // Register the admin agent for this tenant
        drop(tenants);
        let mut admins = self.admins.write().unwrap();
        admins
            .entry(tenant_id.to_string())
            .or_default()
            .push(admin_agent_id.to_string());

        Ok(tenant)
    }

    /// List all tenants accessible to the given agent.
    /// Currently all tenants are listed (agents can see that other tenants exist).
    /// TODO: filter based on visibility permissions.
    pub fn list_for_agent(&self, agent_id: &str) -> Vec<Tenant> {
        let tenants = self.tenants.read().unwrap();
        // For now, return all tenants if the agent is admin of any tenant,
        // or if the agent is a trusted system agent.
        // TODO: implement proper visibility rules.
        let all_tenants: Vec<Tenant> = tenants.values().cloned().collect();

        if agent_id == "kernel" || agent_id == "system" {
            return all_tenants;
        }

        // Return all tenants if the agent is admin of at least one
        let admins = self.admins.read().unwrap();
        let is_admin = admins.values().any(|admins| admins.iter().any(|a| a == agent_id));
        if is_admin {
            all_tenants
        } else {
            // Non-admin agents see only their own tenant
            // For now, return empty list (they should use default tenant)
            vec![]
        }
    }

    /// Check if a tenant exists.
    pub fn exists(&self, tenant_id: &str) -> bool {
        let tenants = self.tenants.read().unwrap();
        tenants.contains_key(tenant_id)
    }

    /// Get tenant metadata.
    pub fn get(&self, tenant_id: &str) -> Option<Tenant> {
        let tenants = self.tenants.read().unwrap();
        tenants.get(tenant_id).cloned()
    }

    /// Remove a tenant (admin only). Fails if tenant doesn't exist.
    pub fn remove(&self, tenant_id: &str) -> std::io::Result<()> {
        let mut tenants = self.tenants.write().unwrap();
        if !tenants.contains_key(tenant_id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("tenant '{}' not found", tenant_id),
            ));
        }
        tenants.remove(tenant_id);

        drop(tenants);
        let mut admins = self.admins.write().unwrap();
        admins.remove(tenant_id);

        Ok(())
    }

    // ─── Persistence (P-3) ─────────────────────────────────────────────

    fn index_path(root: &Path) -> PathBuf {
        root.join("tenant_index.json")
    }

    /// Persist tenant registry to disk.
    pub fn persist(&self, root: &Path) {
        let tenants = self.tenants.read().unwrap();
        let admins = self.admins.read().unwrap();
        let data = TenantPersistData {
            tenants: tenants.clone(),
            admins: admins.clone(),
        };
        crate::kernel::persistence::atomic_write_json(&Self::index_path(root), &data);
    }

    /// Restore tenant registry from disk.
    pub fn restore(root: &Path) -> Self {
        let path = Self::index_path(root);
        if !path.exists() {
            return Self::new();
        }
        match std::fs::read_to_string(&path) {
            Ok(json) => match serde_json::from_str::<TenantPersistData>(&json) {
                Ok(data) => {
                    let store = Self {
                        tenants: RwLock::new(data.tenants),
                        admins: RwLock::new(data.admins),
                    };
                    let count = store.tenants.read().unwrap().len();
                    if count > 0 {
                        tracing::info!("Restored {count} tenants from persistent storage");
                    }
                    return store;
                }
                Err(e) => tracing::warn!("Failed to parse tenant index: {e}"),
            },
            Err(e) => tracing::warn!("Failed to read tenant index: {e}"),
        }
        Self::new()
    }
}

/// Serialized tenant registry data for persistence.
#[derive(Serialize, Deserialize)]
struct TenantPersistData {
    tenants: HashMap<String, Tenant>,
    admins: HashMap<String, Vec<String>>,
}

impl Default for TenantStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Represents a tenant share operation — sharing resources between tenants.
#[derive(Debug, Clone)]
pub struct TenantShare {
    pub from_tenant: String,
    pub to_tenant: String,
    pub resource_type: String,  // "kg" | "memory" | "cas"
    pub resource_pattern: String, // tag pattern or "*"
    pub created_at_ms: u64,
}

impl TenantShare {
    pub fn new(
        from_tenant: String,
        to_tenant: String,
        resource_type: String,
        resource_pattern: String,
    ) -> Self {
        Self {
            from_tenant,
            to_tenant,
            resource_type,
            resource_pattern,
            created_at_ms: now_ms(),
        }
    }

    /// Validate the resource type.
    pub fn is_valid_resource_type(rt: &str) -> bool {
        matches!(rt, "kg" | "memory" | "cas")
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_tenant_requires_non_empty_id() {
        let store = TenantStore::new();
        let result = store.create("", "admin1");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn create_tenant_fails_if_already_exists() {
        let store = TenantStore::new();
        store.create("team-alpha", "admin1").unwrap();
        let result = store.create("team-alpha", "admin2");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn create_and_get_tenant() {
        let store = TenantStore::new();
        let created = store.create("team-alpha", "admin1").unwrap();
        assert_eq!(created.id, "team-alpha");
        assert_eq!(created.admin_agent_id, "admin1");

        let fetched = store.get("team-alpha").unwrap();
        assert_eq!(fetched.id, "team-alpha");
    }

    #[test]
    fn list_tenants_for_admin() {
        let store = TenantStore::new();
        store.create("team-alpha", "admin1").unwrap();
        store.create("team-beta", "admin2").unwrap();

        let list = store.list_for_agent("admin1");
        // admin1 should see all tenants (they're admin of one)
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn list_tenants_for_non_admin() {
        let store = TenantStore::new();
        store.create("team-alpha", "admin1").unwrap();
        store.create("team-beta", "admin2").unwrap();

        let list = store.list_for_agent("random-agent");
        // non-admin sees nothing (or only their own tenant)
        assert!(list.is_empty());
    }

    #[test]
    fn system_agent_sees_all_tenants() {
        let store = TenantStore::new();
        store.create("team-alpha", "admin1").unwrap();
        store.create("team-beta", "admin2").unwrap();

        let list = store.list_for_agent("kernel");
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn tenant_share_resource_type_validation() {
        assert!(TenantShare::is_valid_resource_type("kg"));
        assert!(TenantShare::is_valid_resource_type("memory"));
        assert!(TenantShare::is_valid_resource_type("cas"));
        assert!(!TenantShare::is_valid_resource_type("invalid"));
    }

    #[test]
    fn remove_tenant() {
        let store = TenantStore::new();
        store.create("team-alpha", "admin1").unwrap();
        assert!(store.exists("team-alpha"));

        store.remove("team-alpha").unwrap();
        assert!(!store.exists("team-alpha"));
    }

    #[test]
    fn remove_nonexistent_tenant_fails() {
        let store = TenantStore::new();
        let result = store.remove("nonexistent");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
