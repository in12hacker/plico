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
        intent: Option<String>,
    },

    #[serde(rename = "read")]
    Read { cid: String, agent_id: String },

    #[serde(rename = "search")]
    Search {
        query: String,
        agent_id: String,
        limit: Option<usize>,
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
    },

    #[serde(rename = "delete")]
    Delete { cid: String, agent_id: String },

    #[serde(rename = "register_agent")]
    RegisterAgent { name: String },

    #[serde(rename = "list_agents")]
    ListAgents,

    #[serde(rename = "remember")]
    Remember { agent_id: String, content: String },

    #[serde(rename = "recall")]
    Recall { agent_id: String },

    #[serde(rename = "explore")]
    Explore { cid: String, edge_type: Option<String>, depth: Option<u8>, agent_id: String },

    #[serde(rename = "list_deleted")]
    ListDeleted { agent_id: String },

    #[serde(rename = "restore")]
    Restore { cid: String, agent_id: String },

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
    },

    #[serde(rename = "add_edge")]
    AddEdge {
        src_id: String,
        dst_id: String,
        edge_type: KGEdgeType,
        #[serde(default)]
        weight: Option<f32>,
        agent_id: String,
    },

    #[serde(rename = "list_nodes")]
    ListNodes {
        #[serde(default)]
        node_type: Option<KGNodeType>,
        agent_id: String,
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

    // ── Intent Resolution ─────────────────────────────────────────────

    #[serde(rename = "intent_resolve")]
    IntentResolve { text: String, agent_id: String },

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
    },

    #[serde(rename = "ack_message")]
    AckMessage {
        agent_id: String,
        message_id: String,
    },
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
    pub intent_id: Option<String>,
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

// ── Project Self-Management (Dogfooding Plico) ─────────────────────────────────


// ── Dashboard / Project Status Types ───────────────────────────────────────────

/// Full dashboard status — served over HTTP on a separate port.
/// Runtime kernel metrics — reports live system state, not development plans.
///
/// Follows the health-check + metrics separation pattern:
/// all fields are computed from actual kernel state at query time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardStatus {
    pub timestamp_ms: i64,
    pub cas_object_count: usize,
    pub agent_count: usize,
    pub tag_count: usize,
    pub kg_node_count: usize,
    pub kg_edge_count: usize,
}

impl ApiResponse {
    pub fn ok() -> Self {
        Self {
            ok: true, cid: None, node_id: None, data: None, results: None,
            agent_id: None, agents: None, memory: None, tags: None,
            neighbors: None, deleted: None, events: None, nodes: None,
            paths: None, intent_id: None, agent_state: None,
            pending_intents: None, tools: None, tool_result: None,
            resolved_intents: None, messages: None, error: None,
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
            paths: None, intent_id: None, agent_state: None,
            pending_intents: None, tools: None, tool_result: None,
            resolved_intents: None, messages: None,
            error: Some(msg.into()),
        }
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
