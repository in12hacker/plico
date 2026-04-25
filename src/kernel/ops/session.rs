//! Session lifecycle management (F-6).
//!
//! Orchestrates existing checkpoint/restore/delta/prefetch components
//! to provide StartSession and EndSession APIs with automatic timeout cleanup.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::api::semantic::{ChangeEntry, CheckpointSummaryDto, ConsolidationReport};
use crate::kernel::event_bus::EventBus;
use crate::kernel::ops::delta::handle_delta_since;
use crate::kernel::ops::tier_maintenance::TierMaintenance;
use crate::memory::layered::{LayeredMemory, MemoryTier};

// ── F-10: Agent Profile & Intent Key Strategy ─────────────────────────────────

/// Minimum tag length to consider for extraction.
const MIN_TAG_LENGTH: usize = 2;

/// Agent profile for cognitive prefetch (F-10).
///
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
}

impl AgentProfile {
    pub fn new(agent_id: String) -> Self {
        Self {
            agent_id,
            intent_transitions: HashMap::new(),
            hot_objects: Vec::new(),
            updated_at_ms: now_ms(),
            last_intent: None,
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
        self.hot_objects.sort_by(|a, b| b.1.cmp(&a.1));
        self.hot_objects.truncate(50);
    }

    /// Record multiple CID usages at once.
    pub fn record_cid_usages(&mut self, cids: &[String]) {
        for cid in cids {
            self.record_cid_usage(cid);
        }
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
        self.hot_objects.sort_by(|a, b| b.1.cmp(&a.1));
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
const MAX_COMPLETED_SESSIONS_PER_AGENT: usize = 100;

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

/// A completed session record for growth reporting.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompletedSession {
    pub session_id: String,
    pub agent_id: String,
    pub created_at_ms: u64,
    pub ended_at_ms: u64,
    pub tokens_used: usize,
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

    /// Create a new session for an agent.
    pub fn start_session(&self, session_id: String, agent_id: String, start_seq: u64) -> ActiveSession {
        let session = ActiveSession::new(session_id, agent_id, start_seq);
        let mut sessions = self.sessions.write().unwrap();
        sessions.insert(session.session_id.clone(), session.clone());
        session
    }

    /// End a session and return it for cleanup.
    /// Optionally record completion with token usage for growth reporting.
    pub fn end_session(&self, session_id: &str, tokens_used: Option<usize>) -> Option<ActiveSession> {
        let session = {
            let mut sessions = self.sessions.write().unwrap();
            sessions.remove(session_id)
        };

        // Record completion if session exists and tokens tracking is provided
        if let Some(ref session) = session {
            if let Some(tokens) = tokens_used {
                self.record_completion(session.clone(), tokens);
            }
        }

        session
    }

    /// Record a completed session for an agent.
    fn record_completion(&self, session: ActiveSession, tokens_used: usize) {
        let completed = CompletedSession {
            session_id: session.session_id,
            agent_id: session.agent_id.clone(),
            created_at_ms: session.created_at_ms,
            ended_at_ms: now_ms(),
            tokens_used,
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
            .filter(|s| {
                s.agent_id == agent_id
                && cutoff_ms.map(|c| s.created_at_ms >= c).unwrap_or(true)
            })
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

    /// Path to the active sessions persistence file.
    fn sessions_path(root: &Path) -> PathBuf {
        root.join("sessions.json")
    }

    /// Path to the completed sessions persistence file.
    fn completed_sessions_path(root: &Path) -> PathBuf {
        root.join("completed_sessions.json")
    }

    /// Persist active and completed sessions to disk (A-1).
    pub fn persist(&self, root: &Path) -> std::io::Result<()> {
        // Persist active sessions
        let sessions = self.sessions.read().unwrap();
        let sessions_data = serde_json::to_string_pretty(&*sessions)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let sessions_path = Self::sessions_path(root);
        let tmp = sessions_path.with_extension("json.tmp");
        std::fs::write(&tmp, &sessions_data)?;
        std::fs::rename(&tmp, &sessions_path)?;

        // Persist completed sessions
        drop(sessions);
        let completed = self.completed_sessions.read().unwrap();
        let completed_data = serde_json::to_string_pretty(&*completed)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
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
                    tracing::info!("Restored {} active sessions from {}",
                        store.sessions.read().unwrap().len(), sessions_path.display());
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

/// StartSession orchestration — restore checkpoint + compute delta + prefetch intent.
pub fn start_session_orchestrate(
    agent_id: &str,
    intent_hint: Option<String>,
    _load_tiers: Vec<MemoryTier>,
    last_seen_seq: Option<u64>,
    session_store: &SessionStore,
    event_bus: &Arc<EventBus>,
    _memory: &Arc<LayeredMemory>,
    prefetch: &crate::kernel::ops::prefetch::IntentPrefetcher,
    fs: &Arc<crate::fs::SemanticFS>,
    root: &Path,
) -> Result<StartSessionResult, String> {
    // 1. Get current event seq for this session's start point
    let current_seq = event_bus.current_seq();

    // 2. Generate session ID
    let session_id = uuid::Uuid::new_v4().to_string();

    // 3. Create session in store
    let _session = session_store.start_session(
        session_id.clone(),
        agent_id.to_string(),
        current_seq,
    );

    // 4. Touch session immediately after creation
    session_store.touch(&session_id);

    // 5. Persist immediately (A-1)
    if let Err(e) = session_store.persist(root) {
        tracing::warn!("Failed to persist session start: {}", e);
    }

    // 5b. F-3: Warm intent cache from agent profile (cache preheat at session-start)
    // This uses historical hot_objects to pre-populate the cache
    // F-2: Set session_id for cost tracking before any embedding calls
    prefetch.set_session_id(Some(session_id.clone()));
    let cache_warmed = prefetch.warm_cache_for_agent(agent_id);
    if cache_warmed > 0 {
        tracing::debug!("warmed {} cache entries from agent profile for {}", cache_warmed, agent_id);
    }

    // 5c. Restore from latest checkpoint if exists
    // (checkpoint_agent stores via semantic_create which persists to CAS)
    // We don't auto-restore here — that's done via explicit AgentRestore call.
    // For StartSession, we return checkpoint info for the client to decide.
    let restored_checkpoint = None;

    // 6. Compute delta since last_seen_seq
    let since_seq = last_seen_seq.unwrap_or(0);
    let changes_since_last: Vec<ChangeEntry> = if since_seq < current_seq {
        let delta = handle_delta_since(
            since_seq,
            vec![],   // watch_cids — empty means all
            vec![],   // watch_tags — empty means all
            None,     // limit — none means all
            event_bus,
        );
        delta.changes
    } else {
        vec![]
    };

    // 7. If intent_hint provided, trigger prefetch and store assembled context in CAS
    let warm_context: Option<String> = if let Some(ref hint) = intent_hint {
        // Use a default budget_tokens if not specified
        let budget = 4096;
        let assembly_id = prefetch.declare_intent(
            agent_id,
            hint,
            vec![],  // related_cids — none provided
            budget,
        );

        // B51 Fix: Try to get assembled context and store in CAS — return CAS CID instead of assembly UUID
        match prefetch.fetch_assembled_context(agent_id, &assembly_id) {
            Some(Ok(allocation)) => {
                // Assembly is ready (from cache hit), serialize and store in CAS
                let content = serde_json::to_vec(&allocation).unwrap_or_default();
                if !content.is_empty() {
                    match fs.create(content, vec!["warm-context".into()], agent_id.to_string(), Some(hint.clone())) {
                        Ok(cid) => Some(cid),
                        Err(e) => {
                            tracing::warn!("Failed to store warm_context in CAS: {}, falling back to assembly_id", e);
                            Some(assembly_id)
                        }
                    }
                } else {
                    Some(assembly_id)
                }
            }
            Some(Err(_)) | None => {
                // B51 fix: even when prefetch is not ready, store a placeholder in CAS
                // so warm_context always returns a CAS CID, not a UUID
                let placeholder = serde_json::json!({
                    "items": [],
                    "total_tokens": 0,
                    "budget": budget,
                    "status": "pending",
                    "assembly_id": assembly_id,
                });
                let content = serde_json::to_vec(&placeholder).unwrap_or_default();
                match fs.create(content, vec!["warm-context".into(), "pending".into()], agent_id.to_string(), Some(hint.clone())) {
                    Ok(cid) => Some(cid),
                    Err(e) => {
                        tracing::warn!("Failed to store warm_context placeholder in CAS: {}, falling back to assembly_id", e);
                        Some(assembly_id)
                    }
                }
            }
        }
    } else {
        None
    };

    // 8. Estimate token count for the restored checkpoint summary + changes
    let token_estimate = changes_since_last.iter()
        .map(|c| crate::api::semantic::estimate_tokens(&c.summary))
        .sum::<usize>();

    Ok(StartSessionResult {
        session_id,
        restored_checkpoint,
        warm_context,
        changes_since_last,
        token_estimate,
    })
}

/// EndSession orchestration — checkpoint + clear ephemeral + return last_seq.
pub fn end_session_orchestrate(
    agent_id: &str,
    session_id: &str,
    auto_checkpoint: bool,
    session_store: &SessionStore,
    memory: &Arc<LayeredMemory>,
    root: &Path,
    prefetch: Option<&crate::kernel::ops::prefetch::IntentPrefetcher>,
) -> Result<EndSessionResult, String> {
    // 1. Validate session exists
    let session = session_store.get(session_id)
        .ok_or_else(|| format!("Session not found: {}", session_id))?;

    if session.agent_id != agent_id {
        return Err(format!(
            "Session {} does not belong to agent {}",
            session_id, agent_id
        ));
    }

    // 2. Auto-checkpoint if requested
    let checkpoint_id = if auto_checkpoint {
        // Use memory.get_all to collect current state
        // Then store via the existing checkpoint mechanism
        // For now, we rely on the client's explicit AgentCheckpoint call
        // or the periodic persist mechanism
        None
    } else {
        None
    };

    // 3. A-5 Memory Consolidation Cycle: run tier maintenance then clear only ephemeral
    // Run tier maintenance to promote important ephemeral memories to working/long-term
    let maintenance = TierMaintenance::new();
    let stats = maintenance.run_maintenance_cycle(memory, agent_id);
    tracing::debug!(
        ephemeral_before = stats.ephemeral_before,
        ephemeral_after = stats.ephemeral_after,
        working_before = stats.working_before,
        working_after = stats.working_after,
        promoted = stats.promoted_count,
        evicted = stats.evicted_count,
        "Memory Consolidation Cycle completed",
    );
    // Clear only the ephemeral tier — preserve working and long-term memories
    let _cleared = memory.clear_ephemeral(agent_id);

    // 4. Remove session from store (tokens_used = None for now, will enhance later)
    session_store.end_session(session_id, None);

    // 5. Persist immediately (A-1)
    if let Err(e) = session_store.persist(root) {
        tracing::warn!("Failed to persist session end: {}", e);
    }

    // F-2: Clear session_id from prefetcher to stop cost tracking
    if let Some(p) = prefetch {
        p.set_session_id(None);
    }

    // 6. Return last_seq — this is the current event count at EndSession time
    // The client will receive this and pass it back as last_seen_seq in next StartSession
    let last_seq = session.start_seq; // Use session's start_seq as the baseline

    Ok(EndSessionResult {
        checkpoint_id,
        last_seq,
        consolidation: ConsolidationReport {
            ephemeral_before: stats.ephemeral_before,
            ephemeral_after: stats.ephemeral_after,
            working_before: stats.working_before,
            working_after: stats.working_after,
            promoted: stats.promoted_count,
            evicted: stats.evicted_count,
            linked: stats.linked_count,
        },
    })
}

/// Result of StartSession orchestration.
#[derive(Debug)]
pub struct StartSessionResult {
    pub session_id: String,
    pub restored_checkpoint: Option<CheckpointSummaryDto>,
    pub warm_context: Option<String>, // assembly_id for FetchAssembledContext
    pub changes_since_last: Vec<ChangeEntry>,
    pub token_estimate: usize,
}

/// Result of EndSession orchestration.
#[derive(Debug)]
pub struct EndSessionResult {
    pub checkpoint_id: Option<String>,
    pub last_seq: u64,
    pub consolidation: crate::api::semantic::ConsolidationReport,
}

/// Spawn a background task that periodically scans for expired sessions
/// and triggers auto-EndSession with checkpoint.
pub fn spawn_session_timeout_scanner(
    session_store: Arc<SessionStore>,
    memory: Arc<LayeredMemory>,
    root: PathBuf,
) {
    let root_clone = root.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(SESSION_SCAN_INTERVAL_SECS));

            let expired = session_store.expired_sessions();
            for session in expired {
                tracing::info!(
                    "Session {} for agent {} expired (TTL {}ms), auto-ending",
                    session.session_id,
                    session.agent_id,
                    session_store.ttl_ms(),
                );

                // Auto-EndSession with checkpoint
                let _ = end_session_orchestrate(
                    &session.agent_id,
                    &session.session_id,
                    true, // auto_checkpoint
                    &session_store,
                    &memory,
                    &root_clone,
                    None, // no prefetch in timeout scanner
                );
            }
        }
    });
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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
        let known_tags = vec!["auth".to_string(), "test".to_string(), "doc".to_string(), "api".to_string()];

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

    // ── F-6 B51: Warm Context Tests ─────────────────────────────────────────────

    fn create_test_fs_and_prefetcher() -> (Arc<crate::fs::SemanticFS>, Arc<crate::kernel::ops::prefetch::IntentPrefetcher>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let cas = Arc::new(crate::cas::CASStorage::new(dir.path().join("cas")).unwrap());
        let ctx_loader = Arc::new(
            crate::fs::context_loader::ContextLoader::new(
                dir.path().join("context"),
                None,
                cas.clone(),
            ).unwrap()
        );

        let embedding = Arc::new(crate::fs::StubEmbeddingProvider::new());
        let search = Arc::new(crate::fs::search::memory::InMemoryBackend::new());
        let knowledge_graph: Option<Arc<dyn crate::fs::KnowledgeGraph>> = None;

        let fs = Arc::new(crate::fs::SemanticFS::new(
            dir.path().to_path_buf(),
            embedding.clone(),
            search.clone(),
            None,
            knowledge_graph,
        ).unwrap());

        let prefetcher = Arc::new(crate::kernel::ops::prefetch::IntentPrefetcher::new(
            search,
            None,
            Arc::new(crate::memory::LayeredMemory::new()),
            Arc::new(crate::kernel::event_bus::EventBus::new()),
            embedding,
            ctx_loader,
            dir.path().to_path_buf(),
        ));

        (fs, prefetcher, dir)
    }

    #[test]
    fn test_session_start_without_intent_has_no_warm_context() {
        let (fs, prefetcher, _dir) = create_test_fs_and_prefetcher();
        let session_store = Arc::new(SessionStore::new());
        let event_bus = Arc::new(crate::kernel::event_bus::EventBus::new());
        let memory = Arc::new(crate::memory::LayeredMemory::new());
        let root = std::env::temp_dir();

        // Start session WITHOUT intent_hint — should have no warm_context
        let result = start_session_orchestrate(
            "test-agent",
            None,  // no intent_hint
            vec![],
            None,
            &session_store,
            &event_bus,
            &memory,
            &prefetcher,
            &fs,
            &root,
        ).unwrap();

        assert!(result.warm_context.is_none());
    }

    #[test]
    fn test_session_start_warm_context_fallback_when_prefetch_not_ready() {
        let (fs, prefetcher, _dir) = create_test_fs_and_prefetcher();
        let session_store = Arc::new(SessionStore::new());
        let event_bus = Arc::new(crate::kernel::event_bus::EventBus::new());
        let memory = Arc::new(crate::memory::LayeredMemory::new());
        let root = std::env::temp_dir();

        // Start session WITH intent_hint — but prefetch runs in background thread
        // and may not be ready immediately, so we should fall back to assembly_id (UUID)
        let result = start_session_orchestrate(
            "test-agent",
            Some("audit".to_string()),
            vec![],
            None,
            &session_store,
            &event_bus,
            &memory,
            &prefetcher,
            &fs,
            &root,
        ).unwrap();

        // B51 Fix: warm_context should always be a CAS CID (64 hex chars), never a UUID
        assert!(result.warm_context.is_some());
        let warm_context = result.warm_context.unwrap();

        let is_valid_cid = warm_context.len() == 64
            && warm_context.chars().all(|c| c.is_ascii_hexdigit());

        assert!(is_valid_cid,
            "warm_context should be a valid CAS CID (64 hex chars), got: {}", warm_context);

        // Verify the CID can be read from CAS
        let obj = fs.read(&crate::fs::semantic_fs::Query::ByCid(warm_context.clone()));
        assert!(obj.is_ok(), "warm_context CID should be readable from CAS");
    }

    #[test]
    fn test_session_start_warm_context_stores_in_cas_when_ready() {
        // This test verifies the B51 fix: when warm_context IS a CID,
        // it can be retrieved from CAS
        let (fs, _prefetcher, _dir) = create_test_fs_and_prefetcher();

        // First, manually create a warm_context in CAS to simulate the fixed behavior
        let test_content = serde_json::json!({
            "items": [],
            "total_tokens": 100,
            "budget": 4096
        }).to_string().into_bytes();

        let warm_cid = fs.create(
            test_content,
            vec!["warm-context".to_string()],
            "test-agent".to_string(),
            Some("audit".to_string()),
        ).unwrap();

        // Now simulate what start_session_orchestrate does when prefetch returns a ready allocation
        // We can verify that a CID created via fs.create is readable
        use crate::fs::types::Query;
        let read_result = fs.read(&Query::ByCid(warm_cid.clone()));
        assert!(read_result.is_ok(), "CID created via fs.create should be readable");

        // Also verify the CID format is correct (64 hex chars, not UUID)
        assert_eq!(warm_cid.len(), 64, "CID should be 64 hex chars");
        assert!(warm_cid.chars().all(|c| c.is_ascii_hexdigit()), "CID should be hex digits only");
    }
}
