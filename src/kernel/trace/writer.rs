//! Trace writer — async JSONL writer via mpsc channel.
//!
//! Receives `Span` from API handlers via a channel and writes them
//! to JSONL files asynchronously, avoiding blocking the request path.

use super::Span;
use std::sync::mpsc;
use std::thread;

/// Background writer that receives spans and appends them to JSONL files.
pub struct TraceWriter {
    sender: mpsc::Sender<Span>,
    _handle: thread::JoinHandle<()>,
}

impl TraceWriter {
    /// Create a new TraceWriter with the given root directory.
    /// Spawns a background thread that reads from the channel and writes JSONL.
    pub fn new(root: std::path::PathBuf) -> Self {
        let (sender, receiver) = mpsc::channel::<Span>();
        let handle = thread::spawn(move || {
            while let Ok(span) = receiver.recv() {
                if let Err(e) = Self::write_span(&root, &span) {
                    tracing::error!("Failed to write trace span: {e}");
                }
            }
        });
        Self {
            sender,
            _handle: handle,
        }
    }

    /// Send a span to the background writer (non-blocking).
    pub fn record(&self, span: Span) {
        if let Err(e) = self.sender.send(span) {
            tracing::error!("Failed to send trace span: {e}");
        }
    }

    /// Write a single span to its JSONL file.
    fn write_span(root: &std::path::Path, span: &Span) -> std::io::Result<()> {
        use super::TraceStore;
        let store = TraceStore::new(root.to_path_buf());
        let path = store.file_path(&super::today_str(), &span.agent_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string(span).map_err(std::io::Error::other)?;
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(file, "{json}")
    }
}

#[cfg(test)]
mod tests {
    use super::super::{new_id, today_str, SpanStatus};
    use super::*;

    #[test]
    fn test_writer_records_span() {
        let dir = tempfile::tempdir().unwrap();
        let writer = TraceWriter::new(dir.path().to_path_buf());
        let span = Span {
            trace_id: "t1".into(),
            parent_id: None,
            span_id: new_id(),
            agent_id: "test-agent".into(),
            tool_name: "search".into(),
            input: serde_json::json!({"q": "test"}),
            output: serde_json::json!({"n": 1}),
            status: SpanStatus::Success,
            latency_ms: 10,
            timestamp: "2026-05-17T10:00:00Z".into(),
            session_id: None,
            intent_id: None,
        };
        writer.record(span);
        // Give writer thread time to process
        std::thread::sleep(std::time::Duration::from_millis(50));

        let store = super::super::TraceStore::new(dir.path().to_path_buf());
        let path = store.file_path(&today_str(), "test-agent");
        assert!(path.exists(), "trace file should exist after record()");
        let spans = store.read_spans(&path).unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].tool_name, "search");
    }
}
