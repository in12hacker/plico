//! Diagnostic Store — tracks and surfaces background processing failures.
//!
//! Fulfills Soul 3.0 Axiom 7: "All active optimization behaviors are observable."
//! Provides actionable recovery hints for Agents to self-heal their cognitive environment.

use std::collections::{HashMap, VecDeque};
use std::sync::RwLock;
use serde::{Deserialize, Serialize};
use crate::util::now_ms;

/// Maximum number of diagnostic entries to keep in memory.
const MAX_DIAGNOSTICS: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticReport {
    pub task_id: String,
    pub cid: Option<String>,
    pub agent_id: String,
    pub error_msg: String,
    pub timestamp: u64,
    pub recovery_hint: String,
    pub status: DiagnosticStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DiagnosticStatus {
    Pending,
    Fixed,
    Failed,
}

pub struct DiagnosticStore {
    reports: RwLock<VecDeque<DiagnosticReport>>,
    by_agent: RwLock<HashMap<String, Vec<String>>>, // agent_id -> [task_id]
}

impl DiagnosticStore {
    pub fn new() -> Self {
        Self {
            reports: RwLock::new(VecDeque::with_capacity(MAX_DIAGNOSTICS)),
            by_agent: RwLock::new(HashMap::new()),
        }
    }

    pub fn record_failure(&self, agent_id: &str, cid: Option<String>, error: &str) {
        let task_id = uuid::Uuid::new_v4().to_string();
        let hint = match error {
            e if e.contains("batch size") || e.contains("too large") => 
                "Input content is too large for the current model. Consider manual chunking or increasing server batch limits.",
            e if e.contains("501") || e.contains("not supported") =>
                "The configured inference backend does not support this operation (e.g. embeddings disabled).",
            e if e.contains("Timeout") =>
                "The inference server is overloaded or unresponsive. Check server logs and consider retrying.",
            _ => "Generic processing failure. Verify data format and server connectivity.",
        };

        let report = DiagnosticReport {
            task_id: task_id.clone(),
            cid,
            agent_id: agent_id.to_string(),
            error_msg: error.to_string(),
            timestamp: now_ms(),
            recovery_hint: hint.to_string(),
            status: DiagnosticStatus::Pending,
        };

        let mut reports = self.reports.write().unwrap();
        if reports.len() >= MAX_DIAGNOSTICS {
            reports.pop_front();
        }
        reports.push_back(report);

        let mut by_agent = self.by_agent.write().unwrap();
        by_agent.entry(agent_id.to_string()).or_default().push(task_id);
    }

    pub fn list_for_agent(&self, agent_id: &str) -> Vec<DiagnosticReport> {
        let reports = self.reports.read().unwrap();
        reports.iter()
            .filter(|r| r.agent_id == agent_id && r.status == DiagnosticStatus::Pending)
            .cloned()
            .collect()
    }

    pub fn mark_fixed(&self, task_id: &str) {
        let mut reports = self.reports.write().unwrap();
        if let Some(r) = reports.iter_mut().find(|r| r.task_id == task_id) {
            r.status = DiagnosticStatus::Fixed;
        }
    }
}
