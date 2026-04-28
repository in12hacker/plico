//! AI-Friendly Semantic API — Core Protocol
//!
//! Defines `ApiRequest` (the 60+ method tagged enum) and `ApiResponse`
//! (the universal response envelope). Version types live in `version.rs`,
//! DTO payload types in `dto.rs`.
//!
//! # API Versioning
//!
//! The API uses semantic versioning (major.minor.patch). Clients can declare
//! their API version in requests via the optional `api_version` field:
//! ```json
//! {"method": "create", "api_version": "1.2.0", "params": {...}}
//! ```
//!
//! If no version is declared, the server defaults to the current stable version.

use base64::Engine;
use serde::{Deserialize, Serialize};

// Re-export version types so `crate::api::semantic::ApiVersion` etc. keep working.
pub use super::version::*;
// Re-export all DTOs so `crate::api::semantic::SearchResultDto` etc. keep working.
pub use super::dto::*;

use crate::fs::{EventType, EventRelation, EventSummary, KGNodeType, KGEdgeType};

/// Content encoding field for binary-safe API payloads.
///
/// `"utf8"` (default) — content is a plain UTF-8 string.
/// `"base64"` — content is Base64-encoded (RFC 4648 standard alphabet).
/// Use `"base64"` when transmitting images, audio, video, or any binary data.
///
/// Example (create an image):
/// ```json
/// {"method": "create", "content": "iVBORw0KGgo...", "content_encoding": "base64", "tags": ["image"], "agent_id": "a1"}
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ContentEncoding {
    #[default]
    Utf8,
    Base64,
}

/// Decode a content string according to its encoding.
///
/// Returns the raw bytes, or an error string suitable for `ApiResponse::error`.
pub fn decode_content(content: &str, encoding: &ContentEncoding) -> Result<Vec<u8>, String> {
    match encoding {
        ContentEncoding::Utf8 => Ok(content.as_bytes().to_vec()),
        ContentEncoding::Base64 => {
            base64::engine::general_purpose::STANDARD
                .decode(content)
                .map_err(|e| format!("base64 decode error: {e}"))
        }
    }
}

/// Estimate token count for a text string (F-8).
///
/// Uses the formula: (ascii + 3) / 4 + (non_ascii + 1) / 2
/// This is a coarse approximation (±20%). Can be refined with tiktoken-rs in future versions.
/// Note: This is an estimate, not precise. For code the result may be high,
/// for non-ASCII text (e.g., Chinese) the result may be low.
pub fn estimate_tokens(text: &str) -> usize {
    let ascii = text.chars().filter(|c| c.is_ascii()).count();
    let non_ascii = text.chars().filter(|c| !c.is_ascii()).count();
    ascii.div_ceil(4) + non_ascii.div_ceil(2)
}

pub(crate) fn default_importance() -> u8 { 50 }
pub(crate) fn default_k() -> usize { 10 }
pub(crate) fn default_priority() -> String { "medium".to_string() }
fn default_budget_tokens() -> usize { 4096 }
fn default_auto_checkpoint() -> bool { true }
fn default_max_results() -> usize { 10 }

/// A JSON API request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method")]
pub enum ApiRequest {
    #[serde(rename = "create")]
    Create {
        /// Declared API version for the request (e.g. "1.0.0"). Defaults to current.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_version: Option<ApiVersion>,
        /// Object content. Plain UTF-8 by default; set `content_encoding: "base64"` for binary.
        content: String,
        #[serde(default)]
        content_encoding: ContentEncoding,
        tags: Vec<String>,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_token: Option<String>,
        intent: Option<String>,
    },

    #[serde(rename = "read")]
    Read {
        cid: String,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_token: Option<String>,
    },

    #[serde(rename = "search")]
    Search {
        query: String,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_token: Option<String>,
        limit: Option<usize>,
        #[serde(default)]
        offset: Option<usize>,
        /// Require entries to have all of these tags (AND).
        #[serde(default)]
        require_tags: Vec<String>,
        /// Exclude entries that have any of these tags.
        #[serde(default)]
        exclude_tags: Vec<String>,
        /// Inclusive lower bound on creation time (Unix ms).
        #[serde(default)]
        since: Option<i64>,
        /// Inclusive upper bound on creation time (Unix ms).
        #[serde(default)]
        until: Option<i64>,
        /// Optional intent context for context-dependent gravity re-ranking (F-6).
        /// When provided, search results are boosted based on hot objects from
        /// the agent's profile and current intent alignment.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intent_context: Option<String>,
    },

    #[serde(rename = "update")]
    Update {
        cid: String,
        /// Object content. Plain UTF-8 by default; set `content_encoding: "base64"` for binary.
        content: String,
        #[serde(default)]
        content_encoding: ContentEncoding,
        new_tags: Option<Vec<String>>,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_token: Option<String>,
    },

    #[serde(rename = "delete")]
    Delete {
        cid: String,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_token: Option<String>,
    },

    #[serde(rename = "register_agent")]
    RegisterAgent { name: String },

    #[serde(rename = "list_agents")]
    ListAgents,

    #[serde(rename = "remember")]
    Remember {
        agent_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    #[serde(rename = "recall")]
    Recall {
        agent_id: String,
        #[serde(default)]
        scope: Option<String>,
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
        #[serde(default)]
        tier: Option<String>,
    },

    #[serde(rename = "remember_long_term")]
    RememberLongTerm {
        agent_id: String,
        content: String,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default = "default_importance")]
        importance: u8,
        #[serde(default)]
        scope: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    #[serde(rename = "recall_semantic")]
    RecallSemantic {
        agent_id: String,
        query: String,
        #[serde(default = "default_k")]
        k: usize,
    },

    #[serde(rename = "explore")]
    Explore { cid: String, edge_type: Option<String>, depth: Option<u8>, agent_id: String },

    #[serde(rename = "grant_permission")]
    GrantPermission {
        agent_id: String,
        action: String,
        scope: Option<String>,
        expires_at: Option<u64>,
    },

    #[serde(rename = "revoke_permission")]
    RevokePermission {
        agent_id: String,
        action: String,
    },

    #[serde(rename = "list_permissions")]
    ListPermissions { agent_id: String },

    #[serde(rename = "check_permission")]
    CheckPermission {
        agent_id: String,
        action: String,
    },

    #[serde(rename = "list_deleted")]
    ListDeleted { agent_id: String },

    #[serde(rename = "restore")]
    Restore { cid: String, agent_id: String },

    #[serde(rename = "history")]
    History { cid: String, agent_id: String },

    #[serde(rename = "rollback")]
    Rollback { cid: String, agent_id: String },

    #[serde(rename = "create_event")]
    CreateEvent {
        label: String,
        event_type: EventType,
        start_time: Option<u64>,
        end_time: Option<u64>,
        location: Option<String>,
        tags: Vec<String>,
        agent_id: String,
    },

    #[serde(rename = "list_events")]
    ListEvents {
        since: Option<u64>,
        until: Option<u64>,
        tags: Vec<String>,
        event_type: Option<EventType>,
        agent_id: String,
        #[serde(default)]
        limit: Option<usize>,
        #[serde(default)]
        offset: Option<usize>,
    },

    #[serde(rename = "list_events_text")]
    ListEventsText {
        time_expression: String,
        tags: Vec<String>,
        event_type: Option<EventType>,
        agent_id: String,
    },

    #[serde(rename = "event_attach")]
    EventAttach {
        event_id: String,
        target_id: String,
        relation: EventRelation,
        agent_id: String,
    },

    // ── Knowledge Graph direct operations ────────────────────────────────

    #[serde(rename = "add_node")]
    AddNode {
        label: String,
        node_type: KGNodeType,
        #[serde(default)]
        properties: serde_json::Value,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    #[serde(rename = "add_edge")]
    AddEdge {
        src_id: String,
        dst_id: String,
        edge_type: KGEdgeType,
        #[serde(default)]
        weight: Option<f32>,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    #[serde(rename = "list_nodes")]
    ListNodes {
        #[serde(default)]
        node_type: Option<KGNodeType>,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
        #[serde(default)]
        offset: Option<usize>,
    },

    #[serde(rename = "list_nodes_at_time")]
    ListNodesAtTime {
        #[serde(default)]
        node_type: Option<KGNodeType>,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
        /// Unix timestamp (ms) to query nodes valid at.
        t: u64,
    },

    #[serde(rename = "find_paths")]
    FindPaths {
        src_id: String,
        dst_id: String,
        #[serde(default)]
        max_depth: Option<u8>,
        /// If true, find the highest-weight path using best-first search.
        #[serde(default)]
        weighted: bool,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    // ── Agent Lifecycle operations ────────────────────────────────────

    #[serde(rename = "submit_intent")]
    SubmitIntent {
        description: String,
        priority: String,
        /// JSON-encoded ApiRequest to execute when this intent is dispatched.
        #[serde(default)]
        action: Option<String>,
        agent_id: String,
    },

    #[serde(rename = "agent_status")]
    AgentStatus { agent_id: String },

    #[serde(rename = "agent_suspend")]
    AgentSuspend { agent_id: String },

    #[serde(rename = "agent_resume")]
    AgentResume { agent_id: String },

    #[serde(rename = "agent_terminate")]
    AgentTerminate { agent_id: String },

    // ── Tool operations ──────────────────────────────────────────────

    #[serde(rename = "tool_call")]
    ToolCall {
        tool: String,
        #[serde(default)]
        params: serde_json::Value,
        agent_id: String,
    },

    #[serde(rename = "tool_list")]
    ToolList { agent_id: String },

    #[serde(rename = "tool_describe")]
    ToolDescribe { tool: String, agent_id: String },

    // ── Procedural Memory ────────────────────────────────────────────

    #[serde(rename = "remember_procedural")]
    RememberProcedural {
        agent_id: String,
        name: String,
        description: String,
        steps: Vec<ProcedureStepDto>,
        #[serde(default)]
        learned_from: Option<String>,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default)]
        scope: Option<String>,
    },

    #[serde(rename = "recall_procedural")]
    RecallProcedural {
        agent_id: String,
        #[serde(default)]
        name: Option<String>,
    },

    #[serde(rename = "recall_visible")]
    RecallVisible {
        agent_id: String,
        #[serde(default)]
        groups: Vec<String>,
    },

    // ── Agent Resource Management ─────────────────────────────────────

    #[serde(rename = "agent_set_resources")]
    AgentSetResources {
        agent_id: String,
        #[serde(default)]
        memory_quota: Option<u64>,
        #[serde(default)]
        cpu_time_quota: Option<u64>,
        #[serde(default)]
        allowed_tools: Option<Vec<String>>,
        /// Agent performing the operation (must be owner or trusted).
        caller_agent_id: String,
    },

    // ── Agent Checkpoint & Restore ───────────────────────────────────

    #[serde(rename = "agent_checkpoint")]
    AgentCheckpoint { agent_id: String },

    #[serde(rename = "agent_restore")]
    AgentRestore { agent_id: String, checkpoint_cid: String },

    // ── Agent Messaging ───────────────────────────────────────────────

    #[serde(rename = "send_message")]
    SendMessage {
        from: String,
        to: String,
        payload: serde_json::Value,
    },

    #[serde(rename = "read_messages")]
    ReadMessages {
        agent_id: String,
        #[serde(default)]
        unread_only: bool,
        #[serde(default)]
        limit: Option<usize>,
        #[serde(default)]
        offset: Option<usize>,
    },

    #[serde(rename = "ack_message")]
    AckMessage {
        agent_id: String,
        message_id: String,
    },

    // ── Graph CRUD extensions (v0.7) ─────────────────────────────────

    #[serde(rename = "get_node")]
    GetNode {
        node_id: String,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    #[serde(rename = "list_edges")]
    ListEdges {
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
        #[serde(default)]
        node_id: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
        #[serde(default)]
        offset: Option<usize>,
    },

    #[serde(rename = "remove_node")]
    RemoveNode {
        node_id: String,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    #[serde(rename = "remove_edge")]
    RemoveEdge {
        src_id: String,
        dst_id: String,
        #[serde(default)]
        edge_type: Option<KGEdgeType>,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    #[serde(rename = "update_node")]
    UpdateNode {
        node_id: String,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        properties: Option<serde_json::Value>,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    // ── Agent lifecycle extensions (v0.7) ────────────────────────────

    #[serde(rename = "agent_complete")]
    AgentComplete { agent_id: String },

    #[serde(rename = "agent_fail")]
    AgentFail { agent_id: String, reason: String },

    // ── Memory tier management (v0.7) ────────────────────────────────

    #[serde(rename = "memory_move")]
    MemoryMove {
        agent_id: String,
        entry_id: String,
        target_tier: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    #[serde(rename = "memory_delete")]
    MemoryDeleteEntry {
        agent_id: String,
        entry_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    #[serde(rename = "evict_expired")]
    EvictExpired {
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    // ── Context Loading (v0.9) ──────────────────────────────────────

    #[serde(rename = "load_context")]
    LoadContext {
        cid: String,
        /// "L0", "L1", or "L2"
        layer: String,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    // ── Temporal Edge History (v0.9) ────────────────────────────────

    #[serde(rename = "edge_history")]
    EdgeHistory {
        src_id: String,
        dst_id: String,
        #[serde(default)]
        edge_type: Option<KGEdgeType>,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    // ── Event Bus (v5.0) ───────────────────────────────────────────

    #[serde(rename = "event_subscribe")]
    EventSubscribe {
        agent_id: String,
        #[serde(default)]
        event_types: Option<Vec<String>>,
        #[serde(default)]
        agent_ids: Option<Vec<String>>,
    },

    #[serde(rename = "event_poll")]
    EventPoll { subscription_id: String },

    #[serde(rename = "event_unsubscribe")]
    EventUnsubscribe { subscription_id: String },

    // ── System Status (v5.3 — replaces HTTP dashboard) ───────────

    #[serde(rename = "system_status")]
    SystemStatus,

    // ── Context Budget (v6.0) ────────────────────────────────────

    #[serde(rename = "context_assemble")]
    ContextAssemble {
        agent_id: String,
        cids: Vec<ContextAssembleCandidate>,
        budget_tokens: usize,
    },

    // ── Resource Visibility (v6.1) ──────────────────────────────

    #[serde(rename = "agent_usage")]
    AgentUsage { agent_id: String },

    // ── Edge Cache (v19.0) ─────────────────────────────────────

    #[serde(rename = "cache_stats")]
    CacheStats,

    #[serde(rename = "cache_invalidate")]
    CacheInvalidate,

    // ── Intent Cache (F-9) ────────────────────────────────────────

    #[serde(rename = "intent_cache_stats")]
    IntentCacheStats,

    // ── Distributed Mode (v20.0) ─────────────────────────────────

    #[serde(rename = "cluster_status")]
    ClusterStatus,

    #[serde(rename = "cluster_join")]
    ClusterJoin {
        host: String,
        port: u16,
    },

    #[serde(rename = "cluster_leave")]
    ClusterLeave,

    #[serde(rename = "node_ping")]
    NodePing {
        target_host: String,
        target_port: u16,
    },

    // ── Token Usage (F-8) ─────────────────────────────────────

    #[serde(rename = "query_token_usage")]
    QueryTokenUsage {
        agent_id: String,
        #[serde(default)]
        session_id: Option<String>,
    },

    // ── Delta感知 (F-7) ─────────────────────────────────────

    /// Query changes since a given event sequence number.
    /// Used by agents to efficiently sync state after a session gap.
    #[serde(rename = "delta_since")]
    DeltaSince {
        agent_id: String,
        /// Event sequence number to query from (exclusive).
        /// Agent receives last_seq from EndSession and passes it here.
        since_seq: u64,
        /// Only return changes affecting these CIDs (empty = all).
        #[serde(default)]
        watch_cids: Vec<String>,
        /// Only return changes containing any of these tags (empty = all).
        #[serde(default)]
        watch_tags: Vec<String>,
        /// Maximum number of changes to return.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },

    // ── Session Lifecycle (F-6) ─────────────────────────────────

    /// Start a new session — orchestrates checkpoint restore + delta + prefetch.
    #[serde(rename = "start_session")]
    StartSession {
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_token: Option<String>,
        /// Intent hint — triggers prefetch engine to warm up context.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intent_hint: Option<String>,
        /// Memory tiers to restore from checkpoint (empty = all tiers).
        #[serde(default)]
        load_tiers: Vec<crate::memory::MemoryTier>,
        /// Last seen event sequence number from previous session.
        /// Used for delta calculation. Omit for first session.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_seen_seq: Option<u64>,
    },

    /// End an active session — creates checkpoint and returns last_seq.
    #[serde(rename = "end_session")]
    EndSession {
        agent_id: String,
        session_id: String,
        /// Whether to auto-create a checkpoint before ending (default: true).
        #[serde(default = "default_auto_checkpoint")]
        auto_checkpoint: bool,
    },

    // ── Agent Discovery (v6.2) ──────────────────────────────────

    #[serde(rename = "discover_agents")]
    DiscoverAgents {
        #[serde(default)]
        state_filter: Option<String>,
        #[serde(default)]
        tool_filter: Option<String>,
        agent_id: String,
    },

    // ── Agent Delegation (v6.3 → F-14) ─────────────────────────────────

    /// Delegate a task to another agent with state tracking and deadline support (F-14).
    /// Replaces v6.3 DelegateTask which used intent+messaging approach.
    #[serde(rename = "delegate_task")]
    DelegateTask {
        /// Caller-provided unique task ID.
        task_id: String,
        /// Agent delegating the task.
        from_agent: String,
        /// Agent assigned to execute the task.
        to_agent: String,
        /// Natural-language intent description for the task.
        intent: String,
        /// Content CIDs providing context for the task.
        #[serde(default)]
        context_cids: Vec<String>,
        /// Optional deadline as Unix timestamp in milliseconds.
        /// Task auto-transitions to Failed when deadline expires.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deadline_ms: Option<u64>,
    },

    /// Query the status of a delegated task (F-14).
    #[serde(rename = "query_task_status")]
    QueryTaskStatus {
        task_id: String,
    },

    /// Start working on a task — transitions from Pending to InProgress (F-14).
    #[serde(rename = "task_start")]
    TaskStart {
        task_id: String,
        agent_id: String,
    },

    /// Report task completion with result CIDs (F-14).
    #[serde(rename = "task_complete")]
    TaskComplete {
        task_id: String,
        agent_id: String,
        result_cids: Vec<String>,
    },

    /// Report task failure with reason (F-14).
    #[serde(rename = "task_fail")]
    TaskFail {
        task_id: String,
        agent_id: String,
        reason: String,
    },

    // ── Event History (v7.0) ───────────────────────────────────

    #[serde(rename = "event_history")]
    EventHistory {
        #[serde(default)]
        since_seq: Option<u64>,
        #[serde(default)]
        agent_id_filter: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
    },

    // ── Agent Skill Registry (v8.0) ───────────────────────────

    #[serde(rename = "register_skill")]
    RegisterSkill {
        agent_id: String,
        name: String,
        description: String,
        #[serde(default)]
        tags: Vec<String>,
    },

    #[serde(rename = "discover_skills")]
    DiscoverSkills {
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        agent_id_filter: Option<String>,
        #[serde(default)]
        tag_filter: Option<String>,
    },

    // ── Tenant Management (Phase 3C) ──────────────────────────────

    /// Create a new tenant.
    #[serde(rename = "create_tenant")]
    CreateTenant {
        /// Unique tenant identifier (must be non-empty).
        tenant_id: String,
        /// Agent ID of the tenant administrator.
        admin_agent_id: String,
        /// Agent performing the operation (must be trusted or system).
        caller_agent_id: String,
    },

    /// List all tenants accessible to the calling agent.
    #[serde(rename = "list_tenants")]
    ListTenants {
        /// Agent performing the operation.
        agent_id: String,
    },

    /// Share resources between tenants (requires CrossTenant permission).
    #[serde(rename = "tenant_share")]
    TenantShare {
        /// Source tenant ID.
        from_tenant: String,
        /// Destination tenant ID.
        to_tenant: String,
        /// Resource type: "kg" | "memory" | "cas"
        resource_type: String,
        /// Tag pattern to match resources (e.g., "project-x*" or "*").
        resource_pattern: String,
        /// Agent performing the operation.
        agent_id: String,
    },

    // ── Proactive Context Assembly (F-2) ───────────────────────────

    /// Declare an intent and trigger asynchronous semantic prefetch.
    /// Returns an assembly_id for later FetchAssembledContext call.
    #[serde(rename = "declare_intent")]
    DeclareIntent {
        agent_id: String,
        /// Natural-language intent description (e.g. "修复 auth 模块测试失败").
        intent: String,
        /// Optional: known related object CIDs.
        #[serde(default)]
        related_cids: Vec<String>,
        /// Expected context budget in tokens.
        #[serde(default = "default_budget_tokens")]
        budget_tokens: usize,
    },

    /// Fetch the result of a previously declared intent prefetch.
    #[serde(rename = "fetch_assembled_context")]
    FetchAssembledContext {
        agent_id: String,
        /// The assembly_id returned by DeclareIntent.
        assembly_id: String,
    },

    // ── Adaptive Prefetch (F-15) ─────────────────────────────────────────────

    /// Report feedback about which CIDs were actually used vs prefetched but unused.
    /// Enables adaptive prefetch learning — future prefetches prioritize historically-used CIDs.
    #[serde(rename = "intent_feedback")]
    IntentFeedback {
        /// The intent_id this feedback is for.
        intent_id: String,
        /// CIDs that were actually read/used by the agent.
        used_cids: Vec<String>,
        /// CIDs that were in the prefetch assembly but not used.
        unused_cids: Vec<String>,
        agent_id: String,
    },

    // ── Batch Operations (v15.0) ─────────────────────────────────

    /// Batch create multiple objects in a single call.
    /// Each item is processed independently — one failure does not affect others.
    #[serde(rename = "batch_create")]
    BatchCreate {
        items: Vec<BatchCreateItem>,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    /// Batch store multiple memory entries in a single call.
    /// Each entry is stored independently in the working tier.
    #[serde(rename = "batch_memory_store")]
    BatchMemoryStore {
        entries: Vec<BatchMemoryEntry>,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    /// Batch submit multiple intents in a single call.
    #[serde(rename = "batch_submit_intent")]
    BatchSubmitIntent {
        intents: Vec<IntentSpec>,
        agent_id: String,
    },

    /// Batch query multiple objects/memories in a single call.
    #[serde(rename = "batch_query")]
    BatchQuery {
        queries: Vec<QuerySpec>,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    // ── KG Causal Reasoning (v16.0) ────────────────────────────────────────

    /// Find causal paths between two KG nodes.
    #[serde(rename = "kg_causal_path")]
    KGCausalPath {
        source_id: String,
        target_id: String,
        #[serde(default)]
        max_depth: u8,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    /// Analyze the impact of modifying or removing a node.
    #[serde(rename = "kg_impact_analysis")]
    KGImpactAnalysis {
        node_id: String,
        #[serde(default)]
        propagation_depth: u8,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    /// Get temporal changes between two timestamps.
    #[serde(rename = "kg_temporal_changes")]
    KGTemporalChanges {
        from_ms: u64,
        to_ms: u64,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    // ── Model Hot-Swap (v18.0) ────────────────────────────────────────────

    /// Switch embedding model at runtime without restart.
    #[serde(rename = "switch_embedding_model")]
    SwitchEmbeddingModel {
        /// Backend type: "local", "ollama", "openai", "stub"
        model_type: String,
        /// Model identifier, e.g. "BAAI/bge-small-en-v1.5"
        model_id: String,
        /// Optional python interpreter path for local backend
        #[serde(default, skip_serializing_if = "Option::is_none")]
        python_path: Option<String>,
    },

    /// Switch LLM model at runtime without restart.
    #[serde(rename = "switch_llm_model")]
    SwitchLlmModel {
        /// Backend: "ollama", "openai", "llama", "stub"
        backend: String,
        /// Model name, e.g. "llama3.2"
        model: String,
        /// Optional URL override
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },

    /// Check if a model is currently available and responsive.
    #[serde(rename = "check_model_health")]
    CheckModelHealth {
        /// Model type: "embedding" or "llm"
        model_type: String,
    },

    // ── Hybrid Retrieval / Graph-RAG (F-11) ────────────────────────────────

    /// Hybrid retrieval combining vector search and knowledge graph traversal.
    /// Returns results with provenance showing the causal path from query to result.
    #[serde(rename = "hybrid_retrieve")]
    HybridRetrieve {
        query_text: String,
        /// Optional: KG seed node tags to start graph traversal from.
        #[serde(default)]
        seed_tags: Vec<String>,
        /// Graph traversal depth (default 2).
        #[serde(default)]
        graph_depth: u8,
        /// Optional: filter to only these edge types (e.g., ["causes", "has_resolution"]).
        #[serde(default)]
        edge_types: Vec<String>,
        /// Maximum number of results to return (default 20).
        #[serde(default)]
        max_results: usize,
        /// Token budget limit — stops adding results when budget is reached.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_budget: Option<usize>,
        /// Agent performing the operation.
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    // ── Growth Report (F-13) ─────────────────────────────────────────────

    /// Query a growth report for an agent showing learning progress and efficiency.
    #[serde(rename = "query_growth_report")]
    QueryGrowthReport {
        /// Agent to generate report for.
        agent_id: String,
        /// Time period for the report.
        period: GrowthPeriod,
    },

    // ── Memory Stats (F-17) ───────────────────────────────────────────

    /// Query memory usage statistics for an agent's tier.
    #[serde(rename = "memory_stats")]
    MemoryStats {
        /// Agent to query stats for.
        agent_id: String,
        /// Memory tier to query (defaults to all tiers if omitted).
        #[serde(default)]
        tier: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    // ── Knowledge Discovery (F-16) ──────────────────────────────────

    /// Discover shared knowledge from other agents.
    #[serde(rename = "discover_knowledge")]
    DiscoverKnowledge {
        /// Semantic search query.
        query: String,
        /// Scope of the discovery search.
        #[serde(default)]
        scope: DiscoveryScope,
        /// Filter by knowledge types (empty = all types).
        #[serde(default)]
        knowledge_types: Vec<KnowledgeType>,
        /// Maximum number of results to return.
        #[serde(default = "default_max_results")]
        max_results: usize,
        /// Optional token budget limit.
        #[serde(default)]
        token_budget: Option<usize>,
        /// Agent performing the discovery.
        agent_id: String,
    },

    // ── Storage Governance (F-18) ──────────────────────────────────

    /// Query CAS object usage statistics for a CID.
    #[serde(rename = "object_usage")]
    ObjectUsage {
        cid: String,
        agent_id: String,
    },

    /// Query complete storage statistics.
    #[serde(rename = "storage_stats")]
    StorageStats {
        agent_id: String,
    },

    /// Evict cold (unused) objects from CAS.
    #[serde(rename = "evict_cold")]
    EvictCold {
        agent_id: String,
        #[serde(default)]
        dry_run: bool,
    },

    // ── Hook Management (Daemon-First) ──────────────────────────────

    /// List all registered lifecycle hooks.
    #[serde(rename = "hook_list")]
    HookList,

    /// Register a lifecycle hook via API (block or log action).
    #[serde(rename = "hook_register")]
    HookRegister {
        /// Hook interception point: "PreToolCall", "PostToolCall", "PreWrite", "PreDelete", "PreSessionStart"
        point: String,
        /// Action to take: "block" or "log"
        action: String,
        /// Optional tool name pattern to match (substring match).
        #[serde(default)]
        tool_pattern: Option<String>,
        /// Reason string for block actions.
        #[serde(default)]
        reason: Option<String>,
        /// Priority (lower = earlier). Default: 50.
        #[serde(default)]
        priority: Option<i32>,
    },

    // ── Health Report (Daemon-First) ────────────────────────────────

    /// Query system health report with detailed subsystem status.
    #[serde(rename = "health_report")]
    HealthReport,

    // ── Token Cost Ledger (F-2) ─────────────────────────────────────

    /// Get cost summary for a session (F-2).
    #[serde(rename = "cost_session_summary")]
    CostSessionSummary {
        session_id: String,
    },

    /// Get cost trend for an agent (F-2).
    #[serde(rename = "cost_agent_trend")]
    CostAgentTrend {
        agent_id: String,
        last_n_sessions: usize,
    },

    /// Check for cost anomaly (F-2).
    #[serde(rename = "cost_anomaly_check")]
    CostAnomalyCheck {
        agent_id: String,
    },

    // ── Prompt Registry (v31) ─────────────────────────────────────

    /// List all registered prompt names.
    #[serde(rename = "list_prompts")]
    ListPrompts,

    /// Get info about a prompt (resolved with optional agent override).
    #[serde(rename = "get_prompt_info")]
    GetPromptInfo {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
    },

    /// Set a prompt override (global if agent_id is None, per-agent otherwise).
    #[serde(rename = "set_prompt_override")]
    SetPromptOverride {
        name: String,
        template: String,
        #[serde(default)]
        variables: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
    },

    /// Remove a prompt override.
    #[serde(rename = "remove_prompt_override")]
    RemovePromptOverride {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
    },

    // ── Batch Long-Term Memory (v31) ──────────────────────────────

    /// Batch store multiple long-term memories with a single batched embedding call.
    #[serde(rename = "remember_long_term_batch")]
    RememberLongTermBatch {
        agent_id: String,
        /// Each item: (content, tags, importance).
        items: Vec<BatchLongTermItem>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },

    // ── File Import (v33) ─────────────────────────────────────────

    /// Import files from a local directory into CAS with optional chunking.
    ///
    /// The daemon reads files at the given paths, stores each in CAS,
    /// and applies the specified chunking mode. Tags are auto-generated
    /// from filename and user-supplied extras.
    #[serde(rename = "import_files")]
    ImportFiles {
        /// Absolute paths to files on the daemon's host filesystem.
        paths: Vec<String>,
        agent_id: String,
        /// Extra tags to apply to all imported objects.
        #[serde(default)]
        tags: Vec<String>,
        /// Chunking mode override: `"markdown"` | `"fixed"` | `"semantic"` | `"none"`.
        /// Defaults to auto-detect from file extension.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chunking: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },
}

/// A JSON API response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse {
    /// Always true for successful responses.
    pub ok: bool,
    /// The API version of this response (defaults to current stable version).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<ApiVersion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results: Option<Vec<SearchResultDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<AgentDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub neighbors: Option<Vec<NeighborDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Vec<DeletedDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<EventSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes: Option<Vec<KGNodeDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<Vec<KGNodeDto>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edges: Option<Vec<KGEdgeDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assembly_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_intents: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<crate::tool::ToolDescriptor>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<crate::tool::ToolResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_intents: Option<Vec<crate::intent::ResolvedIntent>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<crate::scheduler::messaging::AgentMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_data: Option<LoadedContextDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Human-readable operation confirmation message (F-47).
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Machine-readable error code for diagnostics (F-48).
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Suggested fix for error recovery (F-48).
    pub fix_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Suggested next actions for error recovery (F-48).
    pub next_actions: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kernel_events: Option<Vec<crate::kernel::event_bus::KernelEvent>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_status: Option<SystemStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_assembly: Option<crate::fs::context_budget::BudgetAllocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_usage: Option<AgentUsageDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_cards: Option<Vec<AgentCardDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delegation: Option<DelegationResultDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_history: Option<Vec<crate::kernel::event_bus::SequencedEvent>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovered_skills: Option<Vec<SkillDto>>,
    /// Token issued to an agent on registration (returned in RegisterAgent response).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// List of tenants (returned in ListTenants response).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenants: Option<Vec<TenantDto>>,
    /// Correlation ID for distributed tracing (v14.0).
    /// Present in responses when a correlation ID was passed or generated for the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    /// Batch create results (v15.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_create: Option<BatchCreateResponse>,
    /// Batch memory store results (v15.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_memory_store: Option<BatchMemoryStoreResponse>,
    /// Batch submit intent results (v15.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_submit_intent: Option<BatchSubmitIntentResponse>,
    /// Batch query results (v15.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_query: Option<BatchQueryResponse>,
    /// Causal path results (v16.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub causal_paths: Option<Vec<CausalPathDto>>,
    /// Impact analysis result (v16.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub impact_analysis: Option<ImpactAnalysisDto>,
    /// Temporal changes result (v16.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporal_changes: Option<Vec<TemporalChangeDto>>,
    /// Model switch response (v18.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_switch: Option<ModelSwitchResponse>,
    /// Model health check response (v18.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_health: Option<ModelHealthResponse>,
    /// Cache statistics (v19.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_stats: Option<CacheStatsDto>,
    /// Intent cache statistics (F-9).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_cache_stats: Option<IntentCacheStatsDto>,
    /// Cluster status (v20.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_status: Option<ClusterStatusDto>,
    /// Token estimate for the response content (F-8).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_estimate: Option<usize>,
    /// Delta result for change queries (F-7).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_result: Option<DeltaResult>,
    /// Session started result (F-6).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_started: Option<SessionStarted>,
    /// Session ended result (F-6).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ended: Option<SessionEnded>,
    /// Hybrid retrieval result — Graph-RAG combining vector search + KG traversal (F-11).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hybrid_result: Option<HybridResult>,
    /// Growth report showing agent learning progress and efficiency (F-13).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub growth_report: Option<GrowthReport>,
    /// Task result for delegation queries (F-14).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_result: Option<TaskResult>,
    /// Memory stats (F-17).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_stats: Option<MemoryStatsResult>,
    /// Knowledge discovery result (F-16).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovery_result: Option<DiscoveryResult>,
    /// Object usage stats (F-18).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_usage: Option<ObjectUsageResult>,
    /// Storage statistics (F-18).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_stats: Option<StorageStatsResult>,
    /// Evict cold result (F-18).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evict_result: Option<EvictColdResult>,
    /// Health report (F-7).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_report: Option<HealthReport>,
    /// Hook list result (Daemon-First).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook_list: Option<Vec<HookEntryDto>>,
    /// Cost session summary (F-2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_session_summary: Option<SessionCostSummary>,
    /// Cost agent trend (F-2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_agent_trend: Option<Vec<SessionCostSummary>>,
    /// Cost anomaly result (F-2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_anomaly: Option<CostAnomalyResult>,
    /// File import results (v33).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_results: Option<Vec<ImportFileResult>>,
}

/// Result for a single file import.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportFileResult {
    pub path: String,
    pub cid: Option<String>,
    pub chunks: usize,
    pub ok: bool,
    pub error: Option<String>,
}

/// Cost summary for a session (F-2).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionCostSummary {
    pub session_id: String,
    pub agent_id: String,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_millicents: u64,
    pub operations_count: u32,
    pub cache_hits: u32,
    pub cache_misses: u32,
}

/// Cost anomaly detection result (F-2).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CostAnomalyResult {
    pub agent_id: String,
    pub severity: String,
    pub message: String,
    pub avg_cost_per_session_before: u64,
    pub avg_cost_per_session_after: u64,
}

impl Default for ApiResponse {
    fn default() -> Self {
        Self {
            ok: false,
            version: Some(ApiVersion::CURRENT),
            cid: None, node_id: None, data: None, results: None,
            agent_id: None, agents: None, memory: None, tags: None,
            neighbors: None, deleted: None, events: None, nodes: None,
            paths: None, edges: None, intent_id: None, assembly_id: None,
            agent_state: None,
            pending_intents: None, tools: None, tool_result: None,
            resolved_intents: None, messages: None, context_data: None,
            error: None, message: None, error_code: None, fix_hint: None, next_actions: None, total_count: None, has_more: None,
            subscription_id: None, kernel_events: None,
            system_status: None,
            context_assembly: None,
            agent_usage: None,
            agent_cards: None,
            delegation: None,
            event_history: None,
            discovered_skills: None,
            token: None,
            tenants: None,
            correlation_id: None,
            batch_create: None,
            batch_memory_store: None,
            batch_submit_intent: None,
            batch_query: None,
            causal_paths: None,
            impact_analysis: None,
            temporal_changes: None,
            model_switch: None,
            model_health: None,
            cache_stats: None,
            intent_cache_stats: None,
            cluster_status: None,
            token_estimate: None,
            delta_result: None,
            session_started: None,
            session_ended: None,
            hybrid_result: None,
            growth_report: None,
            task_result: None,
            memory_stats: None,
            discovery_result: None,
            object_usage: None,
            storage_stats: None,
            evict_result: None,
            health_report: None,
            hook_list: None,
            cost_session_summary: None,
            cost_agent_trend: None,
            cost_anomaly: None,
            import_results: None,
        }
    }
}

impl ApiResponse {
    pub fn ok() -> Self {
        Self { ok: true, ..Self::default() }
    }

    pub fn with_cid(cid: String) -> Self {
        let mut r = Self::ok();
        r.cid = Some(cid);
        r
    }

    pub fn with_node_id(node_id: String) -> Self {
        let mut r = Self::ok();
        r.node_id = Some(node_id);
        r
    }

    pub fn with_data(data: String) -> Self {
        let mut r = Self::ok();
        r.data = Some(data);
        r
    }

    pub fn with_events(events: Vec<EventSummary>) -> Self {
        let mut r = Self::ok();
        r.events = Some(events);
        r
    }

    pub fn with_nodes(nodes: Vec<KGNodeDto>) -> Self {
        let mut r = Self::ok();
        r.nodes = Some(nodes);
        r
    }

    pub fn with_paths(paths: Vec<Vec<KGNodeDto>>) -> Self {
        let mut r = Self::ok();
        r.paths = Some(paths);
        r
    }


    pub fn error(msg: impl Into<String>) -> Self {
        Self { error: Some(msg.into()), ..Self::default() }
    }

    /// Create an ok response with a human-readable confirmation message (F-47).
    pub fn ok_with_message(msg: impl Into<String>) -> Self {
        let mut r = Self::ok();
        r.message = Some(msg.into());
        r
    }

    /// Create an error response with structured diagnostics (F-48).
    pub fn error_with_diagnosis(
        msg: impl Into<String>,
        code: impl Into<String>,
        fix: impl Into<String>,
        next_actions: Vec<String>,
    ) -> Self {
        let mut r = Self::error(msg);
        r.error_code = Some(code.into());
        r.fix_hint = Some(fix.into());
        r.next_actions = Some(next_actions);
        r
    }

    /// Add a correlation ID to this response (for distributed tracing).
    pub fn with_correlation_id(mut self, correlation_id: String) -> Self {
        self.correlation_id = Some(correlation_id);
        self
    }

    /// Calculate and set the top-level token_estimate field based on the
    /// serialized JSON of this response.
    ///
    /// Call this before serializing the response to ensure 100% token cost
    /// visibility in all API responses.
    pub fn with_token_estimate(mut self) -> Self {
        let json = serde_json::to_string(&self).unwrap_or_default();
        self.token_estimate = Some(estimate_tokens(&json));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn decode_utf8_content_returns_bytes() {
        let result = decode_content("hello world", &ContentEncoding::Utf8).unwrap();
        assert_eq!(result, b"hello world");
    }

    #[test]
    fn decode_base64_content_returns_binary() {
        let binary = vec![0u8, 1, 2, 3, 0xFF, 0xFE];
        let encoded = base64::engine::general_purpose::STANDARD.encode(&binary);
        let result = decode_content(&encoded, &ContentEncoding::Base64).unwrap();
        assert_eq!(result, binary);
    }

    #[test]
    fn decode_base64_invalid_returns_error() {
        let result = decode_content("not-valid-base64!!!", &ContentEncoding::Base64);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("base64 decode error"));
    }

    #[test]
    fn content_encoding_default_is_utf8() {
        assert_eq!(ContentEncoding::default(), ContentEncoding::Utf8);
    }

    #[test]
    fn api_request_create_roundtrip_with_base64() {
        let req = ApiRequest::Create {
            api_version: None,
            content: "AAEC".to_string(), // base64 of [0,1,2]
            content_encoding: ContentEncoding::Base64,
            tags: vec!["image".to_string()],
            agent_id: "agent1".to_string(),
            tenant_id: None,
            agent_token: None,
            intent: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: ApiRequest = serde_json::from_str(&json).unwrap();
        if let ApiRequest::Create { content_encoding, .. } = decoded {
            assert_eq!(content_encoding, ContentEncoding::Base64);
        } else {
            panic!("wrong variant");
        }
    }

    // ── API Versioning Tests (v17.0) ─────────────────────────────────────────

    #[test]
    fn test_api_version_parsing() {
        let v = ApiVersion::parse("1.2.3").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
    }

    #[test]
    fn test_api_version_parse_invalid() {
        assert!(ApiVersion::parse("invalid").is_err());
        assert!(ApiVersion::parse("1.2").is_err());
        assert!(ApiVersion::parse("1.2.3.4").is_err());
    }

    #[test]
    fn test_version_constants() {
        assert_eq!(ApiVersion::V1.major, 1);
        assert_eq!(ApiVersion::CURRENT.major, 26);
    }

    #[test]
    fn test_version_display() {
        let v = ApiVersion::parse("1.2.0").unwrap();
        assert_eq!(format!("{}", v), "1.2.0");
    }

    #[test]
    fn test_version_from_str() {
        let v: ApiVersion = "2.0.0".parse().unwrap();
        assert_eq!(v.major, 2);
    }

    #[test]
    fn test_version_comparison() {
        let v1 = ApiVersion::parse("1.0.0").unwrap();
        let v2 = ApiVersion::parse("2.0.0").unwrap();
        let v3 = ApiVersion::parse("1.1.0").unwrap();
        assert!(v1 < v2);
        assert!(v1 < v3);
        assert!(v3 < v2);
    }


    #[test]
    fn test_api_request_with_version() {
        let json = r#"{"method":"create","api_version":"1.0.0","content":"test","tags":[],"agent_id":"a1"}"#;
        let req: ApiRequest = serde_json::from_str(json).unwrap();
        if let ApiRequest::Create { api_version, .. } = req {
            assert_eq!(api_version, Some(ApiVersion::parse("1.0.0").unwrap()));
        } else {
            panic!("expected Create");
        }
    }

    #[test]
    fn test_api_request_without_version_defaults() {
        let json = r#"{"method":"create","content":"test","tags":[],"agent_id":"a1"}"#;
        let req: ApiRequest = serde_json::from_str(json).unwrap();
        if let ApiRequest::Create { api_version, .. } = req {
            assert!(api_version.is_none());
        } else {
            panic!("expected Create");
        }
    }

    #[test]
    fn test_api_response_includes_version() {
        let resp = ApiResponse::ok();
        assert_eq!(resp.version, Some(ApiVersion::CURRENT));
    }

    #[test]
    fn test_version_ord_trait() {
        use std::collections::BTreeSet;
        let mut set = BTreeSet::new();
        set.insert(ApiVersion::parse("2.0.0").unwrap());
        set.insert(ApiVersion::parse("1.0.0").unwrap());
        set.insert(ApiVersion::parse("1.1.0").unwrap());
        let mut iter = set.into_iter();
        assert_eq!(iter.next().unwrap().major, 1);
        assert_eq!(iter.next().unwrap().minor, 1);
        assert_eq!(iter.next().unwrap().major, 2);
    }
}
