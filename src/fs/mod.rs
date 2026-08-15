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

pub mod adaptive_budget;
pub mod chunking;
pub mod context_budget;
pub mod context_loader;
pub mod embedding;
pub mod graph;
pub mod query_augment;
pub mod query_decompose;
pub mod reranker;
pub mod retrieval_fusion;
pub mod retrieval_router;
pub mod search;
pub mod semantic_fs;
pub mod summarizer;
pub mod types;

pub use crate::temporal::{Granularity, TemporalRange, TemporalResolver};
pub use context_loader::{ContextLayer, ContextLoader, LoadedContext};
pub use embedding::{
    AdaptiveEmbeddingProvider, EmbedError, EmbedResult, Embedding, EmbeddingBuilderIdentity, EmbeddingCircuitBreaker,
    EmbeddingIdentityError, EmbeddingInputContract, EmbeddingInputOperation, EmbeddingMeta, EmbeddingNormalization,
    EmbeddingProvider, EmbeddingProviderFamily, LocalEmbeddingBackend, OllamaBackend, OpenAIEmbeddingBackend,
    StubEmbeddingProvider, VerifiedDocumentProviderSnapshot,
};
pub use graph::{KGEdge, KGEdgeType, KGError, KGNode, KGNodeType, KGSearchHit, KnowledgeGraph, PetgraphBackend};
pub use query_augment::{
    augment_query, expand_entities_from_kg, expand_with_tags, extract_time_range, parse_rewrite_response,
    rewrite_prompt, AugmentedQuery,
};
pub use query_decompose::{
    decompose, decomposition_prompt, parse_llm_decomposition, DecomposedQuery, SubQuery, SubQueryRole,
};
pub use reranker::{create_reranker_provider, LlamaCppReranker, RerankError, RerankResult, RerankerProvider};
pub use retrieval_router::{
    classify_by_llm_response, classify_by_rules, intent_classification_prompt, ClassificationMethod, ClassifiedIntent,
    QueryIntent, RetrievalConfig,
};
pub use search::{
    Bm25Index, DiagnosedSearch, EmbeddingDegradation, EmbeddingQueryState, HnswBackend, InMemoryBackend, SearchAccess,
    SearchExecution, SearchFilter, SearchHit, SearchIndexEntry, SearchIndexMeta, SearchPath, SearchPathExecution,
    SearchStageDegradation, SemanticSearch,
};
pub use semantic_fs::{
    AuditAction, AuditEntry, EventRelation, EventSummary, EventType, FSError, Query, RecycleEntry, SearchResult,
    SemanticFS,
};
pub use summarizer::{LlmSummarizer, SummarError, Summarizer, SummaryLayer};
