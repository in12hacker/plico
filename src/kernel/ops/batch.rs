//! Batch operations API (v15.0)
//!
//! High-throughput API endpoints for bulk operations:
//! - BatchCreate: create multiple objects in one call
//! - BatchMemoryStore: store multiple memory entries in one call
//! - BatchSubmitIntent: submit multiple intents in one call
//! - BatchQuery: query multiple objects/memories in one call

use crate::api::semantic::{
    BatchCreateItem, BatchCreateResponse, BatchMemoryEntry, BatchMemoryStoreResponse,
    BatchQueryResponse, BatchSubmitIntentResponse, ContentEncoding, IntentSpec, QuerySpec,
};
use crate::scheduler::IntentPriority;
use super::observability::{OpType, OperationTimer};

impl crate::kernel::AIKernel {
    /// Handle batch create operation.
    /// Processes items in parallel using existing semantic_create.
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

        let mut results = Vec::with_capacity(items.len());
        let mut successful = 0usize;
        let mut failed = 0usize;

        for item in items {
            let result = (|| {
                let bytes = decode_content(&item.content, &item.content_encoding)
                    .map_err(|e| e.to_string())?;
                self.semantic_create(bytes, item.tags, agent_id, item.intent)
                    .map_err(|e| e.to_string())
            })();

            match &result {
                Ok(_) => successful += 1,
                Err(_) => failed += 1,
            }
            results.push(result);
        }

        self.maybe_persist_search_index();

        BatchCreateResponse { results, successful, failed }
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

        let mut results = Vec::with_capacity(entries.len());
        let mut successful = 0usize;
        let mut failed = 0usize;

        for entry in entries {
            let result = self
                .remember_working_scoped(
                    agent_id,
                    tenant_id,
                    entry.content,
                    entry.tags,
                    crate::memory::MemoryScope::Private,
                )
                .map_err(|e| e.to_string());

            match &result {
                Ok(_) => successful += 1,
                Err(_) => failed += 1,
            }
            results.push(result.map(|_| String::new()));
        }

        BatchMemoryStoreResponse { results, successful, failed }
    }

    /// Handle batch submit intent operation.
    pub fn handle_batch_submit_intent(
        &self,
        intents: Vec<IntentSpec>,
        agent_id: &str,
    ) -> BatchSubmitIntentResponse {
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

            let result =
                self.submit_intent(priority, spec.description, spec.action, Some(agent_id.to_string()))
                    .map_err(|e| e.to_string());

            match &result {
                Ok(_) => successful += 1,
                Err(_) => failed += 1,
            }
            results.push(result);
        }

        BatchSubmitIntentResponse { results, successful, failed }
    }

    /// Handle batch query operation.
    pub fn handle_batch_query(
        &self,
        queries: Vec<QuerySpec>,
        agent_id: &str,
        tenant_id: &str,
    ) -> BatchQueryResponse {
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
                QuerySpec::Read { cid } => {
                    match self.get_object(&cid, agent_id, tenant_id) {
                        Ok(obj) => Ok(serde_json::json!({
                            "cid": cid,
                            "content": String::from_utf8_lossy(&obj.data).to_string(),
                            "tags": obj.meta.tags,
                        })),
                        Err(e) => Err(e.to_string()),
                    }
                }
                QuerySpec::Search { query, limit, require_tags, exclude_tags } => {
                    let results_vec = self.semantic_search_with_time(
                        &query,
                        agent_id,
                        tenant_id,
                        limit.unwrap_or(10),
                        require_tags,
                        exclude_tags,
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
                QuerySpec::RecallSemantic { query, k } => {
                    match self.recall_semantic(agent_id, tenant_id, &query, k) {
                        Ok(entries) => {
                            let memories: Vec<String> = entries
                                .into_iter()
                                .filter_map(|m| match m.content {
                                    crate::memory::MemoryContent::Text(t) => Some(t),
                                    _ => None,
                                })
                                .collect();
                            Ok(serde_json::json!({ "memories": memories }))
                        }
                        Err(e) => Err(e),
                    }
                }
            };

            match &result {
                Ok(_) => successful += 1,
                Err(_) => failed += 1,
            }
            results.push(result);
        }

        BatchQueryResponse { results, successful, failed }
    }
}

fn decode_content(content: &str, encoding: &ContentEncoding) -> Result<Vec<u8>, String> {
    crate::api::semantic::decode_content(content, encoding)
}
