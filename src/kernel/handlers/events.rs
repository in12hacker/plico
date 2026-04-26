//! Event lifecycle and event bus handlers.

use crate::api::semantic::{ApiRequest, ApiResponse};
use super::super::event_bus;
use super::super::ops;

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
