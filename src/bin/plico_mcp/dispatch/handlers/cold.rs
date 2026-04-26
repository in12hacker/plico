//! Cold-layer params routing with teaching error messages.

use plico::api::semantic::ApiRequest;
use plico::kernel::AIKernel;
use serde_json::Value;

use crate::dispatch::DEFAULT_AGENT;
use crate::format::format_plico_response;

pub(in crate::dispatch) fn dispatch_cold_layer(args: &Value, kernel: &AIKernel) -> Result<String, String> {
    let agent = args.get("agent_id").and_then(|a| a.as_str()).unwrap_or(DEFAULT_AGENT);

    let params = args.get("params")
        .and_then(|p| p.as_object())
        .ok_or("Cold-layer operation requires 'params' object. Example: {action:'plico', params:{method:'add_node', label:'MyNode', node_type:'entity'}, agent_id:'my-agent'}")?;

    let method = params.get("method")
        .and_then(|m| m.as_str())
        .ok_or("Missing 'method' in params. Valid methods: add_node, add_edge, causal_path, impact, delegate, complete, batch_create, batch_read, register, checkpoint, restore, subscribe, poll, unsubscribe, storage_stats, object_usage, evict_expired")?;

    if let Some(err) = validate_cold_params(method, params) {
        return Err(err);
    }

    let req = build_cold_request(method, params, agent)?;
    let resp = kernel.handle_api_request(req);

    if !resp.ok {
        let err = resp.error.unwrap_or_else(|| "unknown error".to_string());
        return Err(enhance_cold_error(method, &err));
    }

    format_plico_response(resp, args)
}

fn validate_cold_params(method: &str, params: &serde_json::Map<String, Value>) -> Option<String> {
    match method {
        "add_node" => {
            if !params.contains_key("label") {
                return Some("Missing 'label'. Example: {method:'add_node', label:'MyEntity', node_type:'entity', agent_id:'your-agent'}".to_string());
            }
            if !params.contains_key("node_type") {
                return Some("Missing 'node_type'. Example: {method:'add_node', label:'MyEntity', node_type:'entity', agent_id:'your-agent'}".to_string());
            }
        }
        "add_edge" => {
            if !params.contains_key("src_id") {
                return Some("Missing 'src_id'. Example: {method:'add_edge', src_id:'<node_a>', dst_id:'<node_b>', edge_type:'causes', agent_id:'your-agent'}".to_string());
            }
            if !params.contains_key("dst_id") {
                return Some("Missing 'dst_id'. Example: {method:'add_edge', src_id:'<node_a>', dst_id:'<node_b>', edge_type:'causes', agent_id:'your-agent'}".to_string());
            }
        }
        "causal_path" => {
            if !params.contains_key("from_id") {
                return Some("Missing 'from_id'. Example: {method:'causal_path', from_id:'<node_a>', to_id:'<node_b>', agent_id:'your-agent'}".to_string());
            }
            if !params.contains_key("to_id") {
                return Some("Missing 'to_id'. Example: {method:'causal_path', from_id:'<node_a>', to_id:'<node_b>', agent_id:'your-agent'}".to_string());
            }
        }
        "impact" => {
            if !params.contains_key("node_id") {
                return Some("Missing 'node_id'. Example: {method:'impact', node_id:'<node_id>', depth:3, agent_id:'your-agent'}".to_string());
            }
        }
        "delegate" => {
            if !params.contains_key("task_description") {
                return Some("Missing 'task_description'. Example: {method:'delegate', task_description:'analyze logs', to_agent:'<agent>', agent_id:'your-agent'}".to_string());
            }
        }
        "complete" => {
            if !params.contains_key("task_id") {
                return Some("Missing 'task_id'. Example: {method:'complete', task_id:'<task_id>', agent_id:'your-agent'}".to_string());
            }
        }
        "batch_create" => {
            if !params.contains_key("items") {
                return Some("Missing 'items' array. Example: {method:'batch_create', items:[{content:'text', tags:['tag']}], agent_id:'your-agent'}".to_string());
            }
        }
        _ => {}
    }
    None
}

fn build_cold_request(method: &str, params: &serde_json::Map<String, Value>, agent: &str) -> Result<ApiRequest, String> {
    use plico::fs::KGNodeType;
    use plico::fs::KGEdgeType;

    match method {
        "add_node" => {
            let label = params.get("label").and_then(|v| v.as_str()).unwrap();
            let node_type_str = params.get("node_type").and_then(|v| v.as_str()).unwrap_or("entity");
            let node_type = match node_type_str {
                "entity" => KGNodeType::Entity,
                "fact" => KGNodeType::Fact,
                "document" => KGNodeType::Document,
                "agent" => KGNodeType::Agent,
                "memory" => KGNodeType::Memory,
                _ => KGNodeType::Entity,
            };
            Ok(ApiRequest::AddNode {
                label: label.to_string(),
                node_type,
                properties: params.get("properties").cloned().unwrap_or(Value::Null),
                agent_id: agent.to_string(),
                tenant_id: None,
            })
        }
        "add_edge" => {
            let src_id = params.get("src_id").and_then(|v| v.as_str()).unwrap();
            let dst_id = params.get("dst_id").and_then(|v| v.as_str()).unwrap();
            let edge_type_str = params.get("edge_type").and_then(|v| v.as_str()).unwrap_or("causes");
            let edge_type = match edge_type_str {
                "causes" => KGEdgeType::Causes,
                "reminds" => KGEdgeType::Reminds,
                "part_of" => KGEdgeType::PartOf,
                "similar_to" => KGEdgeType::SimilarTo,
                "related_to" => KGEdgeType::RelatedTo,
                _ => KGEdgeType::Causes,
            };
            Ok(ApiRequest::AddEdge {
                src_id: src_id.to_string(),
                dst_id: dst_id.to_string(),
                edge_type,
                weight: params.get("weight").and_then(|v| v.as_f64()).map(|w| w as f32),
                agent_id: agent.to_string(),
                tenant_id: None,
            })
        }
        "causal_path" => {
            let from_id = params.get("from_id").and_then(|v| v.as_str()).unwrap();
            let to_id = params.get("to_id").and_then(|v| v.as_str()).unwrap();
            Ok(ApiRequest::FindPaths {
                src_id: from_id.to_string(),
                dst_id: to_id.to_string(),
                max_depth: params.get("depth").and_then(|v| v.as_u64()).map(|d| d as u8),
                weighted: params.get("weighted").and_then(|v| v.as_bool()).unwrap_or(false),
                agent_id: agent.to_string(),
                tenant_id: None,
            })
        }
        "impact" => {
            let node_id = params.get("node_id").and_then(|v| v.as_str()).unwrap();
            Ok(ApiRequest::FindPaths {
                src_id: node_id.to_string(),
                dst_id: "*".to_string(),
                max_depth: params.get("depth").and_then(|v| v.as_u64()).map(|d| d as u8),
                weighted: params.get("weighted").and_then(|v| v.as_bool()).unwrap_or(false),
                agent_id: agent.to_string(),
                tenant_id: None,
            })
        }
        "delegate" => {
            let task_description = params.get("task_description").and_then(|v| v.as_str()).unwrap();
            let task_id = params.get("task_id").and_then(|v| v.as_str()).map(String::from).unwrap_or_else(|| {
                use std::time::{SystemTime, UNIX_EPOCH};
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos().to_string()
            });
            Ok(ApiRequest::DelegateTask {
                task_id: task_id.to_string(),
                from_agent: agent.to_string(),
                to_agent: params.get("to_agent").and_then(|v| v.as_str()).unwrap_or("default").to_string(),
                intent: task_description.to_string(),
                context_cids: vec![],
                deadline_ms: None,
            })
        }
        "complete" => {
            let task_id = params.get("task_id").and_then(|v| v.as_str()).unwrap();
            Ok(ApiRequest::TaskComplete {
                task_id: task_id.to_string(),
                agent_id: agent.to_string(),
                result_cids: vec![],
            })
        }
        "query_task" => {
            let task_id = params.get("task_id").and_then(|v| v.as_str()).unwrap();
            Ok(ApiRequest::QueryTaskStatus {
                task_id: task_id.to_string(),
            })
        }
        "batch_create" => {
            let items = params.get("items")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter().filter_map(|item| {
                        Some(plico::api::semantic::BatchCreateItem {
                            content: item.get("content")?.as_str()?.to_string(),
                            tags: item.get("tags").and_then(|t| t.as_array())
                                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                                .unwrap_or_default(),
                            content_encoding: Default::default(),
                            intent: item.get("intent").and_then(|v| v.as_str()).map(String::from),
                        })
                    }).collect()
                })
                .unwrap_or_default();
            Ok(ApiRequest::BatchCreate {
                items,
                agent_id: agent.to_string(),
                tenant_id: None,
            })
        }
        "register" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("unnamed");
            Ok(ApiRequest::RegisterAgent {
                name: name.to_string(),
            })
        }
        "checkpoint" => {
            Ok(ApiRequest::AgentCheckpoint {
                agent_id: agent.to_string(),
            })
        }
        "restore" => {
            let checkpoint_cid = params.get("checkpoint_cid").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ApiRequest::AgentRestore {
                agent_id: agent.to_string(),
                checkpoint_cid: checkpoint_cid.to_string(),
            })
        }
        "subscribe" => {
            let event_types = params.get("event_types").and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());
            Ok(ApiRequest::EventSubscribe {
                agent_id: agent.to_string(),
                event_types,
                agent_ids: None,
            })
        }
        "poll" => {
            let subscription_id = params.get("subscription_id").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ApiRequest::EventPoll {
                subscription_id: subscription_id.to_string(),
            })
        }
        "unsubscribe" => {
            let subscription_id = params.get("subscription_id").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ApiRequest::EventUnsubscribe {
                subscription_id: subscription_id.to_string(),
            })
        }
        "storage_stats" => {
            Ok(ApiRequest::CacheStats)
        }
        "object_usage" => {
            let cid = params.get("cid").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ApiRequest::Explore {
                cid: cid.to_string(),
                edge_type: None,
                depth: None,
                agent_id: agent.to_string(),
            })
        }
        "evict_expired" => {
            Ok(ApiRequest::EvictExpired {
                agent_id: agent.to_string(),
                tenant_id: None,
            })
        }
        "discover_knowledge" => {
            let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let scope = params.get("scope").and_then(|v| v.as_str()).unwrap_or("shared");
            let scope_enum = match scope {
                "shared" => plico::api::semantic::DiscoveryScope::Shared,
                "all" => plico::api::semantic::DiscoveryScope::AllAccessible,
                _ => plico::api::semantic::DiscoveryScope::Shared,
            };
            let knowledge_types = params.get("knowledge_types").and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|t| {
                    match t.as_str()? {
                        "memory" => Some(plico::api::semantic::KnowledgeType::Memory),
                        "procedure" => Some(plico::api::semantic::KnowledgeType::Procedure),
                        "knowledge" => Some(plico::api::semantic::KnowledgeType::Knowledge),
                        _ => None,
                    }
                }).collect())
                .unwrap_or_default();
            Ok(ApiRequest::DiscoverKnowledge {
                query: query.to_string(),
                scope: scope_enum,
                knowledge_types,
                max_results: params.get("max_results").and_then(|v| v.as_u64()).unwrap_or(10) as usize,
                token_budget: params.get("token_budget").and_then(|v| v.as_u64()).map(|t| t as usize),
                agent_id: agent.to_string(),
            })
        }
        _ => {
            let available = [
                "add_node", "add_edge", "causal_path", "impact",
                "delegate", "complete", "query_task", "batch_create",
                "register", "checkpoint", "restore",
                "subscribe", "poll", "unsubscribe",
                "storage_stats", "object_usage", "evict_expired", "discover_knowledge",
            ];
            Err(format!(
                "unknown cold method: '{}'. Available methods: {}",
                method,
                available.join(", ")
            ))
        }
    }
}

fn enhance_cold_error(method: &str, error: &str) -> String {
    if !error.contains("missing") && !error.contains("required") && !error.contains("not found") {
        return error.to_string();
    }

    let example = match method {
        "add_node" => "{method:'add_node', label:'MyEntity', node_type:'entity', agent_id:'your-agent'}",
        "add_edge" => "{method:'add_edge', src_id:'<node_a>', dst_id:'<node_b>', edge_type:'causes', agent_id:'your-agent'}",
        "causal_path" => "{method:'causal_path', from_id:'<node_a>', to_id:'<node_b>', agent_id:'your-agent'}",
        "impact" => "{method:'impact', node_id:'<node_id>', depth:3, agent_id:'your-agent'}",
        "delegate" => "{method:'delegate', task_description:'analyze logs', to_agent:'<agent>', agent_id:'your-agent'}",
        "complete" => "{method:'complete', task_id:'<task_id>', agent_id:'your-agent'}",
        "query_task" => "{method:'query_task', task_id:'<task_id>'}",
        "batch_create" => "{method:'batch_create', items:[{content:'text', tags:['tag']}], agent_id:'your-agent'}",
        "register" => "{method:'register', name:'my-agent'}",
        "checkpoint" => "{method:'checkpoint', agent_id:'your-agent'}",
        "restore" => "{method:'restore', agent_id:'your-agent', checkpoint_cid:'<cid>'}",
        "subscribe" => "{method:'subscribe', event_types:['memory_stored','agent_registered'], agent_id:'your-agent'}",
        "poll" => "{method:'poll', subscription_id:'<id>', agent_id:'your-agent'}",
        "unsubscribe" => "{method:'unsubscribe', subscription_id:'<id>'}",
        "storage_stats" => "{method:'storage_stats', agent_id:'your-agent'}",
        "object_usage" => "{method:'object_usage', cid:'<cid>', agent_id:'your-agent'}",
        "evict_expired" => "{method:'evict_expired', agent_id:'your-agent'}",
        "discover_knowledge" => "{method:'discover_knowledge', query:'search terms', scope:'shared', knowledge_types:['memory','procedure']}",
        _ => return error.to_string(),
    };

    format!("{}. Example: {}", error, example)
}
