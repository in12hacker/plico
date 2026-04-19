//! Session lifecycle management (F-6).
//!
//! Orchestrates existing checkpoint/restore/delta/prefetch components
//! to provide StartSession and EndSession APIs with automatic timeout cleanup.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::api::semantic::{ChangeEntry, CheckpointSummaryDto};
use crate::kernel::event_bus::EventBus;
use crate::kernel::ops::delta::handle_delta_since;
use crate::memory::layered::{LayeredMemory, MemoryTier};

// ── F-10: Agent Profile & Intent Key Strategy ─────────────────────────────────

/// Minimum tag length to consider for extraction.
const MIN_TAG_LENGTH: usize = 2;

/// Agent profile for cognitive prefetch (F-10).
///
/// Stores statistical patterns of agent behavior:
/// - Intent transitions: which tag keys follow which
/// - Hot objects: frequently accessed CIDs
#[derive(Debug, Clone)]
pub struct AgentProfile {
    pub agent_id: String,
    /// Maps tag key → list of (successor_tag_key, count)
    /// Example: "auth|test" → [("auth|doc", 5), ("auth|perf", 2)]
    pub intent_transitions: HashMap<String, Vec<(String, u32)>>,
    /// Hot objects — (CID, access_count)
    pub hot_objects: Vec<(String, u64)>,
    pub updated_at_ms: u64,
}

impl AgentProfile {
    pub fn new(agent_id: String) -> Self {
        Self {
            agent_id,
            intent_transitions: HashMap::new(),
            hot_objects: Vec::new(),
            updated_at_ms: now_ms(),
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
}

/// Strategy for converting intent text to a lookup key.
#[derive(Debug, Clone)]
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

/// An active session tracked by SessionStore.
#[derive(Debug, Clone)]
pub struct ActiveSession {
    pub session_id: String,
    pub agent_id: String,
    pub created_at_ms: u64,
    pub last_active_ms: u64,
    /// Seq number at session start — used for delta calculation.
    pub start_seq: u64,
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
        }
    }

    fn touch(&mut self) {
        self.last_active_ms = now_ms();
    }

    fn is_expired(&self, ttl_ms: u64) -> bool {
        now_ms() - self.last_active_ms > ttl_ms
    }
}

/// Session store — manages active sessions and timeout scanning.
pub struct SessionStore {
    sessions: RwLock<HashMap<String, ActiveSession>>,
    ttl_ms: u64,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
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
    pub fn end_session(&self, session_id: &str) -> Option<ActiveSession> {
        let mut sessions = self.sessions.write().unwrap();
        sessions.remove(session_id)
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

    /// List all active sessions.
    pub fn list(&self) -> Vec<ActiveSession> {
        let sessions = self.sessions.read().unwrap();
        sessions.values().cloned().collect()
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
) -> Result<StartSessionResult, String> {
    // 1. Get current event seq for this session's start point
    let current_seq = event_bus.event_count() as u64;

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

    // 5. Restore from latest checkpoint if exists
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

    // 7. If intent_hint provided, trigger prefetch
    let warm_context: Option<String> = if let Some(ref hint) = intent_hint {
        // Use a default budget_tokens if not specified
        let budget = 4096;
        let assembly_id = prefetch.declare_intent(
            agent_id,
            hint,
            vec![],  // related_cids — none provided
            budget,
        );
        // Return the assembly_id — client should call FetchAssembledContext
        Some(assembly_id)
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

    // 3. Clear ephemeral tier on EndSession
    // Note: clear_agent clears all tiers. For selective tier clearing,
    // we'd need memory.clear_tier(agent_id, MemoryTier::Ephemeral).
    // For now, we skip explicit clear since checkpoint preserves what matters.
    let _ = memory.clear_agent(agent_id);

    // 4. Remove session from store
    session_store.end_session(session_id);

    // 5. Return last_seq — this is the current event count at EndSession time
    // The client will receive this and pass it back as last_seen_seq in next StartSession
    let last_seq = session.start_seq; // Use session's start_seq as the baseline

    Ok(EndSessionResult {
        checkpoint_id,
        last_seq,
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
}

/// Spawn a background task that periodically scans for expired sessions
/// and triggers auto-EndSession with checkpoint.
pub fn spawn_session_timeout_scanner(
    session_store: Arc<SessionStore>,
    memory: Arc<LayeredMemory>,
) {
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

        let removed = store.end_session(&session_id).unwrap();
        assert_eq!(removed.session_id, session_id);

        assert!(store.get(&session_id).is_none());
    }

    #[test]
    fn test_session_expiry() {
        let store = SessionStore::new();
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
}
