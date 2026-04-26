//! Memory tier handlers — remember, recall, move, evict, stats.

use crate::api::semantic::{ApiRequest, ApiResponse};
use crate::memory::MemoryScope;
use crate::DEFAULT_TENANT;
use super::super::ops;

pub(super) fn parse_scope(scope: Option<String>) -> MemoryScope {
    match scope.as_deref() {
        None | Some("private") => MemoryScope::Private,
        Some("shared") => MemoryScope::Shared,
        Some(g) if g.starts_with("group:") => MemoryScope::Group(g[6..].to_string()),
        Some(_) => MemoryScope::Private,
    }
}

impl super::super::AIKernel {
    pub(crate) fn handle_memory(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::Remember { agent_id, content, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                match self.remember(&agent_id, &tenant, content) {
                    Ok(_entry_id) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::Recall { agent_id, scope, query, limit, tier } => {
                let memories: Vec<String> = match scope.as_deref() {
                    Some("shared") => {
                        let lim = limit.unwrap_or(20);
                        self.recall_shared(&agent_id, None, query.as_deref(), lim)
                            .into_iter()
                            .filter_map(|m| match m.content {
                                crate::memory::MemoryContent::Text(t) => Some(t),
                                crate::memory::MemoryContent::Procedure(p) => Some(format!("procedure:{}", p.name)),
                                _ => None,
                            }).collect()
                    }
                    _ => {
                        let all_entries = if let Some(tier_str) = tier.as_deref() {
                            let parsed_tier = match tier_str.to_lowercase().replace(['-', '_'], "").as_str() {
                                "ephemeral" | "l0" => crate::memory::MemoryTier::Ephemeral,
                                "working" | "l1" => crate::memory::MemoryTier::Working,
                                "longterm" | "l2" | "lt" => crate::memory::MemoryTier::LongTerm,
                                "procedural" | "l3" => crate::memory::MemoryTier::Procedural,
                                other => {
                                    tracing::warn!("Unknown tier '{}', falling back to all tiers", other);
                                    return {
                                        let mut r = ApiResponse::ok();
                                        r.memory = Some(self.recall(&agent_id, DEFAULT_TENANT).into_iter()
                                            .filter_map(|m| match m.content {
                                                crate::memory::MemoryContent::Text(t) => Some(t),
                                                _ => None,
                                            }).collect());
                                        r
                                    };
                                }
                            };
                            self.memory.get_tier(&agent_id, parsed_tier)
                        } else {
                            self.recall(&agent_id, DEFAULT_TENANT)
                        };
                        let entries = if let Some(q) = query.as_ref().filter(|q| !q.is_empty()) {
                            all_entries.into_iter()
                                .filter(|e| e.content.display().to_lowercase().contains(&q.to_lowercase()))
                                .collect()
                        } else {
                            all_entries
                        };
                        entries.into_iter()
                            .filter_map(|m| match m.content {
                                crate::memory::MemoryContent::Text(t) => Some(t),
                                _ => None,
                            }).collect()
                    }
                };
                let mut r = ApiResponse::ok();
                r.memory = Some(memories);
                r
            }
            ApiRequest::RememberLongTerm { agent_id, content, tags, importance, scope, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                let scope = parse_scope(scope);
                match self.remember_long_term_scoped(&agent_id, &tenant, content, tags.clone(), importance, scope) {
                    Ok(entry_id) => {
                        self.link_memory_to_kg(&entry_id, &agent_id, &tenant, &tags);
                        ApiResponse::ok()
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::RecallSemantic { agent_id, query, k } => {
                let budget = (k * 500).max(1000);
                let entries = self.recall_relevant_semantic(&agent_id, DEFAULT_TENANT, &query, budget);
                let memories: Vec<String> = entries.into_iter()
                    .filter_map(|m| match m.content {
                        crate::memory::MemoryContent::Text(t) => Some(t),
                        _ => None,
                    }).collect();
                let mut r = ApiResponse::ok();
                r.memory = Some(memories);
                r
            }
            ApiRequest::RememberProcedural { agent_id, name, description, steps, learned_from, tags, scope } => {
                let proc_steps: Vec<crate::memory::layered::ProcedureStep> = steps.into_iter().enumerate().map(|(i, s)| {
                    crate::memory::layered::ProcedureStep {
                        step_number: (i + 1) as u32,
                        description: s.description,
                        action: s.action,
                        expected_outcome: s.expected_outcome.unwrap_or_default(),
                    }
                }).collect();
                let scope = parse_scope(scope);
                match self.remember_procedural_scoped(&agent_id, DEFAULT_TENANT, ops::memory::ProceduralEntry {
                    name, description, steps: proc_steps, learned_from: learned_from.unwrap_or_default(), tags,
                }, scope) {
                    Ok(entry_id) => {
                        let mut r = ApiResponse::ok();
                        r.data = Some(serde_json::json!({"entry_id": entry_id}).to_string());
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::RecallProcedural { agent_id, name } => {
                let entries = self.recall_procedural(&agent_id, DEFAULT_TENANT, name.as_deref());
                let mut r = ApiResponse::ok();
                let data: Vec<serde_json::Value> = entries.iter().map(|e| {
                    match &e.content {
                        crate::memory::MemoryContent::Procedure(p) => {
                            serde_json::json!({
                                "id": e.id, "tier": "procedural", "name": p.name,
                                "description": p.description,
                                "steps": p.steps.iter().map(|s| serde_json::json!({
                                    "step_number": s.step_number, "description": s.description,
                                    "action": s.action, "expected_outcome": s.expected_outcome,
                                })).collect::<Vec<_>>(),
                                "learned_from": p.learned_from, "tags": e.tags,
                                "importance": e.importance, "scope": format!("{:?}", e.scope),
                            })
                        }
                        _ => serde_json::json!({
                            "id": e.id, "tier": "procedural", "content": e.content.display(),
                            "tags": e.tags, "importance": e.importance, "scope": format!("{:?}", e.scope),
                        })
                    }
                }).collect();
                r.data = Some(serde_json::to_string(&data).unwrap_or_default());
                r
            }
            ApiRequest::RecallVisible { agent_id, groups } => {
                let entries = self.recall_visible(&agent_id, DEFAULT_TENANT, &groups);
                let memories: Vec<String> = entries.into_iter()
                    .map(|m| format!("[{}:{:?}] {}", m.tier.name(), m.scope, m.content.display()))
                    .collect();
                let mut r = ApiResponse::ok();
                r.memory = Some(memories);
                r
            }
            ApiRequest::MemoryMove { agent_id, entry_id, target_tier, .. } => {
                let normalized = target_tier.to_lowercase().replace('-', "_");
                let tier = match normalized.as_str() {
                    "ephemeral" | "l0" => crate::memory::MemoryTier::Ephemeral,
                    "working" | "l1" => crate::memory::MemoryTier::Working,
                    "long_term" | "longterm" | "l2" | "lt" => crate::memory::MemoryTier::LongTerm,
                    "procedural" | "l3" => crate::memory::MemoryTier::Procedural,
                    _ => return ApiResponse::error(format!("unknown tier: {}", target_tier)),
                };
                if self.memory_move(&agent_id, DEFAULT_TENANT, &entry_id, tier) {
                    ApiResponse::ok()
                } else {
                    ApiResponse::error(format!("memory entry not found: {}", entry_id))
                }
            }
            ApiRequest::MemoryDeleteEntry { agent_id, entry_id, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                if self.memory_delete(&agent_id, &tenant, &entry_id) {
                    ApiResponse::ok()
                } else {
                    ApiResponse::error(format!("memory entry not found: {}", entry_id))
                }
            }
            ApiRequest::EvictExpired { agent_id, .. } => {
                let count = self.evict_expired(&agent_id);
                let mut r = ApiResponse::ok();
                r.data = Some(format!("{}", count));
                r
            }
            ApiRequest::LoadContext { cid, layer, agent_id, .. } => {
                let ctx_layer = match crate::fs::ContextLayer::parse_layer(&layer) {
                    Some(l) => l,
                    None => return ApiResponse::error(format!("Invalid layer '{}'. Use L0, L1, or L2.", layer)),
                };
                match self.context_load(&cid, ctx_layer, &agent_id) {
                    Ok(loaded) => {
                        let mut r = ApiResponse::ok();
                        r.context_data = Some(crate::api::semantic::LoadedContextDto {
                            cid: loaded.cid.clone(), layer: loaded.layer.name().to_string(),
                            content: loaded.content, tokens_estimate: loaded.tokens_estimate,
                            actual_layer: loaded.actual_layer.map(|l| l.name().to_string()),
                            degraded: loaded.degraded, degradation_reason: loaded.degradation_reason,
                        });
                        r
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::BatchMemoryStore { entries, agent_id, tenant_id } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                let batch_results = self.handle_batch_memory_store(entries, &agent_id, &tenant);
                let mut r = ApiResponse::ok();
                r.batch_memory_store = Some(batch_results);
                r
            }
            ApiRequest::MemoryStats { agent_id, tier, tenant_id: _ } => {
                let tier_filter = tier.as_ref().and_then(|t| {
                    match t.as_str() {
                        "ephemeral" => Some(crate::memory::MemoryTier::Ephemeral),
                        "working" => Some(crate::memory::MemoryTier::Working),
                        "long_term" => Some(crate::memory::MemoryTier::LongTerm),
                        "procedural" => Some(crate::memory::MemoryTier::Procedural),
                        _ => None,
                    }
                });
                let stats = self.memory_stats(&agent_id, tier_filter.as_ref());
                let mut r = ApiResponse::ok();
                r.memory_stats = Some(stats);
                r
            }
            ApiRequest::DiscoverKnowledge { query, scope, knowledge_types, max_results, token_budget, agent_id: _ } => {
                let result = ops::memory::discover_knowledge(
                    &self.memory, &query, &scope, &knowledge_types, max_results, token_budget,
                );
                let mut r = ApiResponse::ok();
                r.discovery_result = Some(result);
                r
            }
            _ => unreachable!("non-memory request routed to handle_memory"),
        }
    }
}
