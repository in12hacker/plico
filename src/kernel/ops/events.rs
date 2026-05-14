//! Event operations — knowledge graph event containers.

use crate::fs::{EventType, EventSummary, EventRelation};
use crate::api::permission::{PermissionContext, PermissionAction};
use crate::temporal::{TemporalResolver, RULE_BASED_RESOLVER};
use crate::kernel::event_bus::KernelEvent;

impl crate::kernel::AIKernel {
    /// Create an event and register it in the knowledge graph.
    pub fn create_event(
        &self,
        params: crate::fs::semantic_fs::events::CreateEventParams<'_>,
    ) -> std::io::Result<String> {
        let label = params.label.to_string();
        let agent_id = params.agent_id.to_string();
        let ctx = PermissionContext::new(agent_id.clone(), crate::DEFAULT_TENANT.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        let event_id = self.fs.create_event(params)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        self.event_bus.emit(KernelEvent::EventCreated {
            event_id: event_id.clone(),
            label,
            agent_id,
        });
        Ok(event_id)
    }

    /// List events matching time range, tags, and optional event type.
    pub fn list_events(
        &self,
        since: Option<u64>,
        until: Option<u64>,
        tags: &[String],
        event_type: Option<EventType>,
        agent_id: Option<&str>,
    ) -> Vec<EventSummary> {
        self.fs.list_events(since, until, tags, event_type, agent_id).unwrap_or_default()
    }

    /// List events by natural-language time expression (e.g. "几天前", "上周").
    pub fn list_events_text(
        &self,
        time_expression: &str,
        tags: &[String],
        event_type: Option<EventType>,
        agent_id: Option<&str>,
    ) -> std::io::Result<Vec<EventSummary>> {
        let resolver: &dyn TemporalResolver = &RULE_BASED_RESOLVER;
        self.fs.list_events_by_time(time_expression, tags, event_type, resolver, agent_id)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    /// Attach a target to an event via a typed edge.
    pub fn event_attach(
        &self,
        event_id: &str,
        target_id: &str,
        relation: EventRelation,
        agent_id: &str,
    ) -> std::io::Result<()> {
        let ctx = PermissionContext::new(agent_id.to_string(), crate::DEFAULT_TENANT.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        self.fs.event_attach(event_id, target_id, relation, agent_id)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::tests::make_kernel;
    use crate::fs::semantic_fs::events::CreateEventParams;

    #[test]
    fn test_create_event() {
        let (kernel, _dir) = make_kernel();
        let params = CreateEventParams {
            label: "test-event",
            event_type: EventType::Work,
            start_time: Some(1000),
            end_time: Some(2000),
            location: Some("test-location"),
            tags: vec!["test".to_string()],
            agent_id: "kernel",
        };
        let event_id = kernel.create_event(params).unwrap();
        assert!(!event_id.is_empty());
        assert!(event_id.starts_with("evt:"));
    }

    #[test]
    fn test_list_events_by_time_range_and_tags() {
        let (kernel, _dir) = make_kernel();
        let params = CreateEventParams {
            label: "listable-event",
            event_type: EventType::Task,
            start_time: Some(1000),
            end_time: Some(2000),
            location: None,
            tags: vec!["alpha".to_string(), "beta".to_string()],
            agent_id: "kernel",
        };
        let _event_id = kernel.create_event(params).unwrap();

        // List by time range
        let events = kernel.list_events(Some(500), Some(3000), &[], None, None);
        assert!(!events.is_empty());

        // List by tags
        let events = kernel.list_events(None, None, &["alpha".to_string()], None, None);
        assert!(!events.is_empty());

        // List with non-matching tag should return empty
        let events = kernel.list_events(None, None, &["nonexistent".to_string()], None, None);
        assert!(events.is_empty());
    }

    #[test]
    fn test_event_attach() {
        let (kernel, _dir) = make_kernel();

        // Create an event
        let params = CreateEventParams {
            label: "attach-event",
            event_type: EventType::Report,
            start_time: Some(1000),
            end_time: None,
            location: None,
            tags: vec!["attach".to_string()],
            agent_id: "kernel",
        };
        let event_id = kernel.create_event(params).unwrap();

        // Create a CAS object to use as target
        let cid = kernel.semantic_create(
            b"target content".to_vec(),
            vec!["target".to_string()],
            "kernel",
            None,
        ).unwrap();

        // Attach
        let result = kernel.event_attach(&event_id, &cid, EventRelation::Artifact, "kernel");
        assert!(result.is_ok());
    }
}
