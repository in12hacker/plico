//! Core Polymorphic Verbs handlers (v1.0).
//!
//! Implements 11 polymorphic operations that unify specific domain APIs
//! into a consistent, Plico-native interface.

use crate::api::semantic::{ApiRequest, ApiResponse};
use super::super::AIKernel;

impl AIKernel {
    pub(crate) fn handle_core_ops(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::CoreGet { id, variant, agent_id } => self.core_get(id, variant, agent_id),
            ApiRequest::CoreList { variant, filter, limit, offset, agent_id } => self.core_list(variant, filter, limit, offset, agent_id),
            ApiRequest::CoreSearch { query, variant, filter, limit, agent_id } => self.core_search(query, variant, filter, limit, agent_id),
            ApiRequest::CoreCreate { variant, data, tags, agent_id } => self.core_create(variant, data, tags, agent_id),
            ApiRequest::CoreUpdate { id, variant, data, agent_id } => self.core_update(id, variant, data, agent_id),
            ApiRequest::CoreDelete { id, variant, agent_id } => self.core_delete(id, variant, agent_id),
            ApiRequest::CoreExec { action, params, agent_id } => self.core_exec(action, params, agent_id),
            ApiRequest::CoreObserve { variant, agent_id } => self.core_observe(variant, agent_id),
            ApiRequest::CoreLink { src, dst, relation, weight, agent_id } => self.core_link(src, dst, relation, weight, agent_id),
            ApiRequest::CoreAsk { query, context_ids, agent_id } => self.core_ask(query, context_ids, agent_id),
            ApiRequest::CoreState { action, agent_id } => self.core_state(action, agent_id),
            _ => unreachable!("non-core request routed to handle_core_ops"),
        }
    }

    fn core_get(&self, id: String, variant: Option<String>, agent_id: String) -> ApiResponse {
        let variant = variant.as_deref().unwrap_or("object");
        match variant {
            "object" | "cas" => self.handle_cas(ApiRequest::Read { cid: id, agent_id, tenant_id: None, agent_token: None }),
            "node" | "graph" => self.handle_graph(ApiRequest::GetNode { node_id: id, agent_id, tenant_id: None }),
            "task" => self.handle_agent(ApiRequest::AgentStatus { agent_id: id }),
            "event" => self.handle_events(ApiRequest::ListEvents { since: None, until: None, tags: vec![format!("id:{}", id)], event_type: None, agent_id, limit: Some(1), offset: None }),
            _ => ApiResponse::error(format!("Unsupported core_get variant: {}", variant)),
        }
    }

    fn core_list(&self, variant: Option<String>, _filter: Option<serde_json::Value>, limit: Option<usize>, offset: Option<usize>, agent_id: String) -> ApiResponse {
        let variant = variant.as_deref().unwrap_or("object");
        match variant {
            "agent" => self.handle_agent(ApiRequest::ListAgents),
            "node" | "graph" => self.handle_graph(ApiRequest::ListNodes { node_type: None, agent_id, tenant_id: None, limit, offset }),
            "event" => self.handle_events(ApiRequest::ListEvents { since: None, until: None, tags: vec![], event_type: None, agent_id, limit, offset }),
            _ => ApiResponse::error(format!("Unsupported core_list variant: {}", variant)),
        }
    }

    fn core_search(&self, query: String, variant: Option<String>, _filter: Option<serde_json::Value>, limit: Option<usize>, agent_id: String) -> ApiResponse {
        let variant = variant.as_deref().unwrap_or("semantic");
        match variant {
            "semantic" | "unified" => self.handle_cas(ApiRequest::Search { 
                query, agent_id, tenant_id: None, agent_token: None, 
                limit, offset: None, require_tags: vec![], exclude_tags: vec![], 
                since: None, until: None, intent_context: None 
            }),
            "graph" => self.handle_graph(ApiRequest::Explore { cid: query, edge_type: None, depth: Some(1), agent_id }),
            "keyword" => self.handle_cas(ApiRequest::Search { 
                query, agent_id, tenant_id: None, agent_token: None, 
                limit, offset: None, require_tags: vec![], exclude_tags: vec![], 
                since: None, until: None, intent_context: Some("keyword".into()) 
            }),
            _ => ApiResponse::error(format!("Unsupported core_search variant: {}", variant)),
        }
    }

    fn core_create(&self, variant: Option<String>, data: serde_json::Value, tags: Vec<String>, agent_id: String) -> ApiResponse {
        let variant = variant.as_deref().unwrap_or("object");
        match variant {
            "object" | "cas" => {
                let content = data.as_str().map(|s| s.to_string()).unwrap_or_else(|| data.to_string());
                self.handle_cas(ApiRequest::Create { 
                    api_version: None, content, content_encoding: Default::default(), 
                    tags, agent_id, tenant_id: None, agent_token: None, intent: None 
                })
            },
            _ => ApiResponse::error(format!("Unsupported core_create variant: {}", variant)),
        }
    }

    fn core_update(&self, id: String, variant: Option<String>, data: serde_json::Value, agent_id: String) -> ApiResponse {
        let variant = variant.as_deref().unwrap_or("object");
        match variant {
            "object" | "cas" => {
                let content = data.as_str().map(|s| s.to_string()).unwrap_or_else(|| data.to_string());
                self.handle_cas(ApiRequest::Update { 
                    cid: id, content, content_encoding: Default::default(), 
                    new_tags: None, agent_id, tenant_id: None, agent_token: None 
                })
            },
            _ => ApiResponse::error(format!("Unsupported core_update variant: {}", variant)),
        }
    }

    fn core_delete(&self, id: String, variant: Option<String>, agent_id: String) -> ApiResponse {
        let variant = variant.as_deref().unwrap_or("object");
        match variant {
            "object" | "cas" => self.handle_cas(ApiRequest::Delete { cid: id, agent_id, tenant_id: None, agent_token: None }),
            "node" | "graph" => self.handle_graph(ApiRequest::RemoveNode { node_id: id, agent_id, tenant_id: None }),
            _ => ApiResponse::error(format!("Unsupported core_delete variant: {}", variant)),
        }
    }

    fn core_exec(&self, action: String, params: serde_json::Value, agent_id: String) -> ApiResponse {
        match action.as_str() {
            "tool_call" => self.handle_tools(ApiRequest::ToolCall { 
                tool: params["tool"].as_str().unwrap_or("").to_string(), 
                params: params["params"].clone(), 
                agent_id 
            }),
            "retry_diagnostic" => {
                let task_id = params["task_id"].as_str().unwrap_or("");
                self.diagnostic_store.mark_fixed(task_id);
                let mut resp = ApiResponse::ok();
                resp.message = Some(format!("Recovery initiated for task {}", task_id));
                resp
            }
            _ => ApiResponse::error(format!("Unsupported core_exec action: {}", action)),
        }
    }

    fn core_observe(&self, variant: Option<String>, agent_id: String) -> ApiResponse {
        let variant = variant.as_deref().unwrap_or("metrics");
        match variant {
            "audit" => self.handle_system(ApiRequest::QueryGrowthReport { agent_id, period: crate::api::dto::GrowthPeriod::Last7Days }),
            "metrics" => self.handle_system(ApiRequest::AgentUsage { agent_id }),
            "diagnostic" => {
                let reports = self.diagnostic_store.list_for_agent(&agent_id);
                let mut resp = ApiResponse::ok();
                resp.message = Some(format!("Found {} pending diagnostic reports", reports.len()));
                resp.data = Some(serde_json::to_string(&reports).unwrap_or_default());
                resp
            }
            _ => ApiResponse::error(format!("Unsupported core_observe variant: {}", variant)),
        }
    }

    fn core_link(&self, src: String, dst: String, relation: Option<String>, weight: Option<f32>, agent_id: String) -> ApiResponse {
        use crate::fs::graph::KGEdgeType;
        let edge_type = relation.map(|r| match r.as_str() {
            "supersedes" => KGEdgeType::Supersedes,
            "caused_by" => KGEdgeType::CausedBy,
            _ => KGEdgeType::RelatedTo,
        }).unwrap_or(KGEdgeType::RelatedTo);
        
        self.handle_graph(ApiRequest::AddEdge { src_id: src, dst_id: dst, edge_type, weight, agent_id, tenant_id: None })
    }

    fn core_ask(&self, query: String, _context_ids: Vec<String>, agent_id: String) -> ApiResponse {
        self.handle_memory(ApiRequest::RecallRouted { agent_id, query: query.to_string(), k: 5, tenant_id: None })
    }

    fn core_state(&self, action: Option<String>, agent_id: String) -> ApiResponse {
        let action = action.as_deref().unwrap_or("checkpoint");
        match action {
            "checkpoint" => self.handle_agent(ApiRequest::AgentCheckpoint { agent_id }),
            "suspend" => self.handle_agent(ApiRequest::AgentSuspend { agent_id }),
            "resume" => self.handle_agent(ApiRequest::AgentResume { agent_id }),
            _ => ApiResponse::error(format!("Unsupported core_state action: {}", action)),
        }
    }
}
