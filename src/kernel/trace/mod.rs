//! Tool Call Trace — structured execution recording for debugging and learning.
//!
//! Records every API call as a `Span` in a trace tree. Spans are written
//! asynchronously via a channel to avoid blocking API handlers.
//!
//! Storage: `~/.plico/tool_trace/<YYYY-MM-DD>/<agent_id>.jsonl`
//! Retention: 7 days (configurable via `TRACE_RETENTION_DAYS` env var).

pub mod writer;

use std::path::PathBuf;
use serde::{Deserialize, Serialize};

/// Trace span status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpanStatus {
    Success,
    Error,
    Timeout,
}

impl std::fmt::Display for SpanStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpanStatus::Success => write!(f, "success"),
            SpanStatus::Error => write!(f, "error"),
            SpanStatus::Timeout => write!(f, "timeout"),
        }
    }
}

/// A single trace span — one tool call or intent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Span {
    /// Top-level intent ID (shared across all spans in a trace).
    pub trace_id: String,
    /// Parent span ID (None = root span).
    pub parent_id: Option<String>,
    /// This span's unique ID.
    pub span_id: String,
    /// Agent that made the call.
    pub agent_id: String,
    /// Tool/API name (e.g., "semantic_search", "remember").
    pub tool_name: String,
    /// Serialized input parameters.
    pub input: serde_json::Value,
    /// Serialized output (summary, not full content).
    pub output: serde_json::Value,
    /// Call result status.
    pub status: SpanStatus,
    /// Wall-clock latency in milliseconds.
    pub latency_ms: u64,
    /// When the call started.
    pub timestamp: String,
    /// Associated session ID (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Associated intent ID (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_id: Option<String>,
}

/// Trace store — manages trace file paths and cleanup.
pub struct TraceStore {
    root: PathBuf,
    retention_days: u64,
}

impl TraceStore {
    /// Create a new TraceStore with the given root directory.
    pub fn new(root: PathBuf) -> Self {
        let retention_days = std::env::var("TRACE_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(7);
        Self { root, retention_days }
    }

    /// Trace directory path.
    pub fn trace_dir(&self) -> PathBuf {
        self.root.join("tool_trace")
    }

    /// File path for a specific agent on a specific date.
    pub fn file_path(&self, date: &str, agent_id: &str) -> PathBuf {
        self.trace_dir().join(date).join(format!("{}.jsonl", sanitize_agent_id(agent_id)))
    }

    /// Clean up trace files older than retention period.
    pub fn cleanup_old_traces(&self) -> std::io::Result<usize> {
        let trace_dir = self.trace_dir();
        if !trace_dir.exists() {
            return Ok(0);
        }

        let now = chrono::Utc::now();
        let cutoff = now - chrono::Duration::days(self.retention_days as i64);
        let cutoff_str = cutoff.format("%Y-%m-%d").to_string();

        let mut removed = 0;
        for entry in std::fs::read_dir(&trace_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            // Date directories are YYYY-MM-DD format
            if name.len() == 10 && name.contains('-') && name < cutoff_str {
                std::fs::remove_dir_all(entry.path())?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// List trace files for a given agent and date range.
    pub fn list_files(&self, agent_id: Option<&str>, since: Option<&str>, until: Option<&str>) -> Vec<PathBuf> {
        let trace_dir = self.trace_dir();
        if !trace_dir.exists() {
            return Vec::new();
        }

        let mut files = Vec::new();
        for date_entry in std::fs::read_dir(&trace_dir).into_iter().flatten().flatten() {
            let date_name = date_entry.file_name().to_string_lossy().to_string();
            if date_name.len() != 10 || !date_name.contains('-') {
                continue;
            }
            if let Some(s) = since {
                if date_name.as_str() < s {
                    continue;
                }
            }
            if let Some(u) = until {
                if date_name.as_str() > u {
                    continue;
                }
            }

            if let Some(aid) = agent_id {
                let path = date_entry.path().join(format!("{}.jsonl", sanitize_agent_id(aid)));
                if path.exists() {
                    files.push(path);
                }
            } else {
                // All agents for this date
                if let Ok(dir) = std::fs::read_dir(date_entry.path()) {
                    for f in dir.flatten() {
                        if f.path().extension().is_some_and(|e| e == "jsonl") {
                            files.push(f.path());
                        }
                    }
                }
            }
        }
        files.sort();
        files
    }

    /// Read all spans from a specific trace file.
    pub fn read_spans(&self, path: &std::path::Path) -> std::io::Result<Vec<Span>> {
        let content = std::fs::read_to_string(path)?;
        let mut spans = Vec::new();
        for line in content.lines() {
            if let Ok(span) = serde_json::from_str::<Span>(line) {
                spans.push(span);
            }
        }
        Ok(spans)
    }

    /// Read all spans for a specific trace_id across all files.
    pub fn read_trace(&self, trace_id: &str) -> Vec<Span> {
        let files = self.list_files(None, None, None);
        let mut spans = Vec::new();
        for path in files {
            if let Ok(file_spans) = self.read_spans(&path) {
                spans.extend(file_spans.into_iter().filter(|s| s.trace_id == trace_id));
            }
        }
        spans.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        spans
    }

    /// Read failed spans (error/timeout) for an agent within a time range.
    pub fn read_failures(&self, agent_id: Option<&str>, since: Option<&str>) -> Vec<Span> {
        let files = self.list_files(agent_id, since, None);
        let mut spans = Vec::new();
        for path in files {
            if let Ok(file_spans) = self.read_spans(&path) {
                spans.extend(file_spans.into_iter().filter(|s| s.status != SpanStatus::Success));
            }
        }
        spans
    }
}

/// Sanitize agent ID for use as filename (replace non-alphanumeric with underscore).
fn sanitize_agent_id(agent_id: &str) -> String {
    agent_id.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// Today's date string in YYYY-MM-DD format.
pub fn today_str() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// Generate a UUID for trace/span IDs.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_span_serialization_roundtrip() {
        let span = Span {
            trace_id: "trace-1".into(),
            parent_id: None,
            span_id: "span-1".into(),
            agent_id: "test-agent".into(),
            tool_name: "semantic_search".into(),
            input: serde_json::json!({"query": "test"}),
            output: serde_json::json!({"results_count": 5}),
            status: SpanStatus::Success,
            latency_ms: 42,
            timestamp: "2026-05-17T10:00:00Z".into(),
            session_id: None,
            intent_id: None,
        };

        let json = serde_json::to_string(&span).unwrap();
        let deserialized: Span = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.trace_id, "trace-1");
        assert_eq!(deserialized.tool_name, "semantic_search");
        assert_eq!(deserialized.status, SpanStatus::Success);
        assert_eq!(deserialized.latency_ms, 42);
    }

    #[test]
    fn test_span_with_parent() {
        let span = Span {
            trace_id: "trace-1".into(),
            parent_id: Some("span-root".into()),
            span_id: "span-child".into(),
            agent_id: "test-agent".into(),
            tool_name: "read".into(),
            input: serde_json::json!({"cid": "abc123"}),
            output: serde_json::json!({}),
            status: SpanStatus::Success,
            latency_ms: 5,
            timestamp: "2026-05-17T10:00:01Z".into(),
            session_id: Some("session-1".into()),
            intent_id: None,
        };

        let json = serde_json::to_string(&span).unwrap();
        assert!(json.contains("\"parent_id\":\"span-root\""));
        assert!(json.contains("\"session_id\":\"session-1\""));
    }

    #[test]
    fn test_span_error_status() {
        let span = Span {
            trace_id: "trace-1".into(),
            parent_id: None,
            span_id: "span-err".into(),
            agent_id: "test-agent".into(),
            tool_name: "search".into(),
            input: serde_json::json!({}),
            output: serde_json::json!({"error": "timeout"}),
            status: SpanStatus::Error,
            latency_ms: 5000,
            timestamp: "2026-05-17T10:00:02Z".into(),
            session_id: None,
            intent_id: None,
        };

        let json = serde_json::to_string(&span).unwrap();
        let deserialized: Span = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.status, SpanStatus::Error);
    }

    #[test]
    fn test_trace_store_paths() {
        let store = TraceStore::new(PathBuf::from("/tmp/plico-test"));
        assert_eq!(store.trace_dir(), PathBuf::from("/tmp/plico-test/tool_trace"));
        assert_eq!(
            store.file_path("2026-05-17", "my-agent"),
            PathBuf::from("/tmp/plico-test/tool_trace/2026-05-17/my-agent.jsonl")
        );
    }

    #[test]
    fn test_sanitize_agent_id() {
        assert_eq!(sanitize_agent_id("simple"), "simple");
        assert_eq!(sanitize_agent_id("with-dash"), "with-dash");
        assert_eq!(sanitize_agent_id("with_underscore"), "with_underscore");
        assert_eq!(sanitize_agent_id("with space"), "with_space");
        assert_eq!(sanitize_agent_id("with@special#chars"), "with_special_chars");
    }

    #[test]
    fn test_today_str_format() {
        let today = today_str();
        assert_eq!(today.len(), 10);
        assert!(today.contains('-'));
    }

    #[test]
    fn test_trace_store_cleanup_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::new(dir.path().to_path_buf());
        let removed = store.cleanup_old_traces().unwrap();
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_trace_store_list_files_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::new(dir.path().to_path_buf());
        let files = store.list_files(None, None, None);
        assert!(files.is_empty());
    }

    #[test]
    fn test_trace_store_write_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::new(dir.path().to_path_buf());

        let span = Span {
            trace_id: "trace-1".into(),
            parent_id: None,
            span_id: "span-1".into(),
            agent_id: "test-agent".into(),
            tool_name: "create".into(),
            input: serde_json::json!({"content": "hello"}),
            output: serde_json::json!({"cid": "abc123"}),
            status: SpanStatus::Success,
            latency_ms: 10,
            timestamp: "2026-05-17T10:00:00Z".into(),
            session_id: None,
            intent_id: None,
        };

        // Write span to file
        let path = store.file_path("2026-05-17", "test-agent");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let json = serde_json::to_string(&span).unwrap();
        std::fs::write(&path, format!("{}\n", json)).unwrap();

        // Read back
        let spans = store.read_spans(&path).unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].trace_id, "trace-1");
        assert_eq!(spans[0].tool_name, "create");
    }
}
