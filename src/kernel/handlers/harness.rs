//! Harness 11 Generalized Verbs handlers (v1.0).
//!
//! Implements the core "generalized verbs" from Harness Engineering 
//! (Agent = Model + Harness). These verbs unify the specific domain APIs 
//! into a consistent, token-efficient interface.

use crate::api::semantic::{ApiRequest, ApiResponse};
use super::super::AIKernel;

impl AIKernel {
    pub(crate) fn handle_harness(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::HarnessGet { id, variant, agent_id } => self.harness_get(id, variant, agent_id),
            ApiRequest::HarnessList { variant, filter, limit, offset, agent_id } => self.harness_list(variant, filter, limit, offset, agent_id),
            ApiRequest::HarnessSearch { query, variant, filter, limit, agent_id } => self.harness_search(query, variant, filter, limit, agent_id),
            ApiRequest::HarnessCreate { variant, data, tags, agent_id } => self.harness_create(variant, data, tags, agent_id),
            ApiRequest::HarnessUpdate { id, variant, data, agent_id } => self.harness_update(id, variant, data, agent_id),
            ApiRequest::HarnessDelete { id, variant, agent_id } => self.harness_delete(id, variant, agent_id),
            ApiRequest::HarnessExec { action, params, agent_id } => self.harness_exec(action, params, agent_id),
            ApiRequest::HarnessObserve { variant, agent_id } => self.harness_observe(variant, agent_id),
            ApiRequest::HarnessLink { src, dst, relation, weight, agent_id } => self.harness_link(src, dst, relation, weight, agent_id),
            ApiRequest::HarnessAsk { query, context_ids, agent_id } => self.harness_ask(query, context_ids, agent_id),
            ApiRequest::HarnessState { action, agent_id } => self.harness_state(action, agent_id),
            _ => unreachable!("non-harness request routed to handle_harness"),
        }
    }

    fn harness_get(&self, id: String, variant: Option<String>, agent_id: String) -> ApiResponse {
        let variant = variant.as_deref().unwrap_or("object");
        match variant {
            "object" => self.handle_cas(ApiRequest::Read { cid: id, agent_id, tenant_id: None, agent_token: None }),
            "node" => self.handle_graph(ApiRequest::GetNode { node_id: id, agent_id, tenant_id: None }),
            "task" => self.handle_agent(ApiRequest::AgentStatus { agent_id: id }), // Or specific task status handler
            _ => ApiResponse::error(format!("Unsupported harness_get variant: {}", variant)),
        }
    }

    fn harness_list(&self, variant: Option<String>, _filter: Option<serde_json::Value>, limit: Option<usize>, offset: Option<usize>, agent_id: String) -> ApiResponse {
        let variant = variant.as_deref().unwrap_or("object");
        match variant {
            "agent" => self.handle_agent(ApiRequest::ListAgents),
            "node" => self.handle_graph(ApiRequest::ListNodes { node_type: None, agent_id, tenant_id: None, limit, offset }),
            "event" => self.handle_events(ApiRequest::ListEvents { since: None, until: None, tags: vec![], event_type: None, agent_id, limit, offset }),
            _ => ApiResponse::error(format!("Unsupported harness_list variant: {}", variant)),
        }
    }

    fn harness_search(&self, query: String, variant: Option<String>, _filter: Option<serde_json::Value>, limit: Option<usize>, agent_id: String) -> ApiResponse {
        let variant = variant.as_deref().unwrap_or("semantic");
        match variant {
            "semantic" => self.handle_cas(ApiRequest::Search { 
                query, agent_id, tenant_id: None, agent_token: None, 
                limit, offset: None, require_tags: vec![], exclude_tags: vec![], 
                since: None, until: None, intent_context: None 
            }),
            "graph" => self.handle_graph(ApiRequest::Explore { cid: query, edge_type: None, depth: Some(1), agent_id }),
            _ => ApiResponse::error(format!("Unsupported harness_search variant: {}", variant)),
        }
    }

    fn harness_create(&self, variant: Option<String>, data: serde_json::Value, tags: Vec<String>, agent_id: String) -> ApiResponse {
        let variant = variant.as_deref().unwrap_or("object");
        match variant {
            "object" => {
                let content = data.as_str().map(|s| s.to_string()).unwrap_or_else(|| data.to_string());
                self.handle_cas(ApiRequest::Create { 
                    api_version: None, content, content_encoding: Default::default(), 
                    tags, agent_id, tenant_id: None, agent_token: None, intent: None 
                })
            },
            _ => ApiResponse::error(format!("Unsupported harness_create variant: {}", variant)),
        }
    }

    fn harness_update(&self, id: String, variant: Option<String>, data: serde_json::Value, agent_id: String) -> ApiResponse {
        let variant = variant.as_deref().unwrap_or("object");
        match variant {
            "object" => {
                let content = data.as_str().map(|s| s.to_string()).unwrap_or_else(|| data.to_string());
                self.handle_cas(ApiRequest::Update { 
                    cid: id, content, content_encoding: Default::default(), 
                    new_tags: None, agent_id, tenant_id: None, agent_token: None 
                })
            },
            _ => ApiResponse::error(format!("Unsupported harness_update variant: {}", variant)),
        }
    }

    fn harness_delete(&self, id: String, variant: Option<String>, agent_id: String) -> ApiResponse {
        let variant = variant.as_deref().unwrap_or("object");
        match variant {
            "object" => self.handle_cas(ApiRequest::Delete { cid: id, agent_id, tenant_id: None, agent_token: None }),
            _ => ApiResponse::error(format!("Unsupported harness_delete variant: {}", variant)),
        }
    }

    fn harness_exec(&self, action: String, params: serde_json::Value, agent_id: String) -> ApiResponse {
        match action.as_str() {
            "tool_call" => self.handle_tools(ApiRequest::ToolCall { tool: params["tool"].as_str().unwrap_or("").to_string(), params: params["params"].clone(), agent_id }),
            _ => ApiResponse::error(format!("Unsupported harness_exec action: {}", action)),
        }
    }

    fn harness_observe(&self, variant: Option<String>, agent_id: String) -> ApiResponse {
        let variant = variant.as_deref().unwrap_or("audit");
        match variant {
            "audit" => self.handle_system(ApiRequest::QueryGrowthReport { agent_id, period: crate::api::dto::GrowthPeriod::Daily }),
            "metrics" => self.handle_system(ApiRequest::AgentUsage { agent_id }),
            _ => ApiResponse::error(format!("Unsupported harness_observe variant: {}", variant)),
        }
    }

    fn harness_link(&self, src: String, dst: String, relation: Option<String>, weight: Option<f32>, agent_id: String) -> ApiResponse {
        use crate::fs::graph::KGEdgeType;
        let edge_type = relation.map(|r| match r.as_str() {
            "supersedes" => KGEdgeType::Supersedes,
            "caused_by" => KGEdgeType::CausedBy,
            _ => KGEdgeType::RelatedTo,
        }).unwrap_or(KGEdgeType::RelatedTo);
        
        self.handle_graph(ApiRequest::AddEdge { src_id: src, dst_id: dst, edge_type, weight, agent_id, tenant_id: None })
    }

    fn harness_ask(&self, query: String, _context_ids: Vec<String>, agent_id: String) -> ApiResponse {
        // Implementation for RAG-like asking within the OS
        self.handle_cas(ApiRequest::RecallRouted { agent_id, query, k: 5, tenant_id: None })
    }

    fn harness_state(&self, action: Option<String>, agent_id: String) -> ApiResponse {
        let action = action.as_deref().unwrap_or("checkpoint");
        match action {
            "checkpoint" => self.handle_agent(ApiRequest::AgentCheckpoint { agent_id }),
            "suspend" => self.handle_agent(ApiRequest::AgentSuspend { agent_id }),
            "resume" => self.handle_agent(ApiRequest::AgentResume { agent_id }),
            _ => ApiResponse::error(format!("Unsupported harness_state action: {}", action)),
        }
    }
}
