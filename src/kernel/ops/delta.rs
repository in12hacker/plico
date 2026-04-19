//! Delta-aware change tracking (F-7).
//!
//! Provides efficient change queries using EventBus sequence numbers.
//! Agents use DeltaSince to sync state after a session gap without
//! having to re-read all content.

use crate::api::semantic::{ChangeEntry, DeltaResult};
use crate::kernel::event_bus::{KernelEvent, SequencedEvent};

/// Build a ChangeEntry from a SequencedEvent.
///
/// Format: "{event_type} {cid[..8]} by {agent_id} [{tags}]"
/// Does not depend on LLM — pure metadata concatenation.
pub fn change_entry_from_event(event: &SequencedEvent) -> ChangeEntry {
    let (cid, change_type, tags, changed_by) = match &event.event {
        KernelEvent::ObjectStored { cid, agent_id, tags } => {
            (cid.clone(), "stored".to_string(), tags.clone(), agent_id.clone())
        }
        KernelEvent::MemoryStored { agent_id, tier, .. } => {
            (format!("memory:{}", tier), "memory_stored".to_string(), vec![], agent_id.clone())
        }
        KernelEvent::AgentStateChanged { agent_id, old_state, new_state } => {
            (
                format!("agent:{}", agent_id),
                format!("state_changed:{}->{}", old_state, new_state),
                vec![],
                agent_id.clone(),
            )
        }
        KernelEvent::IntentSubmitted { intent_id, agent_id, .. } => {
            (
                format!("intent:{}", intent_id),
                "intent_submitted".to_string(),
                vec![],
                agent_id.clone().unwrap_or_default(),
            )
        }
        KernelEvent::IntentCompleted { intent_id, success } => {
            (
                format!("intent:{}", intent_id),
                if *success {
                    "intent_completed".to_string()
                } else {
                    "intent_failed".to_string()
                },
                vec![],
                "system".to_string(),
            )
        }
        KernelEvent::EventCreated { event_id, agent_id, .. } => {
            (
                format!("event:{}", event_id),
                "event_created".to_string(),
                vec![],
                agent_id.clone(),
            )
        }
    };

    let summary = if tags.is_empty() {
        format!("{} {} by {}", event.event.event_type_name(), &cid[..8.min(cid.len())], changed_by)
    } else {
        let tags_str = tags.join(",");
        format!(
            "{} {} by {} [{}]",
            event.event.event_type_name(),
            &cid[..8.min(cid.len())],
            changed_by,
            tags_str
        )
    };

    ChangeEntry {
        cid,
        change_type,
        summary,
        changed_at_ms: event.timestamp_ms,
        changed_by,
        seq: event.seq,
    }
}

/// Filter events by watch_cids and watch_tags.
///
/// Returns true if the event matches the filter criteria:
/// - If watch_cids is empty, accept all CIDs
/// - If watch_tags is empty, accept all tags
fn event_matches_filter(event: &SequencedEvent, watch_cids: &[String], watch_tags: &[String]) -> bool {
    // Check watch_cids filter
    if !watch_cids.is_empty() {
        let event_cid = match &event.event {
            KernelEvent::ObjectStored { cid, .. } => Some(cid.as_str()),
            _ => None,
        };
        let Some(cid) = event_cid else { return false; };
        if !watch_cids.iter().any(|w| cid.contains(w) || w.contains(cid)) {
            return false;
        }
    }

    // Check watch_tags filter
    if !watch_tags.is_empty() {
        let event_tags = match &event.event {
            KernelEvent::ObjectStored { tags, .. } => Some(tags.as_slice()),
            _ => None,
        };
        let Some(tags) = event_tags else { return false; };
        if !watch_tags.iter().any(|w| tags.iter().any(|t| t == w || t.contains(w))) {
            return false;
        }
    }

    true
}

/// Handle DeltaSince request — query changes since a given sequence number.
pub fn handle_delta_since(
    since_seq: u64,
    watch_cids: Vec<String>,
    watch_tags: Vec<String>,
    limit: Option<usize>,
    event_bus: &crate::kernel::event_bus::EventBus,
) -> DeltaResult {
    // Get all events since the given sequence
    let mut events = event_bus.events_since(since_seq);

    // Apply filters
    if !watch_cids.is_empty() || !watch_tags.is_empty() {
        events.retain(|e| event_matches_filter(e, &watch_cids, &watch_tags));
    }

    // Apply limit
    if let Some(limit) = limit {
        events.truncate(limit);
    }

    // Build change entries
    let changes: Vec<ChangeEntry> = events.iter().map(change_entry_from_event).collect();

    // Calculate token estimate
    let token_estimate = changes.iter().map(|c| {
        // Rough estimate: summary string length in tokens
        crate::api::semantic::estimate_tokens(&c.summary)
    }).sum();

    let to_seq = events.last().map(|e| e.seq).unwrap_or(since_seq);

    DeltaResult {
        changes,
        from_seq: since_seq,
        to_seq,
        token_estimate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::event_bus::EventBus;

    #[test]
    fn test_change_entry_object_stored() {
        let bus = EventBus::new();
        bus.emit(KernelEvent::ObjectStored {
            cid: "abc123def456".into(),
            agent_id: "agent-1".into(),
            tags: vec!["tag-a".into(), "tag-b".into()],
        });

        let events = bus.events_since(0);
        assert_eq!(events.len(), 1);

        let entry = change_entry_from_event(&events[0]);
        assert_eq!(entry.cid, "abc123def456");
        assert_eq!(entry.change_type, "stored");
        assert!(entry.summary.contains("ObjectStored"));
        assert!(entry.summary.contains("abc123de")); // first 8 chars
        assert!(entry.summary.contains("agent-1"));
        assert!(entry.summary.contains("tag-a,tag-b"));
        assert_eq!(entry.changed_by, "agent-1");
    }

    #[test]
    fn test_change_entry_with_truncated_cid() {
        let bus = EventBus::new();
        bus.emit(KernelEvent::ObjectStored {
            cid: "ab".into(), // shorter than 8 chars
            agent_id: "a1".into(),
            tags: vec![],
        });

        let events = bus.events_since(0);
        let entry = change_entry_from_event(&events[0]);
        // Should not panic and should handle short CID
        assert_eq!(entry.cid, "ab");
        assert!(entry.summary.contains("ab"));
    }

    #[test]
    fn test_delta_since_with_limit() {
        let bus = EventBus::new();
        for i in 0..5 {
            bus.emit(KernelEvent::ObjectStored {
                cid: format!("cid-{}", i),
                agent_id: "a1".into(),
                tags: vec![],
            });
        }

        let result = handle_delta_since(0, vec![], vec![], Some(3), &bus);
        assert_eq!(result.changes.len(), 3);
        assert_eq!(result.from_seq, 0);
        assert_eq!(result.to_seq, 3); // 3rd event has seq=3 (events 1,2,3 after since_seq=0)
    }

    #[test]
    fn test_delta_since_empty_result() {
        let bus = EventBus::new();
        let result = handle_delta_since(100, vec![], vec![], None, &bus);
        assert!(result.changes.is_empty());
        assert_eq!(result.from_seq, 100);
        assert_eq!(result.to_seq, 100);
    }
}