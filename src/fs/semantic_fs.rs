//! Semantic Filesystem Implementation
//!
//! Provides AI-friendly CRUD operations. No paths — only semantic descriptions.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::cas::{AIObject, AIObjectMeta, CASStorage};
use crate::fs::context_loader::ContextLoader;

/// Search query — can be tag-based, semantic, or mixed.
#[derive(Debug, Clone)]
pub enum Query {
    /// Find by exact CID (direct address).
    ByCid(String),
    /// Find by semantic tag(s).
    ByTags(Vec<String>),
    /// Find by natural language query (semantic search).
    Semantic(String),
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
}

#[derive(Debug, Clone)]
struct RecycleEntry {
    cid: String,
    deleted_at: u64,
    original_meta: AIObjectMeta,
}

#[derive(Debug, Clone)]
struct AuditEntry {
    timestamp: u64,
    action: AuditAction,
    cid: String,
    agent_id: String,
}

#[derive(Debug, Clone)]
enum AuditAction {
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
}

impl SemanticFS {
    /// Create a new semantic filesystem backed by CAS at `root_path`.
    pub fn new(root_path: std::path::PathBuf) -> std::io::Result<Self> {
        Ok(Self {
            cas: CASStorage::new(root_path.join("objects"))?,
            tag_index: RwLock::new(HashMap::new()),
            ctx_loader: Arc::new(ContextLoader::new(root_path.join("context"))?),
            recycle_bin: RwLock::new(HashMap::new()),
            audit_log: RwLock::new(Vec::new()),
        })
    }

    /// **Create**: Store content with semantic metadata. Returns CID.
    ///
    /// AI perspective: "Store this. Here is what it means."
    ///
    /// # Example
    ///
    /// ```
    /// let cid = fs.create(
    ///     b"Meeting notes: Project X kickoff...".to_vec(),
    ///     ["meeting", "project-x", "2026-Q2"],
    ///     "Agent_Scheduler_v1",
    ///     Some("Quarterly kickoff meeting notes for project X"),
    /// ).unwrap();
    /// ```
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

        let obj = AIObject::new(content, meta);
        let cid = self.cas.put(&obj)?;

        // Update tag index
        {
            let mut index = self.tag_index.write().unwrap();
            for tag in &tags {
                index.entry(tag.clone()).or_default().push(cid.clone());
            }
        }

        // Log creation
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
            Query::Semantic(query_str) => {
                // Placeholder: full semantic search with embeddings
                // For now, tag-based fallback
                let tags = query_str.split_whitespace().map(String::from).collect();
                self.read(&Query::ByTags(tags))
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

        let new_meta = AIObjectMeta {
            content_type: old_obj.meta.content_type,
            tags: new_tags.unwrap_or(old_obj.meta.tags),
            created_by: old_obj.meta.created_by.clone(),
            created_at: now_ms(),
            intent: old_obj.meta.intent.clone(),
        };

        let new_obj = AIObject::new(new_content, new_meta);
        let new_cid = self.cas.put(&new_obj)?;

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
    /// Placeholder for vector embedding integration.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let query_lower = query.to_lowercase();

        // Simple keyword matching in tags
        let index = self.tag_index.read().unwrap();
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

        results.truncate(limit);
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

    /// Resolve tags to CIDs (union of all matching tags).
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
