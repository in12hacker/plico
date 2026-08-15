//! Durable session lifecycle management.
//!
//! Session start/end are deliberately small state transitions. Derived work such
//! as prefetch, checkpoints, and memory consolidation is not part of the public
//! acknowledgement boundary.

use crate::util::now_ms;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::api::semantic::ChangeEntry;
use crate::kernel::event_bus::EventBus;
use crate::kernel::ops::delta::change_entry_from_event;

// ── F-10: Agent Profile & Intent Key Strategy ─────────────────────────────────

/// Minimum tag length to consider for extraction.
const MIN_TAG_LENGTH: usize = 2;

/// Agent profile for cognitive prefetch (F-10).
///
/// An extracted user/agent preference fact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreferenceFact {
    pub category: String,
    pub preference: String,
    pub confidence: f32,
    pub updated_at_ms: u64,
}

/// Stores statistical patterns of agent behavior:
/// - Intent transitions: which tag keys follow which
/// - Hot objects: frequently accessed CIDs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    pub agent_id: String,
    /// Maps tag key → list of (successor_tag_key, count)
    /// Example: "auth|test" → [("auth|doc", 5), ("auth|perf", 2)]
    pub intent_transitions: HashMap<String, Vec<(String, u32)>>,
    /// Hot objects — (CID, access_count)
    pub hot_objects: Vec<(String, u64)>,
    pub updated_at_ms: u64,
    /// Last intent observed (for transition tracking).
    pub last_intent: Option<String>,
    /// Extracted preference facts for this agent (P1-3).
    pub preference_facts: Vec<PreferenceFact>,
}

impl AgentProfile {
    pub fn new(agent_id: String) -> Self {
        Self {
            agent_id,
            intent_transitions: HashMap::new(),
            hot_objects: Vec::new(),
            updated_at_ms: now_ms(),
            last_intent: None,
            preference_facts: Vec::new(),
        }
    }

    /// Record an intent completion and update transition statistics.
    pub fn record_intent(&mut self, tag_key: &str, next_tag_key: Option<&str>) {
        self.updated_at_ms = now_ms();

        if let Some(next) = next_tag_key {
            let successors = self.intent_transitions.entry(tag_key.to_string()).or_default();
            // Increment count for this transition
            if let Some(entry) = successors.iter_mut().find(|(k, _)| k == next) {
                entry.1 += 1;
            } else {
                successors.push((next.to_string(), 1));
            }
            // Sort by count descending, then alphabetically by key for stable ordering
            successors.sort_by(|a, b| {
                let count_cmp = b.1.cmp(&a.1);
                if count_cmp == std::cmp::Ordering::Equal {
                    a.0.cmp(&b.0)
                } else {
                    count_cmp
                }
            });
            // Keep top 10 successors
            successors.truncate(10);
        }
    }

    /// Get the most likely next tag key based on transition history.
    pub fn predict_next(&self, tag_key: &str) -> Option<String> {
        self.intent_transitions
            .get(tag_key)
            .and_then(|succs| succs.first().map(|(k, _)| k.clone()))
    }

    /// Record CID usage for hot objects tracking.
    pub fn record_cid_usage(&mut self, cid: &str) {
        self.updated_at_ms = now_ms();

        // Find existing entry or add new
        if let Some(existing) = self.hot_objects.iter_mut().find(|(c, _)| c == cid) {
            existing.1 += 1;
        } else {
            self.hot_objects.push((cid.to_string(), 1));
        }

        // Sort by count descending, keep top 50
        self.hot_objects.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        self.hot_objects.truncate(50);
    }

    /// Record multiple CID usages at once.
    pub fn record_cid_usages(&mut self, cids: &[String]) {
        for cid in cids {
            self.record_cid_usage(cid);
        }
    }

    /// Add or update a preference fact for this agent.
    /// If a fact with the same category exists, it is updated if the new confidence is higher.
    pub fn add_preference(&mut self, category: String, preference: String, confidence: f32) {
        self.updated_at_ms = now_ms();
        if let Some(existing) = self.preference_facts.iter_mut().find(|f| f.category == category) {
            if confidence >= existing.confidence {
                existing.preference = preference;
                existing.confidence = confidence;
                existing.updated_at_ms = now_ms();
            }
        } else {
            self.preference_facts.push(PreferenceFact {
                category,
                preference,
                confidence,
                updated_at_ms: now_ms(),
            });
        }
        // Keep top 50 preferences, sorted by confidence
        self.preference_facts.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        self.preference_facts.truncate(50);
    }

    /// Get preference keywords for search augmentation.
    /// Returns preference strings above the given confidence threshold.
    pub fn preference_keywords(&self, min_confidence: f32) -> Vec<&str> {
        self.preference_facts
            .iter()
            .filter(|f| f.confidence >= min_confidence)
            .map(|f| f.preference.as_str())
            .collect()
    }

    /// Decay the access count for an object (used when feedback shows it was unused).
    pub fn decay_object(&mut self, cid: &str) {
        self.updated_at_ms = now_ms();
        if let Some(existing) = self.hot_objects.iter_mut().find(|(c, _)| c == cid) {
            existing.1 = existing.1.saturating_sub(1);
        }
        // Remove objects with zero count
        self.hot_objects.retain(|(_, count)| *count > 0);
        // Re-sort and truncate
        self.hot_objects.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        self.hot_objects.truncate(50);
    }
}

/// Strategy for converting intent text to a lookup key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IntentKeyStrategy {
    /// Extract known tags from intent text, normalize to sorted tag key.
    /// Example: "修复 auth 的测试" → extracts ["auth", "test"] → "auth|test"
    TagExtraction,
    /// Cluster similar intents using embedding (requires real embedding provider).
    EmbeddingCluster { bucket_count: usize },
    /// Disabled — stub embedding mode without tag extraction.
    Disabled,
}

impl IntentKeyStrategy {
    /// Extract tags from intent text using known tag dictionary.
    ///
    /// For TagExtraction mode:
    /// - Finds all known tags that appear in the intent text (case-insensitive)
    /// - Returns normalized key as sorted, pipe-separated tags
    ///
    /// Returns None if no tags found or strategy is Disabled.
    pub fn extract_tag_key(&self, intent: &str, known_tags: &[String]) -> Option<String> {
        match self {
            IntentKeyStrategy::TagExtraction => {
                let intent_lower = intent.to_lowercase();
                let mut matched: Vec<&str> = known_tags
                    .iter()
                    .filter(|tag| {
                        let t = tag.to_lowercase();
                        t.len() >= MIN_TAG_LENGTH && intent_lower.contains(&t)
                    })
                    .map(|s| s.as_str())
                    .collect();

                if matched.is_empty() {
                    return None;
                }

                // Sort and deduplicate
                matched.sort();
                matched.dedup();
                // Take up to 5 tags to avoid overly specific keys
                matched.truncate(5);

                Some(matched.join("|"))
            }
            IntentKeyStrategy::EmbeddingCluster { .. } => {
                // For embedding cluster, we return None here — caller should use embedding
                None
            }
            IntentKeyStrategy::Disabled => None,
        }
    }

    /// Check if this strategy requires embedding (for gating).
    pub fn requires_embedding(&self) -> bool {
        matches!(self, IntentKeyStrategy::EmbeddingCluster { .. })
    }

    /// Check if this strategy is effectively disabled.
    pub fn is_disabled(&self) -> bool {
        matches!(self, IntentKeyStrategy::Disabled)
    }
}

/// Default strategy is TagExtraction when tags are available.
impl Default for IntentKeyStrategy {
    fn default() -> Self {
        IntentKeyStrategy::TagExtraction
    }
}

/// Default session TTL: 30 minutes of inactivity before auto-EndSession.
const DEFAULT_SESSION_TTL_MS: u64 = 30 * 60 * 1000;
/// Interval between session timeout scans: 60 seconds.
const SESSION_SCAN_INTERVAL_SECS: u64 = 60;
/// Maximum completed sessions to retain per agent for growth reporting.
#[cfg(test)]
const MAX_COMPLETED_SESSIONS_PER_AGENT: usize = 100;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("session {session_id} does not belong to agent {agent_id}")]
    Ownership { session_id: String, agent_id: String },
    #[error("failed to persist session {operation}: {source}")]
    Persistence {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },
}

impl SessionError {
    fn category(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "not_found",
            Self::Ownership { .. } => "ownership",
            Self::Persistence { .. } => "persistence",
        }
    }
}

/// An active session tracked by SessionStore.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ActiveSession {
    pub session_id: String,
    pub agent_id: String,
    pub created_at_ms: u64,
    pub last_active_ms: u64,
    /// Seq number at session start — used for delta calculation.
    pub start_seq: u64,
    /// The current declared intent for this session — used for causal tracking.
    pub current_intent: Option<String>,
}

impl ActiveSession {
    fn new(session_id: String, agent_id: String, start_seq: u64) -> Self {
        let now = now_ms();
        Self {
            session_id,
            agent_id,
            created_at_ms: now,
            last_active_ms: now,
            start_seq,
            current_intent: None,
        }
    }

    fn touch(&mut self) {
        self.last_active_ms = now_ms();
    }

    fn is_expired(&self, ttl_ms: u64) -> bool {
        now_ms() - self.last_active_ms > ttl_ms
    }
}

/// A completed session record for growth reporting + cross-session recall.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompletedSession {
    pub session_id: String,
    pub agent_id: String,
    pub created_at_ms: u64,
    pub ended_at_ms: u64,
    pub tokens_used: usize,
    /// Lightweight session summary: top tags + object count.
    #[serde(default)]
    pub summary: Option<SessionSummary>,
    /// CIDs created during this session (for cross-session delta).
    #[serde(default)]
    pub created_cids: Vec<String>,
}

/// Lightweight session summary generated at EndSession for cross-session recall.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionSummary {
    pub top_tags: Vec<String>,
    pub object_count: usize,
    pub intent: Option<String>,
    /// CID of the summary object stored in CAS (for retrieval at next session).
    pub summary_cid: Option<String>,
}

/// Session store — manages active sessions and timeout scanning.
pub struct SessionStore {
    sessions: RwLock<HashMap<String, ActiveSession>>,
    /// Completed sessions for growth reporting, keyed by agent_id.
    completed_sessions: RwLock<HashMap<String, Vec<CompletedSession>>>,
    ttl_ms: u64,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            completed_sessions: RwLock::new(HashMap::new()),
            ttl_ms: DEFAULT_SESSION_TTL_MS,
        }
    }

    /// In-memory helper retained for store-level tests only. Production session
    /// transitions must use [`start_session`] so acknowledgement implies persistence.
    #[cfg(test)]
    fn start_session(&self, session_id: String, agent_id: String, start_seq: u64) -> ActiveSession {
        let session = ActiveSession::new(session_id, agent_id, start_seq);
        let mut sessions = self.sessions.write().unwrap();
        sessions.insert(session.session_id.clone(), session.clone());
        session
    }

    #[cfg(test)]
    fn end_session(&self, session_id: &str, tokens_used: Option<usize>) -> Option<ActiveSession> {
        self.end_session_with_summary(session_id, tokens_used, None, Vec::new())
    }

    /// End a session with an optional summary for cross-session recall.
    #[cfg(test)]
    fn end_session_with_summary(
        &self,
        session_id: &str,
        tokens_used: Option<usize>,
        summary: Option<SessionSummary>,
        created_cids: Vec<String>,
    ) -> Option<ActiveSession> {
        let session = {
            let mut sessions = self.sessions.write().unwrap();
            sessions.remove(session_id)
        };

        if let Some(ref session) = session {
            if let Some(tokens) = tokens_used {
                self.record_completion(session.clone(), tokens, summary, created_cids);
            }
        }

        session
    }

    /// Record a completed session for an agent.
    #[cfg(test)]
    fn record_completion(
        &self,
        session: ActiveSession,
        tokens_used: usize,
        summary: Option<SessionSummary>,
        created_cids: Vec<String>,
    ) {
        let completed = CompletedSession {
            session_id: session.session_id,
            agent_id: session.agent_id.clone(),
            created_at_ms: session.created_at_ms,
            ended_at_ms: now_ms(),
            tokens_used,
            summary,
            created_cids,
        };

        let mut completed_map = self.completed_sessions.write().unwrap();
        let sessions = completed_map.entry(session.agent_id).or_default();
        sessions.push(completed);

        // Limit stored completed sessions per agent
        if sessions.len() > MAX_COMPLETED_SESSIONS_PER_AGENT {
            sessions.remove(0);
        }
    }

    /// Get completed sessions for an agent within a time period.
    pub fn get_completed_sessions(&self, agent_id: &str, period_ms: Option<u64>) -> Vec<CompletedSession> {
        let completed_map = self.completed_sessions.read().unwrap();
        let sessions = completed_map.get(agent_id).cloned().unwrap_or_default();

        if let Some(period) = period_ms {
            let cutoff = now_ms() - period;
            sessions.into_iter().filter(|s| s.ended_at_ms >= cutoff).collect()
        } else {
            sessions
        }
    }

    /// Get the count of completed sessions for an agent.
    pub fn completed_session_count(&self, agent_id: &str) -> u64 {
        let completed_map = self.completed_sessions.read().unwrap();
        completed_map.get(agent_id).map(|s| s.len() as u64).unwrap_or(0)
    }

    /// Get a session by ID.
    pub fn get(&self, session_id: &str) -> Option<ActiveSession> {
        let sessions = self.sessions.read().unwrap();
        sessions.get(session_id).cloned()
    }

    /// Touch a session to update its last_active timestamp.
    pub fn touch(&self, session_id: &str) {
        let mut sessions = self.sessions.write().unwrap();
        if let Some(session) = sessions.get_mut(session_id) {
            session.touch();
        }
    }

    /// Set the current intent for an agent's active session (for causal tracking).
    pub fn set_current_intent(&self, agent_id: &str, intent: Option<String>) {
        let mut sessions = self.sessions.write().unwrap();
        for session in sessions.values_mut() {
            if session.agent_id == agent_id {
                session.current_intent = intent.clone();
            }
        }
    }

    /// List all active sessions.
    pub fn list(&self) -> Vec<ActiveSession> {
        let sessions = self.sessions.read().unwrap();
        sessions.values().cloned().collect()
    }

    /// Get active session count for a specific agent (L-5).
    pub fn active_session_count(&self, agent_id: &str) -> usize {
        let sessions = self.sessions.read().unwrap();
        sessions.values().filter(|s| s.agent_id == agent_id).count()
    }

    /// Get total active session count across all agents (F-7).
    pub fn total_active_count(&self) -> usize {
        let sessions = self.sessions.read().unwrap();
        sessions.len()
    }

    /// Get active sessions for an agent within a time period (L-5).
    pub fn get_active_sessions(&self, agent_id: &str, cutoff_ms: Option<u64>) -> Vec<ActiveSession> {
        let sessions = self.sessions.read().unwrap();
        sessions
            .values()
            .filter(|s| s.agent_id == agent_id && cutoff_ms.map(|c| s.created_at_ms >= c).unwrap_or(true))
            .cloned()
            .collect()
    }

    /// Find all expired sessions (for timeout scanning).
    pub fn expired_sessions(&self) -> Vec<ActiveSession> {
        let sessions = self.sessions.read().unwrap();
        sessions
            .values()
            .filter(|s| s.is_expired(self.ttl_ms))
            .cloned()
            .collect()
    }

    /// Get the TTL in milliseconds.
    pub fn ttl_ms(&self) -> u64 {
        self.ttl_ms
    }

    /// Cross-session recall: get recent completed session summaries for an agent
    /// to provide cross-session context at StartSession time.
    pub fn recent_session_summaries(&self, agent_id: &str, max_sessions: usize) -> Vec<SessionSummary> {
        let completed_map = self.completed_sessions.read().unwrap();
        let sessions = match completed_map.get(agent_id) {
            Some(s) => s,
            None => return vec![],
        };
        sessions
            .iter()
            .rev()
            .take(max_sessions)
            .filter_map(|s| s.summary.clone())
            .collect()
    }

    /// Path to the active sessions persistence file.
    fn sessions_path(root: &Path) -> PathBuf {
        root.join("sessions.json")
    }

    /// Path to the completed sessions persistence file.
    fn completed_sessions_path(root: &Path) -> PathBuf {
        root.join("completed_sessions.json")
    }

    fn persist_active_snapshot(root: &Path, sessions: &HashMap<String, ActiveSession>) -> std::io::Result<()> {
        let sessions_data = serde_json::to_vec_pretty(sessions).map_err(|e| std::io::Error::other(e.to_string()))?;
        let sessions_path = Self::sessions_path(root);
        let tmp = sessions_path.with_extension("json.tmp");
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(&sessions_data)?;
            file.sync_all()?;
        }
        std::fs::rename(&tmp, &sessions_path)
    }

    fn begin_persisted(
        &self,
        session_id: String,
        agent_id: String,
        start_seq: u64,
        root: &Path,
    ) -> Result<ActiveSession, SessionError> {
        let session = ActiveSession::new(session_id, agent_id, start_seq);
        let mut sessions = self.sessions.write().unwrap();
        sessions.insert(session.session_id.clone(), session.clone());

        if let Err(source) = Self::persist_active_snapshot(root, &sessions) {
            sessions.remove(&session.session_id);
            return Err(SessionError::Persistence {
                operation: "start",
                source,
            });
        }

        Ok(session)
    }

    fn finish_persisted(&self, agent_id: &str, session_id: &str, root: &Path) -> Result<ActiveSession, SessionError> {
        let mut sessions = self.sessions.write().unwrap();
        let session = sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;
        if session.agent_id != agent_id {
            return Err(SessionError::Ownership {
                session_id: session_id.to_string(),
                agent_id: agent_id.to_string(),
            });
        }

        sessions.remove(session_id);
        if let Err(source) = Self::persist_active_snapshot(root, &sessions) {
            sessions.insert(session_id.to_string(), session.clone());
            return Err(SessionError::Persistence {
                operation: "end",
                source,
            });
        }

        Ok(session)
    }

    /// Persist active and completed sessions to disk (A-1).
    pub fn persist(&self, root: &Path) -> std::io::Result<()> {
        // Persist active sessions
        let sessions = self.sessions.read().unwrap();
        Self::persist_active_snapshot(root, &sessions)?;

        // Persist completed sessions
        drop(sessions);
        let completed = self.completed_sessions.read().unwrap();
        let completed_data =
            serde_json::to_string_pretty(&*completed).map_err(|e| std::io::Error::other(e.to_string()))?;
        let completed_path = Self::completed_sessions_path(root);
        let tmp = completed_path.with_extension("json.tmp");
        std::fs::write(&tmp, &completed_data)?;
        std::fs::rename(&tmp, &completed_path)?;

        Ok(())
    }

    /// Restore sessions from disk (A-1).
    pub fn restore(root: &Path) -> Self {
        let store = Self::new();

        // Restore active sessions
        let sessions_path = Self::sessions_path(root);
        if sessions_path.exists() {
            if let Ok(data) = std::fs::read_to_string(&sessions_path) {
                if let Ok(sessions) = serde_json::from_str::<HashMap<String, ActiveSession>>(&data) {
                    *store.sessions.write().unwrap() = sessions;
                    tracing::info!(
                        "Restored {} active sessions from {}",
                        store.sessions.read().unwrap().len(),
                        sessions_path.display()
                    );
                }
            }
        }

        // Restore completed sessions
        let completed_path = Self::completed_sessions_path(root);
        if completed_path.exists() {
            if let Ok(data) = std::fs::read_to_string(&completed_path) {
                if let Ok(completed) = serde_json::from_str::<HashMap<String, Vec<CompletedSession>>>(&data) {
                    *store.completed_sessions.write().unwrap() = completed;
                    tracing::info!("Restored completed sessions from {}", completed_path.display());
                }
            }
        }

        store
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Inputs for the durable session start transition.
pub struct StartSessionParams<'a> {
    pub agent_id: &'a str,
    pub last_seen_seq: Option<u64>,
    pub session_store: &'a SessionStore,
    pub event_bus: &'a Arc<EventBus>,
    pub root: &'a Path,
}

/// Start a session and durably record it before acknowledging success.
pub fn start_session(params: StartSessionParams<'_>) -> Result<StartSessionResult, SessionError> {
    let StartSessionParams {
        agent_id,
        last_seen_seq,
        session_store,
        event_bus,
        root,
    } = params;
    let watermark = event_bus.current_seq();

    let session_id = uuid::Uuid::new_v4().to_string();
    match session_store.begin_persisted(session_id.clone(), agent_id.to_string(), watermark, root) {
        Ok(_) => tracing::info!(
            role_id = agent_id,
            session_id,
            ?last_seen_seq,
            current_seq = watermark,
            phase = "persist",
            outcome = "success",
            "session start persisted"
        ),
        Err(error) => {
            tracing::warn!(
                role_id = agent_id,
                session_id,
                ?last_seen_seq,
                current_seq = watermark,
                phase = "persist",
                outcome = "error",
                error_category = error.category(),
                "session start persistence failed"
            );
            return Err(error);
        }
    }

    let since_seq = last_seen_seq.unwrap_or(watermark);
    let changes_since_last: Vec<ChangeEntry> = if since_seq < watermark {
        event_bus
            .events_since(since_seq)
            .into_iter()
            .filter(|event| event.event.agent_id() == Some(agent_id))
            .map(|event| change_entry_from_event(&event))
            .collect()
    } else {
        vec![]
    };

    tracing::info!(
        role_id = agent_id,
        session_id,
        ?last_seen_seq,
        current_seq = watermark,
        phase = "publish",
        outcome = "success",
        "session start acknowledged"
    );

    Ok(StartSessionResult {
        session_id,
        changes_since_last,
        watermark,
    })
}

/// End a session and durably remove it before acknowledging success.
pub fn end_session(
    agent_id: &str,
    session_id: &str,
    session_store: &SessionStore,
    root: &Path,
    event_bus: &EventBus,
) -> Result<EndSessionResult, SessionError> {
    let last_seq = event_bus.current_seq();
    if let Err(error) = session_store.finish_persisted(agent_id, session_id, root) {
        tracing::warn!(
            role_id = agent_id,
            session_id,
            current_seq = last_seq,
            phase = "persist",
            outcome = "error",
            error_category = error.category(),
            "session end persistence failed"
        );
        return Err(error);
    }
    tracing::info!(
        role_id = agent_id,
        session_id,
        current_seq = last_seq,
        phase = "persist",
        outcome = "success",
        "session end persisted"
    );
    tracing::info!(
        role_id = agent_id,
        session_id,
        current_seq = last_seq,
        phase = "publish",
        outcome = "success",
        "session end acknowledged"
    );
    Ok(EndSessionResult { last_seq })
}

/// Result of StartSession orchestration.
#[derive(Debug)]
pub struct StartSessionResult {
    pub session_id: String,
    pub changes_since_last: Vec<ChangeEntry>,
    pub watermark: u64,
}

/// Result of EndSession orchestration.
#[derive(Debug)]
pub struct EndSessionResult {
    pub last_seq: u64,
}

/// Spawn a background task that periodically scans for expired sessions
/// and applies the same durable end transition used by explicit calls.
pub fn spawn_session_timeout_scanner(session_store: Arc<SessionStore>, event_bus: Arc<EventBus>, root: PathBuf) {
    let root_clone = root.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(SESSION_SCAN_INTERVAL_SECS));

        let expired = session_store.expired_sessions();
        for session in expired {
            tracing::info!(
                role_id = session.agent_id,
                session_id = session.session_id,
                current_seq = event_bus.current_seq(),
                phase = "timeout",
                outcome = "detected",
                ttl_ms = session_store.ttl_ms(),
                "expired session detected"
            );

            if let Err(error) = end_session(
                &session.agent_id,
                &session.session_id,
                &session_store,
                &root_clone,
                &event_bus,
            ) {
                tracing::warn!(
                    session_id = session.session_id,
                    role_id = session.agent_id,
                    current_seq = event_bus.current_seq(),
                    phase = "timeout",
                    outcome = "error",
                    error_category = error.category(),
                    "failed to durably end expired session"
                );
            } else {
                tracing::info!(
                    session_id = session.session_id,
                    role_id = session.agent_id,
                    current_seq = event_bus.current_seq(),
                    phase = "timeout",
                    outcome = "success",
                    "expired session durably ended"
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_store_start_end() {
        let store = SessionStore::new();
        let session_id = "test-session-1".to_string();
        let agent_id = "agent-1".to_string();

        let session = store.start_session(session_id.clone(), agent_id.clone(), 0);
        assert_eq!(session.session_id, session_id);
        assert_eq!(session.agent_id, agent_id);

        let retrieved = store.get(&session_id).unwrap();
        assert_eq!(retrieved.agent_id, agent_id);

        let removed = store.end_session(&session_id, None).unwrap();
        assert_eq!(removed.session_id, session_id);

        assert!(store.get(&session_id).is_none());
    }

    #[test]
    fn test_session_expiry() {
        let _store = SessionStore::new();
        // Create a session that's already expired (we can't easily test this without mocking time)
        // But we can test the expiry detection logic
        let session = ActiveSession::new("s1".to_string(), "a1".to_string(), 10);
        // Manually set last_active_ms to something old
        let mut old_session = session;
        old_session.last_active_ms = 0; // 0 ms ago = always expired with any TTL > 0

        // With any reasonable TTL, this should be expired
        assert!(old_session.is_expired(1)); // 1ms TTL
    }

    #[test]
    fn test_session_touch() {
        let store = SessionStore::new();
        let session = store.start_session("s1".to_string(), "a1".to_string(), 0);
        let original_last_active = session.last_active_ms;

        // Sleep a tiny bit then touch
        std::thread::sleep(Duration::from_millis(1));
        store.touch("s1");

        let updated = store.get("s1").unwrap();
        assert!(updated.last_active_ms >= original_last_active);
    }

    #[test]
    fn test_active_session_new() {
        let session = ActiveSession::new("sid".to_string(), "aid".to_string(), 42);
        assert_eq!(session.session_id, "sid");
        assert_eq!(session.agent_id, "aid");
        assert_eq!(session.start_seq, 42);
        assert!(session.created_at_ms > 0);
    }

    // ── F-10: AgentProfile tests ──────────────────────────────────────────────

    #[test]
    fn test_agent_profile_new() {
        let profile = AgentProfile::new("agent-1".to_string());
        assert_eq!(profile.agent_id, "agent-1");
        assert!(profile.intent_transitions.is_empty());
        assert!(profile.hot_objects.is_empty());
        assert!(profile.updated_at_ms > 0);
    }

    #[test]
    fn test_agent_profile_record_intent() {
        let mut profile = AgentProfile::new("agent-1".to_string());

        // Record a transition: auth|test -> auth|doc
        profile.record_intent("auth|test", Some("auth|doc"));

        let successors = profile.intent_transitions.get("auth|test");
        assert!(successors.is_some());
        let succs = successors.unwrap();
        assert_eq!(succs.len(), 1);
        assert_eq!(succs[0].0, "auth|doc");
        assert_eq!(succs[0].1, 1);
    }

    #[test]
    fn test_agent_profile_record_intent_increments_count() {
        let mut profile = AgentProfile::new("agent-1".to_string());

        // Record same transition twice
        profile.record_intent("auth|test", Some("auth|doc"));
        profile.record_intent("auth|test", Some("auth|doc"));

        let successors = profile.intent_transitions.get("auth|test").unwrap();
        assert_eq!(successors.len(), 1);
        assert_eq!(successors[0].1, 2); // Count should be 2
    }

    #[test]
    fn test_agent_profile_predict_next() {
        let mut profile = AgentProfile::new("agent-1".to_string());

        // Add multiple successors
        profile.record_intent("auth|test", Some("auth|doc"));
        profile.record_intent("auth|test", Some("auth|perf"));
        profile.record_intent("auth|test", Some("auth|doc")); // auth|doc becomes more frequent

        let predicted = profile.predict_next("auth|test");
        assert!(predicted.is_some());
        // auth|doc has count 2, should be predicted
        assert_eq!(predicted.unwrap(), "auth|doc");
    }

    #[test]
    fn test_agent_profile_predict_next_no_history() {
        let profile = AgentProfile::new("agent-1".to_string());
        let predicted = profile.predict_next("nonexistent");
        assert!(predicted.is_none());
    }

    #[test]
    fn test_agent_profile_multiple_successors_sorted() {
        let mut profile = AgentProfile::new("agent-1".to_string());

        // Add successors in reverse order
        profile.record_intent("auth", Some("c"));
        profile.record_intent("auth", Some("b"));
        profile.record_intent("auth", Some("a"));

        let successors = profile.intent_transitions.get("auth").unwrap();
        // Should be sorted by count descending, so all have count 1
        assert_eq!(successors.len(), 3);
        // First should be "a" (alphabetically first among equal counts)
        assert_eq!(successors[0].0, "a");
    }

    // ── F-10: IntentKeyStrategy tests ──────────────────────────────────────────

    #[test]
    fn test_tag_extraction_extracts_matching_tags() {
        let strategy = IntentKeyStrategy::TagExtraction;
        let known_tags = vec![
            "auth".to_string(),
            "test".to_string(),
            "doc".to_string(),
            "api".to_string(),
        ];

        // "修复 auth 和 test" should extract "auth" and "test"
        let tag_key = strategy.extract_tag_key("修复 auth 和 test", &known_tags);
        assert!(tag_key.is_some());
        let key = tag_key.unwrap();
        // Tags should be sorted alphabetically
        assert_eq!(key, "auth|test");
    }

    #[test]
    fn test_tag_extraction_no_match() {
        let strategy = IntentKeyStrategy::TagExtraction;
        let known_tags = vec!["auth".to_string(), "test".to_string()];

        // "修复完全不相关的内容" should not match any tags
        let tag_key = strategy.extract_tag_key("修复完全不相关的内容", &known_tags);
        assert!(tag_key.is_none());
    }

    #[test]
    fn test_tag_extraction_empty_tags() {
        let strategy = IntentKeyStrategy::TagExtraction;
        let known_tags: Vec<String> = vec![];

        let tag_key = strategy.extract_tag_key("任何内容", &known_tags);
        assert!(tag_key.is_none());
    }

    #[test]
    fn test_tag_extraction_deduplicates() {
        let strategy = IntentKeyStrategy::TagExtraction;
        let known_tags = vec!["auth".to_string(), "test".to_string()];

        // Intent mentions "auth" twice and test once
        let tag_key = strategy.extract_tag_key("auth auth auth test", &known_tags);
        assert!(tag_key.is_some());
        let key = tag_key.unwrap();
        // Should only have unique tags, sorted alphabetically
        assert_eq!(key, "auth|test");
    }

    #[test]
    fn test_tag_extraction_case_insensitive() {
        let strategy = IntentKeyStrategy::TagExtraction;
        let known_tags = vec!["Auth".to_string(), "TEST".to_string()];

        // Should match regardless of case
        let tag_key = strategy.extract_tag_key("AUTH and test", &known_tags);
        assert!(tag_key.is_some());
    }

    #[test]
    fn test_tag_extraction_short_tag_filter() {
        let strategy = IntentKeyStrategy::TagExtraction;
        // "a" is too short (MIN_TAG_LENGTH = 2)
        let known_tags = vec!["a".to_string(), "ab".to_string()];

        let tag_key = strategy.extract_tag_key("修复 a 和 ab 的问题", &known_tags);
        assert!(tag_key.is_some());
        let key = tag_key.unwrap();
        // Should only include "ab", not "a"
        assert_eq!(key, "ab");
    }

    #[test]
    fn test_tag_extraction_disabled_always_returns_none() {
        let strategy = IntentKeyStrategy::Disabled;
        let known_tags = vec!["auth".to_string(), "test".to_string()];

        let tag_key = strategy.extract_tag_key("修复 auth 测试", &known_tags);
        assert!(tag_key.is_none());
    }

    #[test]
    fn test_embedding_cluster_returns_none() {
        let strategy = IntentKeyStrategy::EmbeddingCluster { bucket_count: 10 };
        let known_tags = vec!["auth".to_string(), "test".to_string()];

        // TagExtraction mode should return None (caller should use embedding)
        let tag_key = strategy.extract_tag_key("修复 auth 测试", &known_tags);
        assert!(tag_key.is_none());
    }

    #[test]
    fn test_strategy_requires_embedding() {
        assert!(!IntentKeyStrategy::TagExtraction.requires_embedding());
        assert!(IntentKeyStrategy::EmbeddingCluster { bucket_count: 5 }.requires_embedding());
        assert!(!IntentKeyStrategy::Disabled.requires_embedding());
    }

    #[test]
    fn test_strategy_is_disabled() {
        assert!(!IntentKeyStrategy::TagExtraction.is_disabled());
        assert!(!IntentKeyStrategy::EmbeddingCluster { bucket_count: 5 }.is_disabled());
        assert!(IntentKeyStrategy::Disabled.is_disabled());
    }

    #[test]
    fn test_default_strategy_is_tag_extraction() {
        let strategy = IntentKeyStrategy::default();
        assert!(matches!(strategy, IntentKeyStrategy::TagExtraction));
    }

    // ── AgentProfile: hot object tracking ──────────────────────────────────────

    #[test]
    fn test_record_cid_usage_new_entry() {
        let mut profile = AgentProfile::new("a1".to_string());
        profile.record_cid_usage("cid-abc");

        assert_eq!(profile.hot_objects.len(), 1);
        assert_eq!(profile.hot_objects[0].0, "cid-abc");
        assert_eq!(profile.hot_objects[0].1, 1);
    }

    #[test]
    fn test_record_cid_usage_increments_existing() {
        let mut profile = AgentProfile::new("a1".to_string());
        profile.record_cid_usage("cid-abc");
        profile.record_cid_usage("cid-abc");
        profile.record_cid_usage("cid-abc");

        assert_eq!(profile.hot_objects.len(), 1);
        assert_eq!(profile.hot_objects[0].1, 3);
    }

    #[test]
    fn test_record_cid_usage_sorted_by_count_desc() {
        let mut profile = AgentProfile::new("a1".to_string());
        profile.record_cid_usage("cid-low");
        profile.record_cid_usage("cid-high");
        profile.record_cid_usage("cid-high");
        profile.record_cid_usage("cid-high");

        assert_eq!(profile.hot_objects[0].0, "cid-high");
        assert_eq!(profile.hot_objects[0].1, 3);
        assert_eq!(profile.hot_objects[1].0, "cid-low");
        assert_eq!(profile.hot_objects[1].1, 1);
    }

    #[test]
    fn test_record_cid_usage_truncates_at_50() {
        let mut profile = AgentProfile::new("a1".to_string());
        for i in 0..55 {
            profile.record_cid_usage(&format!("cid-{:04}", i));
        }
        assert_eq!(profile.hot_objects.len(), 50);
    }

    #[test]
    fn test_record_cid_usages_batch() {
        let mut profile = AgentProfile::new("a1".to_string());
        let cids = vec!["a".to_string(), "b".to_string(), "a".to_string()];
        profile.record_cid_usages(&cids);

        assert_eq!(profile.hot_objects.len(), 2);
        // "a" was recorded twice, so it should be first (higher count)
        assert_eq!(profile.hot_objects[0].0, "a");
        assert_eq!(profile.hot_objects[0].1, 2);
        assert_eq!(profile.hot_objects[1].0, "b");
        assert_eq!(profile.hot_objects[1].1, 1);
    }

    #[test]
    fn test_record_cid_usages_empty_slice() {
        let mut profile = AgentProfile::new("a1".to_string());
        profile.record_cid_usages(&[]);
        assert!(profile.hot_objects.is_empty());
    }

    // ── AgentProfile: decay_object ─────────────────────────────────────────────

    #[test]
    fn test_decay_object_reduces_count() {
        let mut profile = AgentProfile::new("a1".to_string());
        profile.record_cid_usage("cid-1");
        profile.record_cid_usage("cid-1");
        profile.record_cid_usage("cid-1");

        profile.decay_object("cid-1");
        assert_eq!(profile.hot_objects[0].1, 2);
    }

    #[test]
    fn test_decay_object_removes_at_zero() {
        let mut profile = AgentProfile::new("a1".to_string());
        profile.record_cid_usage("cid-1");

        // Count is 1, decay should reduce to 0 and remove
        profile.decay_object("cid-1");
        assert!(profile.hot_objects.is_empty());
    }

    #[test]
    fn test_decay_object_noop_for_unknown_cid() {
        let mut profile = AgentProfile::new("a1".to_string());
        profile.record_cid_usage("cid-1");
        profile.decay_object("nonexistent");
        assert_eq!(profile.hot_objects.len(), 1);
        assert_eq!(profile.hot_objects[0].1, 1);
    }

    #[test]
    fn test_decay_object_resaturating_sub() {
        let mut profile = AgentProfile::new("a1".to_string());
        profile.record_cid_usage("cid-1");
        // Decay twice — second should be a no-op (saturating_sub at 0 removes it)
        profile.decay_object("cid-1");
        profile.decay_object("cid-1");
        assert!(profile.hot_objects.is_empty());
    }

    // ── AgentProfile: preference facts ─────────────────────────────────────────

    #[test]
    fn test_add_preference_new() {
        let mut profile = AgentProfile::new("a1".to_string());
        profile.add_preference("lang".into(), "rust".into(), 0.9);

        assert_eq!(profile.preference_facts.len(), 1);
        assert_eq!(profile.preference_facts[0].category, "lang");
        assert_eq!(profile.preference_facts[0].preference, "rust");
        assert!((profile.preference_facts[0].confidence - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn test_add_preference_updates_when_higher_confidence() {
        let mut profile = AgentProfile::new("a1".to_string());
        profile.add_preference("lang".into(), "rust".into(), 0.5);
        profile.add_preference("lang".into(), "python".into(), 0.8);

        assert_eq!(profile.preference_facts.len(), 1);
        assert_eq!(profile.preference_facts[0].preference, "python");
        assert!((profile.preference_facts[0].confidence - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn test_add_preference_keeps_existing_when_lower_confidence() {
        let mut profile = AgentProfile::new("a1".to_string());
        profile.add_preference("lang".into(), "rust".into(), 0.9);
        profile.add_preference("lang".into(), "python".into(), 0.5);

        assert_eq!(profile.preference_facts.len(), 1);
        assert_eq!(profile.preference_facts[0].preference, "rust");
        assert!((profile.preference_facts[0].confidence - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn test_add_preference_updates_when_equal_confidence() {
        let mut profile = AgentProfile::new("a1".to_string());
        profile.add_preference("lang".into(), "rust".into(), 0.7);
        profile.add_preference("lang".into(), "go".into(), 0.7);

        assert_eq!(profile.preference_facts.len(), 1);
        // Equal confidence should update (>= check)
        assert_eq!(profile.preference_facts[0].preference, "go");
    }

    #[test]
    fn test_add_preference_sorted_by_confidence_desc() {
        let mut profile = AgentProfile::new("a1".to_string());
        profile.add_preference("a".into(), "low".into(), 0.1);
        profile.add_preference("b".into(), "high".into(), 0.9);
        profile.add_preference("c".into(), "mid".into(), 0.5);

        assert_eq!(profile.preference_facts[0].preference, "high");
        assert_eq!(profile.preference_facts[1].preference, "mid");
        assert_eq!(profile.preference_facts[2].preference, "low");
    }

    #[test]
    fn test_add_preference_truncates_at_50() {
        let mut profile = AgentProfile::new("a1".to_string());
        for i in 0..55 {
            profile.add_preference(format!("cat-{}", i), format!("pref-{}", i), 0.5);
        }
        assert_eq!(profile.preference_facts.len(), 50);
    }

    #[test]
    fn test_preference_keywords_filters_by_confidence() {
        let mut profile = AgentProfile::new("a1".to_string());
        profile.add_preference("a".into(), "high".into(), 0.9);
        profile.add_preference("b".into(), "mid".into(), 0.5);
        profile.add_preference("c".into(), "low".into(), 0.1);

        let keywords = profile.preference_keywords(0.5);
        assert_eq!(keywords.len(), 2);
        assert!(keywords.contains(&"high"));
        assert!(keywords.contains(&"mid"));
    }

    #[test]
    fn test_preference_keywords_empty_when_none_above_threshold() {
        let mut profile = AgentProfile::new("a1".to_string());
        profile.add_preference("a".into(), "low".into(), 0.1);

        let keywords = profile.preference_keywords(0.9);
        assert!(keywords.is_empty());
    }

    #[test]
    fn test_preference_keywords_empty_profile() {
        let profile = AgentProfile::new("a1".to_string());
        let keywords = profile.preference_keywords(0.0);
        assert!(keywords.is_empty());
    }

    // ── AgentProfile: record_intent edge cases ─────────────────────────────────

    #[test]
    fn test_record_intent_with_none_next() {
        let mut profile = AgentProfile::new("a1".to_string());
        // When next_tag_key is None, no transition should be recorded
        profile.record_intent("auth|test", None);
        assert!(profile.intent_transitions.is_empty());
    }

    #[test]
    fn test_record_intent_truncates_successors_at_10() {
        let mut profile = AgentProfile::new("a1".to_string());
        // Add 12 distinct successors
        for i in 0..12 {
            profile.record_intent("start", Some(&format!("succ-{:02}", i)));
        }
        let successors = profile.intent_transitions.get("start").unwrap();
        assert_eq!(successors.len(), 10);
    }

    // ── IntentKeyStrategy: truncation at 5 tags ────────────────────────────────

    #[test]
    fn test_tag_extraction_truncates_at_five_tags() {
        let strategy = IntentKeyStrategy::TagExtraction;
        let known_tags = vec![
            "aa".into(),
            "bb".into(),
            "cc".into(),
            "dd".into(),
            "ee".into(),
            "ff".into(),
        ];
        let intent = "aa bb cc dd ee ff";
        let key = strategy.extract_tag_key(intent, &known_tags);
        assert!(key.is_some());
        let key = key.unwrap();
        let parts: Vec<&str> = key.split('|').collect();
        assert_eq!(parts.len(), 5, "should truncate to 5 tags, got: {}", key);
    }

    // ── SessionStore: completed sessions ───────────────────────────────────────

    #[test]
    fn test_end_session_with_summary_records_completion() {
        let store = SessionStore::new();
        store.start_session("s1".into(), "a1".into(), 0);

        let summary = SessionSummary {
            top_tags: vec!["auth".into()],
            object_count: 5,
            intent: Some("fix auth".into()),
            summary_cid: None,
        };

        let removed = store.end_session_with_summary("s1", Some(100), Some(summary), Vec::new());
        assert!(removed.is_some());

        let completed = store.get_completed_sessions("a1", None);
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].tokens_used, 100);
        assert!(completed[0].summary.is_some());
        let s = completed[0].summary.as_ref().unwrap();
        assert_eq!(s.top_tags, vec!["auth"]);
        assert_eq!(s.object_count, 5);
    }

    #[test]
    fn test_end_session_without_tokens_does_not_record_completion() {
        let store = SessionStore::new();
        store.start_session("s1".into(), "a1".into(), 0);
        store.end_session("s1", None);

        let completed = store.get_completed_sessions("a1", None);
        assert!(completed.is_empty());
    }

    #[test]
    fn test_completed_session_count() {
        let store = SessionStore::new();
        for i in 0..3 {
            store.start_session(format!("s{}", i), "a1".into(), 0);
            store.end_session(&format!("s{}", i), Some(10));
        }
        assert_eq!(store.completed_session_count("a1"), 3);
        assert_eq!(store.completed_session_count("unknown"), 0);
    }

    #[test]
    fn test_get_completed_sessions_with_period_filter() {
        let store = SessionStore::new();
        store.start_session("s1".into(), "a1".into(), 0);
        store.end_session("s1", Some(10));

        // With no period filter, should return all
        let all = store.get_completed_sessions("a1", None);
        assert_eq!(all.len(), 1);

        // With a very large period (1 year in ms), should include the just-completed session
        let one_year_ms: u64 = 365 * 24 * 3600 * 1000;
        let with_period = store.get_completed_sessions("a1", Some(one_year_ms));
        assert_eq!(with_period.len(), 1);
    }

    #[test]
    fn test_completed_sessions_max_per_agent() {
        let store = SessionStore::new();
        // Create more than MAX_COMPLETED_SESSIONS_PER_AGENT (100)
        for i in 0..105 {
            store.start_session(format!("s{}", i), "a1".into(), 0);
            store.end_session(&format!("s{}", i), Some(10));
        }
        let completed = store.get_completed_sessions("a1", None);
        assert_eq!(completed.len(), MAX_COMPLETED_SESSIONS_PER_AGENT);
    }

    // ── SessionStore: session queries ──────────────────────────────────────────

    #[test]
    fn test_list_returns_all_active_sessions() {
        let store = SessionStore::new();
        store.start_session("s1".into(), "a1".into(), 0);
        store.start_session("s2".into(), "a2".into(), 0);

        let all = store.list();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_list_empty_when_no_sessions() {
        let store = SessionStore::new();
        assert!(store.list().is_empty());
    }

    #[test]
    fn test_active_session_count_per_agent() {
        let store = SessionStore::new();
        store.start_session("s1".into(), "a1".into(), 0);
        store.start_session("s2".into(), "a1".into(), 0);
        store.start_session("s3".into(), "a2".into(), 0);

        assert_eq!(store.active_session_count("a1"), 2);
        assert_eq!(store.active_session_count("a2"), 1);
        assert_eq!(store.active_session_count("a3"), 0);
    }

    #[test]
    fn test_total_active_count() {
        let store = SessionStore::new();
        assert_eq!(store.total_active_count(), 0);

        store.start_session("s1".into(), "a1".into(), 0);
        store.start_session("s2".into(), "a2".into(), 0);
        assert_eq!(store.total_active_count(), 2);

        store.end_session("s1", None);
        assert_eq!(store.total_active_count(), 1);
    }

    #[test]
    fn test_get_active_sessions_with_cutoff() {
        let store = SessionStore::new();
        store.start_session("s1".into(), "a1".into(), 0);

        // With a future cutoff, no sessions should match
        let future_cutoff = now_ms() + 1_000_000;
        let sessions = store.get_active_sessions("a1", Some(future_cutoff));
        assert!(sessions.is_empty());

        // With no cutoff, should return the session
        let sessions = store.get_active_sessions("a1", None);
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn test_get_active_sessions_filters_by_agent() {
        let store = SessionStore::new();
        store.start_session("s1".into(), "a1".into(), 0);
        store.start_session("s2".into(), "a2".into(), 0);

        let sessions = store.get_active_sessions("a1", None);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "s1");
    }

    // ── SessionStore: set_current_intent ───────────────────────────────────────

    #[test]
    fn test_set_current_intent() {
        let store = SessionStore::new();
        store.start_session("s1".into(), "a1".into(), 0);

        store.set_current_intent("a1", Some("fix bugs".into()));
        let session = store.get("s1").unwrap();
        assert_eq!(session.current_intent.as_deref(), Some("fix bugs"));
    }

    #[test]
    fn test_set_current_intent_clears_with_none() {
        let store = SessionStore::new();
        store.start_session("s1".into(), "a1".into(), 0);

        store.set_current_intent("a1", Some("fix bugs".into()));
        store.set_current_intent("a1", None);
        let session = store.get("s1").unwrap();
        assert!(session.current_intent.is_none());
    }

    #[test]
    fn test_set_current_intent_only_affects_matching_agent() {
        let store = SessionStore::new();
        store.start_session("s1".into(), "a1".into(), 0);
        store.start_session("s2".into(), "a2".into(), 0);

        store.set_current_intent("a1", Some("fix bugs".into()));
        assert_eq!(store.get("s1").unwrap().current_intent.as_deref(), Some("fix bugs"));
        assert!(store.get("s2").unwrap().current_intent.is_none());
    }

    // ── SessionStore: expired_sessions ─────────────────────────────────────────

    #[test]
    fn test_expired_sessions_returns_old_sessions() {
        let store = SessionStore::new();
        // Start a session and manually make it look old
        store.start_session("s1".into(), "a1".into(), 0);

        // Mutate last_active_ms to be far in the past
        {
            let mut sessions = store.sessions.write().unwrap();
            if let Some(s) = sessions.get_mut("s1") {
                s.last_active_ms = 0;
            }
        }

        let expired = store.expired_sessions();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].session_id, "s1");
    }

    #[test]
    fn test_expired_sessions_empty_when_all_active() {
        let store = SessionStore::new();
        store.start_session("s1".into(), "a1".into(), 0);

        // Session was just created, should not be expired with default TTL
        let expired = store.expired_sessions();
        assert!(expired.is_empty());
    }

    // ── SessionStore: ttl_ms ───────────────────────────────────────────────────

    #[test]
    fn test_ttl_ms_returns_default() {
        let store = SessionStore::new();
        assert_eq!(store.ttl_ms(), DEFAULT_SESSION_TTL_MS);
        assert_eq!(store.ttl_ms(), 30 * 60 * 1000);
    }

    // ── SessionStore: recent_session_summaries ─────────────────────────────────

    #[test]
    fn test_recent_session_summaries_returns_summaries() {
        let store = SessionStore::new();
        for i in 0..3 {
            store.start_session(format!("s{}", i), "a1".into(), 0);
            store.end_session_with_summary(
                &format!("s{}", i),
                Some(10),
                Some(SessionSummary {
                    top_tags: vec![format!("tag-{}", i)],
                    object_count: i,
                    intent: None,
                    summary_cid: None,
                }),
                Vec::new(),
            );
        }

        let summaries = store.recent_session_summaries("a1", 10);
        assert_eq!(summaries.len(), 3);
        // Should be in reverse order (most recent first)
        assert_eq!(summaries[0].top_tags[0], "tag-2");
    }

    #[test]
    fn test_recent_session_summaries_limits_count() {
        let store = SessionStore::new();
        for i in 0..5 {
            store.start_session(format!("s{}", i), "a1".into(), 0);
            store.end_session_with_summary(
                &format!("s{}", i),
                Some(10),
                Some(SessionSummary {
                    top_tags: vec![],
                    object_count: 0,
                    intent: None,
                    summary_cid: None,
                }),
                Vec::new(),
            );
        }

        let summaries = store.recent_session_summaries("a1", 2);
        assert_eq!(summaries.len(), 2);
    }

    #[test]
    fn test_recent_session_summaries_empty_for_unknown_agent() {
        let store = SessionStore::new();
        let summaries = store.recent_session_summaries("unknown", 10);
        assert!(summaries.is_empty());
    }

    #[test]
    fn test_recent_session_summaries_skips_sessions_without_summary() {
        let store = SessionStore::new();
        // End session without summary
        store.start_session("s1".into(), "a1".into(), 0);
        store.end_session("s1", Some(10));

        let summaries = store.recent_session_summaries("a1", 10);
        assert!(summaries.is_empty());
    }

    // ── SessionStore: persist / restore ────────────────────────────────────────

    #[test]
    fn test_persist_and_restore_active_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new();
        store.start_session("s1".into(), "a1".into(), 42);
        store.set_current_intent("a1", Some("test intent".into()));

        store.persist(dir.path()).unwrap();

        let restored = SessionStore::restore(dir.path());
        let session = restored.get("s1").unwrap();
        assert_eq!(session.agent_id, "a1");
        assert_eq!(session.start_seq, 42);
        assert_eq!(session.current_intent.as_deref(), Some("test intent"));
    }

    #[test]
    fn test_persist_and_restore_completed_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new();
        store.start_session("s1".into(), "a1".into(), 0);
        store.end_session_with_summary(
            "s1",
            Some(100),
            Some(SessionSummary {
                top_tags: vec!["auth".into()],
                object_count: 5,
                intent: Some("fix".into()),
                summary_cid: None,
            }),
            Vec::new(),
        );

        store.persist(dir.path()).unwrap();

        let restored = SessionStore::restore(dir.path());
        let completed = restored.get_completed_sessions("a1", None);
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].tokens_used, 100);
        assert!(completed[0].summary.is_some());
    }

    #[test]
    fn test_restore_from_nonexistent_dir_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let nonexistent = dir.path().join("nonexistent");
        let restored = SessionStore::restore(&nonexistent);
        assert!(restored.list().is_empty());
    }

    #[test]
    fn test_restore_handles_corrupted_json_gracefully() {
        let dir = tempfile::tempdir().unwrap();
        // Write invalid JSON
        std::fs::write(dir.path().join("sessions.json"), "not valid json!!!").unwrap();
        let restored = SessionStore::restore(dir.path());
        // Should not panic, just return empty
        assert!(restored.list().is_empty());
    }

    // ── SessionStore: end_session non-existent ─────────────────────────────────

    #[test]
    fn test_end_session_returns_none_for_unknown_id() {
        let store = SessionStore::new();
        assert!(store.end_session("nonexistent", Some(10)).is_none());
    }

    // ── SessionStore: touch non-existent ───────────────────────────────────────

    #[test]
    fn test_touch_nonexistent_session_is_noop() {
        let store = SessionStore::new();
        // Should not panic
        store.touch("nonexistent");
    }

    // ── SessionStore: get non-existent ─────────────────────────────────────────

    #[test]
    fn test_get_returns_none_for_unknown_id() {
        let store = SessionStore::new();
        assert!(store.get("nonexistent").is_none());
    }

    // ── SessionStore: Default impl ─────────────────────────────────────────────

    #[test]
    fn test_session_store_default() {
        let store = SessionStore::default();
        assert_eq!(store.ttl_ms(), DEFAULT_SESSION_TTL_MS);
        assert!(store.list().is_empty());
    }

    // ── Durable session lifecycle ──────────────────────────────────────────────

    #[test]
    fn start_persistence_failure_rolls_back_active_session() {
        let dir = tempfile::tempdir().unwrap();
        let missing_root = dir.path().join("missing");
        let store = SessionStore::new();
        let event_bus = Arc::new(EventBus::new());

        let error = start_session(StartSessionParams {
            agent_id: "a1",
            last_seen_seq: None,
            session_store: &store,
            event_bus: &event_bus,
            root: &missing_root,
        })
        .unwrap_err();

        assert!(matches!(error, SessionError::Persistence { operation: "start", .. }));
        assert!(store.list().is_empty());
    }

    #[test]
    fn end_persistence_failure_restores_active_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new();
        let event_bus = Arc::new(EventBus::new());
        let started = start_session(StartSessionParams {
            agent_id: "a1",
            last_seen_seq: None,
            session_store: &store,
            event_bus: &event_bus,
            root: dir.path(),
        })
        .unwrap();

        let error = end_session(
            "a1",
            &started.session_id,
            &store,
            &dir.path().join("missing"),
            &event_bus,
        )
        .unwrap_err();

        assert!(matches!(error, SessionError::Persistence { operation: "end", .. }));
        assert!(store.get(&started.session_id).is_some());
        assert!(SessionStore::restore(dir.path()).get(&started.session_id).is_some());
    }

    #[test]
    fn session_lifecycle_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let event_bus = Arc::new(EventBus::new());
        let store = SessionStore::new();
        let started = start_session(StartSessionParams {
            agent_id: "a1",
            last_seen_seq: None,
            session_store: &store,
            event_bus: &event_bus,
            root: dir.path(),
        })
        .unwrap();

        let restored = SessionStore::restore(dir.path());
        assert!(restored.get(&started.session_id).is_some());
        end_session("a1", &started.session_id, &restored, dir.path(), &event_bus).unwrap();
        assert!(SessionStore::restore(dir.path()).get(&started.session_id).is_none());
    }

    #[test]
    fn end_rejects_wrong_owner_without_mutating_state() {
        let dir = tempfile::tempdir().unwrap();
        let event_bus = Arc::new(EventBus::new());
        let store = SessionStore::new();
        let started = start_session(StartSessionParams {
            agent_id: "owner",
            last_seen_seq: None,
            session_store: &store,
            event_bus: &event_bus,
            root: dir.path(),
        })
        .unwrap();

        let error = end_session("other", &started.session_id, &store, dir.path(), &event_bus).unwrap_err();
        assert!(matches!(error, SessionError::Ownership { .. }));
        assert!(store.get(&started.session_id).is_some());
        assert!(SessionStore::restore(dir.path()).get(&started.session_id).is_some());
    }

    #[test]
    fn end_returns_monotonic_event_bus_watermark() {
        let dir = tempfile::tempdir().unwrap();
        let event_bus = Arc::new(EventBus::new());
        event_bus.emit(crate::kernel::event_bus::KernelEvent::ObjectStored {
            cid: "before".into(),
            agent_id: "a1".into(),
            tags: vec![],
        });
        let store = SessionStore::new();
        let started = start_session(StartSessionParams {
            agent_id: "a1",
            last_seen_seq: Some(0),
            session_store: &store,
            event_bus: &event_bus,
            root: dir.path(),
        })
        .unwrap();
        assert_eq!(started.watermark, event_bus.current_seq());
        assert_eq!(started.changes_since_last.len(), 1);

        event_bus.emit(crate::kernel::event_bus::KernelEvent::ObjectStored {
            cid: "during".into(),
            agent_id: "a1".into(),
            tags: vec![],
        });
        let ended = end_session("a1", &started.session_id, &store, dir.path(), &event_bus).unwrap();
        assert_eq!(ended.last_seq, event_bus.current_seq());
        assert!(ended.last_seq >= started.watermark);
    }

    #[test]
    fn start_changes_are_scoped_to_the_local_role() {
        let dir = tempfile::tempdir().unwrap();
        let event_bus = Arc::new(EventBus::new());
        event_bus.emit(crate::kernel::event_bus::KernelEvent::ObjectStored {
            cid: "role-a-object".into(),
            agent_id: "role-a".into(),
            tags: vec!["private-a".into()],
        });
        event_bus.emit(crate::kernel::event_bus::KernelEvent::ObjectStored {
            cid: "role-b-object".into(),
            agent_id: "role-b".into(),
            tags: vec!["private-b".into()],
        });
        event_bus.emit(crate::kernel::event_bus::KernelEvent::IntentCompleted {
            intent_id: "system-event".into(),
            success: true,
        });

        let started = start_session(StartSessionParams {
            agent_id: "role-a",
            last_seen_seq: Some(0),
            session_store: &SessionStore::new(),
            event_bus: &event_bus,
            root: dir.path(),
        })
        .unwrap();

        assert_eq!(started.changes_since_last.len(), 1);
        assert_eq!(started.changes_since_last[0].cid, "role-a-object");
        let encoded = serde_json::to_string(&started.changes_since_last).unwrap();
        assert!(!encoded.contains("role-b-object"));
        assert!(!encoded.contains("private-b"));
        assert!(!encoded.contains("system-event"));
    }

    // ── ActiveSession: touch via private method ────────────────────────────────

    #[test]
    fn test_active_session_touch_updates_timestamp() {
        let mut session = ActiveSession::new("s1".into(), "a1".into(), 0);
        let before = session.last_active_ms;
        std::thread::sleep(Duration::from_millis(2));
        session.touch();
        assert!(session.last_active_ms >= before);
    }

    #[test]
    fn test_active_session_is_expired_boundary() {
        let mut session = ActiveSession::new("s1".into(), "a1".into(), 0);
        session.last_active_ms = now_ms();
        // Should NOT be expired with a large TTL
        assert!(!session.is_expired(u64::MAX));
        // Should be expired with TTL=0 if last_active is in the past
        session.last_active_ms = 0;
        assert!(session.is_expired(1));
    }

    // ── SessionSummary serialization ───────────────────────────────────────────

    #[test]
    fn test_session_summary_serialization_round_trip() {
        let summary = SessionSummary {
            top_tags: vec!["auth".into(), "test".into()],
            object_count: 42,
            intent: Some("fix bugs".into()),
            summary_cid: Some("abc123".into()),
        };
        let json = serde_json::to_string(&summary).unwrap();
        let deserialized: SessionSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.top_tags, summary.top_tags);
        assert_eq!(deserialized.object_count, 42);
        assert_eq!(deserialized.intent, summary.intent);
        assert_eq!(deserialized.summary_cid, summary.summary_cid);
    }

    #[test]
    fn test_completed_session_deserializes_without_summary() {
        // Ensure backward compat: CompletedSession without summary field
        let json = r#"{
            "session_id": "s1",
            "agent_id": "a1",
            "created_at_ms": 100,
            "ended_at_ms": 200,
            "tokens_used": 50
        }"#;
        let cs: CompletedSession = serde_json::from_str(json).unwrap();
        assert!(cs.summary.is_none());
        assert_eq!(cs.tokens_used, 50);
    }
}
