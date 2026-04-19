//! AI-Friendly Semantic API
//!
//! Provides AI-native interfaces: semantic CLI and TCP server.
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
//! Deprecated endpoints return a deprecation notice in the response.

use base64::Engine;
use serde::{Deserialize, Serialize};
// ── Versioning Types (v17.0) ───────────────────────────────────────────

/// API version with semantic versioning (major.minor.patch).
///
/// # Examples
/// ```
/// use plico::api::semantic::ApiVersion;
/// let v = ApiVersion::parse("1.2.0").unwrap();
/// assert!(v.major == 1 && v.minor == 2 && v.patch == 0);
/// ```
///
/// Serializes/deserializes as a string like "1.2.0".
/// Can be deserialized from either "1.2.0" string or {major, minor, patch} struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ApiVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl serde::Serialize for ApiVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&format!("{}.{}.{}", self.major, self.minor, self.patch))
    }
}

impl<'de> serde::Deserialize<'de> for ApiVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct VersionVisitor;
        impl<'de> serde::de::Visitor<'de> for VersionVisitor {
            type Value = ApiVersion;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a version string like '1.2.0' or an object with major, minor, patch")
            }
            fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                ApiVersion::parse(s).map_err(serde::de::Error::custom)
            }
            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut major = None;
                let mut minor = None;
                let mut patch = None;
                while let Some(key) = map.next_key()? {
                    match key {
                        "major" => major = Some(map.next_value()?),
                        "minor" => minor = Some(map.next_value()?),
                        "patch" => patch = Some(map.next_value()?),
                        _ => {}
                    }
                }
                Ok(ApiVersion {
                    major: major.unwrap_or(0),
                    minor: minor.unwrap_or(0),
                    patch: patch.unwrap_or(0),
                })
            }
        }
        deserializer.deserialize_any(VersionVisitor)
    }
}

impl ApiVersion {
    /// Version 1.0.0 — initial stable release.
    pub const V1: ApiVersion = ApiVersion { major: 1, minor: 0, patch: 0 };
    /// Current stable version.
    pub const CURRENT: ApiVersion = ApiVersion { major: 18, minor: 0, patch: 0 };
    /// Minimum supported version (for compatibility checks).
    pub const MIN_SUPPORTED: ApiVersion = ApiVersion { major: 1, minor: 0, patch: 0 };

    /// Parse a version string like "1.2.0" into an ApiVersion.
    pub fn parse(s: &str) -> Result<Self, String> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(format!("invalid version format '{}', expected 'major.minor.patch'", s));
        }
        let major = parts[0].parse().map_err(|_| format!("invalid major version: {}", parts[0]))?;
        let minor = parts[1].parse().map_err(|_| format!("invalid minor version: {}", parts[1]))?;
        let patch = parts[2].parse().map_err(|_| format!("invalid patch version: {}", parts[2]))?;
        Ok(ApiVersion { major, minor, patch })
    }

    /// Check if this version supports a given feature.
    ///
    /// # Features
    /// - `"batch_operations"` — batch_create, batch_memory_store, batch_submit_intent, batch_query (v15.0+)
    /// - `"kg_causal"` — kg_causal_path, kg_impact_analysis, kg_temporal_changes (v16.0+)
    /// - `"deprecation_notices"` — response includes deprecation field (v17.0+)
    /// - `"tenant_management"` — create_tenant, list_tenants, tenant_share (v14.0+)
    /// - `"model_hot_swap"` — switch_embedding_model, switch_llm_model, check_model_health (v18.0+)
    pub fn supports(&self, feature: &str) -> bool {
        match feature {
            "batch_operations" => *self >= ApiVersion { major: 15, minor: 0, patch: 0 },
            "kg_causal" => *self >= ApiVersion { major: 16, minor: 0, patch: 0 },
            "deprecation_notices" => *self >= ApiVersion { major: 17, minor: 0, patch: 0 },
            "tenant_management" => *self >= ApiVersion { major: 14, minor: 0, patch: 0 },
            "model_hot_swap" => *self >= ApiVersion { major: 18, minor: 0, patch: 0 },
            _ => false,
        }
    }

    /// Check if this version is backward-compatible with another.
    /// Two versions are compatible if they have the same major version.
    pub fn is_compatible(&self, other: ApiVersion) -> bool {
        self.major == other.major
    }

    /// Returns true if this version is deprecated.
    pub fn is_deprecated(&self) -> bool {
        *self < (ApiVersion { major: 18, minor: 0, patch: 0 })
    }
}

impl Default for ApiVersion {
    fn default() -> Self {
        ApiVersion::CURRENT
    }
}

impl std::fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl std::str::FromStr for ApiVersion {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ApiVersion::parse(s)
    }
}

/// Deprecation notice included in API responses for deprecated endpoints.
///
/// When the server responds to a request using an older API version,
/// it may include a deprecation notice to inform the client of:
/// - When the endpoint was first deprecated
/// - When it will be removed entirely (sunset version)
/// - A migration message suggesting the replacement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeprecationNotice {
    /// The API version when this endpoint/field was first deprecated.
    pub deprecated_since: ApiVersion,
    /// The API version when this endpoint will be removed entirely.
    pub sunset_version: ApiVersion,
    /// A human-readable migration message.
    pub message: String,
}

/// Feature flags for version-specific behavior.
#[derive(Debug, Clone, Default)]
pub struct VersionFeatures {
    /// True if the request supports batch operations (v15.0+).
    pub batch_operations: bool,
    /// True if the request supports KG causal reasoning (v16.0+).
    pub kg_causal: bool,
    /// True if the response should include deprecation notices (v17.0+).
    pub deprecation_notices: bool,
    /// True if the request supports tenant management (v14.0+).
    pub tenant_management: bool,
    /// True if the request supports model hot-swap (v18.0+).
    pub model_hot_swap: bool,
}

impl VersionFeatures {
    /// Derive feature flags from an API version.
    pub fn from_version(version: ApiVersion) -> Self {
        VersionFeatures {
            batch_operations: version.supports("batch_operations"),
            kg_causal: version.supports("kg_causal"),
            deprecation_notices: version.supports("deprecation_notices"),
            tenant_management: version.supports("tenant_management"),
            model_hot_swap: version.supports("model_hot_swap"),
        }
    }
}

/// Check if a request version supports a given feature.
/// Returns true for None (defaults to CURRENT, which supports all features).
pub fn version_supports(version: Option<ApiVersion>, feature: &str) -> bool {
    version.unwrap_or(ApiVersion::CURRENT).supports(feature)
}

/// Get a deprecation notice for old API variants.
/// Returns Some(DeprecationNotice) if the request uses a deprecated version.
pub fn get_deprecation_notice(_request: &ApiRequest) -> Option<DeprecationNotice> {
    // Currently all requests default to CURRENT (v17.0), so no deprecation
    // This function is provided for future use when older versions are deprecated
    None
}

// ── Re-exports for use by other modules ───────────────────────────────────────

pub use version_supports as check_version_feature;
pub use get_deprecation_notice as notice_for_request;

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
/// This is a coarse approximation suitable for POC.
/// Note: This is an estimate, not precise. For code the result may be high,
/// for non-ASCII text (e.g., Chinese) the result may be low.
pub fn estimate_tokens(text: &str) -> usize {
    let ascii = text.chars().filter(|c| c.is_ascii()).count();
    let non_ascii = text.chars().filter(|c| !c.is_ascii()).count();
    (ascii + 3) / 4 + (non_ascii + 1) / 2
}

fn default_importance() -> u8 { 50 }
fn default_k() -> usize { 10 }
fn default_priority() -> String { "medium".to_string() }
fn default_budget_tokens() -> usize { 4096 }
fn default_auto_checkpoint() -> bool { true }

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

    // ── Model Hot-Swap (v18.0) ────────────────────────────────────────────

    /// Switch embedding model at runtime without restart.
    #[serde(rename = "switch_embedding_model")]
    SwitchEmbeddingModel {
        /// Backend type: "local", "ollama", "stub"
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
        /// Backend: "ollama", "openai", "stub"
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
    /// Always true for successful responses.
    pub ok: bool,
    /// The API version of this response (defaults to current stable version).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<ApiVersion>,
    /// Deprecation notice if the request used a deprecated API version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<DeprecationNotice>,
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
}

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

// ── Delta感知 structures (F-7) ─────────────────────────────────────────────────

/// A single change entry returned by DeltaSince.
/// Lightweight metadata summary — no LLM required.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeEntry {
    /// Content identifier of the changed object.
    pub cid: String,
    /// Type of change: "created", "modified", "deleted", "tags_changed", etc.
    pub change_type: String,
    /// Human-readable summary: "{event_type} {cid[..8]} by {agent_id} [{tags}]"
    pub summary: String,
    /// Unix timestamp (ms) when the change occurred.
    pub changed_at_ms: u64,
    /// Agent ID that triggered the change.
    pub changed_by: String,
    /// Event sequence number.
    pub seq: u64,
}

/// Response for a DeltaSince query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaResult {
    /// List of changes since the given sequence.
    pub changes: Vec<ChangeEntry>,
    /// The sequence number queried from (exclusive).
    pub from_seq: u64,
    /// The latest sequence number in this result.
    pub to_seq: u64,
    /// Estimated token count for transmitting these changes.
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
    /// Edge cache statistics (v19.0)
    pub cache_stats: Option<CacheStatsDto>,
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
            ok: true,
            version: Some(ApiVersion::CURRENT),
            deprecation: None,
            cid: None, node_id: None, data: None, results: None,
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
            model_switch: None,
            model_health: None,
            cache_stats: None,
            intent_cache_stats: None,
            cluster_status: None,
            token_estimate: None,
            delta_result: None,
            session_started: None,
            session_ended: None,
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

    pub fn with_deprecation(mut self, notice: DeprecationNotice) -> Self {
        self.deprecation = Some(notice);
        self
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            version: Some(ApiVersion::CURRENT),
            deprecation: None,
            cid: None, node_id: None, data: None, results: None,
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
            model_switch: None,
            model_health: None,
            cache_stats: None,
            intent_cache_stats: None,
            cluster_status: None,
            token_estimate: None,
            delta_result: None,
            session_started: None,
            session_ended: None,
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
        assert_eq!(ApiVersion::CURRENT.major, 18);
        assert_eq!(ApiVersion::MIN_SUPPORTED.major, 1);
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
    fn test_version_compatibility() {
        let v1_0 = ApiVersion::parse("1.0.0").unwrap();
        let v1_5 = ApiVersion::parse("1.5.0").unwrap();
        let v2_0 = ApiVersion::parse("2.0.0").unwrap();
        // Same major version = compatible
        assert!(v1_0.is_compatible(v1_5));
        assert!(v1_5.is_compatible(v1_0));
        // Different major version = not compatible
        assert!(!v1_0.is_compatible(v2_0));
        assert!(!v2_0.is_compatible(v1_0));
    }

    #[test]
    fn test_version_supports_feature() {
        let v15 = ApiVersion::parse("15.0.0").unwrap();
        let v16 = ApiVersion::parse("16.0.0").unwrap();
        let v17 = ApiVersion::parse("17.0.0").unwrap();
        let v14 = ApiVersion::parse("14.0.0").unwrap();

        // Batch operations introduced in v15
        assert!(!v14.supports("batch_operations"));
        assert!(v15.supports("batch_operations"));
        assert!(v16.supports("batch_operations"));

        // KG causal introduced in v16
        assert!(!v15.supports("kg_causal"));
        assert!(v16.supports("kg_causal"));
        assert!(v17.supports("kg_causal"));

        // Deprecation notices introduced in v17
        assert!(!v16.supports("deprecation_notices"));
        assert!(v17.supports("deprecation_notices"));

        // Tenant management introduced in v14
        assert!(v14.supports("tenant_management"));
        assert!(!v13_supports_tenant(v14));
    }

    fn v13_supports_tenant(_v: ApiVersion) -> bool {
        false
    }

    #[test]
    fn test_version_supports_none_defaults_to_current() {
        // None version should default to CURRENT which supports all features
        assert!(version_supports(None, "batch_operations"));
        assert!(version_supports(None, "kg_causal"));
        assert!(version_supports(None, "deprecation_notices"));
    }

    #[test]
    fn test_version_is_deprecated() {
        assert!(ApiVersion::parse("16.0.0").unwrap().is_deprecated());
        assert!(ApiVersion::parse("17.0.0").unwrap().is_deprecated());
        assert!(!ApiVersion::parse("18.0.0").unwrap().is_deprecated());
        assert!(ApiVersion::parse("1.0.0").unwrap().is_deprecated());
    }

    #[test]
    fn test_deprecation_notice_structure() {
        let notice = DeprecationNotice {
            deprecated_since: ApiVersion::parse("18.0.0").unwrap(),
            sunset_version: ApiVersion::parse("19.0.0").unwrap(),
            message: "Upgrade to v18.0 or later".to_string(),
        };
        let json = serde_json::to_string(&notice).unwrap();
        let decoded: DeprecationNotice = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.sunset_version.major, 19);
    }

    #[test]
    fn test_version_features_from_version() {
        let features = VersionFeatures::from_version(ApiVersion::parse("17.0.0").unwrap());
        assert!(features.deprecation_notices);
        assert!(features.batch_operations);
        assert!(features.kg_causal);
        assert!(features.tenant_management);
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
        assert!(resp.deprecation.is_none());
    }

    #[test]
    fn test_api_response_with_deprecation() {
        let notice = DeprecationNotice {
            deprecated_since: ApiVersion::parse("17.0.0").unwrap(),
            sunset_version: ApiVersion::parse("18.0.0").unwrap(),
            message: "Deprecated".to_string(),
        };
        let resp = ApiResponse::ok().with_deprecation(notice);
        assert!(resp.deprecation.is_some());
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
