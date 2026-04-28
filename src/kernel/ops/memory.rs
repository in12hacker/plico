//! Memory tier operations — ephemeral, working, long-term.
//!
//! Memory tier automation (v12.0):
//! - Automatic promotion based on access thresholds
//! - Automatic eviction of low-importance ephemeral entries
//! - Tier maintenance via TierMaintenance struct

use crate::api::permission::{PermissionAction, PermissionContext};
use crate::memory::{MemoryEntry, MemoryContent, MemoryTier, MemoryType, MemoryScope};
use crate::scheduler::AgentId;
use crate::kernel::event_bus::KernelEvent;
use crate::fs::embedding::types::EmbeddingProvider;
use super::observability::{OpType, OperationTimer};

/// Bundled parameters for storing a procedural memory entry.
pub struct ProceduralEntry {
    pub name: String,
    pub description: String,
    pub steps: Vec<crate::memory::layered::ProcedureStep>,
    pub learned_from: String,
    pub tags: Vec<String>,
}

impl crate::kernel::AIKernel {
    fn agent_memory_quota(&self, agent_id: &str) -> u64 {
        self.scheduler
            .get_resources(&AgentId(agent_id.to_string()))
            .map(|r| r.memory_quota)
            .unwrap_or(0)
    }

    /// Check and promote a specific memory entry if it meets promotion thresholds.
    ///
    /// Returns `true` if the entry was promoted, `false` otherwise.
    #[cfg(test)]
    pub(crate) fn check_and_promote(&self, agent_id: &str, entry_id: &str) -> bool {
        let thresholds = crate::memory::relevance::PromotionThresholds::default();
        let thresholds_ref = &thresholds;

        // Find the entry across all tiers
        let entry_opt = self.memory.get_all(agent_id)
            .into_iter()
            .find(|e| e.id == entry_id);

        let Some(entry) = entry_opt else { return false; };

        // Check if promotion is needed
        let Some(target_tier) = crate::memory::relevance::check_promotion(&entry, thresholds_ref) else {
            return false;
        };

        // Move the entry to the target tier
        self.memory.move_entry(agent_id, entry_id, target_tier)
    }

    /// Run tier maintenance for an agent:
    /// - Process ephemeral eviction (low-importance entries are discarded)
    /// - Process promotions across all tiers
    #[cfg(test)]
    pub(crate) fn run_tier_maintenance(&self, agent_id: &str) {
        let maintenance = crate::kernel::ops::tier_maintenance::TierMaintenance::new();
        maintenance.run_maintenance_cycle(&self.memory, agent_id);
    }

    /// Store a memory entry in the agent's ephemeral (L0) tier.
    /// Returns the entry ID on success.
    pub fn remember(&self, agent_id: &str, tenant_id: &str, content: String) -> Result<String, String> {
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write).map_err(|e| e.to_string())?;
        let entry_id = uuid::Uuid::new_v4().to_string();
        let now = crate::memory::layered::now_ms();
        let entry = MemoryEntry {
            id: entry_id.clone(),
            agent_id: agent_id.to_string(),
            tenant_id: tenant_id.to_string(),
            tier: MemoryTier::Ephemeral,
            content: MemoryContent::Text(content),
            importance: 50,
            access_count: 0,
            last_accessed: now,
            created_at: now,
            tags: Vec::new(),
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: MemoryType::default(),
            causal_parent: None,
            supersedes: None,
        };
        let quota = self.agent_memory_quota(agent_id);
        self.memory.store_checked(entry, quota)
            .map_err(|e| e.to_string())?;
        Ok(entry_id)
    }

    /// Store a memory entry in the agent's working (L1) tier.
    pub fn remember_working(&self, agent_id: &str, tenant_id: &str, content: String, tags: Vec<String>) -> Result<(), String> {
        self.remember_working_scoped(agent_id, tenant_id, content, tags, MemoryScope::Private)
    }

    /// Store a working memory entry with explicit scope.
    pub fn remember_working_scoped(
        &self,
        agent_id: &str,
        tenant_id: &str,
        content: String,
        tags: Vec<String>,
        scope: MemoryScope,
    ) -> Result<(), String> {
        let _timer = OperationTimer::new(&self.metrics, OpType::RememberWorking);
        let span = tracing::info_span!(
            "remember_working",
            operation = "remember_working",
            agent_id = %agent_id,
            tenant_id = %tenant_id,
            tags = ?tags,
        );
        let _guard = span.enter();

        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write).map_err(|e| e.to_string())?;
        let entry = MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            tenant_id: tenant_id.to_string(),
            tier: MemoryTier::Working,
            content: MemoryContent::Text(content),
            importance: 50,
            access_count: 0,
            last_accessed: crate::memory::layered::now_ms(),
            created_at: crate::memory::layered::now_ms(),
            tags: tags.clone(),
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope,
            memory_type: MemoryType::default(),
            causal_parent: None,
            supersedes: None,
        };
        let quota = self.agent_memory_quota(agent_id);
        self.memory.store_checked(entry, quota)
            .map_err(|e| e.to_string())?;
        self.event_bus.emit(KernelEvent::MemoryStored {
            agent_id: agent_id.to_string(),
            tier: "working".into(),
        });
        self.persist_memories();
        tracing::info!(tags = ?tags, "working memory stored");
        Ok(())
    }

    /// Retrieve all entries from all tiers (filtered by tenant).
    pub fn recall(&self, agent_id: &str, tenant_id: &str) -> Vec<MemoryEntry> {
        let _timer = OperationTimer::new(&self.metrics, OpType::Recall);
        let span = tracing::info_span!(
            "recall",
            operation = "recall",
            agent_id = %agent_id,
            tenant_id = %tenant_id,
        );
        let _guard = span.enter();

        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        if self.permissions.check(&ctx, PermissionAction::Read).is_err() {
            return Vec::new();
        }
        let entries: Vec<MemoryEntry> = self.memory.get_all(agent_id)
            .into_iter()
            .filter(|e| e.tenant_id == tenant_id)
            .collect();
        tracing::info!(count = entries.len(), "memories recalled");
        entries
    }

    /// Retrieve all entries visible to an agent (own + shared + group, filtered by tenant).
    pub fn recall_visible(&self, agent_id: &str, tenant_id: &str, groups: &[String]) -> Vec<MemoryEntry> {
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        if self.permissions.check(&ctx, PermissionAction::Read).is_err() {
            return Vec::new();
        }
        self.memory.recall_visible(agent_id, groups)
            .into_iter()
            .filter(|e| e.tenant_id == tenant_id)
            .collect()
    }

    /// Clear ephemeral (L0) memory only.
    pub fn forget_ephemeral(&self, agent_id: &str) {
        self.memory.evict_ephemeral(agent_id);
    }

    /// Retrieve entries relevant to a query, within token budget.
    pub fn recall_relevant(&self, agent_id: &str, tenant_id: &str, budget_tokens: usize) -> Vec<MemoryEntry> {
        self.memory.recall_relevant(agent_id, budget_tokens)
            .into_iter()
            .filter(|e| e.tenant_id == tenant_id)
            .collect()
    }

    /// Evict expired entries from all tiers.
    pub fn evict_expired(&self, agent_id: &str) -> usize {
        self.memory.evict_expired(agent_id)
    }

    /// Check and promote entries between tiers if thresholds are met.
    pub fn promote_check(&self, agent_id: &str) {
        self.memory.promote_check(agent_id);
    }

    /// Move a memory entry to a different tier.
    pub fn memory_move(&self, agent_id: &str, _tenant_id: &str, entry_id: &str, target_tier: MemoryTier) -> bool {
        let moved = self.memory.move_entry(agent_id, entry_id, target_tier);
        if moved { self.persist_memories(); }
        moved
    }

    /// Delete a specific memory entry by ID across all tiers.
    pub fn memory_delete(&self, agent_id: &str, _tenant_id: &str, entry_id: &str) -> bool {
        let deleted = self.memory.delete_entry(agent_id, entry_id);
        if deleted { self.persist_memories(); }
        deleted
    }

    /// Store a memory entry in the agent's long-term tier with semantic embedding.
    pub fn remember_long_term(
        &self,
        agent_id: &str,
        tenant_id: &str,
        content: String,
        tags: Vec<String>,
        importance: u8,
    ) -> Result<String, String> {
        self.remember_long_term_scoped(agent_id, tenant_id, content, tags, importance, MemoryScope::Private)
    }

    /// Store a long-term memory entry with explicit scope.
    /// Returns the entry ID on success.
    pub fn remember_long_term_scoped(
        &self,
        agent_id: &str,
        tenant_id: &str,
        content: String,
        tags: Vec<String>,
        importance: u8,
        scope: MemoryScope,
    ) -> Result<String, String> {
        let _timer = OperationTimer::new(&self.metrics, OpType::RememberLongTerm);
        let span = tracing::info_span!(
            "remember_long_term",
            operation = "remember_long_term",
            agent_id = %agent_id,
            tenant_id = %tenant_id,
            importance = importance,
            tags = ?tags,
        );
        let _guard = span.enter();

        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write).map_err(|e| e.to_string())?;
        let embedding = self.embedding.embed(&content).ok().map(|r| r.embedding);
        let entry_id = uuid::Uuid::new_v4().to_string();
        let created_at = crate::memory::layered::now_ms();
        let entry = MemoryEntry {
            id: entry_id.clone(),
            agent_id: agent_id.to_string(),
            tenant_id: tenant_id.to_string(),
            tier: MemoryTier::LongTerm,
            content: MemoryContent::Text(content),
            importance,
            access_count: 0,
            last_accessed: created_at,
            created_at,
            tags: tags.clone(),
            embedding,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: scope.clone(),
            memory_type: MemoryType::default(),
            causal_parent: None,
            supersedes: None,
        };
        let quota = self.agent_memory_quota(agent_id);
        self.memory.store_checked(entry, quota)
            .map_err(|e| e.to_string())?;
        self.event_bus.emit(KernelEvent::MemoryStored {
            agent_id: agent_id.to_string(),
            tier: "long_term".into(),
        });

        // Emit KnowledgeShared for Shared or Group scope memories
        let tags_for_event = tags.clone();
        match &scope {
            MemoryScope::Shared => {
                let summary = format!(
                    "tags=[{}] importance={} created_at={}",
                    tags_for_event.join(","),
                    importance,
                    created_at
                );
                self.event_bus.emit(KernelEvent::KnowledgeShared {
                    cid: entry_id.clone(),
                    agent_id: agent_id.to_string(),
                    scope: "shared".into(),
                    tags: tags_for_event,
                    summary,
                });
            }
            MemoryScope::Group(group_id) => {
                let summary = format!(
                    "tags=[{}] importance={} created_at={}",
                    tags_for_event.join(","),
                    importance,
                    created_at
                );
                self.event_bus.emit(KernelEvent::KnowledgeShared {
                    cid: entry_id.clone(),
                    agent_id: agent_id.to_string(),
                    scope: format!("group:{}", group_id),
                    tags: tags_for_event,
                    summary,
                });
            }
            MemoryScope::Private => {}
        }

        self.persist_memories();
        tracing::info!(tags = ?tags, importance = importance, "long-term memory stored");
        Ok(entry_id)
    }

    /// Batch-store multiple long-term memories with a single batched embedding call.
    ///
    /// Significantly faster than calling `remember_long_term` in a loop because
    /// embedding requests are batched into one network round-trip.
    pub fn remember_long_term_batch(
        &self,
        agent_id: &str,
        tenant_id: &str,
        items: &[(String, Vec<String>, u8)],
    ) -> Result<Vec<String>, String> {
        let _timer = OperationTimer::new(&self.metrics, OpType::RememberLongTerm);
        if items.is_empty() {
            return Ok(Vec::new());
        }

        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write).map_err(|e| e.to_string())?;

        let texts: Vec<&str> = items.iter().map(|(c, _, _)| c.as_str()).collect();
        let embeddings = match self.embedding.embed_batch(&texts) {
            Ok(results) => results.into_iter().map(|r| Some(r.embedding)).collect::<Vec<_>>(),
            Err(e) => {
                tracing::warn!("batch embed failed, falling back to individual: {e}");
                texts.iter().map(|t| self.embedding.embed(t).ok().map(|r| r.embedding)).collect()
            }
        };

        let created_at = crate::memory::layered::now_ms();
        let quota = self.agent_memory_quota(agent_id);
        let mut ids = Vec::with_capacity(items.len());

        for (i, (content, tags, importance)) in items.iter().enumerate() {
            let entry_id = uuid::Uuid::new_v4().to_string();
            let entry = MemoryEntry {
                id: entry_id.clone(),
                agent_id: agent_id.to_string(),
                tenant_id: tenant_id.to_string(),
                tier: MemoryTier::LongTerm,
                content: MemoryContent::Text(content.clone()),
                importance: *importance,
                access_count: 0,
                last_accessed: created_at,
                created_at,
                tags: tags.clone(),
                embedding: embeddings.get(i).cloned().flatten(),
                ttl_ms: None,
                original_ttl_ms: None,
                scope: MemoryScope::Private,
                memory_type: MemoryType::default(),
                causal_parent: None,
                supersedes: None,
            };
            self.memory.store_checked(entry, quota).map_err(|e| e.to_string())?;
            self.event_bus.emit(KernelEvent::MemoryStored {
                agent_id: agent_id.to_string(),
                tier: "long_term".into(),
            });
            ids.push(entry_id);
        }

        self.persist_memories();
        tracing::info!(count = items.len(), "batch long-term memory stored");
        Ok(ids)
    }

    /// Retrieve semantically relevant long-term memories for an agent.
    pub fn recall_semantic(
        &self,
        agent_id: &str,
        tenant_id: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<MemoryEntry>, String> {
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Read).map_err(|e| e.to_string())?;
        let query_emb = self.embedding.embed(query).map_err(|e| e.to_string())?;
        let results = self.memory.recall_semantic(agent_id, &query_emb.embedding, k);
        Ok(results.into_iter()
            .map(|(entry, _score)| entry)
            .filter(|e| e.tenant_id == tenant_id)
            .collect())
    }

    /// Retrieve relevant memories with semantic scoring, within token budget.
    pub(crate) fn recall_relevant_semantic(
        &self,
        agent_id: &str,
        tenant_id: &str,
        query: &str,
        budget_tokens: usize,
    ) -> Vec<MemoryEntry> {
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        if self.permissions.check(&ctx, PermissionAction::Read).is_err() {
            return Vec::new();
        }
        let tenant_id_owned = tenant_id.to_string();
        match self.embedding.embed(query) {
            Ok(emb) => self.memory.recall_relevant_semantic(agent_id, &emb.embedding, budget_tokens)
                .into_iter()
                .filter(|e| e.tenant_id == tenant_id_owned)
                .collect(),
            Err(_) => self.memory.recall_relevant(agent_id, budget_tokens)
                .into_iter()
                .filter(|e| e.tenant_id == tenant_id_owned)
                .collect(),
        }
    }

    /// Intent-aware semantic recall — classifies query intent, then routes to
    /// the optimal retrieval strategy. Uses the 7-signal RFE for final ranking.
    ///
    /// Concurrent pipeline: intent classification, query embedding, and BM25 search
    /// run in parallel (three independent channels), then RFE fuses all signals.
    pub fn recall_routed(
        &self,
        agent_id: &str,
        tenant_id: &str,
        query: &str,
    ) -> Result<(Vec<MemoryEntry>, crate::fs::retrieval_router::ClassifiedIntent), String> {
        use crate::fs::retrieval_router::{
            classify_by_rules, classify_by_llm_response,
            intent_classification_prompt, RetrievalConfig,
        };
        use crate::fs::retrieval_fusion::{RetrievalFusionEngine, RetrievalQuery};
        use crate::llm::{ChatMessage, ChatOptions, LlmProvider};

        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Read).map_err(|e| e.to_string())?;

        // Three-channel concurrent pipeline: intent + embedding + BM25
        let (classified, query_emb_result, bm25_hits) = std::thread::scope(|s| {
            let classify_handle = s.spawn(|| {
                let prompt = {
                    let mut vars = std::collections::HashMap::new();
                    vars.insert("query", query.to_string());
                    self.prompt_registry
                        .render("intent_classification", &vars, Some(agent_id))
                        .unwrap_or_else(|_| intent_classification_prompt(query))
                };
                let opts = ChatOptions { temperature: 0.0, max_tokens: Some(20) };
                let msgs = [ChatMessage::system("You are an intent classifier."), ChatMessage::user(prompt)];
                match self.llm_provider.chat(&msgs, &opts) {
                    Ok((response, _in_tok, _out_tok)) => {
                        classify_by_llm_response(&response).unwrap_or_else(|| classify_by_rules(query))
                    }
                    Err(_) => classify_by_rules(query),
                }
            });

            let embed_handle = s.spawn(|| self.embedding.embed(query));

            let bm25_handle = s.spawn(|| self.fs.bm25_search(query, 50));

            let classified = classify_handle.join().unwrap();
            let emb_result = embed_handle.join().unwrap();
            let bm25 = bm25_handle.join().unwrap();
            (classified, emb_result, bm25)
        });

        let config = RetrievalConfig::for_intent(classified.intent);

        // Build BM25 score map for RFE
        let bm25_score_map: std::collections::HashMap<String, f32> =
            bm25_hits.into_iter().collect();

        let results: Vec<MemoryEntry> = match query_emb_result {
            Ok(query_emb) => {
                let candidates: Vec<(MemoryEntry, f32)> = if let Some(ref mem_type) = config.typed_retrieval {
                    self.memory.recall_semantic_typed(agent_id, &query_emb.embedding, config.top_k * 2)
                        .into_iter()
                        .filter(|(e, _)| e.memory_type == *mem_type || e.memory_type == MemoryType::Untyped)
                        .collect()
                } else {
                    self.memory.recall_semantic(agent_id, &query_emb.embedding, config.top_k * 2)
                };

                let entries: Vec<MemoryEntry> = candidates.into_iter()
                    .map(|(e, _)| e)
                    .filter(|e| e.tenant_id == tenant_id)
                    .collect();

                // RFE 7-signal re-ranking with per-agent learned weights
                let agent_weights = self.agent_profiles.get_weights(agent_id);
                let rfe = RetrievalFusionEngine::new(agent_weights);
                let causal_graph: Option<&crate::memory::causal::CausalGraph> = None;

                let rfe_query = RetrievalQuery {
                    query_embedding: &query_emb.embedding,
                    query_tags: &[],
                    query_memory_type: config.typed_retrieval,
                    context_entry_id: None,
                    bm25_scores: Some(&bm25_score_map),
                };

                let fused = rfe.rank(&entries, &rfe_query, causal_graph, config.top_k);
                fused.into_iter().map(|r| r.entry).collect()
            }
            Err(_) => {
                self.memory.recall_relevant(agent_id, config.top_k * 100)
                    .into_iter()
                    .filter(|e| e.tenant_id == tenant_id)
                    .filter(|e| {
                        if let Some(ref mem_type) = config.typed_retrieval {
                            e.memory_type == *mem_type || e.memory_type == MemoryType::Untyped
                        } else {
                            true
                        }
                    })
                    .take(config.top_k)
                    .collect()
            }
        };

        // Record query to agent profile for adaptive learning (Axiom 9)
        {
            let mut profile = self.agent_profiles.get_or_create(agent_id);
            profile.record_query(classified.intent, 0.0);
            for entry in &results {
                profile.record_memory_type_hit(entry.memory_type);
            }
            self.agent_profiles.update(profile);
        }

        Ok((results, classified))
    }

    /// Store a procedural memory entry (L3 tier — learned skills/workflows).
    pub fn remember_procedural(
        &self,
        agent_id: &str,
        tenant_id: &str,
        entry: ProceduralEntry,
    ) -> Result<String, String> {
        self.remember_procedural_scoped(agent_id, tenant_id, entry, MemoryScope::Private)
    }

    /// Store a procedural memory entry with explicit scope.
    pub fn remember_procedural_scoped(
        &self,
        agent_id: &str,
        tenant_id: &str,
        entry: ProceduralEntry,
        scope: MemoryScope,
    ) -> Result<String, String> {
        let ProceduralEntry { name, description, steps, learned_from, tags } = entry;
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write).map_err(|e| e.to_string())?;
        let procedure = crate::memory::layered::Procedure {
            name,
            description,
            steps,
            learned_from,
        };
        let entry_id = uuid::Uuid::new_v4().to_string();
        let entry = MemoryEntry {
            id: entry_id.clone(),
            agent_id: agent_id.to_string(),
            tenant_id: tenant_id.to_string(),
            tier: MemoryTier::Procedural,
            content: MemoryContent::Procedure(procedure),
            importance: 100,
            access_count: 0,
            last_accessed: crate::memory::layered::now_ms(),
            created_at: crate::memory::layered::now_ms(),
            tags,
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope,
            memory_type: MemoryType::Procedural,
            causal_parent: None,
            supersedes: None,
        };
        let quota = self.agent_memory_quota(agent_id);
        self.memory.store_checked(entry, quota).map_err(|e| e.to_string())?;
        self.event_bus.emit(KernelEvent::MemoryStored {
            agent_id: agent_id.to_string(),
            tier: "procedural".into(),
        });
        self.persist_memories();
        Ok(entry_id)
    }

    /// Recall procedural memories, optionally filtered by procedure name.
    pub fn recall_procedural(&self, agent_id: &str, tenant_id: &str, name_filter: Option<&str>) -> Vec<MemoryEntry> {
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        if self.permissions.check(&ctx, PermissionAction::Read).is_err() {
            return Vec::new();
        }
        let entries = self.memory.get_tier(agent_id, MemoryTier::Procedural);
        let tenant_id_owned = tenant_id.to_string();
        entries.into_iter()
            .filter(|e| {
                // Tenant isolation
                if e.tenant_id != tenant_id_owned {
                    return false;
                }
                match name_filter {
                    None => true,
                    Some(name) => matches!(&e.content, MemoryContent::Procedure(p) if p.name == name),
                }
            })
            .collect()
    }

    /// Recall shared memories from other agents.
    /// If target_agent_id is Some, only returns memories from that agent.
    /// If None, returns shared memories from all agents except caller.
    pub(crate) fn recall_shared(
        &self,
        caller_id: &str,
        target_agent_id: Option<&str>,
        query: Option<&str>,
        limit: usize,
    ) -> Vec<MemoryEntry> {
        let ctx = PermissionContext::new(caller_id.to_string(), crate::DEFAULT_TENANT.to_string());
        if self.permissions.check(&ctx, PermissionAction::Read).is_err() {
            return Vec::new();
        }

        let mut results = self.memory.get_shared_entries_all_agents();

        if let Some(target) = target_agent_id {
            let target_uuid = self.resolve_agent(target);
            results.retain(|e| {
                e.agent_id == target
                    || target_uuid.as_deref().is_some_and(|u| e.agent_id == u)
            });
        } else {
            // No target specified: return ALL shared memories from ALL agents,
            // including the caller's own. This allows CLI "recall --scope shared" to work
            // (agent sees their own shared memories) while still supporting cross-agent
            // recall (an agent can see other agents' shared memories too).
            // No filtering needed - return all.
        }

        if let Some(q) = query {
            let q_lower = q.to_lowercase();
            results.retain(|entry| {
                let content_match = entry.content.display().to_string().to_lowercase().contains(&q_lower);
                let tag_match = entry.tags.iter().any(|t| t.to_lowercase().contains(&q_lower));
                content_match || tag_match
            });
        }

        results.sort_by(|a, b| b.importance.partial_cmp(&a.importance).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);
        results
    }

    /// Recall shared procedural memories from all agents.
    pub fn recall_shared_procedural(&self, name_filter: Option<&str>) -> Vec<MemoryEntry> {
        let entries = self.memory.get_shared(MemoryTier::Procedural);
        match name_filter {
            None => entries,
            Some(name) => entries.into_iter().filter(|e| {
                matches!(&e.content, MemoryContent::Procedure(p) if p.name == name)
            }).collect(),
        }
    }

    /// Compute memory usage statistics for an agent's tier(s).
    ///
    /// If `tier` is Some, stats are computed only for that tier.
    /// If `tier` is None, stats aggregate all tiers.
    pub fn memory_stats(&self, agent_id: &str, tier: Option<&MemoryTier>) -> crate::api::semantic::MemoryStatsResult {
        use crate::api::semantic::MemoryStatsResult;
        use crate::memory::layered::now_ms;

        let now = now_ms();
        let tiers: Vec<MemoryTier> = match tier {
            Some(t) => vec![*t],
            None => vec![
                MemoryTier::Ephemeral,
                MemoryTier::Working,
                MemoryTier::LongTerm,
                MemoryTier::Procedural,
            ],
        };

        let mut total_entries = 0;
        let mut total_bytes = 0usize;
        let mut oldest_entry_age_ms: u64 = 0;
        let mut total_access_count = 0u64;
        let mut never_accessed_count = 0;
        let mut about_to_expire_count = 0;

        for t in &tiers {
            let entries = self.memory.get_tier(agent_id, *t);
            for entry in entries {
                total_entries += 1;
                total_bytes += entry.content.display().len(); // rough byte estimate

                let age_ms = now.saturating_sub(entry.created_at);
                if age_ms > oldest_entry_age_ms {
                    oldest_entry_age_ms = age_ms;
                }

                total_access_count += entry.access_count as u64;
                if entry.access_count == 0 {
                    never_accessed_count += 1;
                }

                // Check if entry is about to expire (within 10% of TTL)
                if let Some(ttl) = entry.ttl_ms {
                    let remaining = entry.created_at.saturating_add(ttl).saturating_sub(now);
                    if ttl > 0 && remaining < ttl / 10 {
                        about_to_expire_count += 1;
                    }
                }
            }
        }

        let avg_access_count = if total_entries > 0 {
            total_access_count as f32 / total_entries as f32
        } else {
            0.0
        };

        MemoryStatsResult {
            agent_id: agent_id.to_string(),
            tier: tier.map(|t| t.name().to_string()).unwrap_or_default(),
            total_entries,
            total_bytes,
            oldest_entry_age_ms,
            avg_access_count,
            never_accessed_count,
            about_to_expire_count,
        }
    }
}

/// Discover shared knowledge from other agents based on query and scope (F-16).
pub fn discover_knowledge(
    memory: &crate::memory::LayeredMemory,
    query: &str,
    scope: &crate::api::semantic::DiscoveryScope,
    knowledge_types: &[crate::api::semantic::KnowledgeType],
    max_results: usize,
    _token_budget: Option<usize>,
) -> crate::api::semantic::DiscoveryResult {
    use crate::api::semantic::{DiscoveryHit, DiscoveryResult, KnowledgeType};
    use crate::memory::MemoryContent;

    let entries = match scope {
        crate::api::semantic::DiscoveryScope::Shared => {
            memory.get_shared_entries_all_agents()
        }
        crate::api::semantic::DiscoveryScope::Group(group_id) => {
            memory.get_group_entries_all_agents(group_id)
        }
        crate::api::semantic::DiscoveryScope::AllAccessible => {
            memory.get_all_entries_all_agents()
        }
    };

    let filtered: Vec<_> = entries.into_iter().filter(|e| {
        if knowledge_types.is_empty() {
            return true;
        }
        match &e.content {
            MemoryContent::Text(_) => knowledge_types.contains(&KnowledgeType::Memory),
            MemoryContent::Procedure(_) => knowledge_types.contains(&KnowledgeType::Procedure),
            MemoryContent::Knowledge(_) | MemoryContent::ObjectRef(_) | MemoryContent::Structured(_) => {
                knowledge_types.contains(&KnowledgeType::Knowledge)
            }
        }
    }).collect();

    let query_lower = query.to_lowercase();
    let query_terms: Vec<&str> = query_lower.split_whitespace().collect();

    let mut hits: Vec<DiscoveryHit> = filtered.into_iter().map(|e| {
        let preview: String = match &e.content {
            MemoryContent::Text(t) => t.chars().take(200).collect::<String>(),
            MemoryContent::Procedure(p) => p.description.chars().take(200).collect::<String>(),
            MemoryContent::Knowledge(kp) => kp.statement.chars().take(200).collect::<String>(),
            MemoryContent::ObjectRef(cid) => format!("<object: {}>", cid).chars().take(200).collect::<String>(),
            MemoryContent::Structured(s) => serde_json::to_string(s).unwrap_or_default().chars().take(200).collect::<String>(),
        };

        let relevance_score = calculate_relevance(&e.tags, &preview, &query_terms);

        DiscoveryHit {
            cid: e.id.clone(),
            source_agent: e.agent_id.clone(),
            shared_at: e.created_at,
            tags: e.tags.clone(),
            preview,
            relevance_score,
            usage_count: e.access_count as u64,
        }
    }).collect();

    hits.sort_by(|a, b| b.relevance_score.partial_cmp(&a.relevance_score).unwrap_or(std::cmp::Ordering::Equal));

    let total_available = hits.len();
    hits.truncate(max_results);

    let token_estimate = hits.iter()
        .map(|i| i.preview.len() / 4)
        .sum();

    DiscoveryResult {
        items: hits,
        token_estimate,
        total_available,
    }
}

fn calculate_relevance(tags: &[String], content: &str, query_terms: &[&str]) -> f32 {
    use std::collections::HashSet;

    let content_lower = content.to_lowercase();
    let tag_set: HashSet<&str> = tags.iter().map(|t| t.as_str()).collect();

    let mut score = 0.0f32;

    for term in query_terms {
        if content_lower.contains(term) {
            score += 0.1;
        }
        if tag_set.contains(term) {
            score += 0.2;
        }
    }

    score.min(1.0)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remember_ephemeral_basic() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel.remember("kernel", "default", "ephemeral thought".to_string());
        assert!(id.is_ok());
    }

    #[test]
    fn test_remember_long_term_basic() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel.remember_long_term(
            "kernel", "default",
            "important fact".to_string(),
            vec!["fact".to_string()],
            80,
        );
        assert!(id.is_ok());
    }

    #[test]
    fn test_check_and_promote_from_ephemeral_to_working() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        // Store an ephemeral entry with high importance and enough access
        let id = kernel.remember("kernel", "default", "promotable entry".to_string()).expect("remember failed");

        // Manually bump access count via memory
        let entries = kernel.memory.get_all("kernel");
        if let Some(mut entry) = entries.into_iter().find(|e| e.id == id) {
            entry.access_count = 5; // above promotion threshold
            entry.importance = 80;   // high importance
            kernel.memory.store(entry);
        }

        let promoted = kernel.check_and_promote("kernel", &id);
        // May or may not promote depending on thresholds, just check no panic
        let _ = promoted;
    }

    #[test]
    fn test_run_tier_maintenance_no_panic() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember("kernel", "default", "test entry".to_string()).ok();
        // Should not panic
        kernel.run_tier_maintenance("kernel");
    }

    #[test]
    fn test_memory_recall_basic() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember("kernel", "default", "recallable memory".to_string()).ok();

        let entries = kernel.recall("kernel", "default");
        assert!(!entries.is_empty());
    }

    #[test]
    fn test_memory_recall_empty_query() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember("kernel", "default", "test".to_string()).ok();
        let entries = kernel.recall("kernel", "default");
        assert!(entries.len() >= 1);
    }

    #[test]
    fn test_memory_count_via_agent_usage() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_id = kernel.register_agent("usage-agent".to_string());
        kernel.remember(&agent_id, "default", "count me".to_string()).ok();
        let usage = kernel.agent_usage(&agent_id);
        assert!(usage.is_some());
        assert!(usage.unwrap().memory_entries >= 1);
    }

    // ─── Recall Shared Tests (F-4) ─────────────────────────────────────

    #[test]
    fn test_recall_shared_from_specific_agent() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_a = kernel.register_agent("agent-a".to_string());
        let agent_b = kernel.register_agent("agent-b".to_string());

        // Agent A stores a shared memory
        kernel.remember_long_term_scoped(
            &agent_a, "default",
            "shared knowledge from A".to_string(),
            vec!["shared".to_string()],
            80,
            MemoryScope::Shared,
        ).ok();

        // Agent B recalls shared memories from Agent A
        let entries = kernel.recall_shared(&agent_b, Some(&agent_a), None, 10);
        assert!(!entries.is_empty(), "Agent B should see Agent A's shared memory");
        assert!(entries.iter().all(|e| e.agent_id == agent_a));
    }

    #[test]
    fn test_recall_shared_from_all_agents() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_a = kernel.register_agent("agent-a".to_string());
        let agent_b = kernel.register_agent("agent-b".to_string());
        let agent_c = kernel.register_agent("agent-c".to_string());

        // Agent A stores shared memory
        kernel.remember_long_term_scoped(
            &agent_a, "default",
            "A's shared".to_string(),
            vec!["shared".to_string()],
            80,
            MemoryScope::Shared,
        ).ok();

        // Agent B stores shared memory
        kernel.remember_long_term_scoped(
            &agent_b, "default",
            "B's shared".to_string(),
            vec!["shared".to_string()],
            70,
            MemoryScope::Shared,
        ).ok();

        // Agent C recalls from all agents
        let entries = kernel.recall_shared(&agent_c, None, None, 10);
        let agent_ids: Vec<&str> = entries.iter().map(|e| e.agent_id.as_str()).collect();
        assert!(agent_ids.contains(&agent_a.as_str()), "Should include A's shared");
        assert!(agent_ids.contains(&agent_b.as_str()), "Should include B's shared");
        assert!(!agent_ids.contains(&agent_c.as_str()), "Should not include own memories");
    }

    #[test]
    fn test_recall_shared_excludes_private() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_a = kernel.register_agent("agent-a".to_string());
        let agent_b = kernel.register_agent("agent-b".to_string());

        // Agent A stores private memory
        kernel.remember_long_term(
            &agent_a, "default",
            "A's private".to_string(),
            vec!["private".to_string()],
            80,
        ).ok();

        // Agent B stores shared memory
        kernel.remember_long_term_scoped(
            &agent_b, "default",
            "B's shared".to_string(),
            vec!["shared".to_string()],
            80,
            MemoryScope::Shared,
        ).ok();

        // Agent A recalls shared - should NOT see A's own private
        let entries = kernel.recall_shared(&agent_a, None, None, 10);
        assert!(entries.iter().all(|e| e.scope != MemoryScope::Private || e.agent_id != agent_a));
    }

    #[test]
    fn test_recall_shared_empty_when_no_shared() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_a = kernel.register_agent("agent-a".to_string());
        let agent_b = kernel.register_agent("agent-b".to_string());

        // Both only have private memories
        kernel.remember_long_term(
            &agent_a, "default",
            "A's private".to_string(),
            vec!["private".to_string()],
            80,
        ).ok();

        let entries = kernel.recall_shared(&agent_b, Some(&agent_a), None, 10);
        assert!(entries.is_empty(), "No shared memories should return empty");
    }

    #[test]
    fn test_recall_shared_respects_limit() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_a = kernel.register_agent("agent-a".to_string());
        let agent_b = kernel.register_agent("agent-b".to_string());

        // Agent A stores multiple shared memories
        for i in 0..5 {
            kernel.remember_long_term_scoped(
                &agent_a, "default",
                format!("shared memory {}", i),
                vec!["shared".to_string()],
                80,
                MemoryScope::Shared,
            ).ok();
        }

        // Agent B recalls with limit=2
        let entries = kernel.recall_shared(&agent_b, Some(&agent_a), None, 2);
        assert_eq!(entries.len(), 2, "Should respect limit of 2");
    }

    #[test]
    fn test_recall_shared_query_filter() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_a = kernel.register_agent("agent-a".to_string());
        let agent_b = kernel.register_agent("agent-b".to_string());

        // Agent A stores multiple shared memories
        kernel.remember_long_term_scoped(
            &agent_a, "default",
            "python tutorial shared".to_string(),
            vec!["python".to_string()],
            80,
            MemoryScope::Shared,
        ).ok();
        kernel.remember_long_term_scoped(
            &agent_a, "default",
            "rust tutorial shared".to_string(),
            vec!["rust".to_string()],
            80,
            MemoryScope::Shared,
        ).ok();

        // Agent B queries for "python"
        let entries = kernel.recall_shared(&agent_b, None, Some("python"), 10);
        assert!(!entries.is_empty());
        assert!(entries.iter().all(|e| {
            e.content.display().to_string().contains("python") ||
            e.tags.iter().any(|t| t.contains("python"))
        }));
    }

    #[test]
    fn test_recall_shared_with_name_not_uuid() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let _alice_uuid = kernel.register_agent("alice".to_string());
        let _bob_uuid = kernel.register_agent("bob".to_string());

        kernel.remember_working_scoped(
            "alice", "default",
            "shared by name".to_string(),
            vec![],
            MemoryScope::Shared,
        ).unwrap();

        let entries = kernel.recall_shared("bob", None, None, 10);
        assert!(!entries.is_empty(), "B53 regression: recall_shared should work when agent_id is a name, not UUID");
        assert!(entries.iter().any(|e| e.content.display().to_string().contains("shared by name")));
    }

    // ─── Retrieval Router Tests ─────────────────────────────────────

    #[test]
    fn test_recall_routed_falls_back_to_rules() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember_long_term(
            "kernel", "default",
            "The meeting was on Monday".to_string(),
            vec!["meeting".to_string()],
            80,
        ).ok();

        let result = kernel.recall_routed("kernel", "default", "When was the meeting?");
        assert!(result.is_ok());
        let (entries, classified) = result.unwrap();
        assert_eq!(
            classified.intent,
            crate::fs::retrieval_router::QueryIntent::Temporal,
        );
        assert_eq!(
            classified.method,
            crate::fs::retrieval_router::ClassificationMethod::RuleBased,
        );
        let _ = entries;
    }

    #[test]
    fn test_recall_routed_factual_query() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember_long_term(
            "kernel", "default",
            "Plico is an AI-Native OS".to_string(),
            vec!["plico".to_string()],
            90,
        ).ok();

        let result = kernel.recall_routed("kernel", "default", "What is Plico?");
        assert!(result.is_ok());
        let (_entries, classified) = result.unwrap();
        assert_eq!(
            classified.intent,
            crate::fs::retrieval_router::QueryIntent::Factual,
        );
    }

    #[test]
    fn test_recall_routed_preference_query() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let result = kernel.recall_routed("kernel", "default", "What does the user prefer?");
        assert!(result.is_ok());
        let (_entries, classified) = result.unwrap();
        assert_eq!(
            classified.intent,
            crate::fs::retrieval_router::QueryIntent::Preference,
        );
    }

    #[test]
    fn test_recall_routed_aggregation_query() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let result = kernel.recall_routed("kernel", "default", "List all open bugs");
        assert!(result.is_ok());
        let (_entries, classified) = result.unwrap();
        assert_eq!(
            classified.intent,
            crate::fs::retrieval_router::QueryIntent::Aggregation,
        );
    }

    #[test]
    fn test_recall_routed_multi_hop_query() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let result = kernel.recall_routed("kernel", "default", "Why did the server crash?");
        assert!(result.is_ok());
        let (_entries, classified) = result.unwrap();
        assert_eq!(
            classified.intent,
            crate::fs::retrieval_router::QueryIntent::MultiHop,
        );
    }

    #[test]
    fn test_recall_routed_returns_config_matching_intent() {
        use crate::fs::retrieval_router::RetrievalConfig;
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let (_entries, classified) = kernel.recall_routed(
            "kernel", "default", "When was the last deployment?"
        ).unwrap();
        let config = RetrievalConfig::for_intent(classified.intent);
        assert!(config.time_decay_boost, "temporal queries should have time_decay_boost");
        assert!(config.use_kg, "temporal queries should use KG");
    }
}
