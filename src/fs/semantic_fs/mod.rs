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
use crate::fs::search::{SemanticSearch, SearchFilter, SearchIndexMeta, Bm25Index};
use crate::fs::summarizer::Summarizer;
use crate::fs::graph::KnowledgeGraph;
use crate::fs::graph::{KGEdge, KGEdgeType};

// Re-export types from fs/types (single source of truth)
pub use crate::fs::types::{
    Query, SearchResult, AuditEntry, AuditAction, RecycleEntry, FSError,
    EventType, EventMeta, EventRelation, EventSummary,
};

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
        let tag_index_path = root_path.join("tag_index.json");
        let recycle_bin_path = root_path.join("recycle_bin.json");
        let cas = Arc::new(CASStorage::new(root_path.join("objects"))?);

        let tag_index = if tag_index_path.exists() {
            Self::load_tag_index(&tag_index_path).unwrap_or_else(|e| {
                tracing::warn!("Failed to load tag index, rebuilding from CAS: {}", e);
                Self::rebuild_tag_index(&cas)
            })
        } else {
            Self::rebuild_tag_index(&cas)
        };

        let recycle_bin = if recycle_bin_path.exists() {
            Self::load_recycle_bin(&recycle_bin_path).unwrap_or_else(|e| {
                tracing::warn!("Failed to load recycle bin: {}", e);
                HashMap::new()
            })
        } else {
            HashMap::new()
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

        for cid in &cids {
            let obj = match self.cas.get(cid) {
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
                match self.embedding.embed(&text) {
                    Ok(emb) => {
                        self.search_index.upsert(cid, &emb, SearchIndexMeta {
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
            tenant_id: "default".to_string(),
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
                let query_emb = match self.embedding.embed(text) {
                    Ok(emb) => emb,
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
                    let query_emb = match self.embedding.embed(text) {
                        Ok(emb) => emb,
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
        if let Ok(obj) = self.cas.get(cid) {
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
        }
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

    pub fn search_with_filter(&self, query: &str, limit: usize, filter: SearchFilter) -> Vec<SearchResult> {
        let query_emb = self.embedding.embed(query).ok();

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

        const RRF_K: usize = 60;
        let mut rrf_scores: HashMap<String, f32> = HashMap::new();

        for (cid, score) in &vector_hits {
            rrf_scores.insert(cid.clone(), *score);
        }

        let bm25_cids: std::collections::HashSet<String> =
            bm25_hits.iter().map(|(c, _)| c.clone()).collect();

        for (rank, (cid, _bm25_score)) in bm25_hits.iter().enumerate() {
            if let Ok(obj) = self.cas.get(cid) {
                let meta_for_filter = SearchIndexMeta {
                    cid: cid.clone(),
                    tags: obj.meta.tags.clone(),
                    snippet: String::new(),
                    content_type: format!("{:?}", obj.meta.content_type).to_lowercase(),
                    created_at: obj.meta.created_at,
                };
                if !filter.matches(&meta_for_filter) { continue; }
                let entry = rrf_scores.entry(cid.clone()).or_insert(0.0f32);
                *entry += 1.0f32 / (RRF_K as f32 + rank as f32);
            }
        }

        let vector_cids: Vec<String> = vector_hits.keys().cloned().collect();
        for (rank, cid) in vector_cids.iter().enumerate() {
            if !bm25_cids.contains(cid) {
                if let Some(score) = rrf_scores.get_mut(cid) {
                    *score += 1.0f32 / (RRF_K as f32 + rank as f32);
                }
            }
        }

        let mut sorted: Vec<(String, f32)> = rrf_scores.into_iter().collect();
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        sorted.truncate(limit);

        sorted
            .into_iter()
            .filter_map(|(cid, relevance)| {
                self.cas.get(&cid).ok().map(|obj| SearchResult { cid, relevance, meta: obj.meta })
            })
            .collect()
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
                            results.push(SearchResult { cid: cid.clone(), relevance: 0.8, meta: obj.meta });
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

    pub fn audit_log(&self) -> Vec<AuditEntry> {
        self.audit_log.read().unwrap().clone()
    }

    // ─── Internal helpers ────────────────────────────────────────────────

    const SIMILARITY_THRESHOLD: f32 = 0.5;

    fn add_similar_to_edges(&self, kg: &Arc<dyn KnowledgeGraph>, cid: &str, embedding: &[f32]) {
        let filter = SearchFilter::default();
        let similar = self.search_index.search(embedding, 10, &filter);
        for hit in similar {
            if hit.cid == cid || hit.score < Self::SIMILARITY_THRESHOLD {
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
            match self.embedding.embed(&text) {
                Ok(emb) => {
                    is_real_embedding = true;
                    emb
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
        drop(index);
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

    fn rebuild_tag_index(cas: &CASStorage) -> HashMap<String, Vec<String>> {
        let mut index: HashMap<String, Vec<String>> = HashMap::new();
        if let Ok(cids) = cas.list_cids() {
            for cid in cids {
                if let Ok(obj) = cas.get(&cid) {
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
        drop(index);
        let _ = self.persist_tag_index();
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

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

