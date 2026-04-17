//! Semantic Filesystem
//!
//! Replaces traditional path-based file operations with AI-semantic operations.
//!
//! # Core Design
//!
//! No paths. No directories. AI agents interact via:
//! - **Semantic tags** — describe WHAT something is
//! - **Content queries** — search by meaning, not by name
//! - **Intent-based CRUD** — create, read, update, delete by description
//!
//! # Layered Context Loading
//!
//! | Layer | Size | Use |
//! |-------|------|-----|
//! | L0 | ~100 tokens | File summary for quick filtering |
//! | L1 | ~2k tokens | Key sections for deep understanding |
//! | L2 | Full content | Complete file when needed |
//!
//! # Operations
//!
//! - `create(content, tags, intent)` — store with semantic metadata
//! - `read(query, layer)` — retrieve by CID or semantic query at L0/L1/L2
//! - `update(cid, new_content)` — replace with full audit log
//! - `delete(cid)` — logical delete (soft delete, recycle bin)
//! - `search(query)` — semantic search across all tags and content

pub mod semantic_fs;
pub mod context_loader;
pub mod embedding;
pub mod search;
pub mod summarizer;
pub mod graph;
pub mod types;

pub use semantic_fs::{SemanticFS, FSError, Query, SearchResult, AuditEntry, AuditAction, RecycleEntry, EventType, EventRelation, EventSummary};
pub use context_loader::{ContextLoader, ContextLayer};
pub use embedding::{EmbeddingProvider, Embedding, EmbeddingMeta, EmbedError, OllamaBackend, LocalEmbeddingBackend, StubEmbeddingProvider};
pub use search::{SemanticSearch, SearchHit, SearchIndexMeta, SearchFilter, InMemoryBackend, Bm25Index, SearchIndexEntry};
pub use crate::temporal::{TemporalResolver, TemporalRange, Granularity};
pub use summarizer::{Summarizer, OllamaSummarizer, SummaryLayer, SummarError};
pub use graph::{
    KnowledgeGraph, KGNode, KGEdge, KGNodeType, KGEdgeType, KGSearchHit, KGError,
    PetgraphBackend,
};
