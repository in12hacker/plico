//! Semantic Filesystem Implementation
//!
//! Provides AI-friendly CRUD operations. No paths — only semantic descriptions.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::cas::{AIObject, AIObjectMeta, CASStorage};
use crate::fs::context_loader::ContextLoader;
use crate::fs::embedding::{EmbeddingProvider, EmbedError};
use crate::fs::search::{SemanticSearch, SearchFilter, SearchIndexMeta};
use crate::fs::summarizer::Summarizer;
use crate::fs::graph::KnowledgeGraph;

/// Search query — can be tag-based, semantic, or mixed.
#[derive(Debug, Clone)]
pub enum Query {
    /// Find by exact CID (direct address).
    ByCid(String),
    /// Find by semantic tag(s).
    ByTags(Vec<String>),
    /// Find by natural language query (semantic search).
    /// Uses vector embeddings for semantic similarity.
    Semantic {
        text: String,
        filter: Option<SearchFilter>,
    },
    /// Find by content type.
    ByType(String),
    /// Mixed: tags + semantic query.
    Hybrid {
        tags: Vec<String>,
        semantic: Option<String>,
        content_type: Option<String>,
    },
}

/// A search result with relevance score.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub cid: String,
    pub relevance: f32,
    pub meta: AIObjectMeta,
}

/// The semantic filesystem — a CAS-backed filesystem with AI-friendly operations.
pub struct SemanticFS {
    /// CAS storage backend.
    cas: Arc<CASStorage>,
    /// Tag index: tag → CIDs.
    tag_index: RwLock<HashMap<String, Vec<String>>>,
    /// Path to persist the tag index.
    tag_index_path: std::path::PathBuf,
    /// Context loader for L0/L1/L2 layers.
    ctx_loader: Arc<ContextLoader>,
    /// Recycle bin (logical deletes).
    recycle_bin: RwLock<HashMap<String, RecycleEntry>>,
    /// Update audit log.
    audit_log: RwLock<Vec<AuditEntry>>,
    /// Embedding provider (e.g. Ollama).
    embedding: Arc<dyn EmbeddingProvider>,
    /// Vector search index.
    search_index: Arc<dyn SemanticSearch>,
    /// LLM summarizer for L0/L1 context generation.
    summarizer: Option<Arc<dyn Summarizer>>,
    /// Knowledge graph for entity/relationship tracking.
    knowledge_graph: Option<Arc<dyn KnowledgeGraph>>,
}

#[derive(Debug, Clone)]
struct RecycleEntry {
    cid: String,
    deleted_at: u64,
    original_meta: AIObjectMeta,
}

#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub timestamp: u64,
    pub action: AuditAction,
    pub cid: String,
    pub agent_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuditAction {
    Create,
    Update { previous_cid: String },
    Delete,
}

#[derive(Debug, thiserror::Error)]
pub enum FSError {
    #[error("Object not found: {0}")]
    NotFound(String),

    #[error("CAS error: {0}")]
    CAS(#[from] crate::cas::CASError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Embedding error: {0}")]
    Embedding(#[from] EmbedError),
}

impl SemanticFS {
    /// Create a new semantic filesystem.
    ///
    /// `embedding` — provider for text → vector embeddings (e.g. OllamaBackend).
    /// `search_index` — backend for vector similarity search (e.g. InMemoryBackend).
    /// `summarizer` — optional LLM summarizer for L0/L1 context (e.g. OllamaSummarizer).
    pub fn new(
        root_path: std::path::PathBuf,
        embedding: Arc<dyn EmbeddingProvider>,
        search_index: Arc<dyn SemanticSearch>,
        summarizer: Option<Arc<dyn Summarizer>>,
        knowledge_graph: Option<Arc<dyn KnowledgeGraph>>,
    ) -> std::io::Result<Self> {
        let tag_index_path = root_path.join("tag_index.json");
        let cas = Arc::new(CASStorage::new(root_path.join("objects"))?);

        // Rebuild in-memory tag index from existing CAS objects on startup
        let tag_index = if tag_index_path.exists() {
            Self::load_tag_index(&tag_index_path).unwrap_or_else(|e| {
                tracing::warn!("Failed to load tag index, rebuilding from CAS: {}", e);
                Self::rebuild_tag_index(&cas)
            })
        } else {
            Self::rebuild_tag_index(&cas)
        };

        let fs = Self {
            cas: Arc::clone(&cas),
            tag_index: RwLock::new(tag_index),
            tag_index_path,
            ctx_loader: Arc::new(ContextLoader::new(root_path.join("context"), summarizer.clone(), cas)?),
            recycle_bin: RwLock::new(HashMap::new()),
            audit_log: RwLock::new(Vec::new()),
            embedding,
            search_index,
            summarizer,
            knowledge_graph,
        };

        // Rebuild vector index from persisted CAS objects.
        // The in-memory SemanticSearch index is lost on every restart; re-embed
        // all stored text objects so semantic search works after a cold start.
        fs.rebuild_vector_index();

        Ok(fs)
    }

    /// Rebuild the in-memory vector search index from all CAS objects.
    ///
    /// Called once at startup. Skipped (with a warning) if the embedding
    /// provider is unavailable — the tag-based fallback remains functional.
    fn rebuild_vector_index(&self) {
        let cids = match self.cas.list_cids() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("rebuild_vector_index: failed to list CIDs: {e}");
                return;
            }
        };

        if cids.is_empty() {
            return;
        }

        tracing::debug!("rebuild_vector_index: found {} CIDs", cids.len());

        tracing::info!("Rebuilding vector index for {} objects…", cids.len());
        let mut indexed = 0usize;

        for cid in &cids {
            let obj = match self.cas.get(cid) {
                Ok(o) => o,
                Err(_) => continue,
            };

            // Skip known binary blobs (images, audio, video).
            // Include Unknown — legacy objects stored without type detection.
            if obj.meta.content_type.is_multimedia() {
                continue;
            }

            let text = match std::str::from_utf8(&obj.data) {
                Ok(s) => s.trim().to_string(),
                Err(_) => continue,
            };

            if text.is_empty() {
                continue;
            }

            match self.embedding.embed(&text) {
                Ok(emb) => {
                    self.search_index.upsert(
                        cid,
                        &emb,
                        SearchIndexMeta {
                            cid: cid.clone(),
                            tags: obj.meta.tags.clone(),
                            content_type: obj.meta.content_type.to_string(),
                            snippet: text.chars().take(256).collect(),
                        },
                    );
                    indexed += 1;
                }
                Err(e) => {
                    tracing::warn!("rebuild_vector_index: embed failed for {}: {e}", &cid[..8]);
                    // Stop trying — embedding provider unavailable; tag-based fallback remains.
                    break;
                }
            }
        }

        if indexed > 0 {
            tracing::info!("Vector index rebuilt: {}/{} objects indexed", indexed, cids.len());
        }
    }

    /// **Create**: Store content with semantic metadata. Returns CID.
    ///
    /// Side effects:
    /// 1. Content is stored in CAS
    /// 2. Tags are indexed
    /// 3. Text is embedded and upserted to the vector search index
    /// 4. Audit log entry is created
    pub fn create(
        &self,
        content: Vec<u8>,
        tags: Vec<String>,
        created_by: String,
        intent: Option<String>,
    ) -> std::io::Result<String> {
        // Auto-detect content type: if the bytes are valid UTF-8, treat as text.
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
        };

        let obj = AIObject::new(content.clone(), meta.clone());
        let cid = self.cas.put(&obj)?;

        // Update tag index
        self.update_tag_index(&tags, &cid);

        // Embed and index for semantic search
        self.upsert_semantic_index(&cid, &content, &meta);

        // Upsert to knowledge graph: creates Document node + AssociatesWith edges
        if let Some(ref kg) = self.knowledge_graph {
            if let Err(e) = kg.upsert_document(&cid, &tags, &meta.created_by) {
                tracing::warn!("Failed to upsert document to knowledge graph: {}", e);
            }
        }

        // Audit log
        self.audit_log
            .write()
            .unwrap()
            .push(AuditEntry {
                timestamp: now_ms(),
                action: AuditAction::Create,
                cid: cid.clone(),
                agent_id: String::new(),
            });

        Ok(cid)
    }

    /// **Read**: Retrieve object by CID or query. Optionally at specific context layer.
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
                    if let Ok(obj) = self.cas.get(&cid) {
                        objects.push(obj);
                    }
                }
                Ok(objects)
            }
            Query::Semantic { text, filter } => {
                // Vector semantic search
                let filter = filter.clone().unwrap_or_default();
                let query_emb = match self.embedding.embed(text) {
                    Ok(emb) => emb,
                    Err(e) => {
                        tracing::warn!("Embedding failed for query '{text}': {e}. Falling back to tag search.");
                        // Fallback: tag-based keyword matching
                        let tags = text.split_whitespace().map(String::from).collect();
                        return self.read(&Query::ByTags(tags));
                    }
                };
                let hits = self.search_index.search(&query_emb, 10, &filter);
                let mut objects = Vec::new();
                for hit in hits {
                    if let Ok(obj) = self.cas.get(&hit.cid) {
                        objects.push(obj);
                    }
                }
                Ok(objects)
            }
            Query::ByType(content_type) => {
                // Scan the search index for all entries with the matching content_type.
                let filter = crate::fs::search::SearchFilter {
                    content_type: Some(content_type.clone()),
                    ..Default::default()
                };
                let cids = self.search_index.list_by_filter(&filter);
                let mut objects = Vec::new();
                for cid in cids {
                    if let Ok(obj) = self.cas.get(&cid) {
                        objects.push(obj);
                    }
                }
                Ok(objects)
            }
            Query::Hybrid { tags, semantic, content_type } => {
                // Build a filter from tags + content_type.
                let filter = crate::fs::search::SearchFilter {
                    require_tags: tags.clone(),
                    content_type: content_type.clone(),
                    ..Default::default()
                };

                if let Some(text) = semantic {
                    // Semantic vector search with tag + type filter applied.
                    let query_emb = match self.embedding.embed(text) {
                        Ok(emb) => emb,
                        Err(e) => {
                            tracing::warn!("Embedding failed in Hybrid query: {e}. Falling back to filter scan.");
                            let cids = self.search_index.list_by_filter(&filter);
                            let mut objects = Vec::new();
                            for cid in cids {
                                if let Ok(obj) = self.cas.get(&cid) {
                                    objects.push(obj);
                                }
                            }
                            return Ok(objects);
                        }
                    };
                    let hits = self.search_index.search(&query_emb, 10, &filter);
                    let mut objects = Vec::new();
                    for hit in hits {
                        if let Ok(obj) = self.cas.get(&hit.cid) {
                            objects.push(obj);
                        }
                    }
                    Ok(objects)
                } else {
                    // No semantic text — pure tag+type filter scan.
                    let cids = self.search_index.list_by_filter(&filter);
                    let mut objects = Vec::new();
                    for cid in cids {
                        if let Ok(obj) = self.cas.get(&cid) {
                            objects.push(obj);
                        }
                    }
                    Ok(objects)
                }
            }
        }
    }

    /// **Update**: Replace object content, preserving CID history for rollback.
    /// Returns the new CID (old CID is preserved in audit log).
    pub fn update(
        &self,
        old_cid: &str,
        new_content: Vec<u8>,
        new_tags: Option<Vec<String>>,
        agent_id: String,
    ) -> std::io::Result<String> {
        // Read old object
        let old_obj = self.cas.get(old_cid).map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()))?;

        // Decide on new tags: use new_tags if provided, otherwise keep old ones
        let final_tags = new_tags.unwrap_or_else(|| old_obj.meta.tags.clone());

        let new_meta = AIObjectMeta {
            content_type: old_obj.meta.content_type,
            tags: final_tags.clone(),
            created_by: old_obj.meta.created_by.clone(),
            created_at: now_ms(),
            intent: old_obj.meta.intent.clone(),
        };

        let new_obj = AIObject::new(new_content.clone(), new_meta.clone());
        let new_cid = self.cas.put(&new_obj)?;

        // Update tag index: old CID is gone regardless of whether tags changed,
        // because the content hash changed and index keys on (tag, cid) pairs.
        self.remove_from_tag_index(&old_obj.meta.tags, old_cid);
        self.update_tag_index(&final_tags, &new_cid);

        // Update search index: remove old, add new
        self.search_index.delete(old_cid);
        self.upsert_semantic_index(&new_cid, &new_content, &new_meta);

        // Audit log
        self.audit_log
            .write()
            .unwrap()
            .push(AuditEntry {
                timestamp: now_ms(),
                action: AuditAction::Update {
                    previous_cid: old_cid.to_string(),
                },
                cid: new_cid.clone(),
                agent_id,
            });

        Ok(new_cid)
    }

    /// **Delete**: Logical delete — move to recycle bin (no physical deletion).
    pub fn delete(&self, cid: &str, agent_id: String) -> std::io::Result<()> {
        if let Ok(obj) = self.cas.get(cid) {
            self.recycle_bin
                .write()
                .unwrap()
                .insert(cid.to_string(), RecycleEntry {
                    cid: cid.to_string(),
                    deleted_at: now_ms(),
                    original_meta: obj.meta.clone(),
                });

            // Remove from search index
            self.search_index.delete(cid);

            // Remove from knowledge graph
            if let Some(ref kg) = self.knowledge_graph {
                let _ = kg.remove_node(cid);
            }

            // Remove from tag index
            self.remove_from_tag_index(&obj.meta.tags, cid);

            self.audit_log
                .write()
                .unwrap()
                .push(AuditEntry {
                    timestamp: now_ms(),
                    action: AuditAction::Delete,
                    cid: cid.to_string(),
                    agent_id,
                });
        }
        Ok(())
    }

    /// **Search**: Semantic search across all stored objects.
    /// Uses vector embeddings for semantic similarity.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let query_emb = match self.embedding.embed(query) {
            Ok(emb) => emb,
            Err(e) => {
                tracing::warn!("Embedding failed for query '{query}': {e}. Falling back to tag search.");
                return self.search_by_tags(query);
            }
        };

        let hits = self.search_index.search(&query_emb, limit, &SearchFilter::default());
        hits
            .into_iter()
            .filter_map(|hit| {
                self.cas.get(&hit.cid).ok().map(|obj| SearchResult {
                    cid: hit.cid,
                    relevance: hit.score,
                    meta: obj.meta,
                })
            })
            .collect()
    }

    /// Tag-based keyword search (fallback when embeddings unavailable).
    fn search_by_tags(&self, query: &str) -> Vec<SearchResult> {
        let query_lower = query.to_lowercase();
        let index = self.tag_index.read().unwrap();
        let mut results = Vec::new();

        for (tag, cids) in index.iter() {
            if tag.to_lowercase().contains(&query_lower) {
                for cid in cids {
                    if let Ok(obj) = self.cas.get(cid) {
                        results.push(SearchResult {
                            cid: cid.clone(),
                            relevance: 0.8,
                            meta: obj.meta,
                        });
                    }
                }
            }
        }
        results
    }

    /// List all tags in the filesystem.
    pub fn list_tags(&self) -> Vec<String> {
        let index = self.tag_index.read().unwrap();
        let mut tags: Vec<_> = index.keys().cloned().collect();
        tags.sort();
        tags
    }

    /// Get audit log.
    pub fn audit_log(&self) -> Vec<AuditEntry> {
        self.audit_log.read().unwrap().clone()
    }

    // ─── Internal helpers ────────────────────────────────────────────────

    fn upsert_semantic_index(&self, cid: &str, content: &[u8], meta: &AIObjectMeta) {
        let text = String::from_utf8_lossy(content);

        // Build snippet (first 200 chars of UTF-8 text; empty for binary).
        let snippet = if text.trim().is_empty() {
            String::new()
        } else if text.len() > 200 {
            format!("{}...", &text[..200])
        } else {
            text.to_string()
        };

        // Attempt to embed for semantic search. On failure, use a zero vector so
        // that filter-based queries (ByType, Hybrid tags) still work — only
        // cosine similarity ranking is disabled.
        let embedding = if text.trim().is_empty() {
            vec![0.0f32; self.embedding.dimension()]
        } else {
            match self.embedding.embed(&text) {
                Ok(emb) => emb,
                Err(e) => {
                    tracing::warn!("Failed to embed CID={}: {e}. Indexing with zero vector.", cid);
                    vec![0.0f32; self.embedding.dimension()]
                }
            }
        };

        self.search_index.upsert(cid, &embedding, SearchIndexMeta {
            cid: cid.to_string(),
            tags: meta.tags.clone(),
            snippet,
            content_type: format!("{:?}", meta.content_type).to_lowercase(),
        });
    }

    fn update_tag_index(&self, tags: &[String], cid: &str) {
        let mut index = self.tag_index.write().unwrap();
        for tag in tags {
            index.entry(tag.clone()).or_default().push(cid.to_string());
        }
        drop(index);
        let _ = self.persist_tag_index();
    }

    /// Persist tag index to disk.
    fn persist_tag_index(&self) -> std::io::Result<()> {
        let index = self.tag_index.read().unwrap();
        let json = serde_json::to_vec(&*index)?;
        std::fs::write(&self.tag_index_path, json)
    }

    /// Load tag index from disk.
    fn load_tag_index(path: &std::path::Path) -> std::io::Result<HashMap<String, Vec<String>>> {
        let json = std::fs::read(path)?;
        let index = serde_json::from_slice::<HashMap<String, Vec<String>>>(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(index)
    }

    /// Rebuild tag index by scanning all CAS objects.
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
            if let Some(cids) = index.get_mut(tag) {
                cids.retain(|c| c != cid);
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::embedding::StubEmbeddingProvider;
    use crate::fs::search::InMemoryBackend;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn make_fs(dir: &TempDir) -> SemanticFS {
        SemanticFS::new(
            dir.path().to_path_buf(),
            Arc::new(StubEmbeddingProvider::new()),
            Arc::new(InMemoryBackend::new()),
            None,
            None,
        )
        .unwrap()
    }

    #[test]
    fn context_loader_l2_returns_actual_content() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);

        let expected = b"The quick brown fox";
        let cid = fs
            .create(expected.to_vec(), vec!["test".to_string()], "agent".to_string(), None)
            .unwrap();

        // Load via context loader
        let ctx = fs.ctx_loader.load(&cid, crate::fs::context_loader::ContextLayer::L2).unwrap();
        assert_eq!(ctx.layer, crate::fs::context_loader::ContextLayer::L2);
        assert_eq!(ctx.content.as_bytes(), expected);
        assert!(ctx.tokens_estimate > 0);
    }

    #[test]
    fn by_type_returns_matching_objects() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);

        // Create a text object and a binary object
        let cid_text = fs
            .create(b"hello text".to_vec(), vec!["doc".to_string()], "a".to_string(), None)
            .unwrap();
        let cid_bin = fs
            .create(vec![0x89, 0x50, 0x4E, 0x47], vec!["img".to_string()], "a".to_string(), None)
            .unwrap();

        // Query by type "text"
        let results = fs.read(&Query::ByType("text".to_string())).unwrap();
        let cids: Vec<_> = results.iter().map(|o| o.cid.as_str()).collect();
        assert!(cids.contains(&cid_text.as_str()), "text object must appear in ByType(text)");
        // Binary (PNG magic bytes) should not appear as text
        assert!(!cids.contains(&cid_bin.as_str()), "binary object must not appear in ByType(text)");
    }

    #[test]
    fn hybrid_query_with_tags_filters_correctly() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);

        let cid_a = fs
            .create(b"Rust programming notes".to_vec(), vec!["rust".to_string(), "notes".to_string()], "a".to_string(), None)
            .unwrap();
        let _cid_b = fs
            .create(b"Python tutorial".to_vec(), vec!["python".to_string(), "notes".to_string()], "a".to_string(), None)
            .unwrap();

        // Hybrid with only tags — should return only rust-tagged object
        let results = fs
            .read(&Query::Hybrid {
                tags: vec!["rust".to_string()],
                semantic: None,
                content_type: None,
            })
            .unwrap();

        let cids: Vec<_> = results.iter().map(|o| o.cid.as_str()).collect();
        assert!(cids.contains(&cid_a.as_str()), "rust-tagged object must appear");
        assert_eq!(cids.len(), 1, "only rust-tagged object expected");
    }

    /// Regression test: after update() with unchanged tags, the NEW cid must be
    /// reachable via ByTags and the OLD cid must not appear.
    #[test]
    fn update_tag_index_reflects_new_cid() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);

        let cid1 = fs
            .create(
                b"version one".to_vec(),
                vec!["rust".to_string(), "plico".to_string()],
                "agent-test".to_string(),
                None,
            )
            .unwrap();

        // Update content only (tags unchanged — this was the bug trigger)
        let cid2 = fs
            .update(&cid1, b"version two".to_vec(), None, "agent-test".to_string())
            .unwrap();

        // The two versions must have different CIDs (different content).
        assert_ne!(cid1, cid2, "updated content must produce a new CID");

        // ByTags must return the NEW cid, not the old one.
        let results = fs.read(&Query::ByTags(vec!["rust".to_string()])).unwrap();
        let cids: Vec<_> = results.iter().map(|r| r.cid.as_str()).collect();

        assert!(
            cids.contains(&cid2.as_str()),
            "new CID must be in tag index after update; got {:?}",
            cids
        );
        assert!(
            !cids.contains(&cid1.as_str()),
            "old CID must be removed from tag index after update; got {:?}",
            cids
        );
    }
}
