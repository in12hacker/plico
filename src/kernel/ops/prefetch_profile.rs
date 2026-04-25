//! Agent Profile Store (F-10) + Adaptive Prefetch feedback (F-15).
//!
//! Extracted from `prefetch.rs` for independent evolution.
//! Profile learning and feedback tracking change independently from the core prefetch engine.

use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use crate::kernel::ops::session::{AgentProfile, IntentKeyStrategy};

/// Minimum confidence threshold for triggering prefetch (0.5 = 50%).
const PREFETCH_CONFIDENCE_THRESHOLD: f32 = 0.5;

/// Maximum profiles to keep per agent store.
const MAX_PROFILE_HISTORY: usize = 100;

/// Default maximum feedback entries to keep (1000).
pub(crate) const DEFAULT_MAX_FEEDBACK_ENTRIES: usize = 1000;

/// Feedback entry recording what was actually used vs prefetched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct IntentFeedbackEntry {
    pub(crate) normalized_intent: String,
    pub(crate) used_cids: Vec<String>,
    pub(crate) unused_cids: Vec<String>,
    pub(crate) recorded_at_ms: u64,
}

/// Agent profile store — manages per-agent transition statistics (F-10).
///
/// Thread-safe profile storage for cognitive prefetch.
/// Each agent has a profile that tracks:
/// - Intent transitions (which tag keys follow which)
/// - Hot objects (frequently accessed CIDs)
pub struct AgentProfileStore {
    profiles: RwLock<HashMap<String, AgentProfile>>,
    strategy: IntentKeyStrategy,
}

impl AgentProfileStore {
    pub fn new(strategy: IntentKeyStrategy) -> Self {
        Self {
            profiles: RwLock::new(HashMap::new()),
            strategy,
        }
    }

    pub fn get_or_create(&self, agent_id: &str) -> AgentProfile {
        let mut profiles = self.profiles.write().unwrap();
        profiles
            .entry(agent_id.to_string())
            .or_insert_with(|| AgentProfile::new(agent_id.to_string()))
            .clone()
    }

    /// Record an intent completion and update transition statistics.
    ///
    /// Returns the predicted next tag key if confidence is high enough for prefetch.
    pub fn record_intent_complete(
        &self,
        agent_id: &str,
        intent_tag_key: Option<&str>,
        next_intent_tag_key: Option<&str>,
    ) -> Option<String> {
        let mut profiles = self.profiles.write().unwrap();
        let profile = profiles
            .entry(agent_id.to_string())
            .or_insert_with(|| AgentProfile::new(agent_id.to_string()));

        if let Some(tag_key) = intent_tag_key {
            profile.record_intent(tag_key, next_intent_tag_key);

            if let Some(next) = profile.predict_next(tag_key) {
                if let Some(succs) = profile.intent_transitions.get(tag_key) {
                    if let Some((_, count)) = succs.first() {
                        let total: u32 = succs.iter().map(|(_, c)| c).sum();
                        let confidence = *count as f32 / total.max(1) as f32;
                        if confidence >= PREFETCH_CONFIDENCE_THRESHOLD {
                            return Some(next);
                        }
                    }
                }
            }
        }

        if profile.intent_transitions.len() > MAX_PROFILE_HISTORY {
            let to_keep: Vec<_> = profile.intent_transitions.iter()
                .take(MAX_PROFILE_HISTORY)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            profile.intent_transitions.clear();
            for (k, v) in to_keep {
                profile.intent_transitions.insert(k, v);
            }
        }

        None
    }

    pub fn strategy(&self) -> &IntentKeyStrategy {
        &self.strategy
    }

    pub fn set_strategy(&mut self, strategy: IntentKeyStrategy) {
        self.strategy = strategy;
    }

    pub fn extract_tag_key(&self, intent: &str, known_tags: &[String]) -> Option<String> {
        self.strategy.extract_tag_key(intent, known_tags)
    }

    /// Returns the number of profiles in the store.
    pub(crate) fn len(&self) -> usize {
        self.profiles.read().unwrap().len()
    }

    /// Persist all profiles to a JSON file at `dir/profiles.json`.
    pub(crate) fn persist_to_dir(&self, dir: &Path) -> std::io::Result<()> {
        let profiles = self.profiles.read().unwrap();
        let json = serde_json::to_string_pretty(&*profiles)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(dir.join("profiles.json"), json)
    }

    /// Restore profiles from `dir/profiles.json`.
    /// Missing file is not an error. Returns number of profiles restored.
    pub(crate) fn restore_from_dir(&self, dir: &Path) -> std::io::Result<usize> {
        let path = dir.join("profiles.json");
        if !path.exists() {
            return Ok(0);
        }
        let json = std::fs::read_to_string(&path)?;
        let loaded: std::collections::HashMap<String, AgentProfile> = serde_json::from_str(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let mut profiles = self.profiles.write().unwrap();
        for (id, profile) in loaded {
            profiles.insert(id, profile);
        }

        Ok(profiles.len())
    }

    /// Record CID usage for hot objects tracking (F-1).
    pub fn record_cid_usage(&self, agent_id: &str, cids: &[String]) {
        let mut profiles = self.profiles.write().unwrap();
        let profile = profiles
            .entry(agent_id.to_string())
            .or_insert_with(|| AgentProfile::new(agent_id.to_string()));
        profile.record_cid_usages(cids);
    }

    /// Record execution time for an operation type (F-2).
    pub fn record_execution_time(&self, agent_id: &str, _operation_type: &str, _duration_ms: u64) {
        // For now, store in a separate map - could be integrated into profile later
        // This is a simplified implementation
        let mut profiles = self.profiles.write().unwrap();
        let profile = profiles
            .entry(agent_id.to_string())
            .or_insert_with(|| AgentProfile::new(agent_id.to_string()));
        // Update timestamp to show profile was accessed
        profile.updated_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
    }

    /// Close the feedback loop: update profile from actual usage data.
    pub fn apply_feedback(&self, agent_id: &str, feedback: &IntentFeedbackEntry) {
        let mut profiles = self.profiles.write().unwrap();
        let profile = profiles
            .entry(agent_id.to_string())
            .or_insert_with(|| AgentProfile::new(agent_id.to_string()));

        // Update hot_objects from used_cids
        for cid in &feedback.used_cids {
            profile.record_cid_usage(cid);
        }

        // Update transition matrix: current intent → next intent
        let prev_intent = profile.last_intent.clone();
        if let Some(prev) = prev_intent {
            profile.record_intent(&prev, Some(&feedback.normalized_intent));
        }
        profile.last_intent = Some(feedback.normalized_intent.clone());

        // Decay unused CIDs
        for cid in &feedback.unused_cids {
            profile.decay_object(cid);
        }
    }
}

impl Default for AgentProfileStore {
    fn default() -> Self {
        Self::new(IntentKeyStrategy::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_or_create_returns_profile() {
        let store = AgentProfileStore::default();
        let profile = store.get_or_create("agent-1");
        assert_eq!(profile.agent_id, "agent-1");
    }

    #[test]
    fn get_or_create_idempotent() {
        let store = AgentProfileStore::default();
        let p1 = store.get_or_create("agent-1");
        let p2 = store.get_or_create("agent-1");
        assert_eq!(p1.agent_id, p2.agent_id);
    }

    #[test]
    fn record_intent_complete_no_tag_returns_none() {
        let store = AgentProfileStore::default();
        let result = store.record_intent_complete("a", None, None);
        assert!(result.is_none());
    }

    #[test]
    fn record_intent_builds_transition_matrix() {
        let store = AgentProfileStore::new(IntentKeyStrategy::TagExtraction);
        for _ in 0..5 {
            store.record_intent_complete("a", Some("auth"), Some("deploy"));
        }
        let result = store.record_intent_complete("a", Some("auth"), Some("deploy"));
        assert_eq!(result, Some("deploy".to_string()));
    }

    #[test]
    fn low_confidence_does_not_predict() {
        let store = AgentProfileStore::new(IntentKeyStrategy::TagExtraction);
        store.record_intent_complete("a", Some("auth"), Some("deploy"));
        store.record_intent_complete("a", Some("auth"), Some("test"));
        let result = store.record_intent_complete("a", Some("auth"), Some("other"));
        assert!(result.is_none());
    }

    #[test]
    fn strategy_accessor() {
        let store = AgentProfileStore::new(IntentKeyStrategy::TagExtraction);
        match store.strategy() {
            IntentKeyStrategy::TagExtraction => {}
            _ => panic!("expected TagExtraction"),
        }
    }

    #[test]
    fn set_strategy() {
        let mut store = AgentProfileStore::default();
        store.set_strategy(IntentKeyStrategy::Disabled);
        match store.strategy() {
            IntentKeyStrategy::Disabled => {}
            _ => panic!("expected Disabled"),
        }
    }

    #[test]
    fn extract_tag_key_with_tag_extraction_strategy() {
        let store = AgentProfileStore::new(IntentKeyStrategy::TagExtraction);
        let tags = vec!["auth".to_string(), "deploy".to_string()];
        let key = store.extract_tag_key("fix the auth module", &tags);
        assert_eq!(key, Some("auth".to_string()));
    }

    // F-2: Profile Persistence tests
    #[test]
    fn profile_persist_and_restore() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentProfileStore::new(IntentKeyStrategy::TagExtraction);

        // Build up profile
        for _ in 0..5 {
            store.record_intent_complete("a", Some("auth"), Some("deploy"));
        }

        assert_eq!(store.len(), 1);

        store.persist_to_dir(dir.path()).unwrap();

        let restored = AgentProfileStore::new(IntentKeyStrategy::TagExtraction);
        let count = restored.restore_from_dir(dir.path()).unwrap();
        assert_eq!(count, 1);

        // After restore, prediction should still work
        let predicted = restored.record_intent_complete("a", Some("auth"), None);
        assert_eq!(predicted, Some("deploy".to_string()));
    }

    #[test]
    fn profile_restore_missing_file_is_zero() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentProfileStore::default();
        let count = store.restore_from_dir(dir.path()).unwrap();
        assert_eq!(count, 0);
    }

    // F-1: FeedbackPipeline tests
    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    #[test]
    fn test_feedback_updates_hot_objects() {
        let store = AgentProfileStore::default();
        let feedback = IntentFeedbackEntry {
            normalized_intent: "fix auth".into(),
            used_cids: vec!["cid1".into(), "cid2".into()],
            unused_cids: vec![],
            recorded_at_ms: now_ms(),
        };
        store.apply_feedback("agent-1", &feedback);
        let profile = store.get_or_create("agent-1");
        assert!(profile.hot_objects.iter().any(|(c, _)| c == "cid1"));
        assert!(profile.hot_objects.iter().any(|(c, _)| c == "cid2"));
    }

    #[test]
    fn test_feedback_updates_transition_matrix() {
        let store = AgentProfileStore::default();
        // First feedback sets last_intent
        let f1 = IntentFeedbackEntry {
            normalized_intent: "fix auth".into(),
            used_cids: vec![],
            unused_cids: vec![],
            recorded_at_ms: now_ms(),
        };
        store.apply_feedback("agent-1", &f1);
        // Second feedback should create transition
        let f2 = IntentFeedbackEntry {
            normalized_intent: "test auth".into(),
            used_cids: vec![],
            unused_cids: vec![],
            recorded_at_ms: now_ms(),
        };
        store.apply_feedback("agent-1", &f2);
        let profile = store.get_or_create("agent-1");
        let successors = profile.intent_transitions.get("fix auth");
        assert!(successors.is_some());
        assert!(successors.unwrap().iter().any(|(k, _)| k == "test auth"));
    }

    #[test]
    fn test_feedback_decays_unused_cids() {
        let store = AgentProfileStore::default();
        // First add a hot object via used_cids
        let f1 = IntentFeedbackEntry {
            normalized_intent: "fix auth".into(),
            used_cids: vec!["cid1".into()],
            unused_cids: vec![],
            recorded_at_ms: now_ms(),
        };
        store.apply_feedback("agent-1", &f1);
        // Then mark it as unused
        let f2 = IntentFeedbackEntry {
            normalized_intent: "test auth".into(),
            used_cids: vec![],
            unused_cids: vec!["cid1".into()],
            recorded_at_ms: now_ms(),
        };
        store.apply_feedback("agent-1", &f2);
        let profile = store.get_or_create("agent-1");
        // Should be decayed to 0 and removed
        assert!(!profile.hot_objects.iter().any(|(c, _)| c == "cid1"));
    }

    #[test]
    fn test_feedback_records_last_intent() {
        let store = AgentProfileStore::default();
        let feedback = IntentFeedbackEntry {
            normalized_intent: "fix auth".into(),
            used_cids: vec![],
            unused_cids: vec![],
            recorded_at_ms: now_ms(),
        };
        store.apply_feedback("agent-1", &feedback);
        let profile = store.get_or_create("agent-1");
        assert_eq!(profile.last_intent, Some("fix auth".into()));
    }

    #[test]
    fn test_feedback_idempotent_on_empty() {
        let store = AgentProfileStore::default();
        let feedback = IntentFeedbackEntry {
            normalized_intent: "fix auth".into(),
            used_cids: vec![],
            unused_cids: vec![],
            recorded_at_ms: now_ms(),
        };
        // Should not panic on empty profile
        store.apply_feedback("agent-new", &feedback);
        let profile = store.get_or_create("agent-new");
        assert_eq!(profile.last_intent, Some("fix auth".into()));
    }
}
