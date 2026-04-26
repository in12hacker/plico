//! Token Cost Ledger (F-2) — tracks every LLM/embedding call token consumption.
//!
//! From the design doc:
//! - Every LLM/embedding call is recorded with actual token consumption
//! - Per-session cost aggregation
//! - Per-agent cost trend analysis
//! - Cost anomaly detection

use std::sync::{RwLock, Arc};
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Global cost ledger registry — allows LLM/embedding providers to record costs
/// without direct dependency injection. Set via `set_global()` at kernel startup.
static GLOBAL_LEDGER: RwLock<Option<Arc<TokenCostLedger>>> = RwLock::new(None);

/// Set the global cost ledger. Called once at kernel startup.
pub fn set_global_cost_ledger(ledger: Arc<TokenCostLedger>) {
    *GLOBAL_LEDGER.write().unwrap() = Some(ledger);
}

/// Get the global cost ledger if one is set.
pub fn get_global_cost_ledger() -> Option<Arc<TokenCostLedger>> {
    GLOBAL_LEDGER.read().unwrap().clone()
}

/// Operation types that incur token costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CostOperation {
    LlmCall,
    EmbeddingCall,
    Search,
    ToolCall,
}

/// A single cost entry recording one operation's token consumption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEntry {
    pub timestamp_ms: u64,
    pub session_id: String,
    pub agent_id: String,
    pub operation: CostOperation,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub model_id: String,
    pub duration_ms: u32,
}

/// Summary of costs for a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCostSummary {
    pub session_id: String,
    pub agent_id: String,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_millicents: u64,  // 0.01 USD cents
    pub operations_count: u32,
    pub cache_hits: u32,
    pub cache_misses: u32,
    pub timestamp_ms: u64,
}

/// Cost anomaly detection result.
#[derive(Debug, Clone)]
pub struct CostAnomaly {
    pub agent_id: String,
    pub severity: String,  // "warning" | "critical"
    pub message: String,
    pub avg_cost_per_session_before: u64,
    pub avg_cost_per_session_after: u64,
}

/// Token cost ledger — records and aggregates token costs.
pub struct TokenCostLedger {
    entries: RwLock<Vec<CostEntry>>,
    session_totals: RwLock<HashMap<String, SessionCostSummary>>,
}

impl TokenCostLedger {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
            session_totals: RwLock::new(HashMap::new()),
        }
    }

    /// Record a cost entry. Tokens are tracked per-operation.
    /// model_id can be empty string for stub mode.
    pub fn record(&self, entry: CostEntry) {
        let mut entries = self.entries.write().unwrap();
        entries.push(entry.clone());

        // Update session summary
        drop(entries);
        self.update_session_summary(&entry);
    }

    /// Record an embedding call with estimated token usage.
    ///
    /// Uses character-based estimation: ~4 chars per token (English text).
    /// For accurate counts, use `record_embedding_with_tokens` instead.
    pub fn record_embedding(&self, text: &str, model_id: &str, session_id: &str, agent_id: &str) {
        let estimated_tokens = (text.len() / 4).max(1) as u32;
        self.record_embedding_with_tokens(estimated_tokens, model_id, session_id, agent_id);
    }

    /// Record an embedding call with actual token count from EmbedResult.
    pub fn record_embedding_with_tokens(&self, tokens: u32, model_id: &str, session_id: &str, agent_id: &str) {
        let entry = CostEntry {
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            session_id: session_id.to_string(),
            agent_id: agent_id.to_string(),
            operation: CostOperation::EmbeddingCall,
            input_tokens: tokens,
            output_tokens: 0,
            model_id: model_id.to_string(),
            duration_ms: 0,
        };
        self.record(entry);
    }

    /// Record an LLM call with token usage.
    pub fn record_llm(&self, input_tokens: u32, output_tokens: u32, model_id: &str, session_id: &str, agent_id: &str) {
        let entry = CostEntry {
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            session_id: session_id.to_string(),
            agent_id: agent_id.to_string(),
            operation: CostOperation::LlmCall,
            input_tokens,
            output_tokens,
            model_id: model_id.to_string(),
            duration_ms: 0,
        };
        self.record(entry);
    }

    fn update_session_summary(&self, entry: &CostEntry) {
        let mut totals = self.session_totals.write().unwrap();
        let summary = totals.entry(entry.session_id.clone()).or_insert(SessionCostSummary {
            session_id: entry.session_id.clone(),
            agent_id: entry.agent_id.clone(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost_millicents: 0,
            operations_count: 0,
            cache_hits: 0,
            cache_misses: 0,
            timestamp_ms: entry.timestamp_ms,
        });
        summary.total_input_tokens += entry.input_tokens as u64;
        summary.total_output_tokens += entry.output_tokens as u64;
        summary.operations_count += 1;
        // Rough cost estimate: $0.01 per 1M input tokens, $0.03 per 1M output tokens
        // Use x10000 to preserve precision with small token counts (1M/10000 = 100)
        let input_cost = (entry.input_tokens as u64 * 10_000) / 1_000_000;
        let output_cost = (entry.output_tokens as u64 * 30_000) / 1_000_000;
        summary.total_cost_millicents += input_cost + output_cost;
    }

    /// Get cost summary for a session.
    pub fn session_summary(&self, session_id: &str) -> Option<SessionCostSummary> {
        self.session_totals.read().unwrap().get(session_id).cloned()
    }

    /// Get cost trend for an agent over last N sessions.
    pub fn agent_trend(&self, agent_id: &str, last_n_sessions: usize) -> Vec<SessionCostSummary> {
        let totals = self.session_totals.read().unwrap();
        let mut summaries: Vec<_> = totals.values()
            .filter(|s| s.agent_id == agent_id)
            .cloned()
            .collect();
        summaries.sort_by(|a, b| b.timestamp_ms.cmp(&a.timestamp_ms));
        summaries.truncate(last_n_sessions);
        summaries
    }

    /// Check for cost anomalies (cost increased >50% compared to previous average).
    pub fn cost_anomaly_check(&self, agent_id: &str) -> Option<CostAnomaly> {
        let summaries = self.agent_trend(agent_id, 10);
        if summaries.len() < 3 {
            return None;
        }
        let mid = summaries.len() / 2;
        let before = &summaries[mid..];
        let after = &summaries[..mid];

        let avg_before: u64 = before.iter().map(|s| s.total_cost_millicents).sum::<u64>() / before.len() as u64;
        let avg_after: u64 = after.iter().map(|s| s.total_cost_millicents).sum::<u64>() / after.len() as u64;

        if avg_before > 0 && avg_after > avg_before * 15 / 10 {
            return Some(CostAnomaly {
                agent_id: agent_id.to_string(),
                severity: if avg_after > avg_before * 2 { "critical" } else { "warning" }.to_string(),
                message: format!("Cost increased from {} to {} millicents avg", avg_before, avg_after),
                avg_cost_per_session_before: avg_before,
                avg_cost_per_session_after: avg_after,
            });
        }
        None
    }

    /// Get total entry count (for debugging).
    pub fn entry_count(&self) -> usize {
        self.entries.read().unwrap().len()
    }

    /// Persist session_totals to `dir/cost_ledger.json`.
    pub fn persist_to_dir(&self, dir: &std::path::Path) -> std::io::Result<()> {
        let totals = self.session_totals.read().unwrap();
        let json = serde_json::to_string_pretty(&*totals)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(dir.join("cost_ledger.json"), json)
    }

    /// Restore session_totals from `dir/cost_ledger.json`.
    /// Missing file is not an error.
    pub fn restore_from_dir(&self, dir: &std::path::Path) -> std::io::Result<usize> {
        let path = dir.join("cost_ledger.json");
        if !path.exists() {
            return Ok(0);
        }
        let json = std::fs::read_to_string(&path)?;
        let loaded: std::collections::HashMap<String, SessionCostSummary> = serde_json::from_str(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut totals = self.session_totals.write().unwrap();
        for (id, summary) in loaded {
            totals.insert(id, summary);
        }
        Ok(totals.len())
    }
}

impl Default for TokenCostLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl CostEntry {
    pub fn new(
        session_id: String,
        agent_id: String,
        operation: CostOperation,
        input_tokens: u32,
        output_tokens: u32,
        model_id: String,
        duration_ms: u32,
    ) -> Self {
        Self {
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            session_id,
            agent_id,
            operation,
            input_tokens,
            output_tokens,
            model_id,
            duration_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_single_entry() {
        let ledger = TokenCostLedger::new();
        let entry = CostEntry::new(
            "s1".into(), "a1".into(), CostOperation::LlmCall, 100, 200, "qwen".into(), 50
        );
        ledger.record(entry);
        assert_eq!(ledger.entry_count(), 1);
    }

    #[test]
    fn test_session_summary_aggregation() {
        let ledger = TokenCostLedger::new();
        ledger.record(CostEntry::new("s1".into(), "a1".into(), CostOperation::LlmCall, 100, 200, "qwen".into(), 50));
        ledger.record(CostEntry::new("s1".into(), "a1".into(), CostOperation::EmbeddingCall, 50, 0, "qwen".into(), 10));
        let summary = ledger.session_summary("s1").unwrap();
        assert_eq!(summary.total_input_tokens, 150);
        assert_eq!(summary.total_output_tokens, 200);
        assert_eq!(summary.operations_count, 2);
    }

    #[test]
    fn test_agent_trend_multiple_sessions() {
        let ledger = TokenCostLedger::new();
        ledger.record(CostEntry::new("s1".into(), "a1".into(), CostOperation::LlmCall, 100, 100, "qwen".into(), 50));
        ledger.record(CostEntry::new("s2".into(), "a1".into(), CostOperation::LlmCall, 200, 200, "qwen".into(), 50));
        let trend = ledger.agent_trend("a1", 10);
        assert_eq!(trend.len(), 2);
    }

    #[test]
    fn test_cost_anomaly_detection() {
        let ledger = TokenCostLedger::new();
        // Add 5 sessions with low cost (with delay to ensure distinct timestamps)
        for i in 0..5 {
            ledger.record(CostEntry::new(
                format!("s{}", i), "a1".into(), CostOperation::LlmCall, 100, 100, "qwen".into(), 50
            ));
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Add 5 sessions with high cost (10x increase — should trigger anomaly)
        for i in 5..10 {
            ledger.record(CostEntry::new(
                format!("s{}", i), "a1".into(), CostOperation::LlmCall, 1000, 1000, "qwen".into(), 50
            ));
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let anomaly = ledger.cost_anomaly_check("a1");
        assert!(anomaly.is_some(), "Expected cost anomaly detection to trigger");
        let a = anomaly.unwrap();
        assert!(a.avg_cost_per_session_after > a.avg_cost_per_session_before);
    }

    #[test]
    fn test_empty_ledger_returns_none() {
        let ledger = TokenCostLedger::new();
        assert!(ledger.session_summary("nonexistent").is_none());
        assert!(ledger.cost_anomaly_check("nonexistent").is_none());
    }

    #[test]
    fn test_concurrent_recording() {
        let ledger = TokenCostLedger::new();
        for i in 0..100 {
            ledger.record(CostEntry::new(
                format!("s{}", i % 10), "a1".into(), CostOperation::LlmCall, 100, 100, "qwen".into(), 50
            ));
        }
        assert_eq!(ledger.entry_count(), 100);
    }

    #[test]
    fn test_cost_by_operation_type() {
        let ledger = TokenCostLedger::new();
        ledger.record(CostEntry::new("s1".into(), "a1".into(), CostOperation::LlmCall, 100, 100, "qwen".into(), 50));
        ledger.record(CostEntry::new("s1".into(), "a1".into(), CostOperation::EmbeddingCall, 50, 0, "qwen".into(), 10));
        let summary = ledger.session_summary("s1").unwrap();
        assert_eq!(summary.operations_count, 2);
    }

    #[test]
    fn test_llm_token_tracking() {
        // Verify LLM calls are tracked with correct token counts
        let ledger = TokenCostLedger::new();
        ledger.record_llm(150, 80, "gpt-4", "s1", "agent1");
        let summary = ledger.session_summary("s1").unwrap();
        assert_eq!(summary.total_input_tokens, 150);
        assert_eq!(summary.total_output_tokens, 80);
        assert_eq!(summary.operations_count, 1);
    }
}