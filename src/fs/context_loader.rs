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

use crate::cas::CASStorage;
use super::summarizer::{Summarizer, SummaryLayer};

/// Context loading layer level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

    pub fn parse_layer(s: &str) -> Option<Self> {
        s.parse().ok()
    }
}

impl std::str::FromStr for ContextLayer {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s.to_uppercase().as_str() {
            "L0" => Ok(ContextLayer::L0),
            "L1" => Ok(ContextLayer::L1),
            "L2" => Ok(ContextLayer::L2),
            _ => Err(()),
        }
    }
}

/// A loaded context layer for a CID.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoadedContext {
    pub cid: String,
    pub layer: ContextLayer,
    pub content: String,
    pub tokens_estimate: usize,
    /// A-6: Actual layer returned (may differ from requested if degraded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_layer: Option<ContextLayer>,
    /// A-6: Whether content was degraded from requested layer.
    #[serde(default, skip_serializing_if = "not_false")]
    pub degraded: bool,
    /// A-6: Reason for degradation if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degradation_reason: Option<String>,
}

fn not_false(b: &bool) -> bool { !*b }

/// Context loader — manages L0/L1/L2 summaries for stored objects.
pub struct ContextLoader {
    root: PathBuf,
    /// In-memory cache for L0 summaries (small, frequently accessed).
    l0_cache: RwLock<HashMap<String, String>>,
    /// Optional LLM summarizer — if None, falls back to heuristic.
    summarizer: Option<Arc<dyn Summarizer>>,
    /// CAS storage for L2 (full content) loading.
    cas: Arc<CASStorage>,
}

impl ContextLoader {
    /// Create a new context loader with an optional summarizer.
    ///
    /// `cas` — shared CAS storage for L2 full-content loading.
    /// If `summarizer` is `None`, `compute_l0` uses a simple heuristic
    /// (first + last N words) instead of an LLM.
    pub fn new(
        root: PathBuf,
        summarizer: Option<Arc<dyn Summarizer>>,
        cas: Arc<CASStorage>,
    ) -> std::io::Result<Self> {
        fs::create_dir_all(root.join("l0"))?;
        fs::create_dir_all(root.join("l1"))?;
        Ok(Self {
            root,
            l0_cache: RwLock::new(HashMap::new()),
            summarizer,
            cas,
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

    /// Heuristic L0 summary (first+last 10 words) — used as fallback when no LLM.
    fn heuristic_summary(content: &str) -> String {
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
                actual_layer: Some(ContextLayer::L0),
                degraded: false,
                degradation_reason: None,
            });
        }

        // Load from disk; if not found, compute on demand from CAS content.
        let path = self.l0_path(cid);
        let (content, actual_layer, degraded, reason) = match fs::read_to_string(&path) {
            Ok(s) => (s, ContextLayer::L0, false, None),
            Err(_) => {
                // Compute L0 from CAS full content (handles LLM + heuristic fallback).
                let raw = self.cas.get(cid)
                    .map(|obj| String::from_utf8_lossy(&obj.data).into_owned())
                    .unwrap_or_default();
                let words: Vec<&str> = raw.split_whitespace().collect();

                if words.len() <= 20 {
                    // Short content: return full text as L2 (degraded)
                    (raw.clone(), ContextLayer::L2, true,
                     Some("Content too short for summarization; returning full text".into()))
                } else if let Some(ref summarizer) = self.summarizer {
                    match summarizer.summarize(&raw, SummaryLayer::L0) {
                        Ok(summary) => (summary, ContextLayer::L0, false, None),
                        Err(e) => {
                            let heuristic = Self::heuristic_summary(&raw);
                            (heuristic, ContextLayer::L0, true,
                             Some(format!("LLM summarizer failed: {}; using heuristic", e)))
                        }
                    }
                } else {
                    let heuristic = Self::heuristic_summary(&raw);
                    (heuristic, ContextLayer::L0, true,
                     Some("No LLM summarizer available; using heuristic (first+last 10 words)".into()))
                }
            }
        };
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
            actual_layer: Some(actual_layer),
            degraded,
            degradation_reason: reason,
        })
    }

    /// Store a pre-computed L1 summary for a CID.
    pub fn store_l1(&self, cid: &str, content: String) -> std::io::Result<()> {
        let path = self.l1_path(cid);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)
    }

    fn load_l1(&self, cid: &str) -> std::io::Result<LoadedContext> {
        const L1_CHAR_LIMIT: usize = 8_000;

        let path = self.l1_path(cid);
        let content = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => {
                let raw = self.cas.get(cid)
                    .map(|obj| String::from_utf8_lossy(&obj.data).into_owned())
                    .unwrap_or_default();

                if let Some(ref summarizer) = self.summarizer {
                    match summarizer.summarize(&raw, SummaryLayer::L1) {
                        Ok(summary) => {
                            if let Err(e) = self.store_l1(cid, summary.clone()) {
                                tracing::warn!("Failed to cache L1 for {}: {}", &cid[..8.min(cid.len())], e);
                            }
                            summary
                        }
                        Err(e) => {
                            tracing::warn!("L1 summarization failed for {}: {}. Falling back to prefix.", &cid[..8.min(cid.len())], e);
                            Self::prefix_truncate(&raw, L1_CHAR_LIMIT)
                        }
                    }
                } else {
                    Self::prefix_truncate(&raw, L1_CHAR_LIMIT)
                }
            }
        };
        let tokens_estimate = content.split_whitespace().count() * 3 / 4;
        Ok(LoadedContext {
            cid: cid.to_string(),
            layer: ContextLayer::L1,
            content,
            tokens_estimate,
            actual_layer: Some(ContextLayer::L1),
            degraded: false,
            degradation_reason: None,
        })
    }

    fn prefix_truncate(raw: &str, limit: usize) -> String {
        if raw.len() <= limit { raw.to_string() } else { raw[..limit].to_string() }
    }

    fn load_l2(&self, cid: &str) -> std::io::Result<LoadedContext> {
        // L2 = full content, loaded directly from CAS.
        let obj = self.cas.get(cid).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string())
        })?;
        let content = String::from_utf8_lossy(&obj.data).into_owned();
        let tokens_estimate = content.split_whitespace().count() * 3 / 4;
        Ok(LoadedContext {
            cid: cid.to_string(),
            layer: ContextLayer::L2,
            content,
            tokens_estimate,
            actual_layer: Some(ContextLayer::L2),
            degraded: false,
            degradation_reason: None,
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
