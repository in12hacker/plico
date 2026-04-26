//! Session lifecycle handlers.

use crate::api::semantic::{ApiRequest, ApiResponse};
use crate::scheduler::AgentId;
use super::super::ops;

impl super::super::AIKernel {
    pub(crate) fn handle_session(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::StartSession { agent_id, agent_token, intent_hint, load_tiers, last_seen_seq } => {
                if let Err(e) = self.key_store.verify_agent_token(&agent_id, agent_token.as_deref()) {
                    return ApiResponse::error(e);
                }
                match ops::session::start_session_orchestrate(ops::session::SessionStartParams {
                    agent_id: &agent_id, intent_hint, load_tiers, last_seen_seq,
                    session_store: &self.session_store, event_bus: &self.event_bus,
                    memory: &self.memory, prefetch: &self.prefetch,
                    fs: &self.fs, root: &self.root,
                }) {
                    Ok(result) => {
                        let mut r = ApiResponse::ok();
                        r.session_started = Some(crate::api::semantic::SessionStarted {
                            session_id: result.session_id,
                            restored_checkpoint: result.restored_checkpoint,
                            warm_context: result.warm_context,
                            changes_since_last: result.changes_since_last,
                            token_estimate: result.token_estimate,
                        });
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::EndSession { agent_id, session_id, auto_checkpoint } => {
                match ops::session::end_session_orchestrate(
                    &agent_id, &session_id, auto_checkpoint,
                    &self.session_store, &self.memory, &self.root,
                    Some(&self.prefetch),
                ) {
                    Ok(result) => {
                        self.prefetch.apply_feedback_from_history(&agent_id);
                        let mut r = ApiResponse::ok();
                        r.session_ended = Some(crate::api::semantic::SessionEnded {
                            checkpoint_id: result.checkpoint_id,
                            last_seq: result.last_seq,
                            consolidation: Some(result.consolidation),
                            total_tokens_consumed: self.scheduler.get_usage(&AgentId(agent_id.clone())).total_tokens_consumed,
                        });
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::RegisterSkill { agent_id, name, description, tags } => {
                match self.register_skill(&agent_id, &name, &description, tags) {
                    Ok(node_id) => {
                        let mut r = ApiResponse::ok();
                        r.node_id = Some(node_id);
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::DiscoverSkills { query, agent_id_filter, tag_filter } => {
                let skills = self.discover_skills(
                    query.as_deref(), agent_id_filter.as_deref(), tag_filter.as_deref(),
                );
                let mut r = ApiResponse::ok();
                r.discovered_skills = Some(skills);
                r
            }
            _ => unreachable!("non-session request routed to handle_session"),
        }
    }
}
