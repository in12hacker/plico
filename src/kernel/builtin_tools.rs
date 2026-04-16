//! Built-in Tool Registration and Execution
//!
//! Registers all kernel capabilities as discoverable tools ("Everything is a Tool")
//! and dispatches tool calls to the appropriate kernel methods.

use crate::memory::MemoryEntry;
use crate::fs::{KGNodeType, KGEdgeType};
use crate::tool::{ToolDescriptor, ToolResult};

use super::AIKernel;

impl AIKernel {
    /// Register all built-in kernel capabilities as discoverable tools.
    pub(crate) fn register_builtin_tools(&self) {
        use serde_json::json;
        let reg = &self.tool_registry;

        reg.register(ToolDescriptor {
            name: "cas.create".into(),
            description: "Create a CAS object with content and semantic tags".into(),
            schema: json!({"type":"object","properties":{"content":{"type":"string"},"tags":{"type":"array","items":{"type":"string"}},"intent":{"type":"string"}},"required":["content","tags"]}),
        });
        reg.register(ToolDescriptor {
            name: "cas.read".into(),
            description: "Read a CAS object by its content ID".into(),
            schema: json!({"type":"object","properties":{"cid":{"type":"string"}},"required":["cid"]}),
        });
        reg.register(ToolDescriptor {
            name: "cas.search".into(),
            description: "Semantic search across stored objects".into(),
            schema: json!({"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"},"require_tags":{"type":"array","items":{"type":"string"}},"exclude_tags":{"type":"array","items":{"type":"string"}},"since":{"type":"integer"},"until":{"type":"integer"}},"required":["query"]}),
        });
        reg.register(ToolDescriptor {
            name: "cas.update".into(),
            description: "Update an existing CAS object".into(),
            schema: json!({"type":"object","properties":{"cid":{"type":"string"},"content":{"type":"string"},"new_tags":{"type":"array","items":{"type":"string"}}},"required":["cid","content"]}),
        });
        reg.register(ToolDescriptor {
            name: "cas.delete".into(),
            description: "Soft-delete a CAS object (moves to recycle bin)".into(),
            schema: json!({"type":"object","properties":{"cid":{"type":"string"}},"required":["cid"]}),
        });
        reg.register(ToolDescriptor {
            name: "memory.store".into(),
            description: "Store a memory entry for an agent".into(),
            schema: json!({"type":"object","properties":{"content":{"type":"string"},"tier":{"type":"string","enum":["ephemeral","working"]},"tags":{"type":"array","items":{"type":"string"}},"importance":{"type":"number"},"ttl_ms":{"type":"integer"}},"required":["content"]}),
        });
        reg.register(ToolDescriptor {
            name: "memory.recall".into(),
            description: "Retrieve all memories for an agent".into(),
            schema: json!({"type":"object","properties":{"tier":{"type":"string"},"limit":{"type":"integer"}}}),
        });
        reg.register(ToolDescriptor {
            name: "memory.forget".into(),
            description: "Evict ephemeral memories for an agent".into(),
            schema: json!({"type":"object","properties":{}}),
        });
        reg.register(ToolDescriptor {
            name: "kg.add_node".into(),
            description: "Create a knowledge graph node".into(),
            schema: json!({"type":"object","properties":{"label":{"type":"string"},"type":{"type":"string","enum":["entity","fact","document","agent","memory"]},"properties":{"type":"object"}},"required":["label"]}),
        });
        reg.register(ToolDescriptor {
            name: "kg.add_edge".into(),
            description: "Create a knowledge graph edge between two nodes".into(),
            schema: json!({"type":"object","properties":{"src":{"type":"string"},"dst":{"type":"string"},"type":{"type":"string"},"weight":{"type":"number"}},"required":["src","dst"]}),
        });
        reg.register(ToolDescriptor {
            name: "kg.explore".into(),
            description: "Explore knowledge graph neighbors of a node".into(),
            schema: json!({"type":"object","properties":{"cid":{"type":"string"},"edge_type":{"type":"string"},"depth":{"type":"integer"}},"required":["cid"]}),
        });
        reg.register(ToolDescriptor {
            name: "kg.paths".into(),
            description: "Find paths between two knowledge graph nodes".into(),
            schema: json!({"type":"object","properties":{"src":{"type":"string"},"dst":{"type":"string"},"depth":{"type":"integer"}},"required":["src","dst"]}),
        });
        reg.register(ToolDescriptor {
            name: "agent.register".into(),
            description: "Register a new AI agent".into(),
            schema: json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}),
        });
        reg.register(ToolDescriptor {
            name: "agent.status".into(),
            description: "Query the state of an agent".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"}},"required":["agent_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "agent.suspend".into(),
            description: "Suspend a running agent".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"}},"required":["agent_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "agent.resume".into(),
            description: "Resume a suspended agent".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"}},"required":["agent_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "agent.terminate".into(),
            description: "Permanently terminate an agent".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"}},"required":["agent_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "tools.list".into(),
            description: "List all available tools with their schemas".into(),
            schema: json!({"type":"object","properties":{}}),
        });
        reg.register(ToolDescriptor {
            name: "tools.describe".into(),
            description: "Get the schema and description of a specific tool".into(),
            schema: json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}),
        });
        reg.register(ToolDescriptor {
            name: "intent.resolve".into(),
            description: "Resolve natural language text into structured API actions".into(),
            schema: json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}),
        });
        reg.register(ToolDescriptor {
            name: "agent.set_resources".into(),
            description: "Set an agent's resource limits (memory quota, CPU time, tool allowlist)".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"},"memory_quota":{"type":"integer"},"cpu_time_quota":{"type":"integer"},"allowed_tools":{"type":"array","items":{"type":"string"}}},"required":["agent_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "message.send".into(),
            description: "Send a message to another agent".into(),
            schema: json!({"type":"object","properties":{"to":{"type":"string"},"payload":{"type":"object"}},"required":["to","payload"]}),
        });
        reg.register(ToolDescriptor {
            name: "message.read".into(),
            description: "Read messages for an agent".into(),
            schema: json!({"type":"object","properties":{"unread_only":{"type":"boolean"}}}),
        });
        reg.register(ToolDescriptor {
            name: "message.ack".into(),
            description: "Acknowledge a message (mark as read)".into(),
            schema: json!({"type":"object","properties":{"message_id":{"type":"string"}},"required":["message_id"]}),
        });
    }

    /// Execute a tool by name with JSON parameters.
    ///
    /// Enforces agent tool allowlist: if the agent has a non-empty `allowed_tools`
    /// list, only those tools can be called. Returns error for blocked/unknown tools.
    pub fn execute_tool(&self, name: &str, params: &serde_json::Value, agent_id: &str) -> ToolResult {
        use serde_json::json;

        let aid = crate::scheduler::AgentId(agent_id.to_string());
        if let Some(resources) = self.scheduler.get_resources(&aid) {
            if !resources.allowed_tools.is_empty()
                && !resources.allowed_tools.iter().any(|t| t == name)
            {
                return ToolResult::error(format!(
                    "Tool '{}' not in agent's allowed list: {:?}",
                    name, resources.allowed_tools
                ));
            }
        }

        match name {
            "cas.create" => {
                let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let tags: Vec<String> = params.get("tags")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let intent = params.get("intent").and_then(|v| v.as_str()).map(String::from);
                match self.semantic_create(content.as_bytes().to_vec(), tags, agent_id, intent) {
                    Ok(cid) => ToolResult::ok(json!({"cid": cid})),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "cas.read" => {
                let cid = params.get("cid").and_then(|v| v.as_str()).unwrap_or("");
                match self.get_object(cid, agent_id) {
                    Ok(obj) => ToolResult::ok(json!({
                        "cid": obj.cid,
                        "data": String::from_utf8_lossy(&obj.data),
                        "tags": obj.meta.tags,
                        "content_type": obj.meta.content_type,
                    })),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "cas.search" => {
                let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                let require_tags: Vec<String> = params.get("require_tags")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let exclude_tags: Vec<String> = params.get("exclude_tags")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let since = params.get("since").and_then(|v| v.as_i64());
                let until = params.get("until").and_then(|v| v.as_i64());
                let results = self.semantic_search_with_time(
                    query, agent_id, limit, require_tags, exclude_tags, since, until,
                );
                let dto: Vec<serde_json::Value> = results.into_iter().map(|r| json!({
                    "cid": r.cid, "relevance": r.relevance, "tags": r.meta.tags,
                })).collect();
                ToolResult::ok(json!({"results": dto}))
            }
            "cas.update" => {
                let cid = params.get("cid").and_then(|v| v.as_str()).unwrap_or("");
                let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let new_tags: Option<Vec<String>> = params.get("new_tags")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());
                match self.semantic_update(cid, content.as_bytes().to_vec(), new_tags, agent_id) {
                    Ok(new_cid) => ToolResult::ok(json!({"cid": new_cid})),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "cas.delete" => {
                let cid = params.get("cid").and_then(|v| v.as_str()).unwrap_or("");
                match self.semantic_delete(cid, agent_id) {
                    Ok(()) => ToolResult::ok(json!({"deleted": cid})),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "memory.store" => {
                let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let tier = params.get("tier").and_then(|v| v.as_str()).unwrap_or("ephemeral");
                let tags: Vec<String> = params.get("tags")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let importance = params.get("importance").and_then(|v| v.as_u64()).unwrap_or(50) as u8;
                let ttl_ms = params.get("ttl_ms").and_then(|v| v.as_u64());

                let quota = self.scheduler.get_resources(&aid)
                    .map(|r| r.memory_quota)
                    .unwrap_or(0);

                match tier {
                    "working" => {
                        let mut entry = MemoryEntry::long_term(
                            agent_id,
                            crate::memory::MemoryContent::Text(content),
                            tags,
                        );
                        entry.tier = crate::memory::MemoryTier::Working;
                        entry.importance = importance;
                        entry.ttl_ms = ttl_ms;
                        let id = entry.id.clone();
                        match self.memory.store_checked(entry, quota) {
                            Ok(()) => ToolResult::ok(json!({"id": id, "tier": "working"})),
                            Err(e) => ToolResult::error(e.to_string()),
                        }
                    }
                    _ => {
                        let mut entry = MemoryEntry::ephemeral(agent_id, content);
                        entry.importance = importance;
                        entry.ttl_ms = ttl_ms;
                        if !tags.is_empty() {
                            entry.tags = tags;
                        }
                        let id = entry.id.clone();
                        match self.memory.store_checked(entry, quota) {
                            Ok(()) => ToolResult::ok(json!({"id": id, "tier": "ephemeral"})),
                            Err(e) => ToolResult::error(e.to_string()),
                        }
                    }
                }
            }
            "memory.recall" => {
                let memories = self.recall(agent_id);
                let dto: Vec<serde_json::Value> = memories.into_iter().map(|m| json!({
                    "id": m.id,
                    "tier": m.tier.name(),
                    "content": m.content.display(),
                    "importance": m.importance,
                    "access_count": m.access_count,
                    "tags": m.tags,
                })).collect();
                ToolResult::ok(json!({"memories": dto}))
            }
            "memory.forget" => {
                self.forget_ephemeral(agent_id);
                ToolResult::ok(json!({"forgotten": true}))
            }
            "kg.add_node" => {
                let label = params.get("label").and_then(|v| v.as_str()).unwrap_or("");
                let type_str = params.get("type").and_then(|v| v.as_str()).unwrap_or("entity");
                let node_type = match type_str {
                    "fact" => KGNodeType::Fact,
                    "document" => KGNodeType::Document,
                    "agent" => KGNodeType::Agent,
                    "memory" => KGNodeType::Memory,
                    _ => KGNodeType::Entity,
                };
                let props = params.get("properties").cloned().unwrap_or(serde_json::Value::Null);
                match self.kg_add_node(label, node_type, props, agent_id) {
                    Ok(id) => ToolResult::ok(json!({"node_id": id})),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "kg.add_edge" => {
                let src = params.get("src").and_then(|v| v.as_str()).unwrap_or("");
                let dst = params.get("dst").and_then(|v| v.as_str()).unwrap_or("");
                let type_str = params.get("type").and_then(|v| v.as_str()).unwrap_or("related_to");
                let edge_type = match type_str {
                    "associates_with" => KGEdgeType::AssociatesWith,
                    "mentions" => KGEdgeType::Mentions,
                    "follows" => KGEdgeType::Follows,
                    "causes" => KGEdgeType::Causes,
                    "part_of" => KGEdgeType::PartOf,
                    "similar_to" => KGEdgeType::SimilarTo,
                    _ => KGEdgeType::RelatedTo,
                };
                let weight = params.get("weight").and_then(|v| v.as_f64()).map(|w| w as f32);
                match self.kg_add_edge(src, dst, edge_type, weight, agent_id) {
                    Ok(()) => ToolResult::ok(json!({"created": true})),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "kg.explore" => {
                let cid = params.get("cid").and_then(|v| v.as_str()).unwrap_or("");
                let edge_type = params.get("edge_type").and_then(|v| v.as_str());
                let depth = params.get("depth").and_then(|v| v.as_u64()).unwrap_or(1) as u8;
                let raw = self.graph_explore_raw(cid, edge_type, depth.min(3));
                let neighbors: Vec<serde_json::Value> = raw.into_iter()
                    .map(|(id, label, ntype, etype, auth)| json!({
                        "node_id": id, "label": label, "node_type": ntype,
                        "edge_type": etype, "authority_score": auth,
                    }))
                    .collect();
                ToolResult::ok(json!({"neighbors": neighbors}))
            }
            "kg.paths" => {
                let src = params.get("src").and_then(|v| v.as_str()).unwrap_or("");
                let dst = params.get("dst").and_then(|v| v.as_str()).unwrap_or("");
                let depth = params.get("depth").and_then(|v| v.as_u64()).unwrap_or(3) as u8;
                let paths = self.kg_find_paths(src, dst, depth.min(5));
                let dto: Vec<Vec<serde_json::Value>> = paths.into_iter()
                    .map(|p| p.into_iter().map(|n| json!({"id": n.id, "label": n.label})).collect())
                    .collect();
                ToolResult::ok(json!({"paths": dto}))
            }
            "agent.register" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("unnamed");
                let id = self.register_agent(name.to_string());
                ToolResult::ok(json!({"agent_id": id}))
            }
            "agent.status" => {
                let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
                match self.agent_status(target) {
                    Some((_id, state, pending)) => ToolResult::ok(json!({
                        "agent_id": target, "state": state, "pending_intents": pending,
                    })),
                    None => ToolResult::error(format!("agent not found: {}", target)),
                }
            }
            "agent.suspend" => {
                let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
                match self.agent_suspend(target) {
                    Ok(()) => ToolResult::ok(json!({"suspended": target})),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "agent.resume" => {
                let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
                match self.agent_resume(target) {
                    Ok(()) => ToolResult::ok(json!({"resumed": target})),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "agent.terminate" => {
                let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
                match self.agent_terminate(target) {
                    Ok(()) => ToolResult::ok(json!({"terminated": target})),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "tools.list" => {
                let tools = self.tool_registry.list();
                ToolResult::ok(serde_json::to_value(&tools).unwrap_or_default())
            }
            "tools.describe" => {
                let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                match self.tool_registry.get(tool_name) {
                    Some(desc) => ToolResult::ok(serde_json::to_value(&desc).unwrap_or_default()),
                    None => ToolResult::error(format!("tool not found: {}", tool_name)),
                }
            }
            "intent.resolve" => {
                let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let results = self.intent_resolve(text, agent_id);
                ToolResult::ok(serde_json::to_value(&results).unwrap_or_default())
            }
            "agent.set_resources" => {
                let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
                let mq = params.get("memory_quota").and_then(|v| v.as_u64());
                let cq = params.get("cpu_time_quota").and_then(|v| v.as_u64());
                let at: Option<Vec<String>> = params.get("allowed_tools")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());
                match self.agent_set_resources(target, mq, cq, at) {
                    Ok(()) => ToolResult::ok(json!({"updated": target})),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "message.send" => {
                let to = params.get("to").and_then(|v| v.as_str()).unwrap_or("");
                let payload = params.get("payload").cloned().unwrap_or(serde_json::Value::Null);
                match self.send_message(agent_id, to, payload) {
                    Ok(id) => ToolResult::ok(json!({"message_id": id})),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "message.read" => {
                let unread = params.get("unread_only").and_then(|v| v.as_bool()).unwrap_or(false);
                let msgs = self.read_messages(agent_id, unread);
                ToolResult::ok(serde_json::to_value(&msgs).unwrap_or_default())
            }
            "message.ack" => {
                let msg_id = params.get("message_id").and_then(|v| v.as_str()).unwrap_or("");
                if self.ack_message(agent_id, msg_id) {
                    ToolResult::ok(json!({"acked": msg_id}))
                } else {
                    ToolResult::error(format!("message not found: {}", msg_id))
                }
            }
            _ => ToolResult::error(format!("unknown tool: {}", name)),
        }
    }

    /// Number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tool_registry.count()
    }
}
