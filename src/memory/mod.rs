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
pub mod relevance;
pub mod context_snapshot;
pub mod forgetting;
pub mod distillation;
pub mod causal;
pub mod topology;
pub mod cross_agent;
pub mod foresight;
pub mod pressure;
pub mod meta_memory;

pub use layered::{LayeredMemory, MemoryTier, MemoryType, MemoryEntry, MemoryContent, MemoryError, MemoryScope};
pub use persist::{MemoryPersister, CASPersister, MemoryLoader, PersistError, PersistenceIndex, PersistedTier};

