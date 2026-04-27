//! Async KG Builder — automatic entity/event extraction on CAS writes.
//!
//! Spawns a background worker that receives write notifications via a channel,
//! batches them, and calls the LLM to extract SPO triples. Extracted entities
//! and events are inserted into the knowledge graph. A periodic temporal linking
//! pass connects recent Event nodes with `Follows` edges by `created_at` order.
//!
//! Controlled by env vars:
//! - `PLICO_KG_AUTO_EXTRACT=1` — enable (default: disabled)
//! - `PLICO_KG_EXTRACT_BATCH_SIZE` — batch size before flush (default: 5)
//! - `PLICO_KG_EXTRACT_TIMEOUT_MS` — max wait before flush (default: 3000)

use std::sync::Arc;

use crate::fs::{KnowledgeGraph, KGNode, KGNodeType, KGEdge, KGEdgeType};
use crate::llm::{LlmProvider, ChatMessage, ChatOptions};

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
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
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
#[derive(Clone)]
pub struct KgBuilderHandle {
    tx: std::sync::mpsc::SyncSender<WriteEvent>,
}

impl KgBuilderHandle {
    /// Send a write event to the KG builder worker (non-blocking best-effort).
    pub fn notify(&self, event: WriteEvent) {
        let _ = self.tx.try_send(event);
    }
}

/// Start the KG builder background worker. Returns a handle for sending events.
///
/// The worker runs on a dedicated thread (not async) to avoid blocking the Tokio runtime.
/// If `profile_store` is provided, the worker also extracts user preferences.
pub fn start_kg_builder(
    kg: Arc<dyn KnowledgeGraph>,
    llm: Arc<dyn LlmProvider>,
    config: KgBuilderConfig,
) -> KgBuilderHandle {
    let (tx, rx) = std::sync::mpsc::sync_channel::<WriteEvent>(256);

    std::thread::Builder::new()
        .name("kg-builder".to_string())
        .spawn(move || {
            kg_builder_loop(rx, kg, llm, config);
        })
        .expect("failed to spawn kg-builder thread");

    KgBuilderHandle { tx }
}

fn kg_builder_loop(
    rx: std::sync::mpsc::Receiver<WriteEvent>,
    kg: Arc<dyn KnowledgeGraph>,
    llm: Arc<dyn LlmProvider>,
    config: KgBuilderConfig,
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
                        process_batch(&batch, &kg, &llm);
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
            process_batch(&batch, &kg, &llm);
            batch.clear();
        }
    }
}

fn process_batch(
    batch: &[WriteEvent],
    kg: &Arc<dyn KnowledgeGraph>,
    llm: &Arc<dyn LlmProvider>,
) {
    for event in batch {
        if event.text.trim().is_empty() || event.text.len() < 20 {
            continue;
        }
        match extract_and_insert(event, kg, llm) {
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
        let src_id = format!("ent:{}", triple.subject);
        let dst_id = format!("ent:{}", triple.object);

        let edge_type = map_relation_type(triple.relation_type.as_deref(), &triple.predicate);
        let src_node_type = if edge_type == KGEdgeType::Follows || edge_type == KGEdgeType::Causes {
            KGNodeType::Event
        } else {
            KGNodeType::Entity
        };
        let dst_node_type = src_node_type;

        if kg.get_node(&src_id)?.is_none() {
            let mut node = KGNode::new(
                triple.subject.clone(),
                src_node_type,
                event.agent_id.clone(),
                tenant.clone(),
            );
            node.id = src_id.clone();
            node.valid_at = Some(event.created_at);
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
            kg.add_node(node)?;
        }

        let edge = KGEdge::new_with_episode(
            src_id.clone(),
            dst_id.clone(),
            edge_type,
            0.8,
            event.cid.clone(),
        );
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
        assert!(!config.enabled);
        assert_eq!(config.batch_size, 5);
        assert_eq!(config.timeout_ms, 3000);
    }
}
