//! Layered Context Loader
//!
//! Provides three tiers of context for efficient AI memory usage:
//!
//! - **L0** (~100 tokens): File summary — what is this file about?
//! - **L1** (~2k tokens): Key sections — important parts of the content
//! - **L2**: Full content — entire file
//!
//! # Principle
//!
//! An AI agent should load only the context it needs. Don't feed a 10k-token
//! document when 100 tokens of summary would suffice.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use super::summarizer::{Summarizer, SummaryLayer};

/// Context loading layer level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextLayer {
    /// ~100 tokens: file summary for quick filtering
    L0,
    /// ~2k tokens: key sections for deep understanding
    L1,
    /// Full content
    L2,
}

impl ContextLayer {
    pub fn tokens_approx(&self) -> usize {
        match self {
            ContextLayer::L0 => 100,
            ContextLayer::L1 => 2000,
            ContextLayer::L2 => usize::MAX,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            ContextLayer::L0 => "L0",
            ContextLayer::L1 => "L1",
            ContextLayer::L2 => "L2",
        }
    }
}

/// A loaded context layer for a CID.
#[derive(Debug, Clone)]
pub struct LoadedContext {
    pub cid: String,
    pub layer: ContextLayer,
    pub content: String,
    pub tokens_estimate: usize,
}

/// Context loader — manages L0/L1/L2 summaries for stored objects.
pub struct ContextLoader {
    root: PathBuf,
    /// In-memory cache for L0 summaries (small, frequently accessed).
    l0_cache: RwLock<HashMap<String, String>>,
    /// Optional LLM summarizer — if None, falls back to heuristic.
    summarizer: Option<Arc<dyn Summarizer>>,
}

impl ContextLoader {
    /// Create a new context loader with an optional summarizer.
    ///
    /// If `summarizer` is `None`, `compute_l0` uses a simple heuristic
    /// (first + last N words) instead of an LLM.
    pub fn new(root: PathBuf, summarizer: Option<Arc<dyn Summarizer>>) -> std::io::Result<Self> {
        fs::create_dir_all(root.join("l0"))?;
        fs::create_dir_all(root.join("l1"))?;
        Ok(Self {
            root,
            l0_cache: RwLock::new(HashMap::new()),
            summarizer,
        })
    }

    /// Load context at the specified layer for a CID.
    pub fn load(&self, cid: &str, layer: ContextLayer) -> std::io::Result<LoadedContext> {
        match layer {
            ContextLayer::L0 => self.load_l0(cid),
            ContextLayer::L1 => self.load_l1(cid),
            ContextLayer::L2 => self.load_l2(cid),
        }
    }

    /// Store a pre-computed L0 summary for a CID.
    pub fn store_l0(&self, cid: &str, summary: String) -> std::io::Result<()> {
        // Update cache
        self.l0_cache
            .write()
            .unwrap()
            .insert(cid.to_string(), summary.clone());

        // Persist to disk — create shard directory first
        let path = self.l0_path(cid);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, summary)
    }

    /// Compute an L0 summary for the given content.
    ///
    /// Uses an LLM summarizer if one was provided at construction time;
    /// otherwise falls back to a simple heuristic (first + last N words).
    pub fn compute_l0(&self, content: &str) -> String {
        if let Some(ref summarizer) = self.summarizer {
            match summarizer.summarize(content, SummaryLayer::L0) {
                Ok(summary) => return summary,
                Err(e) => {
                    tracing::warn!("LLM summarization failed: {e}. Falling back to heuristic.");
                }
            }
        }
        // Fallback heuristic
        let words: Vec<&str> = content.split_whitespace().collect();
        if words.len() <= 20 {
            return content.to_string();
        }
        let first = words[..10].join(" ");
        let last = words[words.len() - 10..].join(" ");
        format!("{} ... {}", first, last)
    }

    fn load_l0(&self, cid: &str) -> std::io::Result<LoadedContext> {
        // Check cache first
        if let Some(content) = self.l0_cache.read().unwrap().get(cid).cloned() {
            let tokens_estimate = content.split_whitespace().count() * 3 / 4;
            return Ok(LoadedContext {
                cid: cid.to_string(),
                layer: ContextLayer::L0,
                content,
                tokens_estimate,
            });
        }

        // Load from disk; if not found, return placeholder (L0 summary is optional)
        let path = self.l0_path(cid);
        let content = fs::read_to_string(&path).unwrap_or_else(|_| {
            format!("[L0 summary not pre-computed for CID={}]", cid)
        });
        let tokens_estimate = content.split_whitespace().count() * 3 / 4;

        // Populate cache
        self.l0_cache
            .write()
            .unwrap()
            .insert(cid.to_string(), content.clone());

        Ok(LoadedContext {
            cid: cid.to_string(),
            layer: ContextLayer::L0,
            content,
            tokens_estimate,
        })
    }

    fn load_l1(&self, cid: &str) -> std::io::Result<LoadedContext> {
        let path = self.l1_path(cid);
        let content = fs::read_to_string(&path).unwrap_or_else(|_| {
            format!("[L1 content not pre-computed for CID={}]", cid)
        });
        let tokens_estimate = content.split_whitespace().count() * 3 / 4;
        Ok(LoadedContext {
            cid: cid.to_string(),
            layer: ContextLayer::L1,
            content,
            tokens_estimate,
        })
    }

    fn load_l2(&self, _cid: &str) -> std::io::Result<LoadedContext> {
        // L2 = full content, loaded directly from CAS
        // This is handled by the SemanticFS layer calling CASStorage
        Ok(LoadedContext {
            cid: _cid.to_string(),
            layer: ContextLayer::L2,
            content: "[Full content — load from CASStorage]".to_string(),
            tokens_estimate: 0,
        })
    }

    fn l0_path(&self, cid: &str) -> PathBuf {
        let (prefix, rest) = cid.split_at(2);
        self.root.join("l0").join(prefix).join(rest)
    }

    fn l1_path(&self, cid: &str) -> PathBuf {
        let (prefix, rest) = cid.split_at(2);
        self.root.join("l1").join(prefix).join(rest)
    }
}
