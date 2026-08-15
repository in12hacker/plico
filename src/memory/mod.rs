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
//! The memory manager keeps L0 runtime-only and commits durable tier changes as revisions.

pub mod causal;
pub mod foresight;
pub mod layered;
pub mod ledger;
pub(crate) mod projection;
pub mod relevance;

pub use layered::{
    CanonicalContentHash, DurableMemoryMutationError, LayeredMemory, MemoryContent, MemoryEntry, MemoryError, MemoryId,
    MemoryRevisionId, MemoryScope, MemoryTier, MemoryType,
};
pub(crate) use ledger::{
    CASCanonicalLedger, CanonicalLedger, CanonicalProjectionGuard, CanonicalProjectionSnapshot,
    CanonicalProjectionSource,
};
pub use ledger::{
    CanonicalRevision, CurrentView, ExpectedHead, LedgerCommit, LedgerError, LedgerRoot, PolicyMode, PolicyRecord,
};
