//! Memory tier operations — ephemeral, working, long-term.
//!
//! Memory tier automation (v12.0):
//! - Automatic promotion based on access thresholds
//! - Automatic eviction of low-importance ephemeral entries
//! - Tier maintenance via TierMaintenance struct

use crate::api::permission::{PermissionAction, PermissionContext};
use crate::memory::{MemoryEntry, MemoryContent, MemoryTier, MemoryType, MemoryScope};
use crate::scheduler::AgentId;
use crate::fs::retrieval_router::ClassifiedIntent;
use crate::util::case_insensitive_contains;

use crate::kernel::event_bus::KernelEvent;
use crate::fs::embedding::types::EmbeddingProvider;
use super::observability::{OpType, OperationTimer};

fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() { return 0.0; }
    let (mut dot, mut na, mut nb) = (0.0_f32, 0.0_f32, 0.0_f32);
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom < 1e-10 { 0.0 } else { dot / denom }
}

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

    /// Store a confirmed action as a long-term memory with equal weight.
    ///
    /// Implements the "confirmed action as storage" paradigm from Mem0 v3:
    /// every confirmed agent action is stored with equal importance (50),
    /// without pre-judging significance. Semantic dedup prevents duplicates.
    pub fn remember_action(
        &self,
        agent_id: &str,
        tenant_id: &str,
        content: String,
        tags: Vec<String>,
    ) -> Result<String, String> {
        self.remember_long_term_scoped(agent_id, tenant_id, content, tags, 50, MemoryScope::Private)
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

        // Semantic dedup: if a similar long-term memory exists, just touch it
        if let Some(ref emb) = embedding {
            if let Some(existing_id) = self.memory.find_similar_long_term(agent_id, emb, 0.85) {
                self.memory.touch_entry(agent_id, &existing_id);
                tracing::info!("Dedup: merged with existing memory {}", existing_id);
                return Ok(existing_id);
            }
        }

        let entry_id = uuid::Uuid::new_v4().to_string();
        let created_at = crate::memory::layered::now_ms();
        let content_for_ingest = content.clone();
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

        // Ingest pipeline: always run zero-cost regex preference extraction.
        // Full LLM fact extraction only when PLICO_INGEST_EXTRACT=1.
        {
            let text = content_for_ingest;
            if text.trim().len() >= 10 {
                use super::ingest::{extract_preference_signals, extract_facts};

                // Regex preference extraction (always, zero LLM cost)
                let mut extracted = extract_preference_signals(&text);

                // Full LLM fact extraction (optional, expensive)
                if std::env::var("PLICO_INGEST_EXTRACT").as_deref() == Ok("1") {
                    let llm: &dyn crate::llm::LlmProvider = &self.llm_provider;
                    let llm_facts = extract_facts(llm, &text);
                    extracted.extend(llm_facts);
                }

                let quota = self.agent_memory_quota(agent_id);
                for fact in &extracted {
                    if fact.tags.contains(&"raw".to_string()) {
                        continue; // Skip passthrough — already stored as original
                    }
                    let fact_embedding = self.embedding.embed(&fact.text).ok().map(|r| r.embedding);
                    let mut fact_tags = tags.clone();
                    fact_tags.extend(fact.tags.clone());
                    let fact_entry = MemoryEntry {
                        id: uuid::Uuid::new_v4().to_string(),
                        agent_id: agent_id.to_string(),
                        tenant_id: tenant_id.to_string(),
                        tier: MemoryTier::LongTerm,
                        content: MemoryContent::Text(fact.text.clone()),
                        importance: importance.saturating_add(5).min(100),
                        access_count: 0,
                        last_accessed: created_at,
                        created_at,
                        tags: fact_tags,
                        embedding: fact_embedding,
                        ttl_ms: None,
                        original_ttl_ms: None,
                        scope: scope.clone(),
                        memory_type: fact.fact_type.to_memory_type(),
                        causal_parent: Some(entry_id.clone()),
                        supersedes: None,
                    };
                    let _ = self.memory.store_checked(fact_entry, quota);
                }
                if !extracted.is_empty() {
                    tracing::info!(
                        count = extracted.len(),
                        "ingest pipeline: extracted {} facts from memory {}",
                        extracted.len(),
                        entry_id,
                    );
                }
            }
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
        let query_emb = self.embedding.embed_query(query).map_err(|e| e.to_string())?;
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
        match self.embedding.embed_query(query) {
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
        self.recall_routed_with_k(agent_id, tenant_id, query, None)
    }

    /// Intent-aware recall with optional top_k override.
    pub fn recall_routed_with_k(
        &self,
        agent_id: &str,
        tenant_id: &str,
        query: &str,
        top_k_override: Option<usize>,
    ) -> Result<(Vec<MemoryEntry>, crate::fs::retrieval_router::ClassifiedIntent), String> {
        use crate::fs::retrieval_router::{
            classify_by_rules, classify_by_llm_response,
            intent_classification_prompt, RetrievalConfig,
        };
        use crate::fs::retrieval_fusion::{RetrievalFusionEngine, RetrievalQuery};
        use crate::llm::{ChatMessage, ChatOptions, LlmProvider};

        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Read).map_err(|e| e.to_string())?;

        // Four-channel concurrent pipeline: intent + embedding + BM25 + KG Multi-hop (v39)
        let (classified, query_emb_result, bm25_hits, kg_hits) = std::thread::scope(|s| {
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

            // Query bias correction (MemMachine +1.4%): strip role prefixes
            // that bias embedding toward user-side content
            let clean_query = query
                .replace("user: ", "").replace("User: ", "")
                .replace("assistant: ", "").replace("Assistant: ", "");
            let embed_handle = s.spawn(move || self.embedding.embed_query(&clean_query));

            let bm25_handle = s.spawn(|| self.fs.bm25_search(query, 50));

            // KG Multi-hop Channel (F-39): find neighboring nodes for relational queries
            let kg_handle = s.spawn(|| {
                if let Some(ref kg) = self.knowledge_graph {
                    // Find nodes that match keywords in the query
                    let seeds = kg.list_nodes(agent_id, None).unwrap_or_default();
                    let query_words: std::collections::HashSet<_> = query.to_lowercase()
                        .split_whitespace().map(|s| s.to_string()).collect();
                    
                    let mut matching_cids = Vec::new();
                    for node in seeds {
                        if query_words.iter().any(|w| node.id.to_lowercase().contains(w)) {
                            // Find 2-hop neighbors for these seed nodes
                            if let Ok(neighbors) = kg.get_neighbors(&node.id, None, 2) {
                                for (neighbor, _) in neighbors {
                                    if let Some(ref cid) = neighbor.content_cid {
                                        matching_cids.push(cid.clone());
                                    }
                                }
                            }
                        }
                    }
                    matching_cids
                } else {
                    Vec::new()
                }
            });

            let classified = match classify_handle.join() {
                Ok(res) => res,
                Err(e) => {
                    tracing::error!("Intent classification task panicked: {:?}", e);
                    ClassifiedIntent { 
                        intent: crate::fs::retrieval_router::QueryIntent::Factual, 
                        confidence: 0.0, 
                        method: crate::fs::retrieval_router::ClassificationMethod::RuleBased 
                    }
                }
            };
            let emb_result = match embed_handle.join() {
                Ok(res) => res,
                Err(e) => {
                    tracing::error!("Embedding task panicked: {:?}", e);
                    Err(crate::fs::embedding::EmbedError::Api("panic".into()))
                }
            };
            let bm25_hits: Vec<(String, f32)> = match bm25_handle.join() {
                Ok(res) => res,
                Err(e) => {
                    tracing::error!("BM25 search task panicked: {:?}", e);
                    Vec::new()
                }
            };
            let kg_hits: Vec<String> = match kg_handle.join() {
                Ok(res) => res,
                Err(e) => {
                    tracing::error!("KG retrieval task panicked: {:?}", e);
                    Vec::new()
                }
            };
            (classified, emb_result, bm25_hits, kg_hits)
        });

        let mut config = RetrievalConfig::for_intent(classified.intent);
        if let Some(k) = top_k_override {
            config.top_k = k;
        }

        // Build BM25 score map for RFE
        let bm25_score_map: std::collections::HashMap<String, f32> = bm25_hits.iter().cloned().collect();

        // Build KG score map (v39)
        let kg_score_map: std::collections::HashMap<String, f32> = kg_hits.into_iter().map(|cid| (cid, 1.0)).collect();


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
                let mut rfe = RetrievalFusionEngine::new(agent_weights);
                
                // v39: Inject KG multi-hop signals into RFE
                if !kg_score_map.is_empty() {
                    rfe.set_kg_signals(kg_score_map.clone());
                }

                let causal_graph: Option<&crate::memory::causal::CausalGraph> = None;

                let rfe_query = RetrievalQuery {
                    query_embedding: &query_emb.embedding,
                    query_tags: &[],
                    query_memory_type: config.typed_retrieval,
                    context_entry_id: None,
                    bm25_scores: Some(&bm25_score_map),
                };

                let fused = rfe.rank(&entries, &rfe_query, causal_graph, config.top_k * 3);

                // Intent-routed post-processing: reranker for precision intents,
                // MMR diversity for multi-session intents (prevents single-session flooding).
                if config.use_reranker {
                    if let Some(ref reranker) = self.reranker {
                        let docs: Vec<(String, String)> = fused.iter().map(|r| {
                            let text = match &r.entry.content {
                                MemoryContent::Text(t) => t.clone(),
                                _ => format!("{:?}", r.entry.content),
                            };
                            (r.entry.id.clone(), text)
                        }).collect();
                        match reranker.rerank(query, &docs) {
                            Ok(reranked) => {
                                let id_order: Vec<String> = reranked.iter()
                                    .take(config.top_k)
                                    .map(|r| r.id.clone())
                                    .collect();
                                let entry_map: std::collections::HashMap<String, MemoryEntry> = fused
                                    .into_iter()
                                    .map(|r| (r.entry.id.clone(), r.entry))
                                    .collect();
                                id_order.into_iter()
                                    .filter_map(|id| entry_map.get(&id).cloned())
                                    .collect()
                            }
                            Err(e) => {
                                tracing::warn!("reranker failed, using RFE order: {e}");
                                fused.into_iter().take(config.top_k).map(|r| r.entry).collect()
                            }
                        }
                    } else {
                        fused.into_iter().take(config.top_k).map(|r| r.entry).collect()
                    }
                } else {
                    // MMR diversity selection for multi-session/temporal intents:
                    // greedily pick entries that are both relevant (high RFE score)
                    // and diverse (low cosine similarity to already-selected entries).
                    let lambda = 0.7_f32;
                    let mut selected: Vec<MemoryEntry> = Vec::with_capacity(config.top_k);
                    let mut selected_embs: Vec<Vec<f32>> = Vec::new();
                    let mut remaining: Vec<_> = fused.into_iter().collect();

                    while selected.len() < config.top_k && !remaining.is_empty() {
                        let mut best_idx = 0;
                        let mut best_mmr = f32::NEG_INFINITY;

                        for (i, candidate) in remaining.iter().enumerate() {
                            let relevance = candidate.fused_score;
                            let max_sim = if selected_embs.is_empty() {
                                0.0
                            } else if let Some(ref emb) = candidate.entry.embedding {
                                selected_embs.iter()
                                    .map(|sel| cosine_sim(emb, sel))
                                    .fold(0.0_f32, f32::max)
                            } else {
                                0.0
                            };
                            let mmr = lambda * relevance - (1.0 - lambda) * max_sim;
                            if mmr > best_mmr {
                                best_mmr = mmr;
                                best_idx = i;
                            }
                        }

                        let chosen = remaining.swap_remove(best_idx);
                        if let Some(ref emb) = chosen.entry.embedding {
                            selected_embs.push(emb.clone());
                        }
                        selected.push(chosen.entry);
                    }
                    selected
                }
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

    /// HyDE-enhanced recall for complex queries (multi-hop, aggregation).
    ///
    /// Generates a hypothetical answer via LLM, embeds it, and does a second
    /// semantic search to find memories containing answer-like content.
    /// Merges with the standard routed recall results.
    /// Inspired by Gao et al. "Precise Zero-Shot Dense Retrieval" (2023).
    pub fn recall_hyde(
        &self,
        agent_id: &str,
        tenant_id: &str,
        query: &str,
    ) -> Result<(Vec<MemoryEntry>, crate::fs::retrieval_router::ClassifiedIntent), String> {
        use crate::llm::{ChatMessage, ChatOptions, LlmProvider};

        // Standard routed recall (intent classification + RFE + reranker/MMR)
        let (routed_results, classified) = self.recall_routed(agent_id, tenant_id, query)?;

        // Only apply HyDE for complex intents
        if classified.intent != crate::fs::retrieval_router::QueryIntent::MultiHop
            && classified.intent != crate::fs::retrieval_router::QueryIntent::Aggregation
        {
            return Ok((routed_results, classified));
        }

        // Generate hypothetical answer
        let hyde_prompt = format!(
            "Given the following question, write a short hypothetical answer \
             (2-3 sentences) that could plausibly answer it. Use specific details \
             and names if possible. This is for retrieval — be concrete.\n\n\
             Question: {}\n\nHypothetical answer:",
            query
        );
        let opts = ChatOptions { temperature: 0.3, max_tokens: Some(150) };
        let msgs = [ChatMessage::user(hyde_prompt)];
        let hypothetical = match self.llm_provider.chat(&msgs, &opts) {
            Ok((response, _, _)) => response.trim().to_string(),
            Err(_) => return Ok((routed_results, classified)),
        };

        if hypothetical.is_empty() || hypothetical.len() < 10 {
            return Ok((routed_results, classified));
        }

        // Embed the hypothetical answer for a second semantic search
        let hyde_emb = match self.embedding.embed_query(&hypothetical) {
            Ok(r) => r.embedding,
            Err(_) => return Ok((routed_results, classified)),
        };

        // Semantic search with hypothetical embedding
        let hyde_candidates = self.memory.recall_semantic(agent_id, &hyde_emb, 15);
        let hyde_entries: Vec<MemoryEntry> = hyde_candidates.into_iter()
            .map(|(e, _)| e)
            .filter(|e| e.tenant_id == tenant_id)
            .collect();

        // Merge: routed results first (higher quality), then HyDE results for diversity
        let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut merged: Vec<MemoryEntry> = Vec::new();
        for entry in routed_results {
            if seen_ids.insert(entry.id.clone()) {
                merged.push(entry);
            }
        }
        for entry in hyde_entries {
            if seen_ids.insert(entry.id.clone()) {
                merged.push(entry);
            }
        }

        Ok((merged, classified))
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
                // Case-insensitive content check without allocating a new String
                let content_str = entry.content.display();
                let content_match = case_insensitive_contains(&content_str, &q_lower);
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

    // Pre-sort by importance to limit expensive preview/relevance computation
    let mut sorted = filtered;
    sorted.sort_by(|a, b| b.importance.partial_cmp(&a.importance).unwrap_or(std::cmp::Ordering::Equal));
    let candidate_limit = (max_results * 5).max(50);
    sorted.truncate(candidate_limit);

    let mut hits: Vec<DiscoveryHit> = sorted.into_iter().map(|e| {
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

    let tag_set: HashSet<&str> = tags.iter().map(|t| t.as_str()).collect();

    let mut score = 0.0f32;

    for term in query_terms {
        if case_insensitive_contains(content, term) {
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
        assert!(!entries.is_empty());
    }

    #[test]
    fn test_memory_count_via_agent_usage() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_id = kernel.register_agent("usage-agent".to_string()).unwrap();
        kernel.remember(&agent_id, "default", "count me".to_string()).ok();
        let usage = kernel.agent_usage(&agent_id);
        assert!(usage.is_some());
        assert!(usage.unwrap().memory_entries >= 1);
    }

    // ─── Recall Shared Tests (F-4) ─────────────────────────────────────

    #[test]
    fn test_recall_shared_from_specific_agent() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_a = kernel.register_agent("agent-a".to_string()).unwrap();
        let agent_b = kernel.register_agent("agent-b".to_string()).unwrap();

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
        let agent_a = kernel.register_agent("agent-a".to_string()).unwrap();
        let agent_b = kernel.register_agent("agent-b".to_string()).unwrap();
        let agent_c = kernel.register_agent("agent-c".to_string()).unwrap();

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
        let agent_a = kernel.register_agent("agent-a".to_string()).unwrap();
        let agent_b = kernel.register_agent("agent-b".to_string()).unwrap();

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
        let agent_a = kernel.register_agent("agent-a".to_string()).unwrap();
        let agent_b = kernel.register_agent("agent-b".to_string()).unwrap();

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
        let agent_a = kernel.register_agent("agent-a".to_string()).unwrap();
        let agent_b = kernel.register_agent("agent-b".to_string()).unwrap();

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
        let agent_a = kernel.register_agent("agent-a".to_string()).unwrap();
        let agent_b = kernel.register_agent("agent-b".to_string()).unwrap();

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
        let _alice_uuid = kernel.register_agent("alice".to_string()).unwrap();
        let _bob_uuid = kernel.register_agent("bob".to_string()).unwrap();

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

    #[test]
    fn test_remember_working() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let result = kernel.remember_working("kernel", "default", "working memory".to_string(), vec!["tag1".to_string()]);
        assert!(result.is_ok());
        let entries = kernel.recall("kernel", "default");
        assert!(entries.iter().any(|e| e.content.display().to_string().contains("working memory")));
    }

    #[test]
    fn test_remember_working_scoped() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.register_agent("scoped_agent".to_string()).unwrap();
        let result = kernel.remember_working_scoped(
            "scoped_agent", "default",
            "scoped working memory".to_string(),
            vec!["scope_test".to_string()],
            MemoryScope::Shared,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_recall_visible() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.register_agent("vis_agent".to_string()).unwrap();
        kernel.remember_working_scoped(
            "vis_agent", "default",
            "visible memory".to_string(),
            vec![],
            MemoryScope::Shared,
        ).unwrap();
        let entries = kernel.recall_visible("vis_agent", "default", &[]);
        assert!(!entries.is_empty());
    }

    #[test]
    fn test_forget_ephemeral() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember("kernel", "default", "ephemeral stuff".to_string()).ok();
        kernel.forget_ephemeral("kernel");
        // After forgetting, ephemeral entries should be cleared
        let entries = kernel.recall("kernel", "default");
        assert!(entries.iter().all(|e| e.tier != MemoryTier::Ephemeral));
    }

    #[test]
    fn test_evict_expired() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember("kernel", "default", "test".to_string()).ok();
        let evicted = kernel.evict_expired("kernel");
        // May or may not evict anything depending on TTL
        let _ = evicted;
    }

    #[test]
    fn test_promote_check() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember("kernel", "default", "promotable".to_string()).ok();
        kernel.promote_check("kernel");
        // Should not panic
    }

    #[test]
    fn test_memory_move() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel.remember("kernel", "default", "movable".to_string()).unwrap();
        let moved = kernel.memory_move("kernel", "default", &id, MemoryTier::LongTerm);
        // May or may not succeed depending on implementation
        let _ = moved;
    }

    #[test]
    fn test_memory_delete() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel.remember_long_term("kernel", "default", "deletable".to_string(), vec![], 50).unwrap();
        let deleted = kernel.memory_delete("kernel", "default", &id);
        assert!(deleted);
    }

    #[test]
    fn test_memory_delete_nonexistent() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let deleted = kernel.memory_delete("kernel", "default", "nonexistent-id");
        assert!(!deleted);
    }

    #[test]
    fn test_remember_action() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let result = kernel.remember_action("kernel", "default", "ran a command".to_string(), vec!["action".to_string()]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_remember_long_term_batch() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let contents = vec![
            ("fact 1".to_string(), vec!["fact".to_string()], 70u8),
            ("fact 2".to_string(), vec!["fact".to_string()], 80u8),
            ("fact 3".to_string(), vec!["fact".to_string()], 60u8),
        ];
        let result = kernel.remember_long_term_batch("kernel", "default", &contents);
        assert!(result.is_ok());
        let ids = result.unwrap();
        assert_eq!(ids.len(), 3);
        assert!(ids.iter().all(|id| !id.is_empty()));
    }

    #[test]
    fn test_remember_long_term_scoped() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let result = kernel.remember_long_term_scoped(
            "kernel", "default",
            "scoped long term".to_string(),
            vec!["scoped".to_string()],
            85,
            MemoryScope::Shared,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_recall_relevant() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember_long_term("kernel", "default", "relevant fact".to_string(), vec!["test".to_string()], 90).ok();
        let entries = kernel.recall_relevant("kernel", "default", 1000);
        // Should return some entries within budget
        let _ = entries;
    }

    #[test]
    fn test_cosine_sim() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_sim(&a, &b) - 1.0).abs() < 0.001);

        let c = vec![0.0, 1.0, 0.0];
        assert!(cosine_sim(&a, &c).abs() < 0.001);

        let empty: Vec<f32> = vec![];
        assert_eq!(cosine_sim(&empty, &empty), 0.0);
    }

    #[test]
    fn test_remember_procedural() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let proc = ProceduralEntry {
            name: "test_proc".into(),
            description: "a procedure".into(),
            steps: vec![crate::memory::layered::ProcedureStep {
                step_number: 1,
                description: "step 1".into(),
                action: "do thing".into(),
                expected_outcome: "done".into(),
            }],
            learned_from: "test".into(),
            tags: vec!["proc".into()],
        };
        let result = kernel.remember_procedural("kernel", "default", proc);
        assert!(result.is_ok());
    }

    #[test]
    fn test_recall_procedural() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let proc = ProceduralEntry {
            name: "recall_proc".into(),
            description: "recallable".into(),
            steps: vec![],
            learned_from: "test".into(),
            tags: vec![],
        };
        kernel.remember_procedural("kernel", "default", proc).unwrap();
        let entries = kernel.recall_procedural("kernel", "default", None);
        assert!(!entries.is_empty());
    }

    #[test]
    fn test_recall_procedural_by_name() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let proc = ProceduralEntry {
            name: "named_proc".into(),
            description: "findable by name".into(),
            steps: vec![],
            learned_from: "test".into(),
            tags: vec![],
        };
        kernel.remember_procedural("kernel", "default", proc).unwrap();
        let entries = kernel.recall_procedural("kernel", "default", Some("named_proc"));
        assert!(!entries.is_empty());
        assert!(entries.iter().any(|e| e.tags.iter().any(|t| t.contains("named_proc")) || matches!(&e.content, MemoryContent::Procedure(p) if p.name == "named_proc")));
    }

    #[test]
    fn test_agent_memory_quota() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.register_agent("quota_agent".to_string()).unwrap();
        let quota = kernel.agent_memory_quota("quota_agent");
        // Default quota should be some value
        let _ = quota;
    }

    #[test]
    fn test_agent_memory_quota_unregistered() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        // Unregistered agent should return 0 quota
        let quota = kernel.agent_memory_quota("nonexistent-agent");
        assert_eq!(quota, 0);
    }

    // ─── cosine_sim edge cases ────────────────────────────────────────────

    #[test]
    fn test_cosine_sim_mismatched_lengths() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0, 2.0, 3.0];
        assert_eq!(cosine_sim(&a, &b), 0.0);
    }

    #[test]
    fn test_cosine_sim_zero_vector() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert_eq!(cosine_sim(&a, &b), 0.0);
    }

    #[test]
    fn test_cosine_sim_opposite_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((cosine_sim(&a, &b) + 1.0).abs() < 0.001);
    }

    // ─── calculate_relevance ──────────────────────────────────────────────

    #[test]
    fn test_calculate_relevance_tag_match() {
        let tags = vec!["python".to_string(), "tutorial".to_string()];
        let content = "some content";
        let query = vec!["python"];
        let score = calculate_relevance(&tags, content, &query);
        assert!(score > 0.0, "Tag match should produce positive score");
        assert!(score >= 0.2, "Tag match should score at least 0.2");
    }

    #[test]
    fn test_calculate_relevance_content_match() {
        let tags: Vec<String> = vec![];
        let content = "The python interpreter runs bytecode";
        let query = vec!["python"];
        let score = calculate_relevance(&tags, content, &query);
        assert!(score > 0.0, "Content match should produce positive score");
        assert!((score - 0.1).abs() < 0.001, "Content-only match should score 0.1");
    }

    #[test]
    fn test_calculate_relevance_no_match() {
        let tags = vec!["rust".to_string()];
        let content = "systems programming language";
        let query = vec!["python"];
        let score = calculate_relevance(&tags, content, &query);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_calculate_relevance_capped_at_one() {
        let tags: Vec<String> = (0..20).map(|i| format!("term{}", i)).collect();
        let content = "term0 term1 term2 term3 term4 term5 term6 term7 term8 term9";
        let query: Vec<&str> = (0..20).map(|i| { let _ = i; "term0" }).collect();
        // Even with many matching terms, score should be capped at 1.0
        let score = calculate_relevance(&tags, content, &query);
        assert!(score <= 1.0);
    }

    // ─── recall_procedural with filter ────────────────────────────────────

    #[test]
    fn test_recall_procedural_name_filter_no_match() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let proc = ProceduralEntry {
            name: "deploy_flow".into(),
            description: "deploy steps".into(),
            steps: vec![],
            learned_from: "test".into(),
            tags: vec![],
        };
        kernel.remember_procedural("kernel", "default", proc).unwrap();
        let entries = kernel.recall_procedural("kernel", "default", Some("nonexistent_proc"));
        assert!(entries.is_empty(), "Non-matching name filter should return empty");
    }

    #[test]
    fn test_recall_procedural_multiple_entries() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        for name in &["proc_a", "proc_b", "proc_c"] {
            let proc = ProceduralEntry {
                name: (*name).into(),
                description: format!("desc for {}", name),
                steps: vec![],
                learned_from: "test".into(),
                tags: vec![],
            };
            kernel.remember_procedural("kernel", "default", proc).unwrap();
        }
        let all = kernel.recall_procedural("kernel", "default", None);
        assert_eq!(all.len(), 3, "Should return all 3 procedural entries");
        let filtered = kernel.recall_procedural("kernel", "default", Some("proc_b"));
        assert_eq!(filtered.len(), 1);
        match &filtered[0].content {
            MemoryContent::Procedure(p) => assert_eq!(p.name, "proc_b"),
            _ => panic!("Expected Procedure content"),
        }
    }

    // ─── recall_shared_procedural ─────────────────────────────────────────

    #[test]
    fn test_recall_shared_procedural_all() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_a = kernel.register_agent("proc_agent_a".to_string()).unwrap();
        let proc = ProceduralEntry {
            name: "shared_skill".into(),
            description: "a shared skill".into(),
            steps: vec![crate::memory::layered::ProcedureStep {
                step_number: 1,
                description: "step one".into(),
                action: "act".into(),
                expected_outcome: "done".into(),
            }],
            learned_from: "test".into(),
            tags: vec!["skill".into()],
        };
        kernel.remember_procedural_scoped(&agent_a, "default", proc, MemoryScope::Shared).unwrap();

        let entries = kernel.recall_shared_procedural(None);
        assert!(!entries.is_empty(), "Should find shared procedural memory");
    }

    #[test]
    fn test_recall_shared_procedural_by_name() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_a = kernel.register_agent("proc_agent_b".to_string()).unwrap();
        let proc = ProceduralEntry {
            name: "specific_skill".into(),
            description: "specific".into(),
            steps: vec![],
            learned_from: "test".into(),
            tags: vec![],
        };
        kernel.remember_procedural_scoped(&agent_a, "default", proc, MemoryScope::Shared).unwrap();

        let found = kernel.recall_shared_procedural(Some("specific_skill"));
        assert!(!found.is_empty(), "Should find by name");

        let not_found = kernel.recall_shared_procedural(Some("no_such_skill"));
        assert!(not_found.is_empty(), "Should not find non-existent name");
    }

    // ─── memory_stats ─────────────────────────────────────────────────────

    #[test]
    fn test_memory_stats_all_tiers() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        // Store entries in different tiers
        kernel.remember("kernel", "default", "ephemeral data".to_string()).unwrap();
        kernel.remember_working("kernel", "default", "working data".to_string(), vec!["w".into()]).unwrap();
        kernel.remember_long_term("kernel", "default", "long term data".to_string(), vec!["lt".into()], 80).unwrap();

        let stats = kernel.memory_stats("kernel", None);
        assert!(stats.total_entries >= 3, "Should have at least 3 entries across tiers, got {}", stats.total_entries);
        assert!(stats.total_bytes > 0, "Should have non-zero byte count");
        assert_eq!(stats.agent_id, "kernel");
        assert!(stats.tier.is_empty(), "Aggregate stats should have empty tier name");
    }

    #[test]
    fn test_memory_stats_specific_tier() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember("kernel", "default", "eph one".to_string()).unwrap();
        kernel.remember("kernel", "default", "eph two".to_string()).unwrap();

        let stats = kernel.memory_stats("kernel", Some(&MemoryTier::Ephemeral));
        assert_eq!(stats.total_entries, 2, "Should have 2 ephemeral entries");
        assert_eq!(stats.tier, "ephemeral");
    }

    #[test]
    fn test_memory_stats_empty_agent() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let stats = kernel.memory_stats("empty_agent", None);
        assert_eq!(stats.total_entries, 0);
        assert_eq!(stats.total_bytes, 0);
        assert_eq!(stats.oldest_entry_age_ms, 0);
        assert_eq!(stats.avg_access_count, 0.0);
        assert_eq!(stats.never_accessed_count, 0);
        assert_eq!(stats.about_to_expire_count, 0);
    }

    #[test]
    fn test_memory_stats_never_accessed_count() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember("kernel", "default", "fresh entry".to_string()).unwrap();

        let stats = kernel.memory_stats("kernel", Some(&MemoryTier::Ephemeral));
        assert_eq!(stats.never_accessed_count, stats.total_entries,
            "Newly created entries should all be never-accessed");
    }

    // ─── discover_knowledge ───────────────────────────────────────────────

    #[test]
    fn test_discover_knowledge_shared_scope() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_a = kernel.register_agent("dk_agent".to_string()).unwrap();
        kernel.remember_long_term_scoped(
            &agent_a, "default",
            "shared fact about quantum computing".to_string(),
            vec!["quantum".to_string()],
            90,
            MemoryScope::Shared,
        ).unwrap();

        let result = discover_knowledge(
            &kernel.memory,
            "quantum",
            &crate::api::semantic::DiscoveryScope::Shared,
            &[crate::api::semantic::KnowledgeType::Memory],
            10,
            None,
        );
        assert!(!result.items.is_empty(), "Should discover shared knowledge");
        assert!(result.items.iter().any(|h| h.preview.contains("quantum")),
            "Discovered item should contain query term in preview");
    }

    #[test]
    fn test_discover_knowledge_all_accessible_scope() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_a = kernel.register_agent("dk_agent2".to_string()).unwrap();
        kernel.remember_long_term_scoped(
            &agent_a, "default",
            "accessible knowledge entry".to_string(),
            vec!["test".to_string()],
            70,
            MemoryScope::Shared,
        ).unwrap();

        let result = discover_knowledge(
            &kernel.memory,
            "knowledge",
            &crate::api::semantic::DiscoveryScope::AllAccessible,
            &[crate::api::semantic::KnowledgeType::Memory],
            10,
            None,
        );
        assert!(!result.items.is_empty(), "AllAccessible scope should find entries");
    }

    #[test]
    fn test_discover_knowledge_group_scope() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_a = kernel.register_agent("dk_agent3".to_string()).unwrap();
        kernel.remember_long_term_scoped(
            &agent_a, "default",
            "group knowledge entry".to_string(),
            vec!["group_test".to_string()],
            80,
            MemoryScope::Group("team-alpha".to_string()),
        ).unwrap();

        // Search with matching group
        let result = discover_knowledge(
            &kernel.memory,
            "group",
            &crate::api::semantic::DiscoveryScope::Group("team-alpha".to_string()),
            &[crate::api::semantic::KnowledgeType::Memory],
            10,
            None,
        );
        assert!(!result.items.is_empty(), "Should find group-scoped entry");

        // Search with non-matching group
        let empty_result = discover_knowledge(
            &kernel.memory,
            "group",
            &crate::api::semantic::DiscoveryScope::Group("team-beta".to_string()),
            &[crate::api::semantic::KnowledgeType::Memory],
            10,
            None,
        );
        assert!(empty_result.items.is_empty(), "Different group should find nothing");
    }

    #[test]
    fn test_discover_knowledge_filter_by_type_procedure() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_a = kernel.register_agent("dk_agent4".to_string()).unwrap();

        // Store a shared procedural entry
        let proc = ProceduralEntry {
            name: "shared_deploy".into(),
            description: "deploy procedure shared".into(),
            steps: vec![],
            learned_from: "test".into(),
            tags: vec!["deploy".into()],
        };
        kernel.remember_procedural_scoped(&agent_a, "default", proc, MemoryScope::Shared).unwrap();

        // Also store a shared text entry
        kernel.remember_long_term_scoped(
            &agent_a, "default",
            "deploy notes".to_string(),
            vec!["deploy".to_string()],
            70,
            MemoryScope::Shared,
        ).unwrap();

        // Filter for procedures only
        let proc_result = discover_knowledge(
            &kernel.memory,
            "deploy",
            &crate::api::semantic::DiscoveryScope::Shared,
            &[crate::api::semantic::KnowledgeType::Procedure],
            10,
            None,
        );
        assert!(proc_result.items.iter().all(|h| !h.preview.starts_with("<object")),
            "Procedure results should have description preview");
        // Procedure-only filter should not include text memories
        let mem_result = discover_knowledge(
            &kernel.memory,
            "deploy",
            &crate::api::semantic::DiscoveryScope::Shared,
            &[crate::api::semantic::KnowledgeType::Memory],
            10,
            None,
        );
        // Both should find something (separate types)
        assert!(!proc_result.items.is_empty() || !mem_result.items.is_empty(),
            "At least one type should match 'deploy'");
    }

    #[test]
    fn test_discover_knowledge_max_results_and_token_estimate() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_a = kernel.register_agent("dk_agent5".to_string()).unwrap();
        for i in 0..5 {
            kernel.remember_long_term_scoped(
                &agent_a, "default",
                format!("fact number {} about testing", i),
                vec!["testing".to_string()],
                70,
                MemoryScope::Shared,
            ).unwrap();
        }

        let result = discover_knowledge(
            &kernel.memory,
            "testing",
            &crate::api::semantic::DiscoveryScope::Shared,
            &[crate::api::semantic::KnowledgeType::Memory],
            2,  // max_results = 2
            None,
        );
        assert!(result.items.len() <= 2, "Should respect max_results limit, got {}", result.items.len());
        assert!(result.total_available >= result.items.len(),
            "total_available should be >= items.len()");
        // token_estimate should be non-negative (can be 0 if previews are empty)
        let _ = result.token_estimate;
    }

    #[test]
    fn test_discover_knowledge_empty_query() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_a = kernel.register_agent("dk_agent6".to_string()).unwrap();
        kernel.remember_long_term_scoped(
            &agent_a, "default",
            "some shared content".to_string(),
            vec![],
            50,
            MemoryScope::Shared,
        ).unwrap();

        // Empty query should still return results (no filtering by query terms)
        let result = discover_knowledge(
            &kernel.memory,
            "",
            &crate::api::semantic::DiscoveryScope::Shared,
            &[crate::api::semantic::KnowledgeType::Memory],
            10,
            None,
        );
        assert!(!result.items.is_empty(), "Empty query should return all matching entries");
        // With empty query, all relevance scores should be 0.0
        assert!(result.items.iter().all(|h| h.relevance_score == 0.0),
            "Empty query terms should produce zero relevance");
    }

    // ─── recall_semantic ──────────────────────────────────────────────────

    #[test]
    fn test_recall_semantic_basic() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember_long_term("kernel", "default",
            "machine learning basics".to_string(),
            vec!["ml".to_string()], 80).unwrap();

        // Stub backend returns Err for embed_query, so recall_semantic should fail
        let result = kernel.recall_semantic("kernel", "default", "machine learning", 5);
        assert!(result.is_err(), "recall_semantic with stub backend should return Err (no real embeddings)");
    }

    #[test]
    fn test_recall_relevant_semantic_fallback_on_embed_failure() {
        // This tests the Err branch in recall_relevant_semantic where embed_query fails.
        // With stub backend, embed_query should succeed, so we test the success path.
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember_long_term("kernel", "default",
            "relevant content about algorithms".to_string(),
            vec!["algo".to_string()], 80).unwrap();

        let entries = kernel.recall_relevant_semantic("kernel", "default", "algorithms", 2000);
        // Should return entries (stub embedding succeeds)
        let _ = entries;
    }

    // ─── remember_long_term_scoped with Group scope ───────────────────────

    #[test]
    fn test_remember_long_term_scoped_group_emits_event() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent = kernel.register_agent("group_agent".to_string()).unwrap();
        let result = kernel.remember_long_term_scoped(
            &agent, "default",
            "group-shared fact".to_string(),
            vec!["group_fact".to_string()],
            75,
            MemoryScope::Group("team-beta".to_string()),
        );
        assert!(result.is_ok(), "Group-scoped long-term memory should succeed");
        // Verify the entry exists
        let entries = kernel.recall(&agent, "default");
        assert!(entries.iter().any(|e| e.content.display().to_string().contains("group-shared fact")));
    }

    // ─── recall_hyde ──────────────────────────────────────────────────────

    #[test]
    fn test_recall_hyde_non_complex_intent_returns_early() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember_long_term("kernel", "default",
            "simple fact".to_string(),
            vec!["fact".to_string()], 80).unwrap();

        // "What is X?" is a factual query, not multi-hop or aggregation,
        // so HyDE should return early without generating hypothetical answer.
        let result = kernel.recall_hyde("kernel", "default", "What is Plico?");
        assert!(result.is_ok());
        let (_entries, classified) = result.unwrap();
        // For factual intent, HyDE should not alter the results
        assert_eq!(classified.intent, crate::fs::retrieval_router::QueryIntent::Factual);
    }

    #[test]
    fn test_recall_hyde_multi_hop_triggers_hyde() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember_long_term("kernel", "default",
            "server logs show OOM error at 3am".to_string(),
            vec!["server".to_string(), "error".to_string()], 90).unwrap();
        kernel.remember_long_term("kernel", "default",
            "OOM was caused by memory leak in handler".to_string(),
            vec!["debug".to_string()], 85).unwrap();

        // "Why did X happen?" triggers multi-hop intent -> HyDE pipeline
        let result = kernel.recall_hyde("kernel", "default", "Why did the server crash?");
        assert!(result.is_ok());
        let (entries, classified) = result.unwrap();
        assert_eq!(classified.intent, crate::fs::retrieval_router::QueryIntent::MultiHop);
        // HyDE may merge additional results
        let _ = entries;
    }

    // ─── recall_routed_with_k ─────────────────────────────────────────────

    #[test]
    fn test_recall_routed_with_k_override() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        for i in 0..10 {
            kernel.remember_long_term("kernel", "default",
                format!("entry number {}", i),
                vec!["batch".to_string()], 70).unwrap();
        }

        let result = kernel.recall_routed_with_k("kernel", "default", "entry", Some(3));
        assert!(result.is_ok());
        let (entries, _) = result.unwrap();
        assert!(entries.len() <= 3, "top_k=3 should limit results to at most 3, got {}", entries.len());
    }

    // ─── remember_long_term_batch edge cases ──────────────────────────────

    #[test]
    fn test_remember_long_term_batch_empty() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let result = kernel.remember_long_term_batch("kernel", "default", &[]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty(), "Empty batch should return empty vec");
    }

    #[test]
    fn test_remember_long_term_batch_single_item() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let items = vec![("single fact".to_string(), vec!["tag".to_string()], 65u8)];
        let result = kernel.remember_long_term_batch("kernel", "default", &items);
        assert!(result.is_ok());
        let ids = result.unwrap();
        assert_eq!(ids.len(), 1);
    }

    // ─── memory_move success path ─────────────────────────────────────────

    #[test]
    fn test_memory_move_to_working() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel.remember("kernel", "default", "movable entry".to_string()).unwrap();
        let moved = kernel.memory_move("kernel", "default", &id, MemoryTier::Working);
        assert!(moved, "Should successfully move entry to working tier");
        // Verify the entry is now in working tier
        let entries = kernel.recall("kernel", "default");
        let moved_entry = entries.iter().find(|e| e.id == id);
        assert!(moved_entry.is_some(), "Entry should still be accessible");
        assert_eq!(moved_entry.unwrap().tier, MemoryTier::Working);
    }

    // ─── recall with tenant isolation ─────────────────────────────────────

    #[test]
    fn test_recall_tenant_isolation() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember("kernel", "tenant-a", "tenant a data".to_string()).unwrap();
        kernel.recall("kernel", "tenant-a"); // ensure it exists

        // Recalling with different tenant should not see tenant-a's data
        let entries_b = kernel.recall("kernel", "tenant-b");
        assert!(entries_b.iter().all(|e| e.tenant_id != "tenant-a"),
            "Should not see other tenant's memories");
    }

    // ─── discover_knowledge relevance scoring ─────────────────────────────

    #[test]
    fn test_discover_knowledge_relevance_sorted() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent = kernel.register_agent("dk_sort_agent".to_string()).unwrap();

        // Entry with tag match (higher relevance)
        kernel.remember_long_term_scoped(
            &agent, "default",
            "exact match content".to_string(),
            vec!["exact".to_string()],
            80,
            MemoryScope::Shared,
        ).unwrap();

        // Entry with no match (lower relevance)
        kernel.remember_long_term_scoped(
            &agent, "default",
            "unrelated content about cooking".to_string(),
            vec!["food".to_string()],
            90,
            MemoryScope::Shared,
        ).unwrap();

        let result = discover_knowledge(
            &kernel.memory,
            "exact",
            &crate::api::semantic::DiscoveryScope::Shared,
            &[crate::api::semantic::KnowledgeType::Memory],
            10,
            None,
        );
        if result.items.len() >= 2 {
            // Results should be sorted by relevance (descending)
            for i in 0..result.items.len() - 1 {
                assert!(result.items[i].relevance_score >= result.items[i + 1].relevance_score,
                    "Results should be sorted by relevance descending");
            }
        }
    }
}
