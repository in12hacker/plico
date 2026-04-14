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
    cas: CASStorage,
    /// Tag index: tag → CIDs.
    tag_index: RwLock<HashMap<String, Vec<String>>>,
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
    ) -> std::io::Result<Self> {
        Ok(Self {
            cas: CASStorage::new(root_path.join("objects"))?,
            tag_index: RwLock::new(HashMap::new()),
            ctx_loader: Arc::new(ContextLoader::new(root_path.join("context"), summarizer.clone())?),
            recycle_bin: RwLock::new(HashMap::new()),
            audit_log: RwLock::new(Vec::new()),
            embedding,
            search_index,
            summarizer,
        })
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
        let meta = AIObjectMeta {
            content_type: crate::cas::ContentType::Unknown,
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
            _ => Ok(Vec::new()),
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

        // Update tag index (remove old, add new)
        if final_tags != old_obj.meta.tags {
            self.remove_from_tag_index(&old_obj.meta.tags, old_cid);
            self.update_tag_index(&final_tags, &new_cid);
        }

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

        // Skip empty content
        if text.trim().is_empty() {
            return;
        }

        let embedding = match self.embedding.embed(&text) {
            Ok(emb) => emb,
            Err(e) => {
                tracing::warn!("Failed to embed CID={}: {e}", cid);
                return;
            }
        };

        // Build snippet (first 200 chars)
        let snippet = if text.len() > 200 {
            format!("{}...", &text[..200])
        } else {
            text.to_string()
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
    }

    fn remove_from_tag_index(&self, tags: &[String], cid: &str) {
        let mut index = self.tag_index.write().unwrap();
        for tag in tags {
            if let Some(cids) = index.get_mut(tag) {
                cids.retain(|c| c != cid);
            }
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

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
