//! AI-Friendly Semantic API
//!
//! Provides AI-native interfaces: semantic CLI and TCP server.
//!
//! # CLI Interface (aicli)
//!
//! AI agents invoke operations via structured commands:
//! ```bash
//! aicli create --content "..." --tags "meeting,project-x"
//! aicli read --cid <CID>
//! aicli search --query "project-x meeting notes"
//! aicli update --cid <CID> --content "..."
//! aicli delete --cid <CID>
//! ```
//!
//! # TCP Server (plicod)
//!
//! Long-running daemon exposing a semantic API over TCP for external AI programs.
//! Protocol: JSON messages over TCP.
//!
//! # JSON Protocol
//!
//! Request:
//! ```json
//! {"method": "create", "params": {"content": "...", "tags": ["..."], "agent_id": "agent1"}}
//! ```
//!
//! Response:
//! ```json
//! {"ok": true, "cid": "abc123..."}
//! ```
//!
//! Error:
//! ```json
//! {"ok": false, "error": "permission denied"}
//! ```

use base64::Engine;
use serde::{Deserialize, Serialize};
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

fn default_importance() -> u8 { 50 }
fn default_k() -> usize { 10 }
fn default_priority() -> String { "medium".to_string() }
fn default_budget_tokens() -> usize { 4096 }

/// DTO for procedure steps in API requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcedureStepDto {
    pub description: String,
    pub action: String,
    #[serde(default)]
    pub expected_outcome: Option<String>,
}

/// A JSON API request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method")]
pub enum ApiRequest {
    #[serde(rename = "create")]
    Create {
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
    Recall { agent_id: String },

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

    // ── Agent Discovery (v6.2) ──────────────────────────────────

    #[serde(rename = "discover_agents")]
    DiscoverAgents {
        #[serde(default)]
        state_filter: Option<String>,
        #[serde(default)]
        tool_filter: Option<String>,
        agent_id: String,
    },

    // ── Agent Delegation (v6.3) ─────────────────────────────────

    #[serde(rename = "delegate_task")]
    DelegateTask {
        from: String,
        to: String,
        description: String,
        #[serde(default)]
        action: Option<String>,
        #[serde(default = "default_priority")]
        priority: String,
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
}

/// An item within a BatchCreate request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCreateItem {
    /// Object content. Plain UTF-8 by default; set `content_encoding` for binary.
    pub content: String,
    /// Content encoding (default: utf8).
    #[serde(default)]
    pub content_encoding: ContentEncoding,
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

/// A JSON API response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse {
    pub ok: bool,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultDto {
    pub cid: String,
    pub relevance: f32,
    pub tags: Vec<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadedContextDto {
    pub cid: String,
    pub layer: String,
    pub content: String,
    pub tokens_estimate: usize,
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

// ── Project Self-Management (Dogfooding Plico) ─────────────────────────────────


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
}

/// Agent resource usage and quota snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentUsageDto {
    pub agent_id: String,
    pub memory_entries: usize,
    pub memory_quota: u64,
    pub tool_call_count: u64,
    pub cpu_time_quota: u64,
    pub allowed_tools: Vec<String>,
    pub last_active_ms: u64,
}

/// Agent capability card — what an agent can do and its current state.
/// Enables peer discovery: agents find collaborators by capability match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCardDto {
    pub agent_id: String,
    pub name: String,
    pub state: String,
    pub tools: Vec<String>,
    pub memory_entries: usize,
    pub tool_call_count: u64,
    pub last_active_ms: u64,
}

/// Result of a delegation operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationResultDto {
    pub intent_id: String,
    pub message_id: String,
    pub from: String,
    pub to: String,
}

impl ApiResponse {
    pub fn ok() -> Self {
        Self {
            ok: true, cid: None, node_id: None, data: None, results: None,
            agent_id: None, agents: None, memory: None, tags: None,
            neighbors: None, deleted: None, events: None, nodes: None,
            paths: None, edges: None, intent_id: None, assembly_id: None,
            agent_state: None,
            pending_intents: None, tools: None, tool_result: None,
            resolved_intents: None, messages: None, context_data: None,
            error: None, total_count: None, has_more: None,
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
        }
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
        Self {
            ok: false, cid: None, node_id: None, data: None, results: None,
            agent_id: None, agents: None, memory: None, tags: None,
            neighbors: None, deleted: None, events: None, nodes: None,
            paths: None, edges: None, intent_id: None, assembly_id: None,
            agent_state: None,
            pending_intents: None, tools: None, tool_result: None,
            resolved_intents: None, messages: None, context_data: None,
            error: Some(msg.into()),
            total_count: None, has_more: None,
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
        }
    }

    /// Add a correlation ID to this response (for distributed tracing).
    pub fn with_correlation_id(mut self, correlation_id: String) -> Self {
        self.correlation_id = Some(correlation_id);
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
}
