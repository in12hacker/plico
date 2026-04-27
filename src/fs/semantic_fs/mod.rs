//! Semantic Filesystem — AI-friendly CRUD with no paths, only semantic descriptions.
//!
//! # Module Structure
//! - `mod.rs` — struct, constructor, CRUD, tag index helpers
//! - `events.rs` — event operations (create_event, list_events, event_attach)
//! - `tests.rs` — integration tests

pub mod events;
#[cfg(test)]
pub mod tests;

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::cas::{AIObject, AIObjectMeta, CASStorage};
use crate::fs::context_loader::ContextLoader;
use crate::fs::embedding::EmbeddingProvider;
use crate::fs::reranker::RerankerProvider;
use crate::fs::search::{SemanticSearch, SearchFilter, SearchIndexMeta, Bm25Index};
use crate::fs::summarizer::Summarizer;
use crate::fs::graph::KnowledgeGraph;
use crate::fs::graph::{KGEdge, KGEdgeType};

// Re-export types from fs/types (single source of truth)
pub use crate::fs::types::{
    Query, SearchResult, AuditEntry, AuditAction, RecycleEntry, FSError,
    EventType, EventMeta, EventRelation, EventSummary,
};

// ── Adaptive RRF configuration ──────────────────────────────────────────────

/// Read RRF K constant from env or default to 60.
fn rrf_config_k() -> f32 {
    std::env::var("PLICO_RRF_K")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(60.0)
}

/// Compute adaptive BM25/Vector weights based on query characteristics.
///
/// Static override: if `PLICO_RRF_BM25_WEIGHT` and `PLICO_RRF_VECTOR_WEIGHT` are set,
/// adaptive logic is bypassed.
///
/// Heuristic: short queries (<=3 tokens) favor BM25, long queries (>=8 tokens) favor
/// vector search. BM25 top-1 high-score triggers an additional boost.
fn rrf_weights(query: &str, bm25_hits: &[(String, f32)]) -> (f32, f32) {
    // Static override
    if let (Ok(bw), Ok(vw)) = (
        std::env::var("PLICO_RRF_BM25_WEIGHT"),
        std::env::var("PLICO_RRF_VECTOR_WEIGHT"),
    ) {
        if let (Ok(b), Ok(v)) = (bw.parse::<f32>(), vw.parse::<f32>()) {
            return (b, v);
        }
    }

    let token_count = query.split_whitespace().count();

    // Linear interpolation between BM25-heavy and Vector-heavy
    let (bm25_w, vector_w) = if token_count <= 3 {
        (1.5_f32, 0.8_f32)
    } else if token_count >= 8 {
        (0.8_f32, 1.5_f32)
    } else {
        // Linear interpolation for 4-7 tokens
        let t = (token_count as f32 - 3.0) / 5.0; // 0.0 at 3, 1.0 at 8
        (1.5 - 0.7 * t, 0.8 + 0.7 * t)
    };

    // BM25 exact-match boost: if top-1 BM25 score is unusually high, boost BM25
    let bm25_boost = if let Some((_, top_score)) = bm25_hits.first() {
        if *top_score > 5.0 { 0.3 } else { 0.0 }
    } else {
        0.0
    };

    (bm25_w + bm25_boost, vector_w)
}

/// The semantic filesystem.
pub struct SemanticFS {
    root: std::path::PathBuf,
    cas: Arc<CASStorage>,
    tag_index: RwLock<HashMap<String, Vec<String>>>,
    tag_index_path: std::path::PathBuf,
    recycle_bin_path: std::path::PathBuf,
    ctx_loader: Arc<ContextLoader>,
    recycle_bin: RwLock<HashMap<String, RecycleEntry>>,
    audit_log: RwLock<Vec<AuditEntry>>,
    embedding: Arc<dyn EmbeddingProvider>,
    search_index: Arc<dyn SemanticSearch>,
    summarizer: Option<Arc<dyn Summarizer>>,
    knowledge_graph: Option<Arc<dyn KnowledgeGraph>>,
    bm25_index: Arc<Bm25Index>,
    reranker: Option<Arc<dyn RerankerProvider>>,
    // F-5: Soul alignment — env var disables auto-summarize to prevent OS policy leakage
    disable_auto_summarize: bool,
}

impl SemanticFS {
    pub fn root(&self) -> &std::path::Path { &self.root }

    pub fn ctx_loader(&self) -> &ContextLoader { &self.ctx_loader }

    /// Returns a clone of the internal Arc<ContextLoader>.
    pub fn ctx_loader_arc(&self) -> Arc<ContextLoader> {
        Arc::clone(&self.ctx_loader)
    }

    pub fn new(
        root_path: std::path::PathBuf,
        embedding: Arc<dyn EmbeddingProvider>,
        search_index: Arc<dyn SemanticSearch>,
        summarizer: Option<Arc<dyn Summarizer>>,
        knowledge_graph: Option<Arc<dyn KnowledgeGraph>>,
    ) -> std::io::Result<Self> {
        Self::with_reranker(root_path, embedding, search_index, summarizer, knowledge_graph, None)
    }

    pub fn with_reranker(
        root_path: std::path::PathBuf,
        embedding: Arc<dyn EmbeddingProvider>,
        search_index: Arc<dyn SemanticSearch>,
        summarizer: Option<Arc<dyn Summarizer>>,
        knowledge_graph: Option<Arc<dyn KnowledgeGraph>>,
        reranker: Option<Arc<dyn RerankerProvider>>,
    ) -> std::io::Result<Self> {
        let tag_index_path = root_path.join("tag_index.json");
        let recycle_bin_path = root_path.join("recycle_bin.json");
        let cas = Arc::new(CASStorage::new(root_path.join("objects"))?);

        let recycle_bin = if recycle_bin_path.exists() {
            Self::load_recycle_bin(&recycle_bin_path).unwrap_or_else(|e| {
                tracing::warn!("Failed to load recycle bin: {}", e);
                HashMap::new()
            })
        } else {
            HashMap::new()
        };

        let tag_index = if tag_index_path.exists() {
            Self::load_tag_index(&tag_index_path).unwrap_or_else(|e| {
                tracing::warn!("Failed to load tag index, rebuilding from CAS: {}", e);
                Self::rebuild_tag_index(&cas, &recycle_bin)
            })
        } else {
            Self::rebuild_tag_index(&cas, &recycle_bin)
        };

        let fs = Self {
            root: root_path.clone(),
            cas: Arc::clone(&cas),
            tag_index: RwLock::new(tag_index),
            tag_index_path,
            recycle_bin_path,
            ctx_loader: Arc::new(ContextLoader::new(root_path.join("context"), summarizer.clone(), cas)?),
            recycle_bin: RwLock::new(recycle_bin),
            audit_log: RwLock::new(Vec::new()),
            embedding,
            search_index,
            summarizer,
            knowledge_graph,
            bm25_index: Arc::new(Bm25Index::new()),
            reranker,
            disable_auto_summarize: std::env::var("PLICO_AUTO_SUMMARIZE").as_deref() != Ok("1"),
        };

        fs.rebuild_vector_index();
        Ok(fs)
    }

    fn rebuild_vector_index(&self) {
        let cids = match self.cas.list_cids() {
            Ok(c) => c,
            Err(e) => { tracing::warn!("rebuild_vector_index: failed to list CIDs: {e}"); return; }
        };
        if cids.is_empty() { return; }
        tracing::debug!("rebuild_vector_index: found {} CIDs", cids.len());
        tracing::info!("Rebuilding vector index for {} objects…", cids.len());
        let mut indexed = 0usize;
        let mut embed_available = true;
        let recycle_bin = self.recycle_bin.read().unwrap();

        for cid in &cids {
            if recycle_bin.contains_key(cid) { continue; } // F-43: skip soft-deleted
            let obj = match self.cas.get_raw(cid) {
                Ok(o) => o,
                Err(_) => continue,
            };
            if obj.meta.content_type.is_multimedia() { continue; }
            let text = match std::str::from_utf8(&obj.data) {
                Ok(s) => s.trim().to_string(),
                Err(_) => continue,
            };
            if text.is_empty() { continue; }

            self.bm25_index.upsert(cid, &text);

            if embed_available {
                match self.embedding.embed_document(&text) {
                    Ok(result) => {
                        self.search_index.upsert(cid, &result.embedding, SearchIndexMeta {
                            cid: cid.clone(),
                            tags: obj.meta.tags.clone(),
                            content_type: obj.meta.content_type.to_string(),
                            snippet: text.chars().take(256).collect(),
                            created_at: obj.meta.created_at,
                        });
                        indexed += 1;
                    }
                    Err(e) => {
                        tracing::warn!("rebuild_vector_index: embed failed for {}: {e}", &cid[..8]);
                        embed_available = false;
                    }
                }
            }
        }

        let bm25_count = self.bm25_index.len();
        if indexed > 0 {
            tracing::info!("Vector index rebuilt: {}/{} objects indexed", indexed, cids.len());
        }
        if bm25_count > 0 {
            tracing::info!("BM25 index rebuilt: {} documents indexed", bm25_count);
        }
    }

    pub fn create(
        &self,
        content: Vec<u8>,
        tags: Vec<String>,
        created_by: String,
        intent: Option<String>,
    ) -> std::io::Result<String> {
        let content_type = if std::str::from_utf8(&content).is_ok() {
            crate::cas::ContentType::Text
        } else {
            crate::cas::ContentType::Unknown
        };

        let meta = AIObjectMeta {
            content_type,
            tags: tags.clone(),
            created_by,
            created_at: now_ms(),
            intent,
            tenant_id: crate::DEFAULT_TENANT.to_string(),
        };

        let obj = AIObject::new(content.clone(), meta.clone());
        let cid = self.cas.put(&obj)?;

        self.update_tag_index(&tags, &cid);
        let embedding = self.upsert_semantic_index(&cid, &content, &meta);

        if let Some(ref kg) = self.knowledge_graph {
            if let Err(e) = kg.upsert_document(&cid, &tags, &meta.created_by) {
                tracing::warn!("Failed to upsert document to knowledge graph: {}", e);
            }
            if let Some(ref emb) = embedding {
                self.add_similar_to_edges(kg, &cid, emb);
            }
        }

        // F-5: Only summarize when PLICO_AUTO_SUMMARIZE=1 is set (V-06 soul alignment)
        // By default disabled — OS should not silently invoke LLM summarization
        if !self.disable_auto_summarize {
            if let Some(ref summarizer) = self.summarizer {
                let text = match std::str::from_utf8(&content) {
                    Ok(s) if !s.trim().is_empty() => s.to_string(),
                    _ => String::new(),
                };
                if !text.is_empty() {
                    match summarizer.summarize(&text, crate::fs::summarizer::SummaryLayer::L0) {
                        Ok(summary) => {
                            if let Err(e) = self.ctx_loader.store_l0(&cid, summary) {
                                tracing::warn!("Failed to store L0 summary for {}: {}", &cid[..8], e);
                            }
                        }
                        Err(e) => { tracing::warn!("L0 summarization failed for {}: {}", &cid[..8], e); }
                    }
                }
            }
        }

        self.audit_log.write().unwrap().push(AuditEntry {
            timestamp: now_ms(),
            action: AuditAction::Create,
            cid: cid.clone(),
            agent_id: String::new(),
        });

        // Hierarchical chunking: split large documents into child chunks
        let chunking_mode = crate::fs::chunking::ChunkingMode::from_env();
        if chunking_mode != crate::fs::chunking::ChunkingMode::None {
            if let Ok(text) = std::str::from_utf8(&content) {
                let emb_ref: Option<&dyn EmbeddingProvider> = if chunking_mode == crate::fs::chunking::ChunkingMode::Semantic {
                    Some(self.embedding.as_ref())
                } else {
                    None
                };
                let chunks = crate::fs::chunking::chunk_document(text, chunking_mode, emb_ref);
                if !chunks.is_empty() {
                    tracing::debug!("Chunked document {} into {} child chunks", &cid[..8], chunks.len());
                    for (ci, chunk) in chunks.iter().enumerate() {
                        let mut child_tags = tags.clone();
                        child_tags.push(format!("parent_cid:{}", cid));
                        child_tags.push(format!("chunk_idx:{}", ci));
                        child_tags.push("is_chunk:true".to_string());
                        let child_meta = AIObjectMeta {
                            content_type: crate::cas::ContentType::Text,
                            tags: child_tags.clone(),
                            created_by: meta.created_by.clone(),
                            created_at: meta.created_at,
                            intent: None,
                            tenant_id: meta.tenant_id.clone(),
                        };
                        let child_obj = AIObject::new(chunk.text.as_bytes().to_vec(), child_meta);
                        if let Ok(child_cid) = self.cas.put(&child_obj) {
                            self.update_tag_index(&child_tags, &child_cid);
                            self.upsert_semantic_index(&child_cid, chunk.text.as_bytes(), &AIObjectMeta {
                                content_type: crate::cas::ContentType::Text,
                                tags: child_tags,
                                created_by: meta.created_by.clone(),
                                created_at: meta.created_at,
                                intent: None,
                                tenant_id: meta.tenant_id.clone(),
                            });
                        }
                    }
                }
            }
        }

        Ok(cid)
    }

    /// Create an object using a pre-computed embedding vector (batch optimization path).
    /// Skips the per-item `embed_document` call, using the provided vector directly.
    pub fn create_with_embedding(
        &self,
        content: Vec<u8>,
        tags: Vec<String>,
        created_by: String,
        intent: Option<String>,
        precomputed_embedding: Vec<f32>,
    ) -> std::io::Result<String> {
        let content_type = if std::str::from_utf8(&content).is_ok() {
            crate::cas::ContentType::Text
        } else {
            crate::cas::ContentType::Unknown
        };

        let meta = AIObjectMeta {
            content_type,
            tags: tags.clone(),
            created_by,
            created_at: now_ms(),
            intent,
            tenant_id: crate::DEFAULT_TENANT.to_string(),
        };

        let obj = AIObject::new(content.clone(), meta.clone());
        let cid = self.cas.put(&obj)?;

        self.update_tag_index(&tags, &cid);

        // Use the precomputed embedding directly
        let text = String::from_utf8_lossy(&content);
        let snippet = if text.len() > 200 { format!("{}...", &text[..200]) } else { text.to_string() };
        let is_real = !precomputed_embedding.iter().all(|&v| v == 0.0);

        self.search_index.upsert(&cid, &precomputed_embedding, SearchIndexMeta {
            cid: cid.clone(),
            tags: meta.tags.clone(),
            snippet,
            content_type: format!("{:?}", meta.content_type).to_lowercase(),
            created_at: meta.created_at,
        });

        if !text.trim().is_empty() {
            self.bm25_index.upsert(&cid, &text);
        }

        if let Some(ref kg) = self.knowledge_graph {
            if let Err(e) = kg.upsert_document(&cid, &tags, &meta.created_by) {
                tracing::warn!("Failed to upsert document to knowledge graph: {}", e);
            }
            if is_real {
                self.add_similar_to_edges(kg, &cid, &precomputed_embedding);
            }
        }

        self.audit_log.write().unwrap().push(AuditEntry {
            timestamp: now_ms(),
            action: AuditAction::Create,
            cid: cid.clone(),
            agent_id: String::new(),
        });

        Ok(cid)
    }

    pub fn read(&self, query: &Query) -> std::io::Result<Vec<AIObject>> {
        match query {
            Query::ByCid(cid) => {
                let obj = self.cas.get(cid).map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()))?;
                Ok(vec![obj])
            }
            Query::ByTags(tags) => {
                let cids = self.resolve_tags(tags);
                let mut objects = Vec::new();
                for cid in cids {
                    if let Ok(obj) = self.cas.get(&cid) { objects.push(obj); }
                }
                Ok(objects)
            }
            Query::Semantic { text, filter } => {
                let filter = filter.clone().unwrap_or_default();
                let query_emb = match self.embedding.embed_query(text) {
                    Ok(result) => result.embedding,
                    Err(e) => {
                        tracing::warn!("Embedding failed for query '{text}': {e}. Falling back to tag search.");
                        let tags = text.split_whitespace().map(String::from).collect();
                        return self.read(&Query::ByTags(tags));
                    }
                };
                let hits = self.search_index.search(&query_emb, 10, &filter);
                let mut objects = Vec::new();
                for hit in hits {
                    if let Ok(obj) = self.cas.get(&hit.cid) { objects.push(obj); }
                }
                Ok(objects)
            }
            Query::ByType(content_type) => {
                let filter = crate::fs::search::SearchFilter {
                    content_type: Some(content_type.clone()),
                    ..Default::default()
                };
                let cids = self.search_index.list_by_filter(&filter);
                let mut objects = Vec::new();
                for cid in cids {
                    if let Ok(obj) = self.cas.get(&cid) { objects.push(obj); }
                }
                Ok(objects)
            }
            Query::Hybrid { tags, semantic, content_type } => {
                let filter = crate::fs::search::SearchFilter {
                    require_tags: tags.clone(),
                    content_type: content_type.clone(),
                    ..Default::default()
                };

                if let Some(text) = semantic {
                    let query_emb = match self.embedding.embed_query(text) {
                        Ok(result) => result.embedding,
                        Err(e) => {
                            tracing::warn!("Embedding failed in Hybrid query: {e}. Falling back to filter scan.");
                            let cids = self.search_index.list_by_filter(&filter);
                            let mut objects = Vec::new();
                            for cid in cids {
                                if let Ok(obj) = self.cas.get(&cid) { objects.push(obj); }
                            }
                            return Ok(objects);
                        }
                    };
                    let hits = self.search_index.search(&query_emb, 10, &filter);
                    let mut objects = Vec::new();
                    for hit in hits {
                        if let Ok(obj) = self.cas.get(&hit.cid) { objects.push(obj); }
                    }
                    Ok(objects)
                } else {
                    let cids = self.search_index.list_by_filter(&filter);
                    let mut objects = Vec::new();
                    for cid in cids {
                        if let Ok(obj) = self.cas.get(&cid) { objects.push(obj); }
                    }
                    Ok(objects)
                }
            }
        }
    }

    pub fn update(
        &self,
        old_cid: &str,
        new_content: Vec<u8>,
        new_tags: Option<Vec<String>>,
        agent_id: String,
    ) -> std::io::Result<String> {
        let old_obj = self.cas.get(old_cid).map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()))?;
        let final_tags = new_tags.unwrap_or_else(|| old_obj.meta.tags.clone());

        let new_meta = AIObjectMeta {
            content_type: old_obj.meta.content_type,
            tags: final_tags.clone(),
            created_by: old_obj.meta.created_by.clone(),
            created_at: now_ms(),
            intent: old_obj.meta.intent.clone(),
            tenant_id: old_obj.meta.tenant_id.clone(),
        };

        let new_obj = AIObject::new(new_content.clone(), new_meta.clone());
        let new_cid = self.cas.put(&new_obj)?;

        self.remove_from_tag_index(&old_obj.meta.tags, old_cid);
        self.update_tag_index(&final_tags, &new_cid);

        self.search_index.delete(old_cid);
        let embedding = self.upsert_semantic_index(&new_cid, &new_content, &new_meta);

        if let Some(ref kg) = self.knowledge_graph {
            let _ = kg.upsert_document(&new_cid, &final_tags, &old_obj.meta.created_by);
            if let Some(ref emb) = embedding {
                self.add_similar_to_edges(kg, &new_cid, emb);
            }
            use crate::fs::graph::types::{KGEdgeType, KGEdge};
            let edge = KGEdge::new(new_cid.clone(), old_cid.to_string(), KGEdgeType::Supersedes, 1.0);
            let _ = kg.add_edge(edge);
        }

        self.audit_log.write().unwrap().push(AuditEntry {
            timestamp: now_ms(),
            action: AuditAction::Update { previous_cid: old_cid.to_string() },
            cid: new_cid.clone(),
            agent_id,
        });

        Ok(new_cid)
    }

    pub fn delete(&self, cid: &str, agent_id: String) -> std::io::Result<()> {
        let obj = self.cas.get(cid)?;
        self.recycle_bin.write().unwrap().insert(cid.to_string(), RecycleEntry {
            cid: cid.to_string(),
            deleted_at: now_ms(),
            original_meta: obj.meta.clone(),
        });

        self.search_index.delete(cid);
        self.bm25_index.remove(cid);

        if let Some(ref kg) = self.knowledge_graph { let _ = kg.remove_node(cid); }

        self.remove_from_tag_index(&obj.meta.tags, cid);

        self.audit_log.write().unwrap().push(AuditEntry {
            timestamp: now_ms(),
            action: AuditAction::Delete,
            cid: cid.to_string(),
            agent_id,
        });

        let _ = self.persist_recycle_bin();
        Ok(())
    }

    pub fn list_deleted(&self) -> Vec<RecycleEntry> {
        let bin = self.recycle_bin.read().unwrap();
        let mut entries: Vec<_> = bin.values().cloned().collect();
        entries.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at));
        entries
    }

    pub fn restore(&self, cid: &str, agent_id: String) -> std::io::Result<()> {
        let entry = {
            let mut bin = self.recycle_bin.write().unwrap();
            bin.remove(cid).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, format!("CID not in recycle bin: {cid}"))
            })?
        };

        self.update_tag_index(&entry.original_meta.tags, cid);

        if let Ok(obj) = self.cas.get(cid) {
            let embedding = self.upsert_semantic_index(cid, &obj.data, &obj.meta);

            if let Some(ref kg) = self.knowledge_graph {
                if let Some(ref emb) = embedding {
                    self.add_similar_to_edges(kg, cid, emb);
                }
            }
        }

        if let Some(ref kg) = self.knowledge_graph {
            let _ = kg.upsert_document(cid, &entry.original_meta.tags, &entry.original_meta.created_by);
        }

        let _ = self.persist_recycle_bin();

        self.audit_log.write().unwrap().push(AuditEntry {
            timestamp: now_ms(),
            action: AuditAction::Create,
            cid: cid.to_string(),
            agent_id,
        });

        Ok(())
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        self.search_with_filter(query, limit, SearchFilter::default())
    }

    /// Direct BM25 search (exposes raw BM25 results for hybrid retrieval F-44 fallback).
    pub fn bm25_search(&self, query: &str, limit: usize) -> Vec<(String, f32)> {
        self.bm25_index.search(query, limit)
    }

    pub fn search_with_filter(&self, query: &str, limit: usize, filter: SearchFilter) -> Vec<SearchResult> {
        // Tier 0: Temporal query detection — if the query looks temporal, try KG path first
        if is_temporal_query(query) {
            if let Some(ref kg) = self.knowledge_graph {
                let temporal_results = self.search_temporal_via_kg(kg, query, limit);
                if !temporal_results.is_empty() {
                    tracing::debug!("Temporal KG path returned {} results", temporal_results.len());
                    return temporal_results;
                }
                tracing::debug!("Temporal KG path returned 0 results, degrading to hybrid search");
            }
        }

        // Tier 0.5: PPR multi-hop retrieval — if KG has nodes, try entity-based graph traversal
        let mut ppr_boost: HashMap<String, f32> = HashMap::new();
        if let Some(ref kg) = self.knowledge_graph {
            let query_words: Vec<String> = query.split_whitespace()
                .filter(|w| w.len() > 2)
                .map(|w| w.to_lowercase())
                .collect();

            let all_ids = kg.all_node_ids();
            let seed_nodes: Vec<String> = all_ids.iter()
                .filter(|id| {
                    let id_lower = id.to_lowercase();
                    query_words.iter().any(|w| id_lower.contains(w))
                })
                .take(5)
                .cloned()
                .collect();

            if !seed_nodes.is_empty() {
                match kg.personalized_pagerank(&seed_nodes, 0.15, 50, limit * 2) {
                    Ok(ranked) => {
                        for (node_id, score) in ranked {
                            if let Ok(Some(node)) = kg.get_node(&node_id) {
                                for cid in node.properties.as_object()
                                    .and_then(|o| o.get("cids"))
                                    .and_then(|v| v.as_array())
                                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect::<Vec<_>>())
                                    .unwrap_or_default()
                                {
                                    ppr_boost.insert(cid, score);
                                }
                            }
                        }
                        if !ppr_boost.is_empty() {
                            tracing::debug!("PPR boosted {} documents from {} seed nodes", ppr_boost.len(), seed_nodes.len());
                        }
                    }
                    Err(e) => {
                        tracing::debug!("PPR failed, degrading: {e}");
                    }
                }
            }
        }

        let query_emb = self.embedding.embed_query(query).ok().map(|r| r.embedding);

        let vector_hits: HashMap<String, f32> = match &query_emb {
            Some(emb) => self
                .search_index.search(emb, limit * 2, &filter)
                .into_iter().map(|hit| (hit.cid.clone(), hit.score)).collect(),
            None => HashMap::new(),
        };

        let bm25_hits: Vec<(String, f32)> = self.bm25_index.search(query, limit * 2);

        if vector_hits.is_empty() && bm25_hits.is_empty() {
            return self.search_by_tags_with_filter(query, &filter);
        }

        let rrf_k = rrf_config_k();
        let (bm25_weight, vector_weight) = rrf_weights(query, &bm25_hits);

        let mut rrf_scores: HashMap<String, (f32, usize)> = HashMap::new();

        let mut sorted_vector: Vec<_> = vector_hits.iter().collect();
        sorted_vector.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (rank, (cid, _score)) in sorted_vector.iter().enumerate() {
            let rrf = vector_weight / (rrf_k + rank as f32);
            rrf_scores.insert((*cid).clone(), (rrf, 1usize));
        }

        for (rank, (cid, _score)) in bm25_hits.iter().enumerate() {
            let rrf = bm25_weight / (rrf_k + rank as f32);
            if let Some((existing_rrf, count)) = rrf_scores.get_mut(cid) {
                *existing_rrf += rrf;
                *count += 1;
            } else {
                let obj = match self.cas.get_raw(cid) {
                    Ok(o) => o,
                    Err(_) => continue,
                };
                let meta_for_filter = SearchIndexMeta {
                    cid: cid.clone(),
                    tags: obj.meta.tags.clone(),
                    snippet: String::new(),
                    content_type: format!("{:?}", obj.meta.content_type).to_lowercase(),
                    created_at: obj.meta.created_at,
                };
                if filter.matches(&meta_for_filter) {
                    rrf_scores.insert(cid.clone(), (rrf, 1usize));
                }
            }
        }

        // Inject PPR boost into RRF scores
        for (cid, ppr_score) in &ppr_boost {
            let boost = ppr_score * 0.5;
            if let Some((existing_rrf, _)) = rrf_scores.get_mut(cid) {
                *existing_rrf += boost;
            } else {
                rrf_scores.insert(cid.clone(), (boost, 1));
            }
        }

        let mut sorted: Vec<(String, f32)> = rrf_scores
            .into_iter()
            .map(|(cid, (score, _))| (cid, score))
            .collect();
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Reranker stage: if available, apply cross-encoder reranking on top-N RRF candidates
        if let Some(ref reranker) = self.reranker {
            let rerank_candidates: usize = (limit * 3).min(sorted.len());
            let candidates: Vec<(String, String)> = sorted[..rerank_candidates]
                .iter()
                .filter_map(|(cid, _)| {
                    self.cas.get(cid).ok().map(|obj| {
                        let text = String::from_utf8_lossy(
                            &obj.data[..std::cmp::min(512, obj.data.len())],
                        )
                        .to_string();
                        (cid.clone(), text)
                    })
                })
                .collect();

            match reranker.rerank(query, &candidates) {
                Ok(reranked) => {
                    tracing::debug!(
                        "Reranker refined {} candidates -> {} results",
                        candidates.len(),
                        reranked.len(),
                    );
                    let reranked_results: Vec<SearchResult> = reranked
                        .into_iter()
                        .take(limit)
                        .filter_map(|r| {
                            self.cas.get(&r.id).ok().map(|obj| {
                                let snippet = String::from_utf8_lossy(
                                    &obj.data[..std::cmp::min(200, obj.data.len())],
                                )
                                .to_string();
                                SearchResult {
                                    cid: r.id,
                                    relevance: r.score,
                                    meta: obj.meta,
                                    snippet,
                                }
                            })
                        })
                        .collect();
                    return reranked_results;
                }
                Err(e) => {
                    tracing::warn!("Reranker failed, degrading to RRF: {e}");
                }
            }
        }

        sorted.truncate(limit);

        let results: Vec<SearchResult> = sorted
            .into_iter()
            .filter_map(|(cid, relevance)| {
                self.cas.get(&cid).ok().map(|obj| {
                    SearchResult {
                        cid,
                        relevance,
                        snippet: String::from_utf8_lossy(&obj.data[..std::cmp::min(200, obj.data.len())]).to_string(),
                        meta: obj.meta,
                    }
                })
            })
            .collect();

        self.resolve_parent_chunks(results)
    }

    /// If a search result is a child chunk (has `parent_cid:xxx` tag), resolve the parent
    /// and return the parent's expanded snippet instead. Deduplicates by parent CID.
    fn resolve_parent_chunks(&self, results: Vec<SearchResult>) -> Vec<SearchResult> {
        let mut seen_parents = std::collections::HashSet::new();
        let mut resolved = Vec::with_capacity(results.len());

        for r in results {
            let parent_cid = r.meta.tags.iter()
                .find(|t| t.starts_with("parent_cid:"))
                .map(|t| t["parent_cid:".len()..].to_string());

            if let Some(ref pcid) = parent_cid {
                if !seen_parents.insert(pcid.clone()) {
                    continue;
                }
                if let Ok(parent_obj) = self.cas.get(pcid) {
                    let snippet = String::from_utf8_lossy(
                        &parent_obj.data[..std::cmp::min(500, parent_obj.data.len())]
                    ).to_string();
                    resolved.push(SearchResult {
                        cid: pcid.clone(),
                        relevance: r.relevance,
                        meta: parent_obj.meta,
                        snippet,
                    });
                    continue;
                }
            }
            resolved.push(r);
        }

        resolved
    }

    fn search_by_tags_with_filter(&self, query: &str, filter: &SearchFilter) -> Vec<SearchResult> {
        let query_lower = query.to_lowercase();
        let index = self.tag_index.read().unwrap();
        let mut results = Vec::new();

        for (tag, cids) in index.iter() {
            if tag.to_lowercase().contains(&query_lower) {
                for cid in cids {
                    if let Ok(obj) = self.cas.get(cid) {
                        if filter.matches(&SearchIndexMeta {
                            cid: cid.clone(),
                            tags: obj.meta.tags.clone(),
                            snippet: String::new(),
                            content_type: format!("{}", obj.meta.content_type),
                            created_at: obj.meta.created_at,
                        }) {
                            let snippet = String::from_utf8_lossy(&obj.data[..std::cmp::min(200, obj.data.len())]).to_string();
                            results.push(SearchResult { cid: cid.clone(), relevance: 0.8, meta: obj.meta, snippet });
                        }
                    }
                }
            }
        }
        results
    }

    pub fn list_tags(&self) -> Vec<String> {
        let index = self.tag_index.read().unwrap();
        let mut tags: Vec<_> = index.keys().cloned().collect();
        tags.sort();
        tags
    }

    /// Direct tag-only search (A-8a: B25 fix).
    pub fn search_by_tags(&self, tags: &[String], limit: usize) -> Vec<SearchResult> {
        let index = self.tag_index.read().unwrap();
        let mut results = Vec::new();

        for tag in tags {
            if let Some(cids) = index.get(tag) {
                for cid in cids {
                    if results.len() >= limit {
                        break;
                    }
                    if let Ok(obj) = self.cas.get(cid) {
                        let snippet = String::from_utf8_lossy(&obj.data[..std::cmp::min(200, obj.data.len())]).to_string();
                        results.push(SearchResult { cid: cid.clone(), relevance: 0.8, meta: obj.meta, snippet });
                    }
                }
            }
        }
        results
    }

/// F-4: Tag intersection search — ALL tags must match (AND semantics).
    pub fn search_by_tags_intersection(&self, tags: &[String], limit: usize) -> Vec<SearchResult> {
        use std::collections::HashSet;
        let index = self.tag_index.read().unwrap();
        let mut candidates: Option<HashSet<String>> = None;

        for tag in tags {
            if let Some(cids) = index.get(tag) {
                let set: HashSet<String> = cids.iter().cloned().collect();
                match &mut candidates {
                    Some(existing) => {
                        *existing = existing.intersection(&set).cloned().collect();
                        if existing.is_empty() {
                            return Vec::new();
                        }
                    }
                    None => { candidates = Some(set); }
                }
            } else {
                return Vec::new();
            }
        }

        let mut results = Vec::new();
        if let Some(cids) = candidates {
            for cid in cids {
                if results.len() >= limit {
                    break;
                }
                if let Ok(obj) = self.cas.get(&cid) {
                    let snippet = String::from_utf8_lossy(&obj.data[..std::cmp::min(200, obj.data.len())]).to_string();
                    results.push(SearchResult { cid: cid.clone(), relevance: 0.9, meta: obj.meta, snippet });
                }
            }
        }
        results
    }

    /// Total number of objects stored in this filesystem's CAS.
    pub fn count_objects(&self) -> std::io::Result<usize> {
        self.cas.list_cids().map(|c| c.len())
    }

    pub fn audit_log(&self) -> Vec<AuditEntry> {
        self.audit_log.read().unwrap().clone()
    }

    pub fn cas(&self) -> &CASStorage {
        &self.cas
    }

    // ─── Internal helpers ────────────────────────────────────────────────

    const SIMILARITY_THRESHOLD: f32 = 0.75;
    const MAX_SIMILAR_EDGES: usize = 3;

    fn add_similar_to_edges(&self, kg: &Arc<dyn KnowledgeGraph>, cid: &str, embedding: &[f32]) {
        let filter = SearchFilter::default();
        let similar = self.search_index.search(embedding, Self::MAX_SIMILAR_EDGES + 1, &filter);
        let mut added = 0usize;
        for hit in similar {
            if hit.cid == cid || hit.score < Self::SIMILARITY_THRESHOLD {
                continue;
            }
            if added >= Self::MAX_SIMILAR_EDGES {
                break;
            }
            if kg.get_valid_edge_between(cid, &hit.cid, Some(KGEdgeType::SimilarTo), 0).ok().flatten().is_some() {
                continue;
            }
            let e1 = KGEdge::new_with_episode(
                cid.to_string(),
                hit.cid.clone(),
                KGEdgeType::SimilarTo,
                hit.score,
                cid,
            );
            let e2 = KGEdge::new_with_episode(
                hit.cid.clone(),
                cid.to_string(),
                KGEdgeType::SimilarTo,
                hit.score,
                cid,
            );
            let _ = kg.add_edge(e1);
            let _ = kg.add_edge(e2);
            added += 1;
        }
    }

    fn upsert_semantic_index(&self, cid: &str, content: &[u8], meta: &AIObjectMeta) -> Option<Vec<f32>> {
        let text = String::from_utf8_lossy(content);
        let snippet = if text.trim().is_empty() {
            String::new()
        } else if text.len() > 200 {
            format!("{}...", &text[..200])
        } else {
            text.to_string()
        };

        let is_real_embedding;
        let embedding = if text.trim().is_empty() {
            is_real_embedding = false;
            vec![0.0f32; self.embedding.dimension()]
        } else {
            match self.embedding.embed_document(&text) {
                Ok(result) => {
                    is_real_embedding = true;
                    if let Some(ledger) = crate::kernel::ops::cost_ledger::get_global_cost_ledger() {
                        ledger.record_embedding_with_tokens(result.input_tokens, self.embedding.model_name(), "", &meta.created_by);
                    }
                    result.embedding
                }
                Err(e) => {
                    tracing::warn!("Failed to embed CID={}: {e}. Indexing with zero vector.", cid);
                    is_real_embedding = false;
                    vec![0.0f32; self.embedding.dimension()]
                }
            }
        };

        self.search_index.upsert(cid, &embedding, SearchIndexMeta {
            cid: cid.to_string(),
            tags: meta.tags.clone(),
            snippet,
            content_type: format!("{:?}", meta.content_type).to_lowercase(),
            created_at: meta.created_at,
        });

        if !text.trim().is_empty() {
            self.bm25_index.upsert(cid, &text);
        }

        if is_real_embedding { Some(embedding) } else { None }
    }

    fn update_tag_index(&self, tags: &[String], cid: &str) {
        let mut index = self.tag_index.write().unwrap();
        for tag in tags { index.entry(tag.clone()).or_default().push(cid.to_string()); }
    }

    /// Persist the tag index to disk (called periodically, not on every write).
    pub fn flush_tag_index(&self) {
        let _ = self.persist_tag_index();
    }

    fn persist_recycle_bin(&self) -> std::io::Result<()> {
        let bin = self.recycle_bin.read().unwrap();
        let json = serde_json::to_vec(&*bin)?;
        std::fs::write(&self.recycle_bin_path, json)
    }

    fn load_recycle_bin(path: &std::path::Path) -> std::io::Result<HashMap<String, RecycleEntry>> {
        let json = std::fs::read(path)?;
        serde_json::from_slice(&json).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    fn persist_tag_index(&self) -> std::io::Result<()> {
        let index = self.tag_index.read().unwrap();
        let json = serde_json::to_vec(&*index)?;
        std::fs::write(&self.tag_index_path, json)
    }

    fn load_tag_index(path: &std::path::Path) -> std::io::Result<HashMap<String, Vec<String>>> {
        let json = std::fs::read(path)?;
        serde_json::from_slice(&json).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    fn rebuild_tag_index(cas: &CASStorage, recycle_bin: &HashMap<String, RecycleEntry>) -> HashMap<String, Vec<String>> {
        let mut index: HashMap<String, Vec<String>> = HashMap::new();
        if let Ok(cids) = cas.list_cids() {
            for cid in cids {
                if recycle_bin.contains_key(&cid) { continue; } // F-43: skip soft-deleted
                if let Ok(obj) = cas.get_raw(&cid) {
                    for tag in &obj.meta.tags {
                        index.entry(tag.clone()).or_default().push(cid.clone());
                    }
                }
            }
        }
        index
    }

    fn remove_from_tag_index(&self, tags: &[String], cid: &str) {
        let mut index = self.tag_index.write().unwrap();
        for tag in tags {
            if let Some(cids) = index.get_mut(tag) { cids.retain(|c| c != cid); }
        }
    }

    fn resolve_tags(&self, tags: &[String]) -> Vec<String> {
        let index = self.tag_index.read().unwrap();
        let mut cids: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for tag in tags {
            if let Some(tag_cids) = index.get(tag) {
                for cid in tag_cids {
                    if seen.insert(cid.clone()) {
                        cids.push(cid.clone());
                    }
                }
            }
        }

        cids
    }
}

/// Detect whether a query has temporal intent based on keyword matching.
fn is_temporal_query(query: &str) -> bool {
    let q = query.to_lowercase();
    const TEMPORAL_KEYWORDS: &[&str] = &[
        "after", "before", "when", "then", "during", "since", "until",
        "first", "last", "next", "previous", "recent", "latest", "earliest",
        "sequence", "timeline", "order", "chronolog",
        "之后", "之前", "什么时候", "然后", "期间", "自从", "直到",
        "第一次", "最后", "最近", "最早", "顺序", "时间线", "先后",
    ];
    TEMPORAL_KEYWORDS.iter().any(|kw| q.contains(kw))
}

impl SemanticFS {
    /// Search via KG temporal path: find Event nodes, walk Follows edges,
    /// and assemble context from related documents.
    fn search_temporal_via_kg(
        &self,
        kg: &Arc<dyn crate::fs::graph::KnowledgeGraph>,
        query: &str,
        limit: usize,
    ) -> Vec<SearchResult> {
        use crate::fs::graph::{KGNodeType, KGEdgeType};

        let event_nodes = match kg.list_nodes("", Some(KGNodeType::Event)) {
            Ok(nodes) => nodes,
            Err(_) => return vec![],
        };

        if event_nodes.is_empty() {
            return vec![];
        }

        let query_lower = query.to_lowercase();
        let mut relevant_events: Vec<_> = event_nodes
            .into_iter()
            .filter(|n| n.is_active())
            .filter(|n| {
                n.label.to_lowercase().contains(&query_lower)
                    || query_lower.contains(&n.label.to_lowercase())
                    || query.split_whitespace().any(|w| {
                        n.label.to_lowercase().contains(&w.to_lowercase())
                    })
            })
            .collect();

        if relevant_events.is_empty() {
            return vec![];
        }

        relevant_events.sort_by_key(|n| n.created_at);

        let mut result_cids: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for event in &relevant_events {
            if let Some(ref cid) = event.content_cid {
                if seen.insert(cid.clone()) {
                    result_cids.push(cid.clone());
                }
            }

            if let Ok(neighbors) = kg.get_neighbors(&event.id, Some(KGEdgeType::Follows), 2) {
                for (neighbor, _edge) in neighbors {
                    if let Some(ref cid) = neighbor.content_cid {
                        if seen.insert(cid.clone()) {
                            result_cids.push(cid.clone());
                        }
                    }
                }
            }

            if result_cids.len() >= limit {
                break;
            }
        }

        result_cids.truncate(limit);

        result_cids
            .into_iter()
            .enumerate()
            .filter_map(|(i, cid)| {
                self.cas.get(&cid).ok().map(|obj| {
                    let snippet = String::from_utf8_lossy(
                        &obj.data[..std::cmp::min(200, obj.data.len())],
                    )
                    .to_string();
                    SearchResult {
                        cid,
                        relevance: 1.0 - (i as f32 * 0.05),
                        meta: obj.meta,
                        snippet,
                    }
                })
            })
            .collect()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

