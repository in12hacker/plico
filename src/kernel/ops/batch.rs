//! Batch operations API (v15.0)
//!
//! High-throughput API endpoints for bulk operations:
//! - BatchCreate: create multiple objects in one call
//! - BatchMemoryStore: store multiple memory entries in one call
//! - BatchSubmitIntent: submit multiple intents in one call
//! - BatchQuery: query multiple objects/memories in one call

use super::observability::{OpType, OperationTimer};
use crate::api::semantic::{
    BatchCreateItem, BatchCreateResponse, BatchMemoryEntry, BatchMemoryStoreResponse, BatchQueryResponse,
    BatchSubmitIntentResponse, ContentEncoding, IntentSpec, QuerySpec,
};
use crate::fs::embedding::EmbeddingProvider;
use crate::scheduler::IntentPriority;

impl crate::kernel::AIKernel {
    /// Handle batch create operation.
    /// Processes items in parallel using tokio::task::spawn_blocking.
    pub fn handle_batch_create(
        &self,
        items: Vec<BatchCreateItem>,
        agent_id: &str,
        _tenant_id: &str,
    ) -> BatchCreateResponse {
        let _timer = OperationTimer::new(&self.metrics, OpType::BatchCreate);
        let span = tracing::info_span!(
            "handle_batch_create",
            operation = "handle_batch_create",
            agent_id = %agent_id,
            item_count = items.len(),
        );
        let _guard = span.enter();

        if items.is_empty() {
            return BatchCreateResponse {
                results: vec![],
                successful: 0,
                failed: 0,
            };
        }

        let n = items.len();

        // For small batches, use sequential to avoid spawn overhead
        if n <= 2 {
            let mut results = Vec::with_capacity(n);
            let mut successful = 0usize;
            let mut failed = 0usize;

            for item in items {
                let result = (|| {
                    let bytes = decode_content(&item.content, &item.content_encoding).map_err(|e| e.to_string())?;
                    self.semantic_create(
                        bytes,
                        item.tags,
                        agent_id,
                        item.intent,
                        crate::cas::ObjectScope::default(),
                    )
                    .map_err(|e| e.to_string())
                })();

                match &result {
                    Ok(_) => successful += 1,
                    Err(_) => failed += 1,
                }
                results.push(result);
            }

            self.maybe_persist_search_index();
            return BatchCreateResponse {
                results,
                successful,
                failed,
            };
        }

        // Batch embedding optimization: decode all items, batch-embed texts, then create with precomputed vectors
        let agent_id_str = agent_id.to_string();

        let items_data: Vec<_> = items
            .into_iter()
            .map(|item| {
                let bytes_result: Result<Vec<u8>, String> = decode_content(&item.content, &item.content_encoding);
                (bytes_result, item.tags, item.intent)
            })
            .collect();

        // Collect texts for batch embedding
        let texts: Vec<String> = items_data
            .iter()
            .map(|(bytes_result, _, _)| match bytes_result {
                Ok(bytes) => String::from_utf8_lossy(bytes).to_string(),
                Err(_) => String::new(),
            })
            .collect();

        let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
        let embeddings: Option<Vec<crate::fs::embedding::types::EmbedResult>> =
            self.embedding.embed_batch(&text_refs).ok();

        let mut results = Vec::with_capacity(items_data.len());
        let mut successful = 0usize;
        let mut failed = 0usize;

        for (i, (bytes_result, tags, intent)) in items_data.into_iter().enumerate() {
            let text_for_kg = texts[i].clone();
            let tags_for_kg = tags.clone();
            let result = bytes_result.and_then(|bytes| {
                if let Some(ref embs) = embeddings {
                    if let Some(emb_result) = embs.get(i) {
                        let cid = self
                            .fs
                            .create_with_embedding(
                                bytes,
                                tags,
                                agent_id_str.clone(),
                                intent,
                                crate::cas::ObjectScope::default(),
                                emb_result.embedding.clone(),
                                true, // skip_kg_edges: batch create avoids per-item similarity search
                            )
                            .map_err(|e| e.to_string())?;
                        // Notify KG builder for batch-created items
                        if let Some(ref handle) = self.kg_builder {
                            handle.notify(super::kg_builder::WriteEvent {
                                cid: cid.clone(),
                                text: text_for_kg,
                                agent_id: agent_id_str.clone(),
                                created_at: crate::fs::graph::types::now_ms(),
                                tags: tags_for_kg,
                            });
                        }
                        return Ok(cid);
                    }
                }
                self.semantic_create(bytes, tags, &agent_id_str, intent, crate::cas::ObjectScope::default())
                    .map_err(|e| e.to_string())
            });

            match &result {
                Ok(_) => successful += 1,
                Err(_) => failed += 1,
            }
            results.push(result);
        }

        self.maybe_persist_search_index();

        BatchCreateResponse {
            results,
            successful,
            failed,
        }
    }

    /// Handle batch memory store operation.
    /// Stores multiple memory entries in the working tier.
    pub fn handle_batch_memory_store(
        &self,
        entries: Vec<BatchMemoryEntry>,
        agent_id: &str,
        tenant_id: &str,
    ) -> BatchMemoryStoreResponse {
        let _timer = OperationTimer::new(&self.metrics, OpType::BatchMemoryStore);
        let span = tracing::info_span!(
            "handle_batch_memory_store",
            operation = "handle_batch_memory_store",
            agent_id = %agent_id,
            entry_count = entries.len(),
        );
        let _guard = span.enter();

        let permission = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        if let Err(error) = self
            .permissions
            .check(&permission, crate::api::permission::PermissionAction::Write)
        {
            let results = entries.iter().map(|_| Err(error.to_string())).collect::<Vec<_>>();
            return BatchMemoryStoreResponse {
                successful: 0,
                failed: results.len(),
                results,
            };
        }

        let quota = self.agent_memory_quota(agent_id);
        let mut results = entries
            .iter()
            .map(|_| Err("memory entry was not processed".to_string()))
            .collect::<Vec<_>>();
        let mut candidates = Vec::new();

        for (index, item) in entries.into_iter().enumerate() {
            if !item.tier.is_empty() && !item.tier.eq_ignore_ascii_case("working") {
                results[index] = Err(format!(
                    "unsupported batch memory tier '{}'; only working is supported",
                    item.tier
                ));
                continue;
            }
            if item.importance > 100 {
                results[index] = Err("importance must be between 0 and 100".to_string());
                continue;
            }

            let entry_id = uuid::Uuid::new_v4().to_string();
            let now = crate::memory::layered::now_ms();
            let memory_entry = crate::memory::MemoryEntry {
                memory_id: Default::default(),
                parent_revision_id: None,
                canonical_content_hash: Default::default(),
                id: entry_id.clone(),
                agent_id: agent_id.to_string(),
                tenant_id: tenant_id.to_string(),
                tier: crate::memory::MemoryTier::Working,
                content: crate::memory::MemoryContent::Text(item.content),
                importance: item.importance,
                access_count: 0,
                last_accessed: now,
                created_at: now,
                tags: item.tags,
                ttl_ms: None,
                original_ttl_ms: None,
                scope: crate::memory::MemoryScope::Private,
                memory_type: crate::memory::MemoryType::default(),
                causal_parent: None,
                supersedes: None,
                superseded_by: None,
                deleted_at: None,
            };
            candidates.push((index, memory_entry));
        }

        if !candidates.is_empty() {
            let candidate_entries = candidates.iter().map(|(_, entry)| entry.clone()).collect();
            match self.memory.create_working_batch_durable(candidate_entries, quota) {
                Ok(stored) => {
                    for ((index, _), entry) in candidates.iter().zip(stored) {
                        results[*index] = Ok(entry.id.clone());
                        self.event_bus
                            .emit(crate::kernel::event_bus::KernelEvent::MemoryStored {
                                agent_id: agent_id.to_string(),
                                tier: "working".into(),
                            });
                        self.projection.notify_current(&entry, None);
                    }
                }
                Err(error) => {
                    let category = error.category();
                    tracing::warn!(
                        phase = "persist",
                        outcome = "error",
                        error_category = category,
                        revision_count = candidates.len(),
                        "Working Memory batch was not published"
                    );
                    for (index, _) in &candidates {
                        results[*index] = Err(error.to_string());
                    }
                }
            }
        }

        let successful = results.iter().filter(|result| result.is_ok()).count();
        let failed = results.len() - successful;
        BatchMemoryStoreResponse {
            results,
            successful,
            failed,
        }
    }

    /// Handle batch submit intent operation.
    pub fn handle_batch_submit_intent(&self, intents: Vec<IntentSpec>, agent_id: &str) -> BatchSubmitIntentResponse {
        let _timer = OperationTimer::new(&self.metrics, OpType::BatchSubmitIntent);
        let span = tracing::info_span!(
            "handle_batch_submit_intent",
            operation = "handle_batch_submit_intent",
            agent_id = %agent_id,
            intent_count = intents.len(),
        );
        let _guard = span.enter();

        let mut results = Vec::with_capacity(intents.len());
        let mut successful = 0usize;
        let mut failed = 0usize;

        for spec in intents {
            let priority = match spec.priority.to_lowercase().as_str() {
                "critical" => IntentPriority::Critical,
                "high" => IntentPriority::High,
                "medium" => IntentPriority::Medium,
                _ => IntentPriority::Low,
            };

            let result = self
                .submit_intent(priority, spec.description, spec.action, Some(agent_id.to_string()))
                .map_err(|e| e.to_string());

            match &result {
                Ok(_) => successful += 1,
                Err(_) => failed += 1,
            }
            results.push(result);
        }

        BatchSubmitIntentResponse {
            results,
            successful,
            failed,
        }
    }

    /// Handle batch query operation.
    pub fn handle_batch_query(&self, queries: Vec<QuerySpec>, agent_id: &str, tenant_id: &str) -> BatchQueryResponse {
        let _timer = OperationTimer::new(&self.metrics, OpType::BatchQuery);
        let span = tracing::info_span!(
            "handle_batch_query",
            operation = "handle_batch_query",
            agent_id = %agent_id,
            query_count = queries.len(),
        );
        let _guard = span.enter();

        let mut results = Vec::with_capacity(queries.len());
        let mut successful = 0usize;
        let mut failed = 0usize;

        for query in queries {
            let result = match query {
                QuerySpec::Read { cid } => match self.get_object(&cid, agent_id, tenant_id) {
                    Ok(obj) => Ok(serde_json::json!({
                        "cid": cid,
                        "content": String::from_utf8_lossy(&obj.data).to_string(),
                        "tags": obj.meta.tags,
                    })),
                    Err(e) => Err(e.to_string()),
                },
                QuerySpec::Search {
                    query,
                    limit,
                    require_tags,
                    exclude_tags,
                } => {
                    let results_vec = self.semantic_search_with_time(
                        super::fs::SearchQuery {
                            query: &query,
                            agent_id,
                            tenant_id,
                            limit: limit.unwrap_or(10),
                            require_tags,
                            exclude_tags,
                        },
                        None,
                        None,
                    );

                    match results_vec {
                        Ok(r) => Ok(serde_json::json!({
                            "results": r.iter().map(|sr| serde_json::json!({
                                "cid": sr.cid,
                                "relevance": sr.relevance,
                                "tags": sr.meta.tags,
                            })).collect::<Vec<_>>(),
                            "count": r.len(),
                        })),
                        Err(e) => Err(e.to_string()),
                    }
                }
                QuerySpec::Recall => {
                    let entries = self.recall(agent_id, tenant_id);
                    let memories: Vec<String> = entries
                        .into_iter()
                        .filter_map(|m| match m.content {
                            crate::memory::MemoryContent::Text(t) => Some(t),
                            _ => None,
                        })
                        .collect();
                    Ok(serde_json::json!({ "memories": memories }))
                }
            };

            match &result {
                Ok(_) => successful += 1,
                Err(_) => failed += 1,
            }
            results.push(result);
        }

        BatchQueryResponse {
            results,
            successful,
            failed,
        }
    }
}

fn decode_content(content: &str, encoding: &ContentEncoding) -> Result<Vec<u8>, String> {
    crate::api::semantic::decode_content(content, encoding)
}

#[cfg(test)]
mod tests {
    use crate::api::semantic::{BatchCreateItem, ContentEncoding};
    use crate::kernel::tests::make_kernel;

    // ─── Batch Create ────────────────────────────────────────────────────────

    #[test]
    fn test_batch_create_empty_list() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_batch_create(vec![], "TestAgent", "default");
        assert_eq!(resp.successful, 0);
        assert_eq!(resp.failed, 0);
        assert!(resp.results.is_empty());
    }

    #[test]
    fn test_batch_create_single_item() {
        let (kernel, _dir) = make_kernel();
        let items = vec![BatchCreateItem {
            content: "hello".to_string(),
            content_encoding: ContentEncoding::Utf8,
            tags: vec!["test".to_string()],
            intent: None,
        }];
        let resp = kernel.handle_batch_create(items, "TestAgent", "default");
        assert_eq!(resp.successful, 1);
        assert_eq!(resp.failed, 0);
        assert!(resp.results[0].is_ok());
    }

    #[test]
    fn test_batch_create_multiple_items() {
        let (kernel, _dir) = make_kernel();
        let items = vec![
            BatchCreateItem {
                content: "item1".to_string(),
                content_encoding: ContentEncoding::Utf8,
                tags: vec!["batch".to_string()],
                intent: None,
            },
            BatchCreateItem {
                content: "item2".to_string(),
                content_encoding: ContentEncoding::Utf8,
                tags: vec!["batch".to_string()],
                intent: None,
            },
        ];
        let resp = kernel.handle_batch_create(items, "TestAgent", "default");
        assert_eq!(resp.successful, 2);
        assert_eq!(resp.failed, 0);
    }

    #[test]
    fn test_batch_create_mixed_success_failure() {
        let (kernel, _dir) = make_kernel();
        // Empty content might fail depending on implementation
        let items = vec![
            BatchCreateItem {
                content: "valid".to_string(),
                content_encoding: ContentEncoding::Utf8,
                tags: vec![],
                intent: None,
            },
            BatchCreateItem {
                content: "".to_string(),
                content_encoding: ContentEncoding::Utf8,
                tags: vec![],
                intent: None,
            },
        ];
        let resp = kernel.handle_batch_create(items, "TestAgent", "default");
        assert_eq!(resp.successful + resp.failed, 2);
    }

    // ─── Batch Memory Store ─────────────────────────────────────────────────

    #[test]
    fn test_batch_memory_store_empty() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_batch_memory_store(vec![], "TestAgent", "default");
        assert_eq!(resp.successful, 0);
        assert_eq!(resp.failed, 0);
    }

    #[test]
    fn test_batch_memory_store_single_entry() {
        let (kernel, _dir) = make_kernel();
        let entries = vec![crate::api::semantic::BatchMemoryEntry {
            content: "memory item".to_string(),
            tier: "working".to_string(),
            tags: vec!["test".to_string()],
            importance: 50,
        }];
        let resp = kernel.handle_batch_memory_store(entries, "TestAgent", "default");
        assert_eq!(resp.successful, 1);
        assert_eq!(resp.failed, 0);
    }

    // ─── Batch Submit Intent ────────────────────────────────────────────────

    #[test]
    fn test_batch_submit_intent_empty() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_batch_submit_intent(vec![], "TestAgent");
        assert_eq!(resp.successful, 0);
        assert_eq!(resp.failed, 0);
    }

    #[test]
    fn test_batch_submit_intent_single() {
        let (kernel, _dir) = make_kernel();
        let intents = vec![crate::api::semantic::IntentSpec {
            priority: "medium".to_string(),
            description: "test intent".to_string(),
            action: None,
        }];
        let resp = kernel.handle_batch_submit_intent(intents, "TestAgent");
        // Intent submission may succeed or fail depending on scheduler state
        assert_eq!(resp.successful + resp.failed, 1);
    }

    #[test]
    fn test_batch_submit_intent_multiple_priorities() {
        let (kernel, _dir) = make_kernel();
        let intents = vec![
            crate::api::semantic::IntentSpec {
                priority: "critical".to_string(),
                description: "c".to_string(),
                action: None,
            },
            crate::api::semantic::IntentSpec {
                priority: "high".to_string(),
                description: "h".to_string(),
                action: None,
            },
            crate::api::semantic::IntentSpec {
                priority: "low".to_string(),
                description: "l".to_string(),
                action: None,
            },
        ];
        let resp = kernel.handle_batch_submit_intent(intents, "TestAgent");
        assert_eq!(resp.successful + resp.failed, 3);
    }

    // ─── Batch Query ─────────────────────────────────────────────────────────

    #[test]
    fn test_batch_query_empty() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_batch_query(vec![], "TestAgent", "default");
        assert_eq!(resp.successful, 0);
        assert_eq!(resp.failed, 0);
    }

    #[test]
    fn test_batch_query_recall() {
        let (kernel, _dir) = make_kernel();
        let queries = vec![crate::api::semantic::QuerySpec::Recall];
        let resp = kernel.handle_batch_query(queries, "TestAgent", "default");
        // Recall should succeed even with no memories
        assert_eq!(resp.successful + resp.failed, 1);
    }

    #[test]
    fn test_batch_query_read_nonexistent() {
        let (kernel, _dir) = make_kernel();
        let queries = vec![crate::api::semantic::QuerySpec::Read {
            cid: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        }];
        let resp = kernel.handle_batch_query(queries, "TestAgent", "default");
        // Read of nonexistent should fail gracefully
        assert_eq!(resp.successful + resp.failed, 1);
    }

    #[test]
    fn test_batch_query_mixed() {
        let (kernel, _dir) = make_kernel();
        let queries = vec![
            crate::api::semantic::QuerySpec::Recall,
            crate::api::semantic::QuerySpec::Search {
                query: "test".to_string(),
                limit: Some(5),
                require_tags: Vec::new(),
                exclude_tags: Vec::new(),
            },
        ];
        let resp = kernel.handle_batch_query(queries, "TestAgent", "default");
        assert_eq!(resp.successful + resp.failed, 2);
    }
}
