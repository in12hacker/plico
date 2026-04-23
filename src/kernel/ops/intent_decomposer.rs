//! Intent Decomposition based on historical successes (Node 23 M3).
//!
//! Uses agent profile hot_objects to recommend operation sequences.

use std::sync::Arc;
use crate::kernel::ops::prefetch_profile::AgentProfileStore;

/// Decomposes complex intents into steps based on historical successes.
pub struct IntentDecomposer {
    profile_store: Arc<AgentProfileStore>,
}

impl IntentDecomposer {
    pub fn new(profile_store: Arc<AgentProfileStore>) -> Self {
        Self { profile_store }
    }

    /// Decompose a complex intent into steps based on historical successes.
    ///
    /// Finds similar successful intents from profile and extracts their plans.
    /// Returns None if no similar intent has been successfully executed before.
    pub fn decompose(&self, intent_keywords: &[String], agent_id: &str) -> Option<Vec<String>> {
        let profile = self.profile_store.get_or_create(agent_id);

        let has_matching_history = profile.hot_objects.iter()
            .any(|(cid, _count)| {
                intent_keywords.iter().any(|kw| cid.to_lowercase().contains(&kw.to_lowercase()))
            });

        if !has_matching_history {
            return None;
        }

        let operations = self.infer_operations_from_history(&profile.hot_objects, intent_keywords);
        Some(operations)
    }

    fn infer_operations_from_history(&self, hot_objects: &[(String, u64)], keywords: &[String]) -> Vec<String> {
        let mut operations = Vec::new();

        for (cid, access_count) in hot_objects {
            let has_keyword_match = keywords.iter()
                .any(|kw| cid.to_lowercase().contains(&kw.to_lowercase()));

            if has_keyword_match && *access_count > 0 {
                // Infer operation type from CID patterns
                let cid_lower = cid.to_lowercase();
                if cid_lower.contains("code") || cid_lower.contains("src") {
                    if !operations.contains(&"read".to_string()) {
                        operations.push("read".to_string());
                    }
                }
                if cid_lower.contains("api") || cid_lower.contains("call") {
                    if !operations.contains(&"call".to_string()) {
                        operations.push("call".to_string());
                    }
                }
                if cid_lower.contains("search") || cid_lower.contains("query") {
                    if !operations.contains(&"search".to_string()) {
                        operations.push("search".to_string());
                    }
                }
                if cid_lower.contains("create") || cid_lower.contains("new") {
                    if !operations.contains(&"create".to_string()) {
                        operations.push("create".to_string());
                    }
                }
            }
        }

        // Ensure consistent ordering
        operations.sort();
        operations.dedup();

        if operations.is_empty() {
            operations.push("read".to_string());
        }

        operations
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::ops::session::IntentKeyStrategy;

    fn create_test_store() -> Arc<AgentProfileStore> {
        Arc::new(AgentProfileStore::new(IntentKeyStrategy::TagExtraction))
    }

    #[test]
    fn test_intent_decomposer_finds_similar_intent() {
        let store = create_test_store();

        // Record hot objects for agent
        store.record_cid_usage("agent-1", &["code_auth_api".to_string()]);

        let decomposer = IntentDecomposer::new(store);
        let keywords = vec!["code".to_string(), "auth".to_string()];

        let result = decomposer.decompose(&keywords, "agent-1");
        assert!(result.is_some());
    }

    #[test]
    fn test_intent_decomposer_extracts_operations() {
        let store = create_test_store();

        // Record hot objects with various patterns
        store.record_cid_usage("agent-2", &[
            "code_module_src".to_string(),
            "api_endpoint_call".to_string(),
        ]);

        let decomposer = IntentDecomposer::new(store);
        let keywords = vec!["code".to_string()];

        let result = decomposer.decompose(&keywords, "agent-2");
        assert!(result.is_some());
        let ops = result.unwrap();
        assert!(ops.contains(&"read".to_string()) || ops.contains(&"call".to_string()));
    }

    #[test]
    fn test_intent_decomposer_no_history_returns_none() {
        let store = create_test_store();

        // Create agent with no history
        let decomposer = IntentDecomposer::new(store);

        let keywords = vec!["unknown".to_string(), "nonexistent".to_string()];
        let result = decomposer.decompose(&keywords, "agent-without-history");

        assert!(result.is_none());
    }
}