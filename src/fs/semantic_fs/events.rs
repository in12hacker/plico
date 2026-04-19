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
        self.list_events(since, until, tags, event_type)
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

