//! Event operations for SemanticFS.

use std::collections::HashSet;

use crate::fs::graph::{KGNode, KGNodeType, KGEdge, KGEdgeType};
use crate::fs::types::{EventMeta, EventRelation, EventSummary, EventType, FSError};
use crate::temporal::TemporalResolver;

use super::{now_ms, SemanticFS};

/// Convert EventRelation to KGEdgeType (needed by event_attach).
fn relation_to_edge_type(rel: EventRelation) -> KGEdgeType {
    match rel {
        EventRelation::Participant => KGEdgeType::HasParticipant,
        EventRelation::Artifact => KGEdgeType::HasArtifact,
        EventRelation::Recording => KGEdgeType::HasRecording,
        EventRelation::Resolution => KGEdgeType::HasResolution,
    }
}

impl SemanticFS {
    /// Create an event container — stored as a KG node with EventMeta.
    #[allow(clippy::too_many_arguments)]
    pub fn create_event(
        &self,
        label: &str,
        event_type: EventType,
        start_time: Option<u64>,
        end_time: Option<u64>,
        location: Option<&str>,
        tags: Vec<String>,
        agent_id: &str,
    ) -> Result<String, FSError> {
        let node_id = format!("evt:{}", uuid::Uuid::new_v4());

        if let Some(ref kg) = self.knowledge_graph {
            let meta = EventMeta {
                label: label.to_string(),
                event_type,
                start_time,
                end_time,
                location: location.map(String::from),
                participant_ids: Vec::new(),
                related_cids: Vec::new(),
            };
            let node = KGNode {
                id: node_id.clone(),
                label: label.to_string(),
                node_type: KGNodeType::Entity,
                content_cid: None,
                properties: serde_json::to_value(&meta)
                    .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?,
                agent_id: agent_id.to_string(),
                tenant_id: "default".to_string(),
                created_at: now_ms(),
                valid_at: None,
                invalid_at: None,
                expired_at: None,
            };
            kg.add_node(node)
                .map_err(|e| FSError::Io(std::io::Error::other(e.to_string())))?;
        }

        {
            let mut tag_index = self.tag_index.write().unwrap();
            for tag in &tags {
                tag_index.entry(tag.clone()).or_default().push(node_id.clone());
            }
            drop(tag_index);
            self.persist_tag_index()
                .map_err(FSError::Io)?;
        }

        Ok(node_id)
    }

    /// List events matching the given filters.
    pub fn list_events(
        &self,
        since: Option<u64>,
        until: Option<u64>,
        tags: &[String],
        event_type: Option<EventType>,
        agent_id: Option<&str>,
    ) -> Result<Vec<EventSummary>, FSError> {
        let kg = match self.knowledge_graph.as_ref() {
            Some(g) => g,
            None => return Ok(Vec::new()),
        };

        let candidates: Vec<String> = if tags.is_empty() {
            kg.all_node_ids()
        } else {
            let tag_index = self.tag_index.read().unwrap();
            let mut intersection: Option<HashSet<String>> = None;
            for tag in tags {
                let ids = tag_index.get(tag);
                // Tag not in index → no candidate can match ALL required tags
                let Some(set) = ids else { return Ok(Vec::new()); };
                let set: HashSet<String> = set.iter().cloned().collect();
                match intersection.take() {
                    Some(existing) => intersection = Some(existing.intersection(&set).cloned().collect()),
                    None => intersection = Some(set),
                }
            }
            intersection.unwrap_or_default().into_iter().collect()
        };

        let mut results = Vec::new();
        for node_id in candidates {
            let node = match kg.get_node(&node_id) {
                Ok(Some(n)) => n,
                _ => continue,
            };
            if node.node_type != KGNodeType::Entity { continue; }

            // L-2 / B14: Filter by agent_id if specified
            if let Some(aid) = agent_id {
                if node.agent_id != aid {
                    continue;
                }
            }

            let meta: EventMeta = match serde_json::from_value(node.properties.clone()) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if !meta.in_range(since, until) { continue; }
            if let Some(et) = event_type {
                if meta.event_type != et { continue; }
            }
            results.push(EventSummary {
                id: node.id,
                label: meta.label,
                event_type: meta.event_type,
                start_time: meta.start_time,
                attendee_count: meta.participant_ids.len(),
                related_count: meta.related_cids.len(),
                agent_id: Some(node.agent_id.clone()),
            });
        }

        results.sort_by_key(|e| e.start_time);
        Ok(results)
    }

    /// Resolve a natural-language time expression and list matching events.
    pub fn list_events_by_time(
        &self,
        time_expression: &str,
        tags: &[String],
        event_type: Option<EventType>,
        resolver: &dyn TemporalResolver,
        agent_id: Option<&str>,
    ) -> Result<Vec<EventSummary>, FSError> {
        let range = resolver.resolve(time_expression, None)
            .ok_or_else(|| {
                FSError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("Cannot resolve time expression: {time_expression}"),
                ))
            })?;
        let since = if range.since >= 0 { Some(range.since as u64) } else { None };
        let until = Some(range.until as u64);
        self.list_events(since, until, tags, event_type, agent_id)
    }

    /// Attach a target to an event via a typed edge.
    pub fn event_attach(
        &self,
        event_id: &str,
        target_id: &str,
        relation: EventRelation,
        _agent_id: &str,
    ) -> Result<(), FSError> {
        let kg = self.knowledge_graph.as_ref()
            .ok_or_else(|| FSError::Io(std::io::Error::other("knowledge graph not initialized")))?;

        let edge = KGEdge::new_with_episode(
            event_id.to_string(),
            target_id.to_string(),
            relation_to_edge_type(relation),
            1.0,
            event_id,
        );
        kg.add_edge(edge)
            .map_err(|e| FSError::Io(std::io::Error::other(e.to_string())))?;

        let mut node = kg.get_node(event_id)
            .map_err(|e| FSError::Io(std::io::Error::other(e.to_string())))?
            .ok_or_else(|| FSError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "event not found")))?;
        let mut meta: EventMeta = serde_json::from_value(node.properties.clone())
            .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?;

        match relation {
            EventRelation::Participant => {
                if !meta.participant_ids.contains(&target_id.to_string()) {
                    meta.participant_ids.push(target_id.to_string());
                }
            }
            EventRelation::Artifact | EventRelation::Recording | EventRelation::Resolution => {
                if !meta.related_cids.contains(&target_id.to_string()) {
                    meta.related_cids.push(target_id.to_string());
                }
            }
        }

        node.properties = serde_json::to_value(&meta)
            .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?;
        kg.add_node(node)
            .map_err(|e| FSError::Io(std::io::Error::other(e.to_string())))?;

        Ok(())
    }
}

// ─── Standalone helper for use in unit tests ────────────────────────────────

/// A simple rule-based resolver for testing (always resolves to last 7 days).
pub struct RuleBasedResolver;

impl RuleBasedResolver {
    pub fn resolve_range(_expression: &str) -> Option<(i64, i64)> {
        let now = chrono::Utc::now().timestamp_millis();
        let seven_days = 7 * 86_400_000;
        Some((now - seven_days, now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::tempdir;
    use crate::fs::embedding::StubEmbeddingProvider;
    use crate::fs::search::InMemoryBackend;
    use crate::fs::graph::PetgraphBackend;

    fn make_fs() -> (SemanticFS, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let fs = SemanticFS::new(
            dir.path().to_path_buf(),
            Arc::new(StubEmbeddingProvider::new()),
            Arc::new(InMemoryBackend::new()),
            None,
            None,
        ).unwrap();
        (fs, dir)
    }

    fn make_fs_with_kg() -> (SemanticFS, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let fs = SemanticFS::new(
            dir.path().to_path_buf(),
            Arc::new(StubEmbeddingProvider::new()),
            Arc::new(InMemoryBackend::new()),
            None,
            Some(Arc::new(PetgraphBackend::new())),
        ).unwrap();
        (fs, dir)
    }

    // ─── relation_to_edge_type ───────────────────────────────────────────────

    #[test]
    fn test_relation_to_edge_type() {
        assert_eq!(relation_to_edge_type(EventRelation::Participant), KGEdgeType::HasParticipant);
        assert_eq!(relation_to_edge_type(EventRelation::Artifact), KGEdgeType::HasArtifact);
        assert_eq!(relation_to_edge_type(EventRelation::Recording), KGEdgeType::HasRecording);
        assert_eq!(relation_to_edge_type(EventRelation::Resolution), KGEdgeType::HasResolution);
    }

    // ─── create_event ────────────────────────────────────────────────────────

    #[test]
    fn test_create_event_returns_valid_id() {
        // Works even without KG
        let (fs, _dir) = make_fs();
        let id = fs.create_event("test-event", EventType::Task, None, None, None, vec![], "agent1").unwrap();
        assert!(id.starts_with("evt:"));
    }

    #[test]
    fn test_create_event_with_tags() {
        // Requires KG for list_events to work
        let (fs, _dir) = make_fs_with_kg();
        let _id = fs.create_event(
            "tagged-event",
            EventType::Task,
            None,
            None,
            Some("room-1"),
            vec!["meeting".to_string(), "weekly".to_string()],
            "agent1",
        ).unwrap();
        // List events should find it
        let events = fs.list_events(None, None, &["meeting".to_string()], None, None).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].label, "tagged-event");
    }

    #[test]
    fn test_create_event_with_time_range() {
        let (fs, _dir) = make_fs_with_kg();
        let now = chrono::Utc::now().timestamp_millis() as u64;
        let id = fs.create_event(
            "timed-event",
            EventType::Task,
            Some(now - 3600_000),
            Some(now),
            None,
            vec![],
            "agent1",
        ).unwrap();
        let events = fs.list_events(Some(now - 86_400_000), Some(now + 1000), &[], None, None).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, id);
    }

    // ─── list_events ──────────────────────────────────────────────────────────

    #[test]
    fn test_list_events_empty_by_default() {
        let (fs, _dir) = make_fs_with_kg();
        let events = fs.list_events(None, None, &[], None, None).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_list_events_filters_by_agent() {
        let (fs, _dir) = make_fs_with_kg();
        fs.create_event("event-a", EventType::Task, None, None, None, vec![], "agent-a").unwrap();
        fs.create_event("event-b", EventType::Task, None, None, None, vec![], "agent-b").unwrap();

        let events_a = fs.list_events(None, None, &[], None, Some("agent-a")).unwrap();
        assert_eq!(events_a.len(), 1);
        assert_eq!(events_a[0].label, "event-a");

        let events_b = fs.list_events(None, None, &[], None, Some("agent-b")).unwrap();
        assert_eq!(events_b.len(), 1);
    }

    #[test]
    fn test_list_events_filters_by_type() {
        let (fs, _dir) = make_fs_with_kg();
        fs.create_event("task-e", EventType::Task, None, None, None, vec![], "a").unwrap();
        fs.create_event("meet-e", EventType::Task, None, None, None, vec![], "a").unwrap();

        let tasks = fs.list_events(None, None, &[], Some(EventType::Task), None).unwrap();
        assert_eq!(tasks.len(), 2); // both are Task type
    }

    #[test]
    fn test_list_events_by_time_filters_by_time_range() {
        let (fs, _dir) = make_fs_with_kg();
        let now = chrono::Utc::now().timestamp_millis() as u64;
        // Event starts 2 days ago, ends yesterday — fully within [since, until]
        fs.create_event("old", EventType::Task, Some(now - 86_400_000), Some(now - 43_200_000), None, vec![], "a").unwrap();
        // Event starts 10 days ago, ends 8 days ago — outside [since, until]
        fs.create_event("ancient", EventType::Task, Some(now - 864_000_000), None, None, vec![], "a").unwrap();

        // [since, until] = [now - 86_400_000, now]
        let events = fs.list_events(Some(now - 86_400_000), Some(now), &[], None, None).unwrap();
        assert_eq!(events.len(), 1, "only 'old' within range (ancient starts at now-864000000)");
        assert_eq!(events[0].label, "old");
    }

    // ─── event_attach ────────────────────────────────────────────────────────

    #[test]
    fn test_event_attach_requires_kg() {
        let (fs, _dir) = make_fs();
        // Without KG, event_attach should fail
        let id = fs.create_event("no-kg-event", EventType::Task, None, None, None, vec![], "a").unwrap();
        let result = fs.event_attach(&id, "target-id", EventRelation::Participant, "a");
        assert!(result.is_err());
    }

    // ─── RuleBasedResolver ────────────────────────────────────────────────────

    #[test]
    fn test_rule_based_resolver_always_resolves() {
        let result = RuleBasedResolver::resolve_range("anything");
        assert!(result.is_some());
        let (since, until) = result.unwrap();
        assert!(until > since);
        // Should cover roughly 7 days
        let diff = until - since;
        assert!(diff >= 6 * 86_400_000 && diff <= 8 * 86_400_000);
    }
}

