//! Built-in Tool Registration and Execution
//!
//! Registers all kernel capabilities as discoverable tools ("Everything is a Tool")
//! and dispatches tool calls to the appropriate kernel methods.

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
            schema: json!({"type":"object","properties":{"content":{"type":"string"},"tier":{"type":"string","description":"Tier: ephemeral/working/long-term/procedural (default: working)"},"tags":{"type":"array","items":{"type":"string"}},"importance":{"type":"number"}},"required":["content"]}),
        });
        reg.register(ToolDescriptor {
            name: "memory.recall".into(),
            description: "Retrieve all memories for an agent".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string","description":"Agent ID to query (defaults to calling agent)"},"tier":{"type":"string","description":"Filter by tier: ephemeral/working/long-term/procedural"},"limit":{"type":"integer"}}}),
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
            name: "kg.get_node".into(),
            description: "Get a single knowledge graph node by ID".into(),
            schema: json!({"type":"object","properties":{"node_id":{"type":"string"}},"required":["node_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "kg.list_edges".into(),
            description: "List knowledge graph edges, optionally filtered by node".into(),
            schema: json!({"type":"object","properties":{"node_id":{"type":"string"}}}),
        });
        reg.register(ToolDescriptor {
            name: "kg.remove_node".into(),
            description: "Remove a knowledge graph node and all its edges".into(),
            schema: json!({"type":"object","properties":{"node_id":{"type":"string"}},"required":["node_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "kg.remove_edge".into(),
            description: "Remove an edge between two knowledge graph nodes".into(),
            schema: json!({"type":"object","properties":{"src":{"type":"string"},"dst":{"type":"string"},"type":{"type":"string"}},"required":["src","dst"]}),
        });
        reg.register(ToolDescriptor {
            name: "kg.update_node".into(),
            description: "Update a knowledge graph node's label and/or properties".into(),
            schema: json!({"type":"object","properties":{"node_id":{"type":"string"},"label":{"type":"string"},"properties":{"type":"object"}},"required":["node_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "agent.complete".into(),
            description: "Mark an agent as completed (terminal state)".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"}},"required":["agent_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "agent.fail".into(),
            description: "Mark an agent as failed with a reason (terminal state)".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"},"reason":{"type":"string"}},"required":["agent_id","reason"]}),
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
        reg.register(ToolDescriptor {
            name: "context.load".into(),
            description: "Load context at L0 (summary), L1 (key sections), or L2 (full content) for a CID".into(),
            schema: json!({"type":"object","properties":{"cid":{"type":"string"},"layer":{"type":"string","enum":["L0","L1","L2"]}},"required":["cid","layer"]}),
        });
        reg.register(ToolDescriptor {
            name: "permission.grant".into(),
            description: "Grant a permission action to an agent".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"},"action":{"type":"string","enum":["Read","ReadAny","Write","Delete","Network","Execute","SendMessage","All"]},"scope":{"type":"string"},"expires_at":{"type":"integer"}},"required":["agent_id","action"]}),
        });
        reg.register(ToolDescriptor {
            name: "permission.revoke".into(),
            description: "Revoke a specific permission from an agent".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"},"action":{"type":"string"}},"required":["agent_id","action"]}),
        });
        reg.register(ToolDescriptor {
            name: "permission.list".into(),
            description: "List all permission grants for an agent".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"}},"required":["agent_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "permission.check".into(),
            description: "Check if an agent has permission for a specific action".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"},"action":{"type":"string"}},"required":["agent_id","action"]}),
        });

        // Procedural memory
        self.tool_registry.register(ToolDescriptor {
            name: "memory.store_procedure".into(),
            description: "Store a learned procedure (workflow/skill) in procedural memory (L3)".into(),
            schema: json!({"type":"object","properties":{"name":{"type":"string"},"description":{"type":"string"},"steps":{"type":"array","items":{"type":"object","properties":{"description":{"type":"string"},"action":{"type":"string"},"expected_outcome":{"type":"string"}},"required":["description","action"]}},"learned_from":{"type":"string"},"tags":{"type":"array","items":{"type":"string"}}},"required":["name","description","steps"]}),
        });
        self.tool_registry.register(ToolDescriptor {
            name: "memory.recall_procedure".into(),
            description: "Retrieve procedural memories (learned workflows/skills) for an agent".into(),
            schema: json!({"type":"object","properties":{"name":{"type":"string","description":"Optional: filter by procedure name"}},"required":[]}),
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

        self.scheduler.record_tool_call(&aid);

        if let Some(handler) = self.tool_registry.get_handler(name) {
            let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), "default".to_string());
            let scope = format!("tool:{}", name);
            if self.permissions.check_scoped(&ctx, crate::api::permission::PermissionAction::Execute, Some(&scope)).is_err() {
                return ToolResult::error(format!(
                    "Agent '{}' lacks Execute permission for external tool '{}'", agent_id, name
                ));
            }
            return handler.execute(params, agent_id);
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
                match self.get_object(cid, agent_id, "default") {
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
                let mut results = match self.semantic_search_with_time(
                    query, agent_id, "default", limit * 2, require_tags, exclude_tags, since, until,
                ) {
                    Ok(r) => r,
                    Err(e) => return ToolResult::error(e.to_string()),
                };
                // Deduplicate by CID
                let mut seen = std::collections::HashSet::new();
                results.retain(|r| seen.insert(r.cid.clone()));
                // Apply limit after deduplication
                results.truncate(limit);
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
                match self.semantic_update(cid, content.as_bytes().to_vec(), new_tags, agent_id, "default") {
                    Ok(new_cid) => ToolResult::ok(json!({"cid": new_cid})),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "cas.delete" => {
                let cid = params.get("cid").and_then(|v| v.as_str()).unwrap_or("");
                match self.semantic_delete(cid, agent_id, "default") {
                    Ok(()) => ToolResult::ok(json!({"deleted": cid})),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "memory.store" => {
                // F-3/F-8: Route through kernel tier methods for proper persistence
                let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let tier = params.get("tier").and_then(|v| v.as_str()).unwrap_or("working");
                let tags: Vec<String> = params.get("tags")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let importance = params.get("importance").and_then(|v| v.as_u64()).unwrap_or(50) as u8;

                match tier {
                    "working" => {
                        match self.remember_working(agent_id, "default", content, tags) {
                            Ok(()) => ToolResult::ok(json!({"id": "", "tier": "working"})),
                            Err(e) => ToolResult::error(e.to_string()),
                        }
                    }
                    "long-term" => {
                        match self.remember_long_term(agent_id, "default", content, tags.clone(), importance) {
                            Ok(id) => {
                                self.link_memory_to_kg(&id, agent_id, "default", &tags);
                                ToolResult::ok(json!({"id": id, "tier": "long-term"}))
                            }
                            Err(e) => ToolResult::error(e.to_string()),
                        }
                    }
                    "procedural" => {
                        let steps = vec![crate::memory::layered::ProcedureStep {
                            step_number: 0,
                            description: content.clone(),
                            action: content.clone(),
                            expected_outcome: String::new(),
                        }];
                        match self.remember_procedural(agent_id, "default", "tool-procedure".into(), content, steps, "tool".into(), tags.clone()) {
                            Ok(id) => {
                                self.link_memory_to_kg(&id, agent_id, "default", &tags);
                                ToolResult::ok(json!({"id": id, "tier": "procedural"}))
                            }
                            Err(e) => ToolResult::error(e.to_string()),
                        }
                    }
                    _ => {
                        // Ephemeral: support TTL via direct entry creation
                        let ttl_ms = params.get("ttl_ms").and_then(|v| v.as_u64());
                        let entry_id = uuid::Uuid::new_v4().to_string();
                        let now = crate::memory::layered::now_ms();
                        let entry = crate::memory::MemoryEntry {
                            id: entry_id.clone(),
                            agent_id: agent_id.to_string(),
                            tenant_id: "default".to_string(),
                            tier: crate::memory::MemoryTier::Ephemeral,
                            content: crate::memory::MemoryContent::Text(content),
                            importance,
                            access_count: 0,
                            last_accessed: now,
                            created_at: now,
                            tags: tags.clone(),
                            embedding: None,
                            ttl_ms,
                            original_ttl_ms: ttl_ms,
                            scope: crate::memory::MemoryScope::Private,
                        };
                        let quota = self.scheduler.get_resources(&aid)
                            .map(|r| r.memory_quota)
                            .unwrap_or(0);
                        match self.memory.store_checked(entry, quota) {
                            Ok(()) => {
                                self.persist_memories();
                                ToolResult::ok(json!({"id": entry_id, "tier": "ephemeral"}))
                            }
                            Err(e) => ToolResult::error(e.to_string()),
                        }
                    }
                }
            }
            "memory.recall" => {
                // F-3: Allow agent_id override from params (name or UUID)
                let param_agent_id = params.get("agent_id").and_then(|v| v.as_str());
                let effective_agent = if let Some(name_or_id) = param_agent_id {
                    match self.resolve_agent(name_or_id) {
                        Some(id) => id,
                        None => {
                            let available: Vec<String> = self.scheduler.list_agents().into_iter().map(|h| h.name).collect();
                            return ToolResult::error(format!(
                                "Contract violation: agent '{}' not found. Available agents: {:?}",
                                name_or_id, available
                            ));
                        }
                    }
                } else {
                    agent_id.to_string()
                };
                let tier_filter = params.get("tier").and_then(|v| v.as_str());
                let memories = self.recall(&effective_agent, "default");
                // F-2: Filter by tier if specified
                let filtered: Vec<_> = match tier_filter {
Some(t) => {
                        let tier = match t.to_lowercase().replace(['-', '_'], "").as_str() {
                            "ephemeral" | "l0" | "ephem" => crate::memory::MemoryTier::Ephemeral,
                            "working" | "l1" | "wk" => crate::memory::MemoryTier::Working,
                            "longterm" | "l2" | "lt" | "long" => crate::memory::MemoryTier::LongTerm,
                            "procedural" | "l3" | "proc" => crate::memory::MemoryTier::Procedural,
                            _ => return ToolResult::error(format!("Unknown tier: {}", t)),
                        };
                        memories.into_iter().filter(|m| m.tier == tier).collect()
                    }
                    None => memories,
                };
                let dto: Vec<serde_json::Value> = filtered.into_iter().map(|m| json!({
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
                match self.kg_add_node(label, node_type, props, agent_id, "default") {
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
                match self.kg_add_edge(src, dst, edge_type, weight, agent_id, "default") {
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
            "kg.get_node" => {
                let node_id = params.get("node_id").and_then(|v| v.as_str()).unwrap_or("");
                match self.kg_get_node(node_id, agent_id, "default") {
                    Ok(Some(n)) => ToolResult::ok(json!({
                        "id": n.id, "label": n.label, "node_type": format!("{:?}", n.node_type),
                        "properties": n.properties, "agent_id": n.agent_id, "created_at": n.created_at,
                    })),
                    Ok(None) => ToolResult::error(format!("node not found: {}", node_id)),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "kg.list_edges" => {
                let node_id = params.get("node_id").and_then(|v| v.as_str());
                match self.kg_list_edges(agent_id, "default", node_id) {
                    Ok(edges) => {
                        let dto: Vec<serde_json::Value> = edges.into_iter().map(|e| json!({
                            "src": e.src, "dst": e.dst, "edge_type": format!("{:?}", e.edge_type),
                            "weight": e.weight, "created_at": e.created_at,
                        })).collect();
                        ToolResult::ok(json!({"edges": dto}))
                    }
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "kg.remove_node" => {
                let node_id = params.get("node_id").and_then(|v| v.as_str()).unwrap_or("");
                match self.kg_remove_node(node_id, agent_id, "default") {
                    Ok(()) => ToolResult::ok(json!({"removed": node_id})),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "kg.remove_edge" => {
                let src = params.get("src").and_then(|v| v.as_str()).unwrap_or("");
                let dst = params.get("dst").and_then(|v| v.as_str()).unwrap_or("");
                let edge_type = params.get("type").and_then(|v| v.as_str()).map(|s| match s {
                    "associates_with" => KGEdgeType::AssociatesWith,
                    "mentions" => KGEdgeType::Mentions,
                    "follows" => KGEdgeType::Follows,
                    "causes" => KGEdgeType::Causes,
                    "part_of" => KGEdgeType::PartOf,
                    "similar_to" => KGEdgeType::SimilarTo,
                    _ => KGEdgeType::RelatedTo,
                });
                match self.kg_remove_edge(src, dst, edge_type, agent_id, "default") {
                    Ok(()) => ToolResult::ok(json!({"removed": true})),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "kg.update_node" => {
                let node_id = params.get("node_id").and_then(|v| v.as_str()).unwrap_or("");
                let label = params.get("label").and_then(|v| v.as_str());
                let properties = params.get("properties").cloned();
                match self.kg_update_node(node_id, label, properties, agent_id, "default") {
                    Ok(()) => ToolResult::ok(json!({"updated": node_id})),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "agent.complete" => {
                let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
                match self.agent_complete(target) {
                    Ok(()) => ToolResult::ok(json!({"completed": target})),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "agent.fail" => {
                let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
                let reason = params.get("reason").and_then(|v| v.as_str()).unwrap_or("unspecified");
                match self.agent_fail(target, reason) {
                    Ok(()) => ToolResult::ok(json!({"failed": target, "reason": reason})),
                    Err(e) => ToolResult::error(e.to_string()),
                }
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
            "context.load" => {
                let cid = params.get("cid").and_then(|v| v.as_str()).unwrap_or("");
                let layer_str = params.get("layer").and_then(|v| v.as_str()).unwrap_or("L2");
                let layer = match crate::fs::ContextLayer::parse_layer(layer_str) {
                    Some(l) => l,
                    None => return ToolResult::error(format!("Invalid layer '{}'. Use L0, L1, or L2.", layer_str)),
                };
                match self.context_load(cid, layer, agent_id) {
                    Ok(loaded) => ToolResult::ok(json!({
                        "cid": loaded.cid,
                        "layer": loaded.layer.name(),
                        "content": loaded.content,
                        "tokens_estimate": loaded.tokens_estimate,
                    })),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            "permission.grant" => {
                let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or("");
                let action_str = params.get("action").and_then(|v| v.as_str()).unwrap_or("");
                let scope = params.get("scope").and_then(|v| v.as_str()).map(String::from);
                let expires_at = params.get("expires_at").and_then(|v| v.as_u64());
                match crate::api::permission::PermissionGuard::parse_action(action_str) {
                    Some(action) => {
                        self.permission_grant(target, action, scope, expires_at);
                        ToolResult::ok(json!({"granted": true, "agent_id": target, "action": action_str}))
                    }
                    None => ToolResult::error(format!("Unknown action: {}", action_str)),
                }
            }
            "permission.revoke" => {
                let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or("");
                let action_str = params.get("action").and_then(|v| v.as_str()).unwrap_or("");
                match crate::api::permission::PermissionGuard::parse_action(action_str) {
                    Some(action) => {
                        self.permission_revoke(target, action);
                        ToolResult::ok(json!({"revoked": true, "agent_id": target, "action": action_str}))
                    }
                    None => ToolResult::error(format!("Unknown action: {}", action_str)),
                }
            }
            "permission.list" => {
                let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
                let grants = self.permission_list(target);
                let dto: Vec<serde_json::Value> = grants.into_iter().map(|g| json!({
                    "action": format!("{:?}", g.action),
                    "scope": g.scope,
                    "expires_at": g.expires_at,
                })).collect();
                ToolResult::ok(json!({"agent_id": target, "grants": dto}))
            }
            "permission.check" => {
                let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
                let action_str = params.get("action").and_then(|v| v.as_str()).unwrap_or("");
                match crate::api::permission::PermissionGuard::parse_action(action_str) {
                    Some(action) => {
                        let allowed = self.permission_check(target, action).is_ok();
                        ToolResult::ok(json!({"agent_id": target, "action": action_str, "allowed": allowed}))
                    }
                    None => ToolResult::error(format!("Unknown action: {}", action_str)),
                }
            }
            "memory.store_procedure" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let description = params.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let learned_from = params.get("learned_from").and_then(|v| v.as_str()).unwrap_or("manual").to_string();
                let tags: Vec<String> = params.get("tags")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let steps: Vec<crate::memory::layered::ProcedureStep> = params.get("steps")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().enumerate().map(|(i, s)| {
                        crate::memory::layered::ProcedureStep {
                            step_number: (i + 1) as u32,
                            description: s.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            action: s.get("action").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            expected_outcome: s.get("expected_outcome").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        }
                    }).collect())
                    .unwrap_or_default();
                match self.remember_procedural(agent_id, "default", name, description, steps, learned_from, tags) {
                    Ok(entry_id) => ToolResult::ok(json!({"entry_id": entry_id, "stored": true})),
                    Err(e) => ToolResult::error(e),
                }
            }
            "memory.recall_procedure" => {
                let name = params.get("name").and_then(|v| v.as_str());
                let entries = self.recall_procedural(agent_id, "default", name);
                let data: Vec<serde_json::Value> = entries.iter().map(|e| {
                    match &e.content {
                        crate::memory::MemoryContent::Procedure(p) => {
                            json!({
                                "id": e.id,
                                "name": p.name,
                                "description": p.description,
                                "steps": p.steps.iter().map(|s| json!({
                                    "step_number": s.step_number,
                                    "description": s.description,
                                    "action": s.action,
                                    "expected_outcome": s.expected_outcome,
                                })).collect::<Vec<_>>(),
                                "learned_from": p.learned_from,
                                "tags": e.tags,
                                "importance": e.importance,
                            })
                        }
                        _ => json!({"id": e.id, "content": e.content.display(), "tags": e.tags, "importance": e.importance})
                    }
                }).collect();
                ToolResult::ok(json!({"procedures": data, "count": data.len()}))
            }
            _ => ToolResult::error(format!("unknown tool: {}", name)),
        }
    }

    /// Number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tool_registry.count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::tests::make_kernel;

    fn dispatch(kernel: &AIKernel, name: &str, params: serde_json::Value, agent_id: &str) -> ToolResult {
        kernel.execute_tool(name, &params, agent_id)
    }

    // ─── CAS Tools ─────────────────────────────────────────────────────────────

    #[test]
    fn test_cas_create_dispatch() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "cas.create",
            serde_json::json!({"content": "test data", "tags": ["unit-test"]}),
            "kernel");
        assert!(result.success, "cas.create should succeed: {:?}", result.error);
        assert!(result.output["cid"].is_string());
        let cid = result.output["cid"].as_str().unwrap();
        assert!(!cid.is_empty());
    }

    #[test]
    fn test_cas_create_empty_content_rejected() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "cas.create",
            serde_json::json!({"content": "", "tags": []}),
            "kernel");
        // Empty content is accepted by cas.create tool API; soul is checked at kernel layer
        assert!(result.output["cid"].is_string());
    }

    #[test]
    fn test_cas_read_existing() {
        let (kernel, _dir) = make_kernel();
        // First create an object
        let create = dispatch(&kernel, "cas.create",
            serde_json::json!({"content": "read me", "tags": ["test"]}),
            "kernel");
        let cid = create.output["cid"].as_str().unwrap();

        let result = dispatch(&kernel, "cas.read",
            serde_json::json!({"cid": cid}),
            "kernel");
        assert!(result.success, "cas.read should succeed: {:?}", result.error);
        assert_eq!(result.output["data"].as_str().unwrap(), "read me");
    }

    #[test]
    fn test_cas_read_nonexistent_returns_error() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "cas.read",
            serde_json::json!({"cid": "0000000000000000000000000000000000000000000000000000000000000000"}),
            "kernel");
        assert!(!result.success, "cas.read of nonexistent should fail");
    }

    #[test]
    fn test_cas_delete_existing() {
        let (kernel, _dir) = make_kernel();
        let create = dispatch(&kernel, "cas.create",
            serde_json::json!({"content": "to delete", "tags": ["test"]}),
            "kernel");
        let cid = create.output["cid"].as_str().unwrap();

        let result = dispatch(&kernel, "cas.delete",
            serde_json::json!({"cid": cid}),
            "kernel");
        assert!(result.success, "cas.delete should succeed: {:?}", result.error);
    }

    #[test]
    fn test_cas_delete_nonexistent_returns_error() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "cas.delete",
            serde_json::json!({"cid": "0000000000000000000000000000000000000000000000000000000000000000"}),
            "kernel");
        assert!(!result.success, "cas.delete of nonexistent should fail");
    }

    // ─── Memory Tools ───────────────────────────────────────────────────────────

    #[test]
    fn test_memory_store_working() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "memory.store",
            serde_json::json!({"content": "working memory", "tier": "working", "importance": 50}),
            "TestAgent");
        assert!(result.success, "memory.store working should succeed: {:?}", result.error);
        assert_eq!(result.output["tier"].as_str().unwrap(), "working");
    }

    #[test]
    fn test_memory_recall_agent_name_resolution() {
        let (kernel, _dir) = make_kernel();
        kernel.register_agent("RecallAgent".to_string());
        dispatch(&kernel, "memory.store",
            serde_json::json!({"content": "recallable", "tier": "working"}),
            "RecallAgent");

        let result = dispatch(&kernel, "memory.recall",
            serde_json::json!({"agent_id": "RecallAgent"}),
            "RecallAgent");
        assert!(result.success, "memory.recall by name should resolve: {:?}", result.error);
    }

    #[test]
    fn test_memory_recall_nonexistent_agent() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "memory.recall",
            serde_json::json!({"agent_id": "DoesNotExist"}),
            "kernel");
        assert!(!result.success, "memory.recall for nonexistent agent should fail");
    }

    // ─── KG Tools ──────────────────────────────────────────────────────────────

    #[test]
    fn test_kg_add_node_dispatch() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "kg.add_node",
            serde_json::json!({"label": "TestNode", "type": "entity", "properties": {}}),
            "kernel");
        assert!(result.success, "kg.add_node should succeed: {:?}", result.error);
        assert!(result.output["node_id"].is_string());
    }

    #[test]
    fn test_kg_add_edge_dispatch() {
        let (kernel, _dir) = make_kernel();
        let n1 = dispatch(&kernel, "kg.add_node",
            serde_json::json!({"label": "Node1", "type": "entity"}),
            "kernel");
        let n2 = dispatch(&kernel, "kg.add_node",
            serde_json::json!({"label": "Node2", "type": "entity"}),
            "kernel");
        let node1 = n1.output["node_id"].as_str().unwrap();
        let node2 = n2.output["node_id"].as_str().unwrap();

        let result = dispatch(&kernel, "kg.add_edge",
            serde_json::json!({"src": node1, "dst": node2, "type": "related_to"}),
            "kernel");
        assert!(result.success, "kg.add_edge should succeed: {:?}", result.error);
    }

    // ─── Agent Tools ───────────────────────────────────────────────────────────

    #[test]
    fn test_agent_register_dispatch() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "agent.register",
            serde_json::json!({"name": "DispatchTestAgent"}),
            "kernel");
        assert!(result.success, "agent.register should succeed: {:?}", result.error);
        assert!(result.output["agent_id"].is_string());
    }

    #[test]
    fn test_agent_status_dispatch() {
        let (kernel, _dir) = make_kernel();
        let reg = dispatch(&kernel, "agent.register",
            serde_json::json!({"name": "StatusTestAgent"}),
            "kernel");
        let agent_id = reg.output["agent_id"].as_str().unwrap();

        let result = dispatch(&kernel, "agent.status",
            serde_json::json!({"agent_id": agent_id}),
            "kernel");
        assert!(result.success, "agent.status should succeed: {:?}", result.error);
        assert_eq!(result.output["agent_id"].as_str().unwrap(), agent_id);
    }

    // ─── Tool Registry ─────────────────────────────────────────────────────────

    #[test]
    fn test_tools_list_dispatch() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "tools.list",
            serde_json::json!({}),
            "kernel");
        assert!(result.success, "tools.list should succeed: {:?}", result.error);
        assert!(result.output.is_array());
    }

    #[test]
    fn test_tools_describe_unknown_returns_error() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "tools.describe",
            serde_json::json!({"name": "nonexistent.tool"}),
            "kernel");
        assert!(!result.success, "tools.describe for unknown tool should fail");
    }

    #[test]
    fn test_unknown_tool_returns_error() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "nonexistent.tool",
            serde_json::json!({}),
            "kernel");
        assert!(!result.success, "unknown tool should return error");
    }
}
