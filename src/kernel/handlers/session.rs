//! Session lifecycle handlers.

use super::super::ops;
use crate::api::semantic::{ApiRequest, ApiResponse};

impl super::super::AIKernel {
    pub(crate) fn handle_session(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::StartSession {
                agent_id,
                agent_token,
                last_seen_seq,
            } => {
                if let Err(e) = self.key_store.verify_agent_token(&agent_id, agent_token.as_deref()) {
                    return ApiResponse::error(e);
                }
                match ops::session::start_session(ops::session::StartSessionParams {
                    agent_id: &agent_id,
                    last_seen_seq,
                    session_store: &self.session_store,
                    event_bus: &self.event_bus,
                    root: &self.root,
                }) {
                    Ok(result) => {
                        let mut r = ApiResponse::ok();
                        r.session_started = Some(crate::api::semantic::SessionStarted {
                            session_id: result.session_id,
                            changes_since_last: result.changes_since_last,
                            watermark: result.watermark,
                        });
                        r
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::EndSession { agent_id, session_id } => {
                match ops::session::end_session(
                    &agent_id,
                    &session_id,
                    &self.session_store,
                    &self.root,
                    &self.event_bus,
                ) {
                    Ok(result) => {
                        let mut r = ApiResponse::ok();
                        r.session_ended = Some(crate::api::semantic::SessionEnded {
                            last_seq: result.last_seq,
                        });
                        r
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::RegisterSkill {
                agent_id,
                name,
                description,
                tags,
            } => match self.register_skill(&agent_id, &name, &description, tags) {
                Ok(node_id) => {
                    let mut r = ApiResponse::ok();
                    r.node_id = Some(node_id);
                    r
                }
                Err(e) => ApiResponse::error(e),
            },
            ApiRequest::DiscoverSkills {
                query,
                agent_id_filter,
                tag_filter,
            } => {
                let skills = self.discover_skills(query.as_deref(), agent_id_filter.as_deref(), tag_filter.as_deref());
                let mut r = ApiResponse::ok();
                r.discovered_skills = Some(skills);
                r
            }
            _ => unreachable!("non-session request routed to handle_session"),
        }
    }
}
#[cfg(test)]
mod tests {
    use crate::api::semantic::ApiRequest;
    use crate::kernel::tests::make_kernel;

    #[test]
    fn test_start_session() {
        let (kernel, _tmp) = make_kernel();
        // Register agent first to get a token
        let reg = kernel.handle_api_request(ApiRequest::RegisterAgent {
            name: "session_agent".to_string(),
        });
        let agent_id = reg.agent_id.unwrap();
        let token = reg.token.unwrap();

        let resp = kernel.handle_api_request(ApiRequest::StartSession {
            agent_id,
            agent_token: Some(token),
            last_seen_seq: None,
        });
        assert!(resp.ok, "StartSession should succeed: {:?}", resp.error);
        let started = resp.session_started.unwrap();
        assert!(!started.session_id.is_empty(), "should return session_id");
    }

    #[test]
    fn test_start_and_end_session() {
        let (kernel, _tmp) = make_kernel();
        let reg = kernel.handle_api_request(ApiRequest::RegisterAgent {
            name: "cycle_agent".to_string(),
        });
        let agent_id = reg.agent_id.clone().unwrap();
        let token = reg.token.unwrap();

        let start = kernel.handle_api_request(ApiRequest::StartSession {
            agent_id: agent_id.clone(),
            agent_token: Some(token),
            last_seen_seq: None,
        });
        assert!(start.ok);
        let session_id = start.session_started.unwrap().session_id;

        let resp = kernel.handle_api_request(ApiRequest::EndSession { agent_id, session_id });
        assert!(resp.ok, "EndSession should succeed: {:?}", resp.error);
        let ended = resp.session_ended.unwrap();
        assert_eq!(ended.last_seq, kernel.event_bus.current_seq());
    }

    #[test]
    fn test_register_skill() {
        let (kernel, _tmp) = make_kernel();
        // Register agent to get a valid UUID
        let reg = kernel.handle_api_request(ApiRequest::RegisterAgent {
            name: "skill_agent".to_string(),
        });
        let agent_id = reg.agent_id.unwrap();
        let resp = kernel.handle_api_request(ApiRequest::RegisterSkill {
            agent_id,
            name: "my_skill".to_string(),
            description: "A test skill".to_string(),
            tags: vec!["test".to_string()],
        });
        assert!(resp.ok, "RegisterSkill should succeed: {:?}", resp.error);
        assert!(resp.node_id.is_some(), "should return node_id");
    }

    #[test]
    fn test_discover_skills() {
        let (kernel, _tmp) = make_kernel();
        let reg = kernel.handle_api_request(ApiRequest::RegisterAgent {
            name: "discover_agent".to_string(),
        });
        let agent_id = reg.agent_id.unwrap();
        kernel.handle_api_request(ApiRequest::RegisterSkill {
            agent_id,
            name: "search_skill".to_string(),
            description: "Search skill".to_string(),
            tags: vec!["search".to_string()],
        });
        let resp = kernel.handle_api_request(ApiRequest::DiscoverSkills {
            query: None,
            agent_id_filter: None,
            tag_filter: None,
        });
        assert!(resp.ok, "DiscoverSkills should succeed: {:?}", resp.error);
        let skills = resp.discovered_skills.unwrap();
        assert_eq!(skills.len(), 1, "should find 1 skill");
    }

    #[test]
    fn test_discover_skills_with_query() {
        let (kernel, _tmp) = make_kernel();
        let reg = kernel.handle_api_request(ApiRequest::RegisterAgent {
            name: "query_agent".to_string(),
        });
        let agent_id = reg.agent_id.unwrap();
        kernel.handle_api_request(ApiRequest::RegisterSkill {
            agent_id: agent_id.clone(),
            name: "data_analysis".to_string(),
            description: "Analyze data".to_string(),
            tags: vec!["analytics".to_string()],
        });
        kernel.handle_api_request(ApiRequest::RegisterSkill {
            agent_id,
            name: "web_search".to_string(),
            description: "Search the web".to_string(),
            tags: vec!["search".to_string()],
        });
        let resp = kernel.handle_api_request(ApiRequest::DiscoverSkills {
            query: Some("analysis".to_string()),
            agent_id_filter: None,
            tag_filter: None,
        });
        assert!(resp.ok, "DiscoverSkills with query should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_start_session_invalid_token() {
        let (kernel, _tmp) = make_kernel();
        let reg = kernel.handle_api_request(ApiRequest::RegisterAgent {
            name: "token_agent".to_string(),
        });
        let agent_id = reg.agent_id.unwrap();

        let resp = kernel.handle_api_request(ApiRequest::StartSession {
            agent_id,
            agent_token: Some("wrong_token".to_string()),
            last_seen_seq: None,
        });
        assert!(!resp.ok, "StartSession with invalid token should fail");
    }
}
