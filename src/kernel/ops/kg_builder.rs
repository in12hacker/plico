//! Async KG Builder — automatic entity/event extraction on CAS writes.
//!
//! Spawns a background worker that receives write notifications via a channel,
//! batches them, and calls the LLM to extract SPO triples. Extracted entities
//! and events are inserted into the knowledge graph. A periodic temporal linking
//! pass connects recent Event nodes with `Follows` edges by `created_at` order.
//!
//! Controlled by env vars:
//! - `PLICO_KG_AUTO_EXTRACT=0` — disable (default: enabled)
//! - `PLICO_KG_EXTRACT_BATCH_SIZE` — batch size before flush (default: 5)
//! - `PLICO_KG_EXTRACT_TIMEOUT_MS` — max wait before flush (default: 3000)

use std::sync::Arc;

use crate::fs::embedding::EmbeddingProvider;
use crate::fs::{KnowledgeGraph, KGNode, KGNodeType, KGEdge, KGEdgeType};
use crate::llm::{LlmProvider, ChatMessage, ChatOptions};
use crate::kernel::ops::entity_resolver::EntityResolver;

/// A write event sent from the CAS create path to the KG builder worker.
#[derive(Debug, Clone)]
pub struct WriteEvent {
    pub cid: String,
    pub text: String,
    pub agent_id: String,
    pub created_at: u64,
    pub tags: Vec<String>,
}

/// Extracted preference from LLM output.
#[derive(Debug, Clone, serde::Deserialize)]
struct ExtractedPreference {
    category: String,
    preference: String,
    #[serde(default = "default_confidence")]
    confidence: f32,
}

fn default_confidence() -> f32 { 0.7 }

/// Extracted SPO triple from LLM output.
#[derive(Debug, Clone, serde::Deserialize)]
struct Triple {
    subject: String,
    predicate: String,
    object: String,
    #[serde(default)]
    #[serde(rename = "type")]
    relation_type: Option<String>,
}

/// Configuration for the KG builder worker.
pub struct KgBuilderConfig {
    pub enabled: bool,
    pub batch_size: usize,
    pub timeout_ms: u64,
}

impl KgBuilderConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("PLICO_KG_AUTO_EXTRACT")
                .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
                .unwrap_or(true),
            batch_size: std::env::var("PLICO_KG_EXTRACT_BATCH_SIZE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            timeout_ms: std::env::var("PLICO_KG_EXTRACT_TIMEOUT_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3000),
        }
    }
}

/// Channel-based KG builder handle. Clone-friendly for distributing across threads.
pub struct KgBuilderHandle {
    tx: Option<std::sync::mpsc::SyncSender<WriteEvent>>,
    join_handle: Option<std::thread::JoinHandle<()>>,
}

impl KgBuilderHandle {
    /// Send a write event to the KG builder worker (non-blocking best-effort).
    pub fn notify(&self, event: WriteEvent) {
        if let Some(ref tx) = self.tx {
            let _ = tx.try_send(event);
        }
    }
}

impl Drop for KgBuilderHandle {
    fn drop(&mut self) {
        // Close the channel first so the worker thread exits its loop.
        drop(self.tx.take());
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Start the KG builder background worker. Returns a handle for sending events.
///
/// The worker runs on a dedicated thread (not async) to avoid blocking the Tokio runtime.
/// If `profile_store` is provided, the worker also extracts user preferences.
pub fn start_kg_builder(
    kg: Arc<dyn KnowledgeGraph>,
    llm: Arc<dyn LlmProvider>,
    event_bus: Arc<crate::kernel::event_bus::EventBus>,
    config: KgBuilderConfig,
    embedder: Option<Arc<dyn EmbeddingProvider>>,
) -> KgBuilderHandle {
    let (tx, rx) = std::sync::mpsc::sync_channel::<WriteEvent>(256);

    let handle = std::thread::Builder::new()
        .name("kg-builder".to_string())
        .spawn(move || {
            kg_builder_loop(rx, kg, llm, event_bus, config, embedder);
        })
        .expect("failed to spawn kg-builder thread");

    KgBuilderHandle { tx: Some(tx), join_handle: Some(handle) }
}

fn kg_builder_loop(
    rx: std::sync::mpsc::Receiver<WriteEvent>,
    kg: Arc<dyn KnowledgeGraph>,
    llm: Arc<dyn LlmProvider>,
    event_bus: Arc<crate::kernel::event_bus::EventBus>,
    config: KgBuilderConfig,
    embedder: Option<Arc<dyn EmbeddingProvider>>,
) {
    let timeout = std::time::Duration::from_millis(config.timeout_ms);
    let mut batch: Vec<WriteEvent> = Vec::with_capacity(config.batch_size);

    loop {
        let event = if batch.is_empty() {
            match rx.recv() {
                Ok(e) => Some(e),
                Err(_) => break, // channel closed
            }
        } else {
            match rx.recv_timeout(timeout) {
                Ok(e) => Some(e),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    if !batch.is_empty() {
                        process_batch(&batch, &kg, &llm, &event_bus, &embedder);
                    }
                    break;
                }
            }
        };

        let received = event.is_some();
        if let Some(e) = event {
            batch.push(e);
        }

        if batch.len() >= config.batch_size || (!batch.is_empty() && !received) {
            process_batch(&batch, &kg, &llm, &event_bus, &embedder);
            batch.clear();
        }
    }
}

fn process_batch(
    batch: &[WriteEvent],
    kg: &Arc<dyn KnowledgeGraph>,
    llm: &Arc<dyn LlmProvider>,
    event_bus: &Arc<crate::kernel::event_bus::EventBus>,
    embedder: &Option<Arc<dyn EmbeddingProvider>>,
) {
    let resolver = embedder.as_ref().map(|e| EntityResolver::new(kg.clone(), e.clone(), 0.85));

    for event in batch {
        if event.text.trim().is_empty() || event.text.len() < 20 {
            continue;
        }
        match extract_and_insert(event, kg, llm, event_bus, &resolver) {
            Ok(preferences) => {
                for pref in &preferences {
                    tracing::debug!(
                        "Preference extracted for agent={}: [{}] {} (conf={:.2})",
                        event.agent_id, pref.category, pref.preference, pref.confidence
                    );
                }
            }
            Err(e) => {
                tracing::warn!("KG extraction failed for CID={}: {e}", &event.cid[..8.min(event.cid.len())]);
            }
        }
    }

    if let Err(e) = temporal_link_pass(kg) {
        tracing::debug!("temporal link pass error: {e}");
    }
}

const EXTRACT_PROMPT: &str = r#"Extract entities, relationships, and user preferences from the following text.
Output ONLY valid JSON with two arrays: "triples" and "preferences".
Format: {"triples": [{"subject":"...","predicate":"...","object":"...","type":"..."}], "preferences": [{"category":"...","preference":"...","confidence":0.8}]}
Valid triple types: causes, follows, mentions, part_of, related_to, has_participant, has_fact.
Preference categories: topic, style, tool, language, domain, format.
Rules:
- lowercase all values
- use concise predicates (1-3 words)
- replace pronouns with entity names
- extract temporal events with "follows" or "causes" relations
- extract preferences only when the user clearly states or implies a preference

Text: "#;

fn extract_and_insert(
    event: &WriteEvent,
    kg: &Arc<dyn KnowledgeGraph>,
    llm: &Arc<dyn LlmProvider>,
    event_bus: &crate::kernel::event_bus::EventBus,
    resolver: &Option<EntityResolver>,
) -> Result<Vec<ExtractedPreference>, Box<dyn std::error::Error>> {
    let truncated = if event.text.len() > 2000 {
        &event.text[..2000]
    } else {
        &event.text
    };

    let prompt = format!("{EXTRACT_PROMPT}{truncated}");
    let messages = vec![
        ChatMessage::system("You are a knowledge graph and preference extraction engine. Output only valid JSON."),
        ChatMessage::user(prompt),
    ];
    let opts = ChatOptions { temperature: 0.0, max_tokens: Some(1024) };

    let (response, _input_tokens, _output_tokens) = llm.chat(&messages, &opts)?;

    let extraction = parse_extraction(&response);
    tracing::info!(cid = %crate::util::safe_truncate(&event.cid, 8), triples = extraction.triples.len(), prefs = extraction.preferences.len(), "KG extraction complete");
    if extraction.triples.is_empty() && extraction.preferences.is_empty() {
        return Ok(Vec::new());
    }

    let triples = extraction.triples;

    let tenant = "default".to_string();

    // First, create a Document node for this CID if not already present
    let doc_node_id = format!("doc:{}", &event.cid);
    if kg.get_node(&doc_node_id)?.is_none() {
        let mut doc_node = KGNode::with_content(
            event.cid[..12.min(event.cid.len())].to_string(),
            KGNodeType::Document,
            event.cid.clone(),
            event.agent_id.clone(),
            tenant.clone(),
        );
        doc_node.id = doc_node_id.clone();
        kg.add_node(doc_node)?;
    }

    for triple in &triples {
        let edge_type = map_relation_type(triple.relation_type.as_deref(), &triple.predicate);
        let src_node_type = if edge_type == KGEdgeType::Follows || edge_type == KGEdgeType::Causes {
            KGNodeType::Event
        } else {
            KGNodeType::Entity
        };
        let dst_node_type = src_node_type;

        // F-37: Active Entity Linking (Cross-Session)
        // Tier 1+2: Exact match + semantic resolution via EntityResolver
        let (src_id, src_emb) = if let Some(ref resolver) = resolver {
            match resolver.resolve(&triple.subject, src_node_type, &event.agent_id) {
                Ok(result) => {
                    let id = result.resolved_id.unwrap_or_else(|| format!("ent:{}", triple.subject));
                    (id, Some(result.embedding))
                }
                Err(e) => {
                    tracing::debug!("Entity resolver error for '{}': {}", triple.subject, e);
                    (format!("ent:{}", triple.subject), None)
                }
            }
        } else {
            // Fallback: exact match only (no embedder available)
            let id = kg.list_nodes(&event.agent_id, Some(src_node_type))
                .ok()
                .and_then(|nodes| nodes.iter().find(|n| n.label.eq_ignore_ascii_case(&triple.subject)).map(|n| n.id.clone()))
                .unwrap_or_else(|| format!("ent:{}", triple.subject));
            (id, None)
        };
        let (dst_id, dst_emb) = if let Some(ref resolver) = resolver {
            match resolver.resolve(&triple.object, dst_node_type, &event.agent_id) {
                Ok(result) => {
                    let id = result.resolved_id.unwrap_or_else(|| format!("ent:{}", triple.object));
                    (id, Some(result.embedding))
                }
                Err(e) => {
                    tracing::debug!("Entity resolver error for '{}': {}", triple.object, e);
                    (format!("ent:{}", triple.object), None)
                }
            }
        } else {
            let id = kg.list_nodes(&event.agent_id, Some(dst_node_type))
                .ok()
                .and_then(|nodes| nodes.iter().find(|n| n.label.eq_ignore_ascii_case(&triple.object)).map(|n| n.id.clone()))
                .unwrap_or_else(|| format!("ent:{}", triple.object));
            (id, None)
        };

        if kg.get_node(&src_id)?.is_none() {
            let mut node = KGNode::new(
                triple.subject.clone(),
                src_node_type,
                event.agent_id.clone(),
                tenant.clone(),
            );
            node.id = src_id.clone();
            node.valid_at = Some(event.created_at);
            // Store embedding for future entity resolution
            if let Some(ref emb) = src_emb {
                let emb_json: Vec<serde_json::Value> = emb.iter().map(|v| serde_json::json!(*v)).collect();
                node.properties["embedding"] = serde_json::Value::Array(emb_json);
            }
            kg.add_node(node)?;
        }

        if kg.get_node(&dst_id)?.is_none() {
            let mut node = KGNode::new(
                triple.object.clone(),
                dst_node_type,
                event.agent_id.clone(),
                tenant.clone(),
            );
            node.id = dst_id.clone();
            node.valid_at = Some(event.created_at);
            // Store embedding for future entity resolution
            if let Some(ref emb) = dst_emb {
                let emb_json: Vec<serde_json::Value> = emb.iter().map(|v| serde_json::json!(*v)).collect();
                node.properties["embedding"] = serde_json::Value::Array(emb_json);
            }
            kg.add_node(node)?;
        }

        // Tier 3: Link resolved entities — create IsAliasOf edge if entity matched a different node
        if let Some(ref resolver) = resolver {
            if let Some(ref emb) = src_emb {
                let expected_id = format!("ent:{}", triple.subject);
                if src_id != expected_id {
                    let _ = resolver.link_and_store(
                        &expected_id, &triple.subject, &src_id, emb,
                        &event.agent_id,
                    );
                }
            }
            if let Some(ref emb) = dst_emb {
                let expected_id = format!("ent:{}", triple.object);
                if dst_id != expected_id {
                    let _ = resolver.link_and_store(
                        &expected_id, &triple.object, &dst_id, emb,
                        &event.agent_id,
                    );
                }
            }
        }

        let mut edge = KGEdge::new_with_episode(
            src_id.clone(),
            dst_id.clone(),
            edge_type,
            0.8,
            event.cid.clone(),
        );

        // F-37: If predicate is "is" or "alias", also add IsAliasOf edge
        if triple.predicate.to_lowercase() == "is" || triple.predicate.to_lowercase() == "alias" {
            edge.edge_type = KGEdgeType::IsAliasOf;
        }

        // F-37: Temporal Consolidation & Conflict Detection
        if let Ok(existing_edges) = kg.list_edges(&event.agent_id) {
            for mut old_edge in existing_edges {
                if old_edge.src == src_id && old_edge.edge_type == edge.edge_type && old_edge.dst != dst_id && old_edge.invalid_at.is_none() {
                    tracing::info!(src = %src_id, old = %old_edge.dst, new = %dst_id, type = ?edge.edge_type, "Cognitive conflict/update detected");
                    
                    // Invalidate old fact (Temporal Consolidation)
                    old_edge.invalid_at = Some(event.created_at);
                    let _ = kg.add_edge(old_edge.clone());

                    // Emit diagnostic event
                    event_bus.emit(crate::kernel::event_bus::KernelEvent::VerificationFailed {
                        tool_name: "KgBuilder".into(),
                        operation: "ConflictDetection".into(),
                        reason: format!("Entity {} has conflicting {:?} targets: {} vs {}", src_id, edge.edge_type, old_edge.dst, dst_id),
                        agent_id: event.agent_id.clone(),
                    });
                }
            }
        }

        let _ = kg.add_edge(edge);

        // Link both entities to the source document
        let _ = kg.add_edge(KGEdge::new_with_episode(
            doc_node_id.clone(),
            src_id,
            KGEdgeType::Mentions,
            0.7,
            event.cid.clone(),
        ));
        let _ = kg.add_edge(KGEdge::new_with_episode(
            doc_node_id.clone(),
            dst_id,
            KGEdgeType::Mentions,
            0.7,
            event.cid.clone(),
        ));
    }

    tracing::debug!(
        "KG extracted {} triples, {} preferences from CID={}",
        triples.len(),
        extraction.preferences.len(),
        &event.cid[..8.min(event.cid.len())]
    );
    Ok(extraction.preferences)
}

/// Temporal linking pass: connect recent Event nodes by `created_at` with `Follows` edges.
fn temporal_link_pass(kg: &Arc<dyn KnowledgeGraph>) -> Result<(), Box<dyn std::error::Error>> {
    let events = kg.list_nodes("", Some(KGNodeType::Event))?;
    if events.len() < 2 {
        return Ok(());
    }

    let mut active_events: Vec<&KGNode> = events.iter().filter(|n| n.is_active()).collect();
    active_events.sort_by_key(|n| n.valid_at.unwrap_or(n.created_at));

    // Only link the most recent window to avoid O(N^2) on the full graph
    let window = active_events.len().min(20);
    let recent = &active_events[active_events.len() - window..];

    for pair in recent.windows(2) {
        let prev = pair[0];
        let next = pair[1];

        let existing = kg.get_valid_edge_between(
            &prev.id,
            &next.id,
            Some(KGEdgeType::Follows),
            next.created_at,
        )?;
        if existing.is_none() {
            let edge = KGEdge::new(
                prev.id.clone(),
                next.id.clone(),
                KGEdgeType::Follows,
                0.9,
            );
            let _ = kg.add_edge(edge);
        }
    }
    Ok(())
}

/// Combined extraction result from LLM.
#[derive(Debug, serde::Deserialize, Default)]
struct ExtractionResult {
    #[serde(default)]
    triples: Vec<Triple>,
    #[serde(default)]
    preferences: Vec<ExtractedPreference>,
}

fn parse_extraction(response: &str) -> ExtractionResult {
    let trimmed = response.trim();

    // Try parsing as the new combined format: look for top-level `{"triples":...}`
    // Only try if the JSON structure starts with `{` before any `[`
    let first_brace = trimmed.find('{');
    let first_bracket = trimmed.find('[');
    let is_object_first = match (first_brace, first_bracket) {
        (Some(b), Some(a)) => b < a,
        (Some(_), None) => true,
        _ => false,
    };

    if is_object_first {
        if let (Some(start), Some(end)) = (first_brace, trimmed.rfind('}')) {
            if let Ok(result) = serde_json::from_str::<ExtractionResult>(&trimmed[start..=end]) {
                return result;
            }
        }
    }

    // Fallback: try parsing as a plain array of triples (backward compatible)
    let triples = match (first_bracket, trimmed.rfind(']')) {
        (Some(s), Some(e)) if e > s => {
            serde_json::from_str::<Vec<Triple>>(&trimmed[s..=e]).unwrap_or_default()
        }
        _ => Vec::new(),
    };

    ExtractionResult { triples, preferences: Vec::new() }
}

fn map_relation_type(type_hint: Option<&str>, predicate: &str) -> KGEdgeType {
    let key = type_hint.unwrap_or(predicate).to_lowercase();
    match key.as_str() {
        "causes" | "caused" | "caused_by" => KGEdgeType::Causes,
        "follows" | "followed" | "after" | "then" | "next" => KGEdgeType::Follows,
        "mentions" | "references" | "refers_to" => KGEdgeType::Mentions,
        "part_of" | "belongs_to" | "member_of" => KGEdgeType::PartOf,
        "has_participant" | "involves" | "participated" => KGEdgeType::HasParticipant,
        "has_fact" | "states" | "asserts" => KGEdgeType::HasFact,
        "depends_on" | "requires" | "needs" => KGEdgeType::DependsOn,
        "produces" | "creates" | "generates" => KGEdgeType::Produces,
        "similar_to" | "resembles" => KGEdgeType::SimilarTo,
        _ => KGEdgeType::RelatedTo,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_extraction_combined_format() {
        let input = r#"{"triples":[{"subject":"alice","predicate":"met","object":"bob","type":"related_to"}],"preferences":[{"category":"topic","preference":"rust programming","confidence":0.9}]}"#;
        let result = parse_extraction(input);
        assert_eq!(result.triples.len(), 1);
        assert_eq!(result.triples[0].subject, "alice");
        assert_eq!(result.preferences.len(), 1);
        assert_eq!(result.preferences[0].category, "topic");
    }

    #[test]
    fn test_parse_extraction_array_fallback() {
        let input = "Here are the triples:\n[{\"subject\":\"a\",\"predicate\":\"b\",\"object\":\"c\"}]";
        let result = parse_extraction(input);
        assert_eq!(result.triples.len(), 1);
        assert!(result.preferences.is_empty());
    }

    #[test]
    fn test_parse_extraction_invalid() {
        let result = parse_extraction("not json at all");
        assert!(result.triples.is_empty());
        assert!(result.preferences.is_empty());
    }

    #[test]
    fn test_map_relation_type() {
        assert_eq!(map_relation_type(Some("causes"), ""), KGEdgeType::Causes);
        assert_eq!(map_relation_type(Some("follows"), ""), KGEdgeType::Follows);
        assert_eq!(map_relation_type(None, "mentions"), KGEdgeType::Mentions);
        assert_eq!(map_relation_type(None, "unknown_pred"), KGEdgeType::RelatedTo);
    }

    #[test]
    fn test_kg_builder_config_defaults() {
        let config = KgBuilderConfig::from_env();
        assert!(config.enabled); // defaults to true (PLICO_KG_AUTO_EXTRACT not set)
        assert_eq!(config.batch_size, 5);
        assert_eq!(config.timeout_ms, 3000);
    }

    // ── Test helpers ──────────────────────────────────────────────────────────

    use crate::fs::graph::PetgraphBackend;
    use crate::llm::stub::StubProvider;
    use crate::kernel::event_bus::EventBus;

    fn make_test_kg() -> Arc<dyn KnowledgeGraph> {
        Arc::new(PetgraphBackend::open(std::env::temp_dir().join(format!(
            "plico_test_kg_builder_{}",
            std::process::id()
        ))))
    }

    fn make_event_bus() -> Arc<EventBus> {
        Arc::new(EventBus::new())
    }

    fn make_write_event(cid: &str, text: &str) -> WriteEvent {
        WriteEvent {
            cid: cid.to_string(),
            text: text.to_string(),
            agent_id: "test_agent".to_string(),
            created_at: 1000,
            tags: vec!["test".to_string()],
        }
    }

    fn llm_response_with_triples() -> String {
        r#"{"triples":[{"subject":"rust","predicate":"is","object":"programming language","type":"related_to"},{"subject":"rust","predicate":"used_for","object":"systems programming","type":"related_to"}],"preferences":[{"category":"topic","preference":"rust programming","confidence":0.9}]}"#.to_string()
    }

    fn llm_response_empty() -> String {
        r#"{"triples":[],"preferences":[]}"#.to_string()
    }

    // ── map_relation_type tests ───────────────────────────────────────────────

    #[test]
    fn test_map_relation_type_all_variants() {
        // Causes group
        assert_eq!(map_relation_type(Some("causes"), ""), KGEdgeType::Causes);
        assert_eq!(map_relation_type(Some("caused"), ""), KGEdgeType::Causes);
        assert_eq!(map_relation_type(Some("caused_by"), ""), KGEdgeType::Causes);

        // Follows group
        assert_eq!(map_relation_type(Some("follows"), ""), KGEdgeType::Follows);
        assert_eq!(map_relation_type(Some("followed"), ""), KGEdgeType::Follows);
        assert_eq!(map_relation_type(Some("after"), ""), KGEdgeType::Follows);
        assert_eq!(map_relation_type(Some("then"), ""), KGEdgeType::Follows);
        assert_eq!(map_relation_type(Some("next"), ""), KGEdgeType::Follows);

        // Mentions group
        assert_eq!(map_relation_type(Some("mentions"), ""), KGEdgeType::Mentions);
        assert_eq!(map_relation_type(Some("references"), ""), KGEdgeType::Mentions);
        assert_eq!(map_relation_type(Some("refers_to"), ""), KGEdgeType::Mentions);

        // PartOf group
        assert_eq!(map_relation_type(Some("part_of"), ""), KGEdgeType::PartOf);
        assert_eq!(map_relation_type(Some("belongs_to"), ""), KGEdgeType::PartOf);
        assert_eq!(map_relation_type(Some("member_of"), ""), KGEdgeType::PartOf);

        // HasParticipant group
        assert_eq!(map_relation_type(Some("has_participant"), ""), KGEdgeType::HasParticipant);
        assert_eq!(map_relation_type(Some("involves"), ""), KGEdgeType::HasParticipant);
        assert_eq!(map_relation_type(Some("participated"), ""), KGEdgeType::HasParticipant);

        // HasFact group
        assert_eq!(map_relation_type(Some("has_fact"), ""), KGEdgeType::HasFact);
        assert_eq!(map_relation_type(Some("states"), ""), KGEdgeType::HasFact);
        assert_eq!(map_relation_type(Some("asserts"), ""), KGEdgeType::HasFact);

        // DependsOn group
        assert_eq!(map_relation_type(Some("depends_on"), ""), KGEdgeType::DependsOn);
        assert_eq!(map_relation_type(Some("requires"), ""), KGEdgeType::DependsOn);
        assert_eq!(map_relation_type(Some("needs"), ""), KGEdgeType::DependsOn);

        // Produces group
        assert_eq!(map_relation_type(Some("produces"), ""), KGEdgeType::Produces);
        assert_eq!(map_relation_type(Some("creates"), ""), KGEdgeType::Produces);
        assert_eq!(map_relation_type(Some("generates"), ""), KGEdgeType::Produces);

        // SimilarTo group
        assert_eq!(map_relation_type(Some("similar_to"), ""), KGEdgeType::SimilarTo);
        assert_eq!(map_relation_type(Some("resembles"), ""), KGEdgeType::SimilarTo);

        // Predicate-based fallback (no type hint)
        assert_eq!(map_relation_type(None, "causes"), KGEdgeType::Causes);
        assert_eq!(map_relation_type(None, "follows"), KGEdgeType::Follows);

        // Unknown → RelatedTo
        assert_eq!(map_relation_type(Some("unknown_type"), ""), KGEdgeType::RelatedTo);
        assert_eq!(map_relation_type(None, "some_random_pred"), KGEdgeType::RelatedTo);
    }

    #[test]
    fn test_map_relation_type_case_insensitive() {
        assert_eq!(map_relation_type(Some("CAUSES"), ""), KGEdgeType::Causes);
        assert_eq!(map_relation_type(Some("Follows"), ""), KGEdgeType::Follows);
        assert_eq!(map_relation_type(None, "MENTIONS"), KGEdgeType::Mentions);
    }

    // ── parse_extraction tests ────────────────────────────────────────────────

    #[test]
    fn test_parse_extraction_empty_string() {
        let result = parse_extraction("");
        assert!(result.triples.is_empty());
        assert!(result.preferences.is_empty());
    }

    #[test]
    fn test_parse_extraction_whitespace_only() {
        let result = parse_extraction("   \n\t  ");
        assert!(result.triples.is_empty());
        assert!(result.preferences.is_empty());
    }

    #[test]
    fn test_parse_extraction_object_with_no_triples_key() {
        // JSON object but without "triples" key — should use defaults
        let input = r#"{"other_key": "value"}"#;
        let result = parse_extraction(input);
        assert!(result.triples.is_empty());
        assert!(result.preferences.is_empty());
    }

    #[test]
    fn test_parse_extraction_array_only_triples() {
        let input = r#"[{"subject":"a","predicate":"b","object":"c","type":"causes"}]"#;
        let result = parse_extraction(input);
        assert_eq!(result.triples.len(), 1);
        assert_eq!(result.triples[0].subject, "a");
        assert!(result.preferences.is_empty());
    }

    #[test]
    fn test_parse_extraction_combined_with_multiple_triples() {
        let input = r#"{
            "triples": [
                {"subject":"a","predicate":"is","object":"b","type":"related_to"},
                {"subject":"b","predicate":"causes","object":"c","type":"causes"},
                {"subject":"c","predicate":"follows","object":"d"}
            ],
            "preferences": [
                {"category":"topic","preference":"testing","confidence":0.8},
                {"category":"style","preference":"concise","confidence":0.6}
            ]
        }"#;
        let result = parse_extraction(input);
        assert_eq!(result.triples.len(), 3);
        assert_eq!(result.preferences.len(), 2);
        assert_eq!(result.preferences[1].category, "style");
    }

    #[test]
    fn test_parse_extraction_pref_with_default_confidence() {
        let input = r#"{"triples":[],"preferences":[{"category":"topic","preference":"rust"}]}"#;
        let result = parse_extraction(input);
        assert_eq!(result.preferences.len(), 1);
        assert!((result.preferences[0].confidence - 0.7).abs() < f32::EPSILON);
    }

    // ── WriteEvent tests ──────────────────────────────────────────────────────

    #[test]
    fn test_write_event_creation() {
        let event = make_write_event("abc123", "some text content here for testing");
        assert_eq!(event.cid, "abc123");
        assert_eq!(event.text, "some text content here for testing");
        assert_eq!(event.agent_id, "test_agent");
        assert_eq!(event.created_at, 1000);
        assert_eq!(event.tags, vec!["test"]);
    }

    #[test]
    fn test_write_event_clone() {
        let event = make_write_event("cid1", "text");
        let cloned = event.clone();
        assert_eq!(cloned.cid, event.cid);
        assert_eq!(cloned.text, event.text);
    }

    // ── extract_and_insert tests ──────────────────────────────────────────────

    #[test]
    fn test_extract_and_insert_creates_doc_node() {
        let kg = make_test_kg();
        let llm: Arc<dyn LlmProvider> = Arc::new(StubProvider::new(llm_response_with_triples()));
        let event_bus = make_event_bus();
        let event = make_write_event("abcdef1234567890", "This is a long enough text for extraction testing purposes here.");

        let result = extract_and_insert(&event, &kg, &llm, &event_bus, &None);
        assert!(result.is_ok());

        // Document node should be created
        let doc_node = kg.get_node("doc:abcdef1234567890").unwrap();
        assert!(doc_node.is_some(), "Document node should be created");
        let doc = doc_node.unwrap();
        assert_eq!(doc.node_type, KGNodeType::Document);
        assert_eq!(doc.content_cid, Some("abcdef1234567890".to_string()));
    }

    #[test]
    fn test_extract_and_insert_creates_entity_nodes() {
        let kg = make_test_kg();
        let llm: Arc<dyn LlmProvider> = Arc::new(StubProvider::new(llm_response_with_triples()));
        let event_bus = make_event_bus();
        let event = make_write_event("cid_entity_test_001", "This is a long enough text for entity extraction testing purposes here.");

        extract_and_insert(&event, &kg, &llm, &event_bus, &None).unwrap();

        // Entity nodes should be created for "rust" and "programming language"
        let rust_node = kg.get_node("ent:rust").unwrap();
        assert!(rust_node.is_some(), "Entity node 'rust' should be created");

        let lang_node = kg.get_node("ent:programming language").unwrap();
        assert!(lang_node.is_some(), "Entity node 'programming language' should be created");
    }

    #[test]
    fn test_extract_and_insert_creates_edges() {
        let kg = make_test_kg();
        let llm: Arc<dyn LlmProvider> = Arc::new(StubProvider::new(llm_response_with_triples()));
        let event_bus = make_event_bus();
        let event = make_write_event("cid_edge_test_0001", "This is a long enough text for edge extraction testing purposes here.");

        extract_and_insert(&event, &kg, &llm, &event_bus, &None).unwrap();

        // Edges should exist
        let edges = kg.list_edges("test_agent").unwrap();
        assert!(!edges.is_empty(), "Edges should be created");

        // Should have the triple edge and Mentions edges to doc node
        let mentions_edges: Vec<_> = edges.iter().filter(|e| e.edge_type == KGEdgeType::Mentions).collect();
        assert!(!mentions_edges.is_empty(), "Mentions edges to doc node should be created");
    }

    #[test]
    fn test_extract_and_insert_empty_llm_response() {
        let kg = make_test_kg();
        let llm: Arc<dyn LlmProvider> = Arc::new(StubProvider::new(llm_response_empty()));
        let event_bus = make_event_bus();
        let event = make_write_event("cid_empty_resp_001", "This is a long enough text for empty response testing purposes here.");

        let result = extract_and_insert(&event, &kg, &llm, &event_bus, &None);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty(), "No preferences from empty response");
    }

    #[test]
    fn test_extract_and_insert_alias_predicate() {
        let kg = make_test_kg();
        let response = r#"{"triples":[{"subject":"alice","predicate":"is","object":"bob","type":"related_to"}],"preferences":[]}"#;
        let llm: Arc<dyn LlmProvider> = Arc::new(StubProvider::new(response.to_string()));
        let event_bus = make_event_bus();
        let event = make_write_event("cid_alias_pred_001", "This is a long enough text for alias predicate testing purposes here.");

        extract_and_insert(&event, &kg, &llm, &event_bus, &None).unwrap();

        // The "is" predicate should be converted to IsAliasOf edge type
        let edges = kg.list_edges("test_agent").unwrap();
        let alias_edges: Vec<_> = edges.iter()
            .filter(|e| e.edge_type == KGEdgeType::IsAliasOf)
            .collect();
        assert!(!alias_edges.is_empty(), "Predicate 'is' should produce IsAliasOf edge");
    }

    #[test]
    fn test_extract_and_insert_alias_keyword_predicate() {
        let kg = make_test_kg();
        let response = r#"{"triples":[{"subject":"x","predicate":"alias","object":"y","type":"related_to"}],"preferences":[]}"#;
        let llm: Arc<dyn LlmProvider> = Arc::new(StubProvider::new(response.to_string()));
        let event_bus = make_event_bus();
        let event = make_write_event("cid_alias_kw_0001", "This is a long enough text for alias keyword testing purposes here.");

        extract_and_insert(&event, &kg, &llm, &event_bus, &None).unwrap();

        let edges = kg.list_edges("test_agent").unwrap();
        let alias_edges: Vec<_> = edges.iter()
            .filter(|e| e.edge_type == KGEdgeType::IsAliasOf)
            .collect();
        assert!(!alias_edges.is_empty(), "Predicate 'alias' should produce IsAliasOf edge");
    }

    #[test]
    fn test_extract_and_insert_returns_preferences() {
        let kg = make_test_kg();
        let llm: Arc<dyn LlmProvider> = Arc::new(StubProvider::new(llm_response_with_triples()));
        let event_bus = make_event_bus();
        let event = make_write_event("cid_prefs_000001", "This is a long enough text for preferences extraction testing purposes here.");

        let prefs = extract_and_insert(&event, &kg, &llm, &event_bus, &None).unwrap();
        assert_eq!(prefs.len(), 1);
        assert_eq!(prefs[0].category, "topic");
        assert_eq!(prefs[0].preference, "rust programming");
    }

    #[test]
    fn test_extract_and_insert_llm_error() {
        let kg = make_test_kg();
        // StubProvider with empty string won't error, so let's test that path
        // The function returns Ok even with empty extraction
        let llm: Arc<dyn LlmProvider> = Arc::new(StubProvider::new("not valid json at all".to_string()));
        let event_bus = make_event_bus();
        let event = make_write_event("cid_llm_err_0001", "This is a long enough text for LLM error testing purposes here.");

        let result = extract_and_insert(&event, &kg, &llm, &event_bus, &None);
        assert!(result.is_ok(), "Should handle invalid LLM response gracefully");
    }

    #[test]
    fn test_extract_and_insert_truncates_long_text() {
        let kg = make_test_kg();
        let llm: Arc<dyn LlmProvider> = Arc::new(StubProvider::new(llm_response_empty()));
        let event_bus = make_event_bus();
        // Create text longer than 2000 chars
        let long_text = "a".repeat(3000);
        let event = make_write_event("cid_trunc_00001", &long_text);

        let result = extract_and_insert(&event, &kg, &llm, &event_bus, &None);
        assert!(result.is_ok(), "Should handle long text by truncating");
    }

    #[test]
    fn test_extract_and_insert_conflict_detection() {
        let kg = make_test_kg();
        let event_bus = make_event_bus();

        // First extraction: rust → related_to → python
        let response1 = r#"{"triples":[{"subject":"rust","predicate":"related_to","object":"python","type":"related_to"}],"preferences":[]}"#;
        let llm: Arc<dyn LlmProvider> = Arc::new(StubProvider::new(response1.to_string()));
        let event1 = make_write_event("cid_conflict_001", "This is a long enough text for conflict detection testing purposes here.");
        extract_and_insert(&event1, &kg, &llm, &event_bus, &None).unwrap();

        // Second extraction: rust → related_to → go (same src + edge_type, different dst)
        let response2 = r#"{"triples":[{"subject":"rust","predicate":"related_to","object":"go","type":"related_to"}],"preferences":[]}"#;
        let llm2: Arc<dyn LlmProvider> = Arc::new(StubProvider::new(response2.to_string()));
        let event2 = WriteEvent {
            cid: "cid_conflict_002".to_string(),
            text: "This is a long enough text for conflict detection testing purposes here.".to_string(),
            agent_id: "test_agent".to_string(),
            created_at: 2000,
            tags: vec!["test".to_string()],
        };
        extract_and_insert(&event2, &kg, &llm2, &event_bus, &None).unwrap();

        // The old edge should be invalidated
        let edges = kg.list_edges("test_agent").unwrap();
        let old_edges: Vec<_> = edges.iter()
            .filter(|e| e.src == "ent:rust" && e.edge_type == KGEdgeType::RelatedTo && e.dst == "ent:python")
            .collect();
        // The old edge should have invalid_at set
        for edge in &old_edges {
            assert!(edge.invalid_at.is_some(), "Old edge should be invalidated after conflict");
        }
    }

    // ── temporal_link_pass tests ──────────────────────────────────────────────

    #[test]
    fn test_temporal_link_pass_no_events() {
        let kg = make_test_kg();
        let result = temporal_link_pass(&kg);
        assert!(result.is_ok());
    }

    #[test]
    fn test_temporal_link_pass_single_event() {
        let kg = make_test_kg();
        let mut node = KGNode::new("event1".into(), KGNodeType::Event, "agent1".into(), "default".into());
        node.id = "evt:1".into();
        node.valid_at = Some(1000);
        kg.add_node(node).unwrap();

        let result = temporal_link_pass(&kg);
        assert!(result.is_ok());
        // Only 1 event, so no Follows edges should be created
        let edges = kg.list_edges("agent1").unwrap();
        let follows: Vec<_> = edges.iter().filter(|e| e.edge_type == KGEdgeType::Follows).collect();
        assert!(follows.is_empty(), "No Follows edges with only 1 event");
    }

    #[test]
    fn test_temporal_link_pass_two_events() {
        let kg = make_test_kg();

        // temporal_link_pass queries with empty agent_id, so nodes must use ""
        let mut node1 = KGNode::new("event1".into(), KGNodeType::Event, "".into(), "default".into());
        node1.id = "evt:1".into();
        node1.valid_at = Some(1000);
        kg.add_node(node1).unwrap();

        let mut node2 = KGNode::new("event2".into(), KGNodeType::Event, "".into(), "default".into());
        node2.id = "evt:2".into();
        node2.valid_at = Some(2000);
        kg.add_node(node2).unwrap();

        temporal_link_pass(&kg).unwrap();

        // Should create a Follows edge from evt:1 → evt:2
        let edges = kg.list_edges("").unwrap();
        let follows: Vec<_> = edges.iter()
            .filter(|e| e.edge_type == KGEdgeType::Follows && e.src == "evt:1" && e.dst == "evt:2")
            .collect();
        assert_eq!(follows.len(), 1, "Should create Follows edge between consecutive events");
    }

    #[test]
    fn test_temporal_link_pass_skips_existing_follows() {
        let kg = make_test_kg();

        let mut node1 = KGNode::new("event1".into(), KGNodeType::Event, "".into(), "default".into());
        node1.id = "evt:s1".into();
        node1.valid_at = Some(1000);
        kg.add_node(node1).unwrap();

        let mut node2 = KGNode::new("event2".into(), KGNodeType::Event, "".into(), "default".into());
        node2.id = "evt:s2".into();
        node2.valid_at = Some(2000);
        kg.add_node(node2).unwrap();

        // Pre-create the Follows edge with valid_at set to match next.created_at
        let mut existing = KGEdge::new("evt:s1".into(), "evt:s2".into(), KGEdgeType::Follows, 0.9);
        // Set valid_at to match what temporal_link_pass will check (next.created_at)
        let next_node = kg.get_node("evt:s2").unwrap().unwrap();
        existing.valid_at = Some(next_node.created_at);
        kg.add_edge(existing).unwrap();

        temporal_link_pass(&kg).unwrap();

        // Should still only have one Follows edge (not duplicated)
        let edges = kg.list_edges("").unwrap();
        let follows: Vec<_> = edges.iter()
            .filter(|e| e.edge_type == KGEdgeType::Follows && e.src == "evt:s1" && e.dst == "evt:s2")
            .collect();
        assert_eq!(follows.len(), 1, "Should not duplicate existing Follows edge");
    }

    #[test]
    fn test_temporal_link_pass_orders_by_valid_at() {
        let kg = make_test_kg();

        // Insert events out of order; use empty agent_id to match temporal_link_pass query
        let mut node2 = KGNode::new("event_b".into(), KGNodeType::Event, "".into(), "default".into());
        node2.id = "evt:o2".into();
        node2.valid_at = Some(2000);
        kg.add_node(node2).unwrap();

        let mut node1 = KGNode::new("event_a".into(), KGNodeType::Event, "".into(), "default".into());
        node1.id = "evt:o1".into();
        node1.valid_at = Some(1000);
        kg.add_node(node1).unwrap();

        let mut node3 = KGNode::new("event_c".into(), KGNodeType::Event, "".into(), "default".into());
        node3.id = "evt:o3".into();
        node3.valid_at = Some(3000);
        kg.add_node(node3).unwrap();

        temporal_link_pass(&kg).unwrap();

        let edges = kg.list_edges("").unwrap();
        let follows: Vec<_> = edges.iter()
            .filter(|e| e.edge_type == KGEdgeType::Follows)
            .collect();

        // Should link o1→o2 and o2→o3 (sorted by valid_at)
        let o1_o2 = follows.iter().any(|e| e.src == "evt:o1" && e.dst == "evt:o2");
        let o2_o3 = follows.iter().any(|e| e.src == "evt:o2" && e.dst == "evt:o3");
        assert!(o1_o2, "Should link o1→o2");
        assert!(o2_o3, "Should link o2→o3");
    }

    // ── process_batch tests ───────────────────────────────────────────────────

    #[test]
    fn test_process_batch_filters_short_text() {
        let kg = make_test_kg();
        let llm: Arc<dyn LlmProvider> = Arc::new(StubProvider::new(llm_response_with_triples()));
        let event_bus = make_event_bus();

        // Text shorter than 20 chars should be skipped
        let short_event = make_write_event("cid_short_001", "short");
        let batch = vec![short_event];

        process_batch(&batch, &kg, &llm, &event_bus, &None);

        // No nodes should be created for short text
        let nodes = kg.list_nodes("test_agent", None).unwrap();
        assert!(nodes.is_empty(), "Short text events should be skipped");
    }

    #[test]
    fn test_process_batch_filters_empty_text() {
        let kg = make_test_kg();
        let llm: Arc<dyn LlmProvider> = Arc::new(StubProvider::new(llm_response_with_triples()));
        let event_bus = make_event_bus();

        let empty_event = make_write_event("cid_empty_txt1", "   ");
        let batch = vec![empty_event];

        process_batch(&batch, &kg, &llm, &event_bus, &None);

        let nodes = kg.list_nodes("test_agent", None).unwrap();
        assert!(nodes.is_empty(), "Empty text events should be skipped");
    }

    #[test]
    fn test_process_batch_processes_valid_events() {
        let kg = make_test_kg();
        let llm: Arc<dyn LlmProvider> = Arc::new(StubProvider::new(llm_response_with_triples()));
        let event_bus = make_event_bus();

        let event = make_write_event("cid_batch_valid1", "This is a long enough text for batch processing testing purposes here.");
        let batch = vec![event];

        process_batch(&batch, &kg, &llm, &event_bus, &None);

        // Nodes should be created
        let nodes = kg.list_nodes("test_agent", None).unwrap();
        assert!(!nodes.is_empty(), "Valid events should be processed");
    }

    // ── KgBuilderHandle tests ─────────────────────────────────────────────────

    #[test]
    fn test_kg_builder_handle_notify_disabled() {
        // Handle with None tx should be a no-op
        let handle = KgBuilderHandle { tx: None, join_handle: None };
        let event = make_write_event("cid_notify_001", "text");
        handle.notify(event); // should not panic
    }

    // ── ExtractionResult default tests ────────────────────────────────────────

    #[test]
    fn test_extraction_result_default() {
        let result = ExtractionResult::default();
        assert!(result.triples.is_empty());
        assert!(result.preferences.is_empty());
    }

    #[test]
    fn test_extracted_preference_default_confidence() {
        let conf = default_confidence();
        assert!((conf - 0.7).abs() < f32::EPSILON);
    }
}
