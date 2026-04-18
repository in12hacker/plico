//! Event operations — knowledge graph event containers.

use crate::fs::{EventType, EventSummary, EventRelation};
use crate::api::permission::{PermissionContext, PermissionAction};
use crate::temporal::{TemporalResolver, RULE_BASED_RESOLVER};
use crate::kernel::event_bus::KernelEvent;

impl crate::kernel::AIKernel {
    /// Create an event and register it in the knowledge graph.
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
    ) -> std::io::Result<String> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        let event_id = self.fs.create_event(label, event_type, start_time, end_time, location, tags, agent_id)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        self.event_bus.emit(KernelEvent::EventCreated {
            event_id: event_id.clone(),
            label: label.to_string(),
            agent_id: agent_id.to_string(),
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
    ) -> Vec<EventSummary> {
        self.fs.list_events(since, until, tags, event_type).unwrap_or_default()
    }

    /// List events by natural-language time expression (e.g. "几天前", "上周").
    pub fn list_events_text(
        &self,
        time_expression: &str,
        tags: &[String],
        event_type: Option<EventType>,
    ) -> std::io::Result<Vec<EventSummary>> {
        let resolver: &dyn TemporalResolver = &RULE_BASED_RESOLVER;
        self.fs.list_events_by_time(time_expression, tags, event_type, resolver)
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
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        self.fs.event_attach(event_id, target_id, relation, agent_id)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }
}
