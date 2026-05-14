//! Event lifecycle and event bus handlers.

use crate::api::semantic::{ApiRequest, ApiResponse};
use super::super::event_bus;
use super::super::ops;

#[cfg(test)]
mod tests {
    use crate::kernel::tests::make_kernel;
    use crate::api::semantic::ApiRequest;
    use crate::fs::{EventType, EventRelation};

    #[test]
    fn test_create_event() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::CreateEvent {
            label: "Sprint Planning".to_string(),
            event_type: EventType::Task,
            start_time: Some(1000),
            end_time: Some(2000),
            location: Some("virtual".to_string()),
            tags: vec!["planning".to_string()],
            agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok, "CreateEvent should succeed: {:?}", resp.error);
        assert!(resp.cid.is_some(), "should return event id");
    }

    #[test]
    fn test_list_events() {
        let (kernel, _tmp) = make_kernel();
        // Create a couple of events
        kernel.handle_api_request(ApiRequest::CreateEvent {
            label: "Event 1".to_string(),
            event_type: EventType::Work,
            start_time: Some(1000),
            end_time: Some(2000),
            location: None,
            tags: vec!["work".to_string()],
            agent_id: "test_agent".to_string(),
        });
        kernel.handle_api_request(ApiRequest::CreateEvent {
            label: "Event 2".to_string(),
            event_type: EventType::Sync,
            start_time: Some(3000),
            end_time: Some(4000),
            location: None,
            tags: vec!["meeting".to_string()],
            agent_id: "test_agent".to_string(),
        });
        let resp = kernel.handle_api_request(ApiRequest::ListEvents {
            since: None,
            until: None,
            tags: vec![],
            event_type: None,
            agent_id: "test_agent".to_string(),
            limit: None,
            offset: None,
        });
        assert!(resp.ok, "ListEvents should succeed: {:?}", resp.error);
        let events = resp.events.unwrap();
        assert_eq!(events.len(), 2, "should list 2 events");
        assert_eq!(resp.total_count, Some(2));
    }

    #[test]
    fn test_list_events_with_limit_offset() {
        let (kernel, _tmp) = make_kernel();
        for i in 0..5 {
            kernel.handle_api_request(ApiRequest::CreateEvent {
                label: format!("Event {}", i),
                event_type: EventType::Work,
                start_time: Some(i * 1000),
                end_time: None,
                location: None,
                tags: vec![],
                agent_id: "test_agent".to_string(),
            });
        }
        let resp = kernel.handle_api_request(ApiRequest::ListEvents {
            since: None,
            until: None,
            tags: vec![],
            event_type: None,
            agent_id: "test_agent".to_string(),
            limit: Some(2),
            offset: Some(1),
        });
        assert!(resp.ok);
        let events = resp.events.unwrap();
        assert_eq!(events.len(), 2, "should return 2 events (limit=2)");
        assert_eq!(resp.total_count, Some(5));
        assert_eq!(resp.has_more, Some(true));
    }

    #[test]
    fn test_event_subscribe_poll_unsubscribe() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::EventSubscribe {
            agent_id: "test_agent".to_string(),
            event_types: None,
            agent_ids: None,
        });
        assert!(resp.ok, "EventSubscribe should succeed: {:?}", resp.error);
        let sub_id = resp.subscription_id.unwrap();

        // Poll — should return empty initially
        let resp = kernel.handle_api_request(ApiRequest::EventPoll {
            subscription_id: sub_id.clone(),
        });
        assert!(resp.ok, "EventPoll should succeed: {:?}", resp.error);

        // Unsubscribe
        let resp = kernel.handle_api_request(ApiRequest::EventUnsubscribe {
            subscription_id: sub_id,
        });
        assert!(resp.ok, "EventUnsubscribe should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_event_poll_unknown_subscription() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::EventPoll {
            subscription_id: "nonexistent_sub".to_string(),
        });
        assert!(!resp.ok, "EventPoll for unknown subscription should fail");
    }

    #[test]
    fn test_event_unsubscribe_unknown() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::EventUnsubscribe {
            subscription_id: "nonexistent_sub".to_string(),
        });
        assert!(!resp.ok, "EventUnsubscribe for unknown subscription should fail");
    }

    #[test]
    fn test_event_history() {
        let (kernel, _tmp) = make_kernel();
        // Create an event to generate history
        kernel.handle_api_request(ApiRequest::CreateEvent {
            label: "History Event".to_string(),
            event_type: EventType::Work,
            start_time: None,
            end_time: None,
            location: None,
            tags: vec![],
            agent_id: "test_agent".to_string(),
        });
        let resp = kernel.handle_api_request(ApiRequest::EventHistory {
            since_seq: None,
            agent_id_filter: None,
            limit: None,
        });
        assert!(resp.ok, "EventHistory should succeed: {:?}", resp.error);
        assert!(resp.event_history.is_some());
    }

    #[test]
    fn test_event_attach() {
        let (kernel, _tmp) = make_kernel();
        let create = kernel.handle_api_request(ApiRequest::CreateEvent {
            label: "Attach Event".to_string(),
            event_type: EventType::Work,
            start_time: None,
            end_time: None,
            location: None,
            tags: vec![],
            agent_id: "test_agent".to_string(),
        });
        let event_id = create.cid.unwrap();
        let resp = kernel.handle_api_request(ApiRequest::EventAttach {
            event_id,
            target_id: "some_target".to_string(),
            relation: EventRelation::Artifact,
            agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok, "EventAttach should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_delta_since() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::DeltaSince {
            agent_id: "test_agent".to_string(),
            since_seq: 0,
            watch_cids: vec![],
            watch_tags: vec![],
            limit: Some(10),
        });
        assert!(resp.ok, "DeltaSince should succeed: {:?}", resp.error);
        assert!(resp.delta_result.is_some());
    }
}

impl super::super::AIKernel {
    pub(crate) fn handle_events(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::CreateEvent { label, event_type, start_time, end_time, location, tags, agent_id } => {
                match self.create_event(crate::fs::semantic_fs::events::CreateEventParams {
                    label: &label, event_type, start_time, end_time,
                    location: location.as_deref(), tags, agent_id: &agent_id,
                }) {
                    Ok(id) => ApiResponse::with_cid(id),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::ListEvents { since, until, tags, event_type, agent_id, limit, offset } => {
                let all_events = self.list_events(since, until, &tags, event_type,
                    if agent_id.is_empty() { None } else { Some(&agent_id) });
                let total = all_events.len();
                let off = offset.unwrap_or(0);
                let lim = limit.unwrap_or(total);
                let page: Vec<_> = all_events.into_iter().skip(off).take(lim).collect();
                let mut r = ApiResponse::with_events(page.clone());
                r.total_count = Some(total);
                r.has_more = Some(off + page.len() < total);
                r
            }
            ApiRequest::ListEventsText { time_expression, tags, event_type, agent_id } => {
                match self.list_events_text(&time_expression, &tags, event_type,
                    if agent_id.is_empty() { None } else { Some(&agent_id) }) {
                    Ok(events) => ApiResponse::with_events(events),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::EventAttach { event_id, target_id, relation, agent_id } => {
                match self.event_attach(&event_id, &target_id, relation, &agent_id) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::EventSubscribe { agent_id: _, event_types, agent_ids } => {
                let filter = if event_types.is_some() || agent_ids.is_some() {
                    Some(event_bus::EventFilter { event_types, agent_ids })
                } else {
                    None
                };
                let sub_id = self.event_subscribe_filtered(filter);
                let mut r = ApiResponse::ok();
                r.subscription_id = Some(sub_id);
                r
            }
            ApiRequest::EventPoll { subscription_id } => {
                match self.event_poll(&subscription_id) {
                    Some(events) => {
                        let mut r = ApiResponse::ok();
                        r.kernel_events = Some(events);
                        r
                    }
                    None => ApiResponse::error(format!("Unknown subscription: {}", subscription_id)),
                }
            }
            ApiRequest::EventUnsubscribe { subscription_id } => {
                if self.event_unsubscribe(&subscription_id) {
                    ApiResponse::ok()
                } else {
                    ApiResponse::error(format!("Unknown subscription: {}", subscription_id))
                }
            }
            ApiRequest::EventHistory { since_seq, agent_id_filter, limit } => {
                let events = match (&since_seq, &agent_id_filter) {
                    (_, Some(aid)) => {
                        let mut evts = self.event_bus.events_by_agent(aid);
                        if let Some(seq) = since_seq {
                            evts.retain(|e| e.seq > seq);
                        }
                        evts
                    }
                    (Some(seq), None) => self.event_bus.events_since(*seq),
                    (None, None) => self.event_bus.snapshot_events(),
                };
                let limited = if let Some(lim) = limit {
                    events.into_iter().take(lim).collect()
                } else {
                    events
                };
                let mut r = ApiResponse::ok();
                r.event_history = Some(limited);
                r
            }
            ApiRequest::DeltaSince { agent_id: _, since_seq, watch_cids, watch_tags, limit } => {
                let result = ops::delta::handle_delta_since(
                    since_seq, watch_cids, watch_tags, limit, &self.event_bus,
                );
                let mut r = ApiResponse::ok();
                r.delta_result = Some(result);
                r
            }
            _ => unreachable!("non-events request routed to handle_events"),
        }
    }
}
