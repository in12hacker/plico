//! AI Kernel — Orchestrates all Plico subsystems
//!
//! The AI Kernel is the central coordinator. It wires together:
//! - CAS Storage (persistence)
//! - Layered Memory (agent memory management)
//! - Agent Scheduler (intent scheduling)
//! - Semantic FS (file operations)
//! - Permission Guardrails (access control)
//!
//! Upper-layer AI agents interact with the kernel through the semantic API.

use std::path::PathBuf;
use std::sync::Arc;

use crate::cas::{CASStorage, AIObject, AIObjectMeta};
use crate::memory::{LayeredMemory, MemoryEntry, MemoryQuery, MemoryResult, MemoryTier};
use crate::scheduler::{AgentScheduler, Agent, Intent, IntentPriority, AgentHandle};
use crate::fs::{SemanticFS, Query};
use crate::api::permission::{PermissionGuard, PermissionContext, PermissionAction};

/// The AI Kernel — all subsystems wired together.
pub struct AIKernel {
    pub cas: Arc<CASStorage>,
    pub memory: Arc<LayeredMemory>,
    pub scheduler: Arc<AgentScheduler>,
    pub fs: Arc<SemanticFS>,
    pub permissions: Arc<PermissionGuard>,
}

impl AIKernel {
    /// Initialize the AI Kernel with the given storage root.
    pub fn new(root: PathBuf) -> std::io::Result<Self> {
        let cas = Arc::new(CASStorage::new(root.join("cas"))?);
        let memory = Arc::new(LayeredMemory::new());
        let scheduler = Arc::new(AgentScheduler::new());
        let fs = Arc::new(SemanticFS::new(root)?);
        let permissions = Arc::new(PermissionGuard::new());

        Ok(Self {
            cas,
            memory,
            scheduler,
            fs,
            permissions,
        })
    }

    // ─── CAS Operations ────────────────────────────────────────────────

    /// Store an object directly in CAS.
    pub fn store_object(
        &self,
        data: Vec<u8>,
        meta: AIObjectMeta,
        agent_id: &str,
    ) -> std::io::Result<String> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        let obj = AIObject::new(data, meta);
        self.cas.put(&obj)
    }

    /// Retrieve an object by CID.
    pub fn get_object(&self, cid: &str, agent_id: &str) -> std::io::Result<AIObject> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Read)?;
        self.cas.get(cid).map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()))
    }

    // ─── Semantic FS Operations ────────────────────────────────────────

    /// Create an object with semantic metadata.
    pub fn semantic_create(
        &self,
        content: Vec<u8>,
        tags: Vec<String>,
        agent_id: &str,
        intent: Option<String>,
    ) -> std::io::Result<String> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        self.fs.create(content, tags, agent_id.to_string(), intent)
    }

    /// Semantic search.
    pub fn semantic_search(&self, query: &str, agent_id: &str, limit: usize) -> Vec<crate::fs::SearchResult> {
        let ctx = PermissionContext::new(agent_id.to_string());
        let _ = self.permissions.check(&ctx, PermissionAction::Read);
        self.fs.search(query, limit)
    }

    /// Semantic read.
    pub fn semantic_read(&self, query: &Query, agent_id: &str) -> std::io::Result<Vec<AIObject>> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Read)?;
        self.fs.read(query)
    }

    /// Semantic update.
    pub fn semantic_update(
        &self,
        cid: &str,
        new_content: Vec<u8>,
        new_tags: Option<Vec<String>>,
        agent_id: &str,
    ) -> std::io::Result<String> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        self.fs.update(cid, new_content, new_tags, agent_id.to_string())
    }

    /// Semantic delete (soft delete).
    pub fn semantic_delete(&self, cid: &str, agent_id: &str) -> std::io::Result<()> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Delete)?;
        self.fs.delete(cid, agent_id.to_string())
    }

    // ─── Agent Operations ──────────────────────────────────────────────

    /// Register a new agent.
    pub fn register_agent(&self, name: String) -> String {
        let agent = Agent::new(name);
        let id = agent.id().to_string();
        self.scheduler.register(agent);
        id
    }

    /// List all active agents.
    pub fn list_agents(&self) -> Vec<AgentHandle> {
        self.scheduler.list_agents()
    }

    /// Submit an intent for scheduling.
    pub fn submit_intent(&self, priority: IntentPriority, description: String, agent_id: Option<String>) {
        let mut intent = Intent::new(priority, description);
        if let Some(aid) = agent_id {
            intent = intent.with_agent(crate::scheduler::AgentId(aid));
        }
        self.scheduler.submit(intent);
    }

    // ─── Memory Operations ─────────────────────────────────────────────

    /// Store an ephemeral memory for an agent.
    pub fn remember(&self, agent_id: &str, content: String) {
        let entry = MemoryEntry::ephemeral(agent_id, content);
        self.memory.store(entry);
    }

    /// Retrieve all memories for an agent.
    pub fn recall(&self, agent_id: &str) -> Vec<MemoryEntry> {
        self.memory.get_all(agent_id)
    }

    /// Evict ephemeral memories for an agent.
    pub fn forget_ephemeral(&self, agent_id: &str) {
        self.memory.evict_ephemeral(agent_id);
    }
}
