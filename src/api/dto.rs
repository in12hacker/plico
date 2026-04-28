//! API Data Transfer Objects — response payload types used by ApiResponse.
//!
//! All DTO structs are leaf types: they carry data between kernel and client
//! but contain no business logic. Extracted from `semantic.rs` so that
//! adding/modifying a DTO does not touch the protocol core (ApiRequest/ApiResponse).

use serde::{Deserialize, Serialize};

use crate::fs::{KGNodeType, KGEdgeType};

// Re-import serde default helpers from semantic.rs
use super::semantic::{default_importance, default_k, default_priority};

/// DTO for procedure steps in API requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcedureStepDto {
    pub description: String,
    pub action: String,
    #[serde(default)]
    pub expected_outcome: Option<String>,
}

/// Scope of knowledge discovery.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum DiscoveryScope {
    /// Search only entries with scope=Shared.
    Shared,
    /// Search only entries with scope=Group(id).
    Group(String),
    /// Search all accessible entries.
    #[default]
    AllAccessible,
}


/// Type of knowledge to filter by in discovery.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeType {
    /// Text memory entries.
    Memory,
    /// Procedural memory entries (learned skills/workflows).
    Procedure,
    /// Factual knowledge entries (KnowledgePiece).
    Knowledge,
}

/// An item within a BatchCreate request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCreateItem {
    /// Object content. Plain UTF-8 by default; set `content_encoding` for binary.
    pub content: String,
    /// Content encoding (default: utf8).
    #[serde(default)]
    pub content_encoding: super::semantic::ContentEncoding,
    /// Semantic tags for the object.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Optional intent description associated with this object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
}

/// An entry within a BatchMemoryStore request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchMemoryEntry {
    /// Memory content (text).
    pub content: String,
    /// Memory tier to store in (default: working).
    #[serde(default)]
    pub tier: String,
    /// Importance score 0-100 (default: 50).
    #[serde(default = "default_importance")]
    pub importance: u8,
    /// Semantic tags.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// An item within a RememberLongTermBatch request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchLongTermItem {
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_importance")]
    pub importance: u8,
}

/// An intent specification within a BatchSubmitIntent request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentSpec {
    /// Natural-language intent description.
    pub description: String,
    /// Priority: "critical", "high", "medium", or "low" (default: medium).
    #[serde(default = "default_priority")]
    pub priority: String,
    /// Optional JSON-encoded ApiRequest to execute when dispatched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

/// A query specification within a BatchQuery request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "query_type")]
pub enum QuerySpec {
    /// Read an object by CID.
    #[serde(rename = "read")]
    Read {
        cid: String,
    },
    /// Search for objects by query string.
    #[serde(rename = "search")]
    Search {
        query: String,
        #[serde(default)]
        limit: Option<usize>,
        #[serde(default)]
        require_tags: Vec<String>,
        #[serde(default)]
        exclude_tags: Vec<String>,
    },
    /// Recall ephemeral memories.
    #[serde(rename = "recall")]
    Recall,
    /// Semantic memory recall.
    #[serde(rename = "recall_semantic")]
    RecallSemantic {
        query: String,
        #[serde(default = "default_k")]
        k: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextAssembleCandidate {
    pub cid: String,
    pub relevance: f32,
}

// ── Response DTOs ───────────────────────────────────────────────────────

/// Response for a successful model switch operation (v18.0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSwitchResponse {
    /// True if the switch was successful.
    pub success: bool,
    /// The model that was active before the switch.
    pub previous_model: String,
    /// The newly activated model.
    pub new_model: String,
    /// Human-readable status message.
    pub message: String,
}

/// Response for a model health check (v18.0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelHealthResponse {
    /// True if the model is available and responsive.
    pub available: bool,
    /// The model identifier that was checked.
    pub model: String,
    /// Observed latency in milliseconds, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    /// Error message if the model is unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultDto {
    pub cid: String,
    pub relevance: f32,
    pub tags: Vec<String>,
    /// Content preview — first 200 characters (F-37).
    pub snippet: String,
    /// MIME content type (F-37).
    pub content_type: String,
    /// Creation timestamp in Unix ms (F-37).
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDto {
    pub id: String,
    pub name: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeighborDto {
    pub node_id: String,
    pub label: String,
    pub node_type: String,
    pub edge_type: String,
    pub authority_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletedDto {
    pub cid: String,
    pub deleted_at: u64,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KGNodeDto {
    pub id: String,
    pub label: String,
    pub node_type: KGNodeType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_cid: Option<String>,
    pub properties: serde_json::Value,
    pub agent_id: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KGEdgeDto {
    pub src: String,
    pub dst: String,
    pub edge_type: KGEdgeType,
    pub weight: f32,
    pub created_at: u64,
}

fn not_false(b: &bool) -> bool { !*b }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadedContextDto {
    pub cid: String,
    pub layer: String,
    pub content: String,
    pub tokens_estimate: usize,
    /// Actual layer returned (may differ from requested if degraded) (F-8c).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_layer: Option<String>,
    /// Whether content was degraded from requested layer (F-8c).
    #[serde(default, skip_serializing_if = "not_false")]
    pub degraded: bool,
    /// Reason for degradation if applicable (F-8c).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degradation_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDto {
    pub node_id: String,
    pub name: String,
    pub description: String,
    pub agent_id: String,
    pub tags: Vec<String>,
}

/// Tenant descriptor — returned by ListTenants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantDto {
    pub id: String,
    pub admin_agent_id: String,
    pub created_at_ms: u64,
}

// ── Delta感知 structures (F-7) ─────────────────────────────────────────────────

/// A single change entry returned by DeltaSince.
/// Lightweight metadata summary — no LLM required.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeEntry {
    /// Content identifier of the changed object.
    pub cid: String,
    /// Type of change: "stored", "deleted", "tags_changed", etc.
    pub change_type: String,
    /// Human-readable summary: "{event_type} {cid[..8]} by {agent_id} [{tags}]"
    pub summary: String,
    /// Unix timestamp of the event in milliseconds.
    pub changed_at_ms: u64,
    /// Agent that triggered the event.
    pub changed_by: String,
    /// EventBus sequence number.
    pub seq: u64,
}

/// Result of a DeltaSince query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaResult {
    /// List of changes since the given timestamp or sequence number.
    pub changes: Vec<ChangeEntry>,
    /// Sequence number of the first event included (the since_seq input).
    pub from_seq: u64,
    /// Sequence number of the last event included.
    pub to_seq: u64,
    /// Estimated token count for the change list.
    pub token_estimate: usize,
}

// ── Batch Response Structures (v15.0) ──────────────────────────────────────────

/// Response for a batch create operation.
/// Each entry in `results` corresponds to a BatchCreateItem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCreateResponse {
    /// Results per item: Ok(cid) for success, Err(message) for failure.
    pub results: Vec<Result<String, String>>,
    pub successful: usize,
    pub failed: usize,
}

/// Response for a batch memory store operation.
/// Each entry in `results` corresponds to a BatchMemoryEntry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchMemoryStoreResponse {
    /// Results per entry: Ok(entry_id) for success, Err(message) for failure.
    pub results: Vec<Result<String, String>>,
    pub successful: usize,
    pub failed: usize,
}

/// Response for a batch submit intent operation.
/// Each entry in `results` corresponds to an IntentSpec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSubmitIntentResponse {
    /// Results per intent: Ok(intent_id) for success, Err(message) for failure.
    pub results: Vec<Result<String, String>>,
    pub successful: usize,
    pub failed: usize,
}

/// Response for a batch query operation.
/// Each entry in `results` corresponds to a QuerySpec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchQueryResponse {
    /// Results per query: Ok(json_data) for success, Err(message) for failure.
    pub results: Vec<Result<serde_json::Value, String>>,
    pub successful: usize,
    pub failed: usize,
}

// ── KG Causal Reasoning DTOs (v16.0) ───────────────────────────────────────────

/// A causal path result — path of cause-effect relationships between nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalPathDto {
    pub nodes: Vec<KGNodeDto>,
    pub edges: Vec<KGEdgeDto>,
    pub causal_strength: f32,
}

/// An impact analysis result — predicted effects of modifying a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactAnalysisDto {
    pub affected_nodes: Vec<String>,
    pub propagation_depth: u8,
    pub severity: f32,
}

/// A temporal change record — node created, modified, or deleted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalChangeDto {
    pub before: Option<KGNodeDto>,
    pub after: Option<KGNodeDto>,
    pub change_type: String,
    pub timestamp_ms: u64,
}

// ── Dashboard / Project Status Types ───────────────────────────────────────────

/// Runtime kernel metrics — live system state at query time.
/// Queried via `ApiRequest::SystemStatus`, not via HTTP dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStatus {
    pub timestamp_ms: i64,
    pub cas_object_count: usize,
    pub agent_count: usize,
    pub tag_count: usize,
    pub kg_node_count: usize,
    pub kg_edge_count: usize,
    /// Edge cache statistics (v19.0)
    pub cache_stats: Option<CacheStatsDto>,
    /// Health indicators (F-19)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<HealthIndicators>,
}

/// Health indicators for system observability (F-19).
/// Provides a quick snapshot of system health across memory, cache, eventbus, and scheduler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthIndicators {
    /// True if memory usage is below the healthy threshold (90%).
    pub memory_healthy: bool,
    /// Estimated memory usage as a percentage of total system memory [0.0, 100.0].
    pub memory_usage_percent: f64,
    /// Total physical memory in bytes (0 if unavailable).
    pub memory_total_bytes: u64,
    /// Used memory in bytes (0 if unavailable).
    pub memory_used_bytes: u64,
    /// True if cache hit rate is above the minimum healthy threshold (30%).
    pub cache_healthy: bool,
    /// Average cache hit rate across all cache tiers [0.0, 100.0].
    pub cache_hit_rate_percent: f64,
    /// True if EventBus queue depth is below the healthy threshold (1000).
    pub eventbus_healthy: bool,
    /// Number of events currently buffered in the EventBus.
    pub eventbus_queue_depth: usize,
    /// Number of active EventBus subscriptions.
    pub eventbus_subscriber_count: usize,
    /// True if the scheduler has fewer than 100 active agents.
    pub scheduler_healthy: bool,
    /// Number of currently active agents.
    pub scheduler_active_agents: usize,
    /// Number of pending intents in the scheduler queue.
    pub scheduler_pending_intents: usize,
    /// True if all subsystems are healthy.
    pub overall_healthy: bool,
    /// Overall health score [0.0, 1.0], where 1.0 means fully healthy.
    pub health_score: f64,
}

/// Degradation entry for health report (F-7).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Degradation {
    pub component: String,
    pub severity: String,
    pub message: String,
}

/// Health report for system observability (F-7).
/// Provides comprehensive health status including CAS/Agent/KG counts,
/// active sessions, embedding backend, and degradation list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    /// True if system is healthy (no critical degradations).
    pub healthy: bool,
    /// Unix timestamp in milliseconds when report was generated.
    pub timestamp_ms: i64,
    /// Number of CAS objects in storage.
    pub cas_objects: usize,
    /// Number of registered agents.
    pub agents: usize,
    /// Number of KG nodes.
    pub kg_nodes: usize,
    /// Number of KG edges.
    pub kg_edges: usize,
    /// Number of active sessions.
    pub active_sessions: usize,
    /// Embedding backend name (e.g., "stub", "ollama", "local").
    pub embedding_backend: String,
    /// List of active degradations.
    pub degradations: Vec<Degradation>,
    /// True if roundtrip test passed.
    pub roundtrip_ok: bool,
    /// Roundtrip latency in milliseconds (0 if failed).
    pub roundtrip_ms: u64,
}

/// Cache statistics for observability (v19.0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStatsDto {
    pub embedding_cache_entries: usize,
    pub kg_cache_entries: usize,
    pub search_cache_entries: usize,
    pub embedding_hit_rate: f64,
    pub kg_hit_rate: f64,
    pub search_hit_rate: f64,
}

/// Intent cache statistics (F-9).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentCacheStatsDto {
    pub entries: usize,
    pub memory_bytes: usize,
    pub hits: u64,
}

/// Cluster status for distributed mode (v20.0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterStatusDto {
    pub cluster_name: String,
    pub total_nodes: usize,
    pub local_node_id: String,
    pub is_seed: bool,
    pub version: u64,
    pub pending_migrations: usize,
    pub known_nodes: Vec<NodeInfoDto>,
}

/// Node information in cluster (v20.0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfoDto {
    pub node_id: String,
    pub host: String,
    pub port: u16,
    pub is_seed: bool,
    pub last_heartbeat_ms: u64,
    pub is_stale: bool,
}

/// Agent checkpoint result (v21.0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCheckpointDto {
    pub checkpoint_id: String,
    pub agent_id: String,
    pub created_at_ms: u64,
    pub agent_state: String,
    pub pending_intents: usize,
    pub memory_count: usize,
    pub kg_associations: usize,
    pub last_intent_description: Option<String>,
}

/// List of checkpoint IDs for an agent (v21.0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCheckpointsDto {
    pub agent_id: String,
    pub checkpoints: Vec<CheckpointSummaryDto>,
}

/// Summary of a single checkpoint (v21.0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointSummaryDto {
    pub checkpoint_id: String,
    pub created_at_ms: u64,
    pub agent_state: String,
    pub memory_count: usize,
}

/// A registered hook entry (Daemon-First).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEntryDto {
    pub point: String,
    pub priority: i32,
}

/// Session started response (F-6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStarted {
    pub session_id: String,
    /// Summary of the checkpoint restored (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restored_checkpoint: Option<CheckpointSummaryDto>,
    /// Assembly ID for fetching warm context (if intent_hint was provided).
    /// Client should call FetchAssembledContext with this ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warm_context: Option<String>,
    /// Changes since the last session (based on last_seen_seq).
    #[serde(default)]
    pub changes_since_last: Vec<ChangeEntry>,
    /// Estimated token count for the changes.
    pub token_estimate: usize,
}

/// Session ended response (F-6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEnded {
    /// Checkpoint ID if auto_checkpoint was true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    /// Current EventBus sequence number.
    /// Client should save this and pass as last_seen_seq in next StartSession.
    pub last_seq: u64,
    /// Memory consolidation report (F-6).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consolidation: Option<ConsolidationReport>,
    /// Cumulative token consumption for this session (F-4).
    pub total_tokens_consumed: u64,
}

/// Memory consolidation report (F-6).
/// Reports the results of the Memory Consolidation Cycle at session-end.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationReport {
    /// Number of ephemeral memories before consolidation.
    pub ephemeral_before: usize,
    /// Number of ephemeral memories after consolidation.
    pub ephemeral_after: usize,
    /// Number of working memories before consolidation.
    pub working_before: usize,
    /// Number of working memories after consolidation.
    pub working_after: usize,
    /// Number of memories promoted to long-term.
    pub promoted: usize,
    /// Number of memories evicted.
    pub evicted: usize,
    /// Number of memories linked to KG.
    pub linked: usize,
}

// ── Hybrid Retrieval / Graph-RAG (F-11) ───────────────────────────────────────

/// A single step in the provenance chain showing how a result was reached.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceStep {
    /// CID of the source node at this hop.
    pub from_cid: String,
    /// Type of edge traversed to reach this node.
    pub edge_type: String,
    /// Hop number (0 = direct result).
    pub hop: u8,
}

/// A single hybrid retrieval result — combining vector and graph scores.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridHit {
    /// Content identifier of the result.
    pub cid: String,
    /// Human-readable content preview (first 200 chars).
    pub content_preview: String,
    /// Vector similarity score [0, 1].
    pub vector_score: f32,
    /// Knowledge graph authority score [0, 1].
    pub graph_score: f32,
    /// Combined score: α × vector_score + (1-α) × graph_score, α = 0.6.
    pub combined_score: f32,
    /// Provenance chain showing the path from query to result.
    pub provenance: Vec<ProvenanceStep>,
}

/// Hybrid retrieval result — combines vector search with KG traversal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridResult {
    /// Ordered list of hybrid hits (descending by combined_score).
    pub items: Vec<HybridHit>,
    /// Estimated total token count for transmitting all items.
    pub token_estimate: usize,
    /// Number of results that came from vector search.
    pub vector_hits: usize,
    /// Number of results that came from graph traversal.
    pub graph_hits: usize,
    /// Number of causal paths discovered.
    pub paths_found: usize,
}

// ── Growth Report (F-13) ─────────────────────────────────────────────────────

/// Time period for growth report queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GrowthPeriod {
    /// Last 7 days.
    Last7Days,
    /// Last 30 days.
    Last30Days,
    /// All time — since the agent first registered.
    AllTime,
}

/// Growth report showing an agent's learning progress and efficiency metrics.
///
/// Read-only statistics — OS presents the data, Agent decides whether to adjust strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrowthReport {
    /// Agent this report is for.
    pub agent_id: String,
    /// Time period covered by this report.
    pub period: GrowthPeriod,
    /// Total number of completed sessions in the period.
    pub sessions_total: u64,
    /// Average tokens per session for the first 5 sessions (or all if fewer).
    pub avg_tokens_per_session_first_5: usize,
    /// Average tokens per session for the last 5 sessions.
    pub avg_tokens_per_session_last_5: usize,
    /// Token efficiency ratio: last_5 / first_5 (lower is better).
    pub token_efficiency_ratio: f32,
    /// Intent cache hit rate: hits / total_lookups.
    pub intent_cache_hit_rate: f32,
    /// Number of memories stored in the period.
    pub memories_stored: u64,
    /// Number of memories shared with other agents.
    pub memories_shared: u64,
    /// Number of procedures learned (procedural memories stored).
    pub procedures_learned: u64,
    /// Number of KG nodes created in the period.
    pub kg_nodes_created: u64,
    /// Number of KG edges created in the period.
    pub kg_edges_created: u64,
}

// ── Task Delegation (F-14) ─────────────────────────────────────────────────

/// Task status enum — complete lifecycle for delegated tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Task created, waiting for target agent to pick up.
    Pending,
    /// Target agent started working on the task.
    InProgress,
    /// Task completed successfully with results.
    Completed,
    /// Task failed or timed out.
    Failed,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Pending => write!(f, "pending"),
            TaskStatus::InProgress => write!(f, "in_progress"),
            TaskStatus::Completed => write!(f, "completed"),
            TaskStatus::Failed => write!(f, "failed"),
        }
    }
}

/// Task result returned by query_task_status (F-14).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    /// Task identifier.
    pub task_id: String,
    /// Agent that executed (or is executing) the task.
    pub agent_id: String,
    /// Current status.
    pub status: TaskStatus,
    /// Result CIDs produced by the task (empty if not completed).
    pub result_cids: Vec<String>,
    /// Optional failure reason (present if status is Failed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    /// When the task was created (Unix ms).
    pub created_at_ms: u64,
    /// When the task was last updated (Unix ms).
    pub updated_at_ms: u64,
}

// ── Memory Stats (F-17) ─────────────────────────────────────────────

/// Memory usage statistics for an agent's tier or all tiers.
/// Queried via `ApiRequest::MemoryStats`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStatsResult {
    /// Agent this stats report is for.
    pub agent_id: String,
    /// Tier this stats report covers (empty string = all tiers).
    pub tier: String,
    /// Total number of memory entries.
    pub total_entries: usize,
    /// Total approximate memory size in bytes (estimated from content).
    pub total_bytes: usize,
    /// Age of the oldest entry in milliseconds (0 if no entries).
    pub oldest_entry_age_ms: u64,
    /// Average access count across all entries (0 if no entries).
    pub avg_access_count: f32,
    /// Number of entries that have never been accessed (access_count == 0).
    pub never_accessed_count: usize,
    /// Number of entries approaching expiration (within 10% of TTL remaining).
    pub about_to_expire_count: usize,
}

// ── Knowledge Discovery (F-16) ─────────────────────────────────────

/// A single hit returned by knowledge discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryHit {
    /// Content identifier of the discovered knowledge.
    pub cid: String,
    /// Agent that shared this knowledge.
    pub source_agent: String,
    /// When the knowledge was shared (Unix ms).
    pub shared_at: u64,
    /// Semantic tags associated with the knowledge.
    pub tags: Vec<String>,
    /// Content preview (first 200 chars).
    pub preview: String,
    /// Relevance score [0, 1] based on query match.
    pub relevance_score: f32,
    /// Number of times other agents have used this knowledge.
    pub usage_count: u64,
}

/// Result of a knowledge discovery query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryResult {
    /// Discovered knowledge items.
    pub items: Vec<DiscoveryHit>,
    /// Estimated token count for transmitting all items.
    pub token_estimate: usize,
    /// Total number of accessible items matching the query (may be greater than items returned).
    pub total_available: usize,
}

// ── Storage Governance (F-18) ─────────────────────────────────────

/// Object usage statistics returned by object_usage query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectUsageResult {
    /// When the object was created (Unix ms).
    pub created_at: u64,
    /// When the object was last accessed (Unix ms).
    pub last_accessed_at: u64,
    /// Number of times the object has been accessed.
    pub access_count: u64,
    /// True if the object is referenced by the knowledge graph.
    pub referenced_by_kg: bool,
    /// True if the object is referenced by memory.
    pub referenced_by_memory: bool,
}

/// Storage statistics for the CAS layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStatsResult {
    /// Total number of objects in CAS.
    pub total_objects: usize,
    /// Total size in bytes.
    pub total_bytes: usize,
    /// Per-tier breakdown of objects and bytes.
    pub by_tier: TierStats,
    /// Number of cold (rarely accessed) objects.
    pub cold_objects: usize,
    /// Number of objects approaching expiration.
    pub about_to_expire: usize,
}

/// Per-tier storage statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierStats {
    pub ephemeral_count: usize,
    pub ephemeral_bytes: usize,
    pub working_count: usize,
    pub working_bytes: usize,
    pub longterm_count: usize,
    pub longterm_bytes: usize,
}

/// Result of evict_cold operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvictColdResult {
    /// Number of objects evicted.
    pub evicted_count: usize,
    /// Number of bytes freed.
    pub evicted_bytes: usize,
    /// Number of cold objects remaining after eviction.
    pub remaining_cold: usize,
}

/// Agent resource usage and quota snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentUsageDto {
    pub agent_id: String,
    pub memory_entries: usize,
    pub memory_quota: u64,
    pub tool_call_count: u64,
    pub total_tokens_consumed: u64,
    pub cpu_time_quota: u64,
    pub allowed_tools: Vec<String>,
    pub last_active_ms: u64,
}

/// Agent capability card — what an agent can do and its current state.
/// Enables peer discovery: agents find collaborators by capability match.
/// A2A-compliant (RFC draft 2025).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCardDto {
    /// Unique agent identifier (UUID).
    pub agent_id: String,
    /// Human-readable agent name.
    pub name: String,
    /// Human-readable description of agent's purpose/capabilities.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Agent version string (semver).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// Current lifecycle state.
    pub state: String,
    /// Memory resource quota (0 = unlimited).
    #[serde(default)]
    pub memory_quota: u64,
    /// CPU time quota per intent (ms, 0 = unlimited).
    #[serde(default)]
    pub cpu_time_quota: u64,
    /// Available tools (empty = all tools allowed).
    pub tools: Vec<String>,
    /// Number of memories stored for this agent.
    pub memory_entries: usize,
    /// Total tool calls executed by this agent.
    pub tool_call_count: u64,
    /// When agent was last active (ms since epoch, 0 = never).
    pub last_active_ms: u64,
    /// When agent was registered (ms since epoch).
    #[serde(default)]
    pub created_at_ms: u64,
}

/// Result of a delegation operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationResultDto {
    pub intent_id: String,
    pub message_id: String,
    pub from: String,
    pub to: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T: serde::Serialize + serde::de::DeserializeOwned>(val: &T) -> T {
        let json = serde_json::to_string(val).unwrap();
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn procedure_step_dto_roundtrip() {
        let step = ProcedureStepDto {
            description: "check auth".into(),
            action: "verify_token".into(),
            expected_outcome: Some("valid".into()),
        };
        let rt = roundtrip(&step);
        assert_eq!(rt.description, "check auth");
        assert_eq!(rt.expected_outcome, Some("valid".into()));
    }

    #[test]
    fn procedure_step_dto_optional_outcome_defaults_none() {
        let json = r#"{"description":"d","action":"a"}"#;
        let step: ProcedureStepDto = serde_json::from_str(json).unwrap();
        assert!(step.expected_outcome.is_none());
    }

    #[test]
    fn discovery_scope_serde_variants() {
        let cases = vec![
            (DiscoveryScope::Shared, r#""shared""#),
            (DiscoveryScope::AllAccessible, r#""all_accessible""#),
        ];
        for (val, expected_json) in cases {
            let json = serde_json::to_string(&val).unwrap();
            assert_eq!(json, expected_json);
            let rt: DiscoveryScope = serde_json::from_str(&json).unwrap();
            assert_eq!(rt, val);
        }

        let group = DiscoveryScope::Group("team-a".into());
        let json = serde_json::to_string(&group).unwrap();
        let rt: DiscoveryScope = serde_json::from_str(&json).unwrap();
        assert_eq!(rt, DiscoveryScope::Group("team-a".into()));
    }

    #[test]
    fn discovery_scope_default_is_all_accessible() {
        assert_eq!(DiscoveryScope::default(), DiscoveryScope::AllAccessible);
    }

    #[test]
    fn knowledge_type_serde() {
        let kt = KnowledgeType::Procedure;
        let json = serde_json::to_string(&kt).unwrap();
        assert_eq!(json, r#""procedure""#);
        let rt: KnowledgeType = serde_json::from_str(&json).unwrap();
        assert_eq!(rt, KnowledgeType::Procedure);
    }

    #[test]
    fn batch_memory_entry_defaults() {
        let json = r#"{"content":"test"}"#;
        let entry: BatchMemoryEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.content, "test");
        assert_eq!(entry.importance, 50);
        assert!(entry.tags.is_empty());
    }

    #[test]
    fn intent_spec_defaults() {
        let json = r#"{"description":"fix bug"}"#;
        let spec: IntentSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.priority, "medium");
        assert!(spec.action.is_none());
    }

    #[test]
    fn query_spec_tagged_union_read() {
        let json = r#"{"query_type":"read","cid":"abc123"}"#;
        let spec: QuerySpec = serde_json::from_str(json).unwrap();
        match spec {
            QuerySpec::Read { cid } => assert_eq!(cid, "abc123"),
            _ => panic!("expected Read variant"),
        }
    }

    #[test]
    fn query_spec_tagged_union_search() {
        let json = r#"{"query_type":"search","query":"auth","limit":5}"#;
        let spec: QuerySpec = serde_json::from_str(json).unwrap();
        match spec {
            QuerySpec::Search { query, limit, .. } => {
                assert_eq!(query, "auth");
                assert_eq!(limit, Some(5));
            }
            _ => panic!("expected Search variant"),
        }
    }

    #[test]
    fn query_spec_recall_semantic_default_k() {
        let json = r#"{"query_type":"recall_semantic","query":"test"}"#;
        let spec: QuerySpec = serde_json::from_str(json).unwrap();
        match spec {
            QuerySpec::RecallSemantic { k, .. } => assert_eq!(k, 10),
            _ => panic!("expected RecallSemantic"),
        }
    }

    #[test]
    fn loaded_context_dto_degraded_skip_serializing() {
        let dto = LoadedContextDto {
            cid: "c1".into(),
            layer: "L0".into(),
            content: "hello".into(),
            tokens_estimate: 5,
            actual_layer: None,
            degraded: false,
            degradation_reason: None,
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(!json.contains("degraded"));
        assert!(!json.contains("actual_layer"));
    }

    #[test]
    fn loaded_context_dto_degraded_present_when_true() {
        let dto = LoadedContextDto {
            cid: "c1".into(),
            layer: "L0".into(),
            content: "hello".into(),
            tokens_estimate: 5,
            actual_layer: Some("L1".into()),
            degraded: true,
            degradation_reason: Some("too large".into()),
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("degraded"));
        assert!(json.contains("actual_layer"));
    }

    #[test]
    fn model_switch_response_roundtrip() {
        let resp = ModelSwitchResponse {
            success: true,
            previous_model: "old".into(),
            new_model: "new".into(),
            message: "switched".into(),
        };
        let rt = roundtrip(&resp);
        assert!(rt.success);
        assert_eq!(rt.new_model, "new");
    }

    #[test]
    fn model_health_optional_fields_skip() {
        let resp = ModelHealthResponse {
            available: true,
            model: "m1".into(),
            latency_ms: None,
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(!json.contains("latency_ms"));
        assert!(!json.contains("error"));
    }

    #[test]
    fn task_status_display() {
        assert_eq!(TaskStatus::Pending.to_string(), "pending");
        assert_eq!(TaskStatus::InProgress.to_string(), "in_progress");
        assert_eq!(TaskStatus::Completed.to_string(), "completed");
        assert_eq!(TaskStatus::Failed.to_string(), "failed");
    }

    #[test]
    fn task_status_serde() {
        let status = TaskStatus::InProgress;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, r#""in_progress""#);
        let rt: TaskStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(rt, TaskStatus::InProgress);
    }

    #[test]
    fn growth_period_serde() {
        let period = GrowthPeriod::Last7Days;
        let json = serde_json::to_string(&period).unwrap();
        assert_eq!(json, r#""last7days""#);
        let rt: GrowthPeriod = serde_json::from_str(&json).unwrap();
        assert_eq!(rt, GrowthPeriod::Last7Days);
    }

    #[test]
    fn delta_result_roundtrip() {
        let dr = DeltaResult {
            changes: vec![ChangeEntry {
                cid: "abc".into(),
                change_type: "stored".into(),
                summary: "stored abc by agent1".into(),
                changed_at_ms: 1000,
                changed_by: "agent1".into(),
                seq: 42,
            }],
            from_seq: 40,
            to_seq: 42,
            token_estimate: 100,
        };
        let rt = roundtrip(&dr);
        assert_eq!(rt.changes.len(), 1);
        assert_eq!(rt.changes[0].seq, 42);
        assert_eq!(rt.from_seq, 40);
    }

    #[test]
    fn batch_create_response_roundtrip() {
        let resp = BatchCreateResponse {
            results: vec![Ok("cid1".into()), Err("failed".into())],
            successful: 1,
            failed: 1,
        };
        let rt = roundtrip(&resp);
        assert_eq!(rt.successful, 1);
        assert_eq!(rt.failed, 1);
    }

    #[test]
    fn session_started_optional_fields() {
        let json = r#"{"session_id":"s1","token_estimate":0,"changes_since_last":[]}"#;
        let ss: SessionStarted = serde_json::from_str(json).unwrap();
        assert_eq!(ss.session_id, "s1");
        assert!(ss.restored_checkpoint.is_none());
        assert!(ss.warm_context.is_none());
    }

    #[test]
    fn agent_card_dto_empty_description_skipped() {
        let card = AgentCardDto {
            agent_id: "a1".into(),
            name: "Agent".into(),
            description: String::new(),
            version: String::new(),
            state: "running".into(),
            memory_quota: 0,
            cpu_time_quota: 0,
            tools: vec![],
            memory_entries: 0,
            tool_call_count: 0,
            last_active_ms: 0,
            created_at_ms: 0,
        };
        let json = serde_json::to_string(&card).unwrap();
        assert!(!json.contains("description"));
        assert!(!json.contains("version"));
    }

    #[test]
    fn not_false_helper() {
        // not_false is used as skip_serializing_if: skip when false (returns true)
        assert!(not_false(&false));
        assert!(!not_false(&true));
    }

    #[test]
    fn kg_node_dto_roundtrip() {
        let node = KGNodeDto {
            id: "n1".into(),
            label: "test".into(),
            node_type: KGNodeType::Entity,
            content_cid: Some("cid".into()),
            properties: serde_json::json!({"key": "val"}),
            agent_id: "a1".into(),
            created_at: 1000,
        };
        let rt = roundtrip(&node);
        assert_eq!(rt.id, "n1");
        assert_eq!(rt.node_type, KGNodeType::Entity);
    }

    #[test]
    fn batch_create_item_defaults() {
        let json = r#"{"content":"hello"}"#;
        let item: BatchCreateItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.content, "hello");
        assert!(item.tags.is_empty());
        assert!(item.intent.is_none());
    }

    #[test]
    fn hybrid_hit_roundtrip() {
        let hit = HybridHit {
            cid: "c1".into(),
            content_preview: "preview".into(),
            vector_score: 0.8,
            graph_score: 0.6,
            combined_score: 0.72,
            provenance: vec![ProvenanceStep {
                from_cid: "c0".into(),
                edge_type: "related_to".into(),
                hop: 0,
            }],
        };
        let rt = roundtrip(&hit);
        assert_eq!(rt.provenance.len(), 1);
        assert_eq!(rt.provenance[0].hop, 0);
    }
}
