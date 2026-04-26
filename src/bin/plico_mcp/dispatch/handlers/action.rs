//! Plico action handler — dispatches individual plico actions to kernel API requests.

use plico::api::semantic::{ApiRequest, ContextAssembleCandidate, DiscoveryScope, KnowledgeType};
use plico::kernel::AIKernel;
use serde_json::Value;

use crate::dispatch::{DEFAULT_AGENT, PLICO_ACTIONS, check_read_only, generate_help_response};
use crate::format::format_plico_response;

pub(in crate::dispatch) fn dispatch_plico_action(action: &str, args: &Value, kernel: &AIKernel) -> Result<String, String> {
    check_read_only(action, PLICO_ACTIONS)?;

    let agent = args.get("agent_id").and_then(|a| a.as_str()).unwrap_or(DEFAULT_AGENT);

    match action {
        "help" => Ok(generate_help_response()),

        "plico" => {
            super::cold::dispatch_cold_layer(args, kernel)
        }

        "put" => {
            let content = args.get("content")
                .and_then(|c| c.as_str())
                .ok_or("put requires content")?;
            let tags: Vec<String> = args.get("tags")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let req = ApiRequest::Create {
                api_version: None,
                content: content.to_string(),
                content_encoding: Default::default(),
                tags,
                agent_id: agent.to_string(),
                tenant_id: None,
                agent_token: None,
                intent: None,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "get" => {
            let cid = args.get("cid")
                .and_then(|c| c.as_str())
                .ok_or("get requires cid")?;
            let req = ApiRequest::Read {
                cid: cid.to_string(),
                agent_id: agent.to_string(),
                tenant_id: None,
                agent_token: None,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "session_start" => {
            let handover_mode = args.get("handover_mode").and_then(|v| v.as_str());

            let req = ApiRequest::StartSession {
                agent_id: agent.to_string(),
                agent_token: args.get("agent_token").and_then(|v| v.as_str()).map(String::from),
                intent_hint: args.get("intent_hint").and_then(|v| v.as_str()).map(String::from),
                load_tiers: vec![],
                last_seen_seq: args.get("last_seen_seq").and_then(|v| v.as_u64()),
            };
            let resp = kernel.handle_api_request(req);
            let mut json_str = format_plico_response(resp, args)?;

            if let Some(mode) = handover_mode {
                if mode != "none" {
                    let handover = assemble_handover(kernel, mode);
                    if let Ok(parsed) = serde_json::from_str::<Value>(&json_str) {
                        let mut ordered = serde_json::Map::new();
                        ordered.insert("handover".to_string(), handover);
                        if let Some(ss) = parsed.get("session_started") {
                            ordered.insert("session_started".to_string(), ss.clone());
                        }
                        for (k, v) in parsed.as_object().into_iter().flatten() {
                            if k != "session_started" && k != "ok" {
                                ordered.entry(k.clone()).or_insert_with(|| v.clone());
                            }
                        }
                        ordered.insert("ok".to_string(), Value::Bool(true));
                        json_str = serde_json::to_string_pretty(&Value::Object(ordered))
                            .unwrap_or(json_str);
                    }
                }
            }

            Ok(json_str)
        }

        "session_end" => {
            let session_id = args.get("session_id")
                .and_then(|s| s.as_str())
                .ok_or("session_end requires session_id")?;
            let req = ApiRequest::EndSession {
                agent_id: agent.to_string(),
                session_id: session_id.to_string(),
                auto_checkpoint: true,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "remember" => {
            let content = args.get("content")
                .and_then(|c| c.as_str())
                .ok_or("remember requires content")?;

            let req = ApiRequest::Remember {
                agent_id: agent.to_string(),
                content: content.to_string(),
                tenant_id: None,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "because" => {
            let cause_cid = args.get("cause_cid")
                .and_then(|c| c.as_str())
                .ok_or("because requires cause_cid")?;
            let effect_cid = args.get("effect_cid")
                .and_then(|c| c.as_str())
                .ok_or("because requires effect_cid")?;
            let reason = args.get("reason").and_then(|r| r.as_str()).unwrap_or("");

            let kg = kernel.knowledge_graph()
                .ok_or("knowledge graph not available")?;
            kg.upsert_document(cause_cid, &[], agent)
                .map_err(|e| format!("failed to upsert cause node: {e}"))?;
            kg.upsert_document(effect_cid, &[], agent)
                .map_err(|e| format!("failed to upsert effect node: {e}"))?;

            kernel.kg_add_edge(
                cause_cid, effect_cid,
                plico::fs::graph::types::KGEdgeType::Causes,
                Some(0.9),
                agent,
                "default",
            ).map_err(|e| format!("failed to add causal edge: {e}"))?;

            let mut resp = serde_json::json!({
                "ok": true,
                "edge": { "src": cause_cid, "dst": effect_cid, "type": "Causes" }
            });
            if !reason.is_empty() {
                resp["edge"]["reason"] = serde_json::Value::String(reason.to_string());
            }
            Ok(serde_json::to_string_pretty(&resp).unwrap_or_default())
        }

        "recall" => {
            let scope = args.get("scope").and_then(|s| s.as_str()).map(String::from);
            let query = args.get("query").and_then(|q| q.as_str()).map(String::from);
            let limit = args.get("limit").and_then(|l| l.as_u64()).map(|l| l as usize);
            let tier = args.get("tier").and_then(|t| t.as_str()).map(String::from);
            let req = ApiRequest::Recall {
                agent_id: agent.to_string(),
                scope,
                query,
                limit,
                tier,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "recall_semantic" => {
            let query = args.get("query")
                .and_then(|q| q.as_str())
                .ok_or("recall_semantic requires query")?;
            let k = args.get("k").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

            let req = ApiRequest::RecallSemantic {
                agent_id: agent.to_string(),
                query: query.to_string(),
                k,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "search" => {
            let query = args.get("query")
                .and_then(|q| q.as_str())
                .ok_or("search requires query")?;
            let limit = args.get("limit").and_then(|l| l.as_u64()).map(|l| l as usize);
            let require_tags: Vec<String> = args.get("require_tags")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let exclude_tags: Vec<String> = args.get("exclude_tags")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let req = ApiRequest::Search {
                query: query.to_string(),
                agent_id: agent.to_string(),
                tenant_id: None,
                agent_token: None,
                limit,
                offset: None,
                require_tags,
                exclude_tags,
                since: None,
                until: None,
                intent_context: None,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "hybrid" => {
            let query = args.get("query")
                .and_then(|q| q.as_str())
                .ok_or("hybrid requires query")?;
            let max_results = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(20) as usize;
            let token_budget = args.get("token_budget").and_then(|t| t.as_u64()).map(|t| t as usize);
            let seed_tags: Vec<String> = args.get("tags")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let req = ApiRequest::HybridRetrieve {
                query_text: query.to_string(),
                agent_id: agent.to_string(),
                tenant_id: None,
                seed_tags,
                graph_depth: 2,
                edge_types: vec![],
                max_results,
                token_budget,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "intent_declare" => {
            let content = args.get("content")
                .and_then(|c| c.as_str())
                .ok_or("intent_declare requires content")?;
            let priority = args.get("priority").and_then(|p| p.as_str()).unwrap_or("normal");

            let req = ApiRequest::SubmitIntent {
                description: content.to_string(),
                priority: priority.to_string(),
                action: None,
                agent_id: agent.to_string(),
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "intent_fetch" => {
            let cids_json = args.get("cids");
            let cids: Vec<ContextAssembleCandidate> = cids_json
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter().filter_map(|v| {
                        Some(ContextAssembleCandidate {
                            cid: v.get("cid")?.as_str()?.to_string(),
                            relevance: v.get("relevance").and_then(|r| r.as_f64()).unwrap_or(1.0) as f32,
                        })
                    }).collect()
                })
                .unwrap_or_default();
            let budget_tokens = args.get("token_budget").and_then(|t| t.as_u64()).unwrap_or(4096) as usize;

            let req = ApiRequest::ContextAssemble {
                agent_id: agent.to_string(),
                cids,
                budget_tokens,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "delta" => {
            let since_seq = args.get("since_seq")
                .and_then(|s| s.as_u64())
                .ok_or("delta requires since_seq")?;
            let limit = args.get("limit").and_then(|l| l.as_u64()).map(|l| l as usize);
            let watch_cids: Vec<String> = args.get("watch_cids")
                .and_then(|c| c.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let watch_tags: Vec<String> = args.get("watch_tags")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let req = ApiRequest::DeltaSince {
                agent_id: agent.to_string(),
                since_seq,
                watch_cids,
                watch_tags,
                limit,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "growth" => {
            let req = ApiRequest::AgentUsage {
                agent_id: agent.to_string(),
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "status" => {
            let req = ApiRequest::SystemStatus;
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "discover" => {
            let scope = args.get("scope").and_then(|s| s.as_str()).unwrap_or("shared");
            let knowledge_types = args.get("knowledge_types").and_then(|kt| kt.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                .unwrap_or_default();
            let req = ApiRequest::DiscoverKnowledge {
                query: args.get("query").and_then(|q| q.as_str()).unwrap_or("").to_string(),
                scope: match scope {
                    "shared" => DiscoveryScope::Shared,
                    "all" => DiscoveryScope::AllAccessible,
                    _ => DiscoveryScope::Shared,
                },
                knowledge_types: knowledge_types.iter().map(|kt| match *kt {
                    "memory" => KnowledgeType::Memory,
                    "procedure" => KnowledgeType::Procedure,
                    "knowledge" => KnowledgeType::Knowledge,
                    _ => KnowledgeType::Memory,
                }).collect(),
                max_results: args.get("max_results").and_then(|v| v.as_u64()).unwrap_or(10) as usize,
                token_budget: args.get("token_budget").and_then(|v| v.as_u64()).map(|t| t as usize),
                agent_id: agent.to_string(),
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "memory_stats" => {
            let tier = args.get("tier").and_then(|t| t.as_str()).map(String::from);
            let req = ApiRequest::MemoryStats {
                agent_id: agent.to_string(),
                tier,
                tenant_id: None,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        _ => Err(format!("unknown action: {action}")),
    }
}

pub(in crate::dispatch) fn dispatch_plico_action_remote(action: &str, args: &Value, client: &dyn plico::client::KernelClient) -> Result<String, String> {
    check_read_only(action, PLICO_ACTIONS)?;
    let agent = args.get("agent_id").and_then(|a| a.as_str()).unwrap_or(DEFAULT_AGENT);

    let req = match action {
        "help" => return Ok(generate_help_response()),
        "put" => {
            let content = args.get("content").and_then(|c| c.as_str()).ok_or("put requires content")?;
            let tags: Vec<String> = args.get("tags")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            ApiRequest::Create { api_version: None, content: content.to_string(), content_encoding: Default::default(), tags, agent_id: agent.to_string(), tenant_id: None, agent_token: None, intent: None }
        }
        "get" => {
            let cid = args.get("cid").and_then(|c| c.as_str()).ok_or("get requires cid")?;
            ApiRequest::Read { cid: cid.to_string(), agent_id: agent.to_string(), tenant_id: None, agent_token: None }
        }
        "search" => {
            let query = args.get("query").and_then(|q| q.as_str()).ok_or("search requires query")?;
            let limit = args.get("limit").and_then(|l| l.as_u64()).map(|l| l as usize);
            ApiRequest::Search { query: query.to_string(), agent_id: agent.to_string(), tenant_id: None, agent_token: None, limit, offset: None, require_tags: vec![], exclude_tags: vec![], since: None, until: None, intent_context: None }
        }
        "remember" => {
            let content = args.get("content").and_then(|c| c.as_str()).ok_or("remember requires content")?;
            ApiRequest::Remember { agent_id: agent.to_string(), content: content.to_string(), tenant_id: None }
        }
        "recall" => {
            let scope = args.get("scope").and_then(|s| s.as_str()).map(String::from);
            let query = args.get("query").and_then(|q| q.as_str()).map(String::from);
            let limit = args.get("limit").and_then(|l| l.as_u64()).map(|l| l as usize);
            let tier = args.get("tier").and_then(|t| t.as_str()).map(String::from);
            ApiRequest::Recall { agent_id: agent.to_string(), scope, query, limit, tier }
        }
        "status" => ApiRequest::SystemStatus,
        "session_start" => {
            ApiRequest::StartSession { agent_id: agent.to_string(), agent_token: None, intent_hint: args.get("intent_hint").and_then(|v| v.as_str()).map(String::from), load_tiers: vec![], last_seen_seq: args.get("last_seen_seq").and_then(|v| v.as_u64()) }
        }
        "session_end" => {
            let session_id = args.get("session_id").and_then(|s| s.as_str()).ok_or("session_end requires session_id")?;
            ApiRequest::EndSession { agent_id: agent.to_string(), session_id: session_id.to_string(), auto_checkpoint: true }
        }
        "delta" => {
            let since_seq = args.get("since_seq").and_then(|s| s.as_u64()).ok_or("delta requires since_seq")?;
            let limit = args.get("limit").and_then(|l| l.as_u64()).map(|l| l as usize);
            ApiRequest::DeltaSince { agent_id: agent.to_string(), since_seq, watch_cids: vec![], watch_tags: vec![], limit }
        }
        "hybrid" => {
            let query = args.get("query").and_then(|q| q.as_str()).ok_or("hybrid requires query")?;
            ApiRequest::HybridRetrieve { query_text: query.to_string(), agent_id: agent.to_string(), tenant_id: None, seed_tags: vec![], graph_depth: 2, edge_types: vec![], max_results: 20, token_budget: None }
        }
        _ => return Err(format!("action '{}' not available in daemon mode", action)),
    };

    let resp = client.request(req);
    format_plico_response(resp, args)
}

fn assemble_handover(kernel: &AIKernel, mode: &str) -> Value {
    let tags = kernel.list_tags();
    let status = kernel.system_status();

    let max_results = match mode { "compact" => 3, _ => 10 };
    let tag_limit = match mode { "compact" => 10, _ => 20 };

    let resp = kernel.handle_api_request(ApiRequest::Search {
        query: String::new(),
        agent_id: "system".to_string(),
        tenant_id: None,
        agent_token: None,
        limit: Some(max_results),
        offset: None,
        require_tags: vec![],
        exclude_tags: vec![],
        since: None,
        until: None,
        intent_context: None,
    });

    let recent: Vec<Value> = resp.results.unwrap_or_default().into_iter().map(|h| {
        serde_json::json!({ "cid": h.cid, "tags": h.tags, "relevance": h.relevance })
    }).collect();

    let active_tags: Vec<&String> = tags.iter().take(tag_limit).collect();

    let kg_causal: Vec<Value> = if mode != "compact" {
        kernel.knowledge_graph()
            .and_then(|kg| kg.get_valid_edges_at(u64::MAX).ok())
            .map(|edges| {
                edges.into_iter()
                    .filter(|e| !matches!(e.edge_type, plico::fs::graph::types::KGEdgeType::AssociatesWith))
                    .take(5)
                    .map(|e| serde_json::json!({
                        "src": e.src, "dst": e.dst,
                        "type": format!("{:?}", e.edge_type),
                        "weight": e.weight,
                    }))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        vec![]
    };

    serde_json::json!({
        "mode": mode,
        "recent_objects": recent,
        "active_tags": active_tags,
        "kg_causal_edges": kg_causal,
        "summary": {
            "total_objects": status.cas_object_count,
            "total_tags": tags.len(),
            "agents": status.agent_count,
        }
    })
}
