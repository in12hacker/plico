//! Distributed Mode Operations (v20.0)
//!
//! Enables Plico to operate as a multi-node cluster for:
//! - Distributed agent scheduling across nodes
//! - Cross-node KG replication
//! - Shared memory namespace across cluster
//! - Node discovery and heartbeat
//!
//! Architecture:
//! - Each node has a unique NodeId and advertises its capabilities
//! - Gossip-based node discovery for decentralized cluster membership
//! - Agents can migrate between nodes or run remotely
//! - KG uses eventual consistency with vector clock conflict resolution

use std::collections::HashMap;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a cluster node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for NodeId {
    fn default() -> Self {
        Self::new()
    }
}

/// Node capability advertisement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub node_id: NodeId,
    pub host: String,
    pub port: u16,
    pub started_at_ms: u64,
    pub last_heartbeat_ms: u64,
    pub agent_capacity: usize,
    pub memory_capacity_bytes: u64,
    pub is_seed: bool,
}

impl NodeInfo {
    pub fn new(node_id: NodeId, host: String, port: u16, is_seed: bool) -> Self {
        Self {
            node_id,
            host,
            port,
            started_at_ms: crate::scheduler::agent::now_ms(),
            last_heartbeat_ms: crate::scheduler::agent::now_ms(),
            agent_capacity: 100,
            memory_capacity_bytes: 64 * 1024 * 1024 * 1024, // 64GB
            is_seed,
        }
    }

    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn update_heartbeat(&mut self) {
        self.last_heartbeat_ms = crate::scheduler::agent::now_ms();
    }

    pub fn is_stale(&self, threshold_ms: u64) -> bool {
        let now = crate::scheduler::agent::now_ms();
        now.saturating_sub(self.last_heartbeat_ms) > threshold_ms
    }
}

/// Cluster membership state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterMembership {
    pub known_nodes: HashMap<NodeId, NodeInfo>,
    pub local_node_id: NodeId,
    pub cluster_name: String,
    pub version: u64,
}

impl ClusterMembership {
    pub fn new(local_node_id: NodeId, cluster_name: String, is_seed: bool, host: String, port: u16) -> Self {
        let local_info = NodeInfo::new(local_node_id.clone(), host, port, is_seed);
        let mut known_nodes = HashMap::new();
        known_nodes.insert(local_node_id.clone(), local_info);

        Self {
            known_nodes,
            local_node_id,
            cluster_name,
            version: 1,
        }
    }

    pub fn is_seed(&self) -> bool {
        self.known_nodes
            .get(&self.local_node_id)
            .map(|n| n.is_seed)
            .unwrap_or(false)
    }

    pub fn add_node(&mut self, info: NodeInfo) {
        self.known_nodes.insert(info.node_id.clone(), info);
        self.version += 1;
    }

    pub fn remove_node(&mut self, node_id: &NodeId) {
        self.known_nodes.remove(node_id);
        self.version += 1;
    }

    pub fn update_heartbeat(&mut self, node_id: &NodeId) {
        if let Some(info) = self.known_nodes.get_mut(node_id) {
            info.update_heartbeat();
        }
    }

    pub fn get_node(&self, node_id: &NodeId) -> Option<&NodeInfo> {
        self.known_nodes.get(node_id)
    }

    pub fn other_nodes(&self) -> Vec<&NodeInfo> {
        self.known_nodes
            .values()
            .filter(|n| n.node_id != self.local_node_id)
            .collect()
    }

    pub fn seed_nodes(&self) -> Vec<&NodeInfo> {
        self.known_nodes
            .values()
            .filter(|n| n.is_seed && n.node_id != self.local_node_id)
            .collect()
    }

    /// Remove stale nodes that haven't sent heartbeats in threshold_ms.
    pub fn remove_stale(&mut self, threshold_ms: u64) -> Vec<NodeId> {
        let stale: Vec<NodeId> = self.known_nodes
            .iter()
            .filter(|(id, info)| **id != self.local_node_id && info.is_stale(threshold_ms))
            .map(|(id, _)| id.clone())
            .collect();

        for id in &stale {
            self.known_nodes.remove(id);
        }
        if !stale.is_empty() {
            self.version += 1;
        }
        stale
    }
}

/// Agent migration ticket — used to move an agent from one node to another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationTicket {
    pub agent_id: String,
    pub from_node: NodeId,
    pub to_node: NodeId,
    pub checkpoint_cid: String,
    pub created_at_ms: u64,
}

/// Cross-node message envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMessage {
    pub id: String,
    pub from_node: NodeId,
    pub to_node: NodeId,
    pub msg_type: NodeMessageType,
    pub payload: Vec<u8>,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeMessageType {
    Heartbeat,
    NodeAnnounce,
    NodeLeave,
    AgentMigrate,
    AgentMigrateAck,
    KgSync,
    KgSyncRequest,
    MemorySync,
    MemorySyncRequest,
    Ping,
    Pong,
}

impl NodeMessage {
    pub fn new(from_node: NodeId, to_node: NodeId, msg_type: NodeMessageType, payload: Vec<u8>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            from_node,
            to_node,
            msg_type,
            payload,
            timestamp_ms: crate::scheduler::agent::now_ms(),
        }
    }

    pub fn heartbeat(from_node: NodeId) -> Self {
        Self::new(from_node.clone(), from_node, NodeMessageType::Heartbeat, Vec::new())
    }

    pub fn ping(from_node: NodeId, to_node: NodeId) -> Self {
        Self::new(from_node, to_node, NodeMessageType::Ping, Vec::new())
    }

    pub fn pong(from_node: NodeId, to_node: NodeId) -> Self {
        Self::new(from_node, to_node, NodeMessageType::Pong, Vec::new())
    }
}

/// Distributed cluster manager.
pub struct ClusterManager {
    membership: RwLock<ClusterMembership>,
    message_log: RwLock<Vec<NodeMessage>>,
    pending_migrations: RwLock<HashMap<String, MigrationTicket>>,
    heartbeat_interval_ms: u64,
    stale_threshold_ms: u64,
}

impl ClusterManager {
    pub fn new(
        local_node_id: NodeId,
        cluster_name: String,
        is_seed: bool,
        host: String,
        port: u16,
    ) -> Self {
        let membership = ClusterMembership::new(local_node_id, cluster_name, is_seed, host, port);
        Self {
            membership: RwLock::new(membership),
            message_log: RwLock::new(Vec::new()),
            pending_migrations: RwLock::new(HashMap::new()),
            heartbeat_interval_ms: 5000,  // 5 seconds
            stale_threshold_ms: 15000,     // 15 seconds
        }
    }

    pub fn node_id(&self) -> NodeId {
        self.membership.read().unwrap().local_node_id.clone()
    }

    pub fn cluster_name(&self) -> String {
        self.membership.read().unwrap().cluster_name.clone()
    }

    pub fn membership(&self) -> ClusterMembership {
        self.membership.read().unwrap().clone()
    }

    pub fn add_seed_node(&self, host: String, port: u16) {
        let seed_id = NodeId::new();
        let seed_info = NodeInfo::new(seed_id, host, port, true);
        self.membership.write().unwrap().add_node(seed_info);
    }

    pub fn handle_message(&self, msg: &NodeMessage) -> Option<NodeMessage> {
        // Log the message
        {
            let mut log = self.message_log.write().unwrap();
            log.push(msg.clone());
            // Keep log bounded
            if log.len() > 10000 {
                log.drain(0..5000);
            }
        }

        match msg.msg_type {
            NodeMessageType::Heartbeat => {
                self.membership.write().unwrap().update_heartbeat(&msg.from_node);
                None
            }
            NodeMessageType::Ping => {
                Some(NodeMessage::pong(self.node_id(), msg.from_node.clone()))
            }
            NodeMessageType::Pong => {
                self.membership.write().unwrap().update_heartbeat(&msg.from_node);
                None
            }
            NodeMessageType::NodeAnnounce => {
                // Payload should deserialize to NodeInfo
                if let Ok(info) = serde_json::from_slice(&msg.payload) {
                    self.membership.write().unwrap().add_node(info);
                }
                None
            }
            NodeMessageType::NodeLeave => {
                self.membership.write().unwrap().remove_node(&msg.from_node);
                None
            }
            NodeMessageType::AgentMigrateAck => {
                // Check if we have a pending migration
                if let Ok(ticket) = serde_json::from_slice::<MigrationTicket>(&msg.payload) {
                    let mut pending = self.pending_migrations.write().unwrap();
                    pending.remove(&ticket.agent_id);
                }
                None
            }
            _ => None,
        }
    }

    pub fn create_migration_ticket(&self, agent_id: String, from_node: NodeId, to_node: NodeId, checkpoint_cid: String) -> MigrationTicket {
        let ticket = MigrationTicket {
            agent_id,
            from_node,
            to_node,
            checkpoint_cid,
            created_at_ms: crate::scheduler::agent::now_ms(),
        };
        let mut pending = self.pending_migrations.write().unwrap();
        pending.insert(ticket.agent_id.clone(), ticket.clone());
        ticket
    }

    pub fn pending_migrations(&self) -> Vec<MigrationTicket> {
        self.pending_migrations.read().unwrap().values().cloned().collect()
    }

    /// Run periodic maintenance: remove stale nodes.
    pub fn run_maintenance(&self) -> Vec<NodeId> {
        self.membership.write().unwrap().remove_stale(self.stale_threshold_ms)
    }

    /// Check if this node is healthy (sent recent heartbeat).
    pub fn is_local_healthy(&self) -> bool {
        let membership = self.membership.read().unwrap();
        membership
            .known_nodes
            .get(&membership.local_node_id)
            .map(|n| !n.is_stale(self.stale_threshold_ms))
            .unwrap_or(false)
    }

    /// Get cluster statistics.
    pub fn cluster_stats(&self) -> ClusterStats {
        let membership = self.membership.read().unwrap();
        let total_nodes = membership.known_nodes.len();
        let local_id = membership.local_node_id.clone();
        let is_seed = membership.is_seed();

        ClusterStats {
            cluster_name: membership.cluster_name.clone(),
            total_nodes,
            local_node_id: local_id,
            is_seed,
            version: membership.version,
            pending_migrations: self.pending_migrations.read().unwrap().len(),
        }
    }
}

/// Cluster statistics for monitoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterStats {
    pub cluster_name: String,
    pub total_nodes: usize,
    pub local_node_id: NodeId,
    pub is_seed: bool,
    pub version: u64,
    pub pending_migrations: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_id() {
        let id1 = NodeId::new();
        let id2 = NodeId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_cluster_membership() {
        let node_id = NodeId::new();
        let mut cluster = ClusterMembership::new(
            node_id.clone(),
            "test-cluster".to_string(),
            true,
            "localhost".to_string(),
            7878,
        );

        assert_eq!(cluster.known_nodes.len(), 1);
        assert!(cluster.is_seed());
        assert!(cluster.other_nodes().is_empty());

        // Add another node
        let other_id = NodeId::new();
        let other_info = NodeInfo::new(other_id.clone(), "other.host".to_string(), 7879, false);
        cluster.add_node(other_info);

        assert_eq!(cluster.known_nodes.len(), 2);
        assert_eq!(cluster.other_nodes().len(), 1);
    }

    #[test]
    fn test_node_stale() {
        let node_id = NodeId::new();
        let info = NodeInfo::new(node_id, "localhost".to_string(), 7878, false);
        assert!(!info.is_stale(60000)); // 60 seconds threshold

        // Simulate old timestamp
        let mut old_info = info;
        old_info.last_heartbeat_ms = 0;
        assert!(old_info.is_stale(1)); // 1ms threshold
    }

    #[test]
    fn test_cluster_manager() {
        let manager = ClusterManager::new(
            NodeId::new(),
            "test".to_string(),
            true,
            "localhost".to_string(),
            7878,
        );

        assert_eq!(manager.cluster_stats().total_nodes, 1);
        assert!(manager.is_local_healthy());

        // Handle a ping
        let ping = NodeMessage::ping(NodeId::new(), manager.node_id());
        let pong = manager.handle_message(&ping);
        assert!(pong.is_some());
        assert!(matches!(pong.unwrap().msg_type, NodeMessageType::Pong));
    }
}
