//! Layered Memory Management
//!
//! Plico's memory system mirrors AI cognitive architecture with 4 tiers:
//!
//! | Tier | Name | Analog | Purpose |
//! |------|------|--------|---------|
//! | L0 | Ephemeral Context | CPU Cache | Active conversation state, current task |
//! | L1 | Working Memory | RAM | Mid-term project context, recent operations |
//! | L2 | Long-term Memory | Disk/DB | Persistent knowledge, vector database |
//! | L3 | Procedural Memory | Learned Skills | Workflows, skills, learned procedures |
//!
//! # Design
//!
//! Memory is managed per-agent. Each AI agent has its own memory hierarchy.
//! The memory manager handles tier promotion (L0→L1→L2) and retrieval.

pub mod layered;
pub mod persist;

pub use layered::{LayeredMemory, MemoryTier, MemoryEntry, MemoryContent, MemoryError};
pub use persist::{MemoryPersister, CASPersister, MemoryLoader, PersistError, PersistenceIndex, PersistedTier};

use serde::{Deserialize, Serialize};

/// A memory query — used by agents to retrieve relevant context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryQuery {
    /// Natural language query text (will be embedded for semantic search)
    pub query: String,
    /// Which tier to search (None = all tiers)
    pub tier: Option<MemoryTier>,
    /// Maximum number of results
    pub limit: usize,
    /// Agent ID to scope the search
    pub agent_id: String,
}

/// A memory query result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryResult {
    pub entries: Vec<MemoryEntry>,
    pub tier: MemoryTier,
    pub total: usize,
}
