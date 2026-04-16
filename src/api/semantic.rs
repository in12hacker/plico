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
use crate::fs::{EventType, EventRelation, EventSummary, UserFact, ActionSuggestion};

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

    // ── Phase C: Behavioral Pipeline ─────────────────────────────────────────
    #[serde(rename = "add_event_observation")]
    AddEventObservation {
        event_id: String,
        observation_id: String,
        agent_id: String,
    },

    #[serde(rename = "get_event_observations")]
    GetEventObservations {
        event_id: String,
    },

    #[serde(rename = "add_user_fact")]
    AddUserFact {
        fact: UserFact,
    },

    #[serde(rename = "get_user_facts")]
    GetUserFacts {
        subject_id: String,
    },

    #[serde(rename = "infer_suggestions")]
    InferSuggestions {
        event_id: String,
    },

    #[serde(rename = "get_pending_suggestions")]
    GetPendingSuggestions,

    #[serde(rename = "confirm_suggestion")]
    ConfirmSuggestion {
        suggestion_id: String,
    },

    #[serde(rename = "dismiss_suggestion")]
    DismissSuggestion {
        suggestion_id: String,
    },

    // ── Project Self-Management ──────────────────────────────────────────────
    #[serde(rename = "project_status")]
    ProjectStatus { agent_id: String },

    /// Sync KG project nodes from current git state.
    /// Updates iter12 commit_hash, completed_phases, and DesignDoc nodes.
    #[serde(rename = "sync_project_state")]
    SyncProjectState,
}

/// A JSON API response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
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
    // ── Phase C: Behavioral Pipeline ─────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observations: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_facts: Option<Vec<UserFact>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestions: Option<Vec<ActionSuggestion>>,
    // ── Project Self-Management ──────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_status: Option<ProjectStatus>,
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

// ── Project Self-Management (Dogfooding Plico) ─────────────────────────────────

/// Project status — describes Plico's own development state.
/// Stored as KG nodes so it lives alongside all other AI memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectStatus {
    /// Current iteration number (e.g. 12).
    pub iteration: u32,
    /// Git branch name.
    pub git_branch: String,
    /// Git commit short hash.
    pub git_commit: String,
    /// All iterations in this project.
    pub iterations: Vec<IterationDto>,
    /// All active plans.
    pub plans: Vec<PlanDto>,
    /// All design documents.
    pub design_docs: Vec<DesignDocDto>,
    /// Soul alignment score (0–100).
    pub soul_alignment_percent: u8,
    /// Key gaps blocking 100% alignment.
    pub key_gaps: Vec<GapDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IterationDto {
    pub id: String,
    pub name: String,
    pub completed_phases: Vec<String>,
    pub active_phase: Option<String>,
    pub commit_hash: String,
    pub date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanDto {
    pub id: String,
    pub title: String,
    pub phase: String,
    pub status: String, // "pending" | "in_progress" | "done"
    pub priority: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignDocDto {
    pub id: String,
    pub name: String,
    pub path: String,
    pub version: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GapDto {
    pub title: String,
    pub priority: String, // "P0" | "P1" | "P2"
    pub blocks: Vec<String>, // what this gap blocks
    pub description: String,
}

// ── Dashboard / Project Status Types ───────────────────────────────────────────

/// Full dashboard status — served over HTTP on a separate port.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardStatus {
    pub iteration: u32,
    pub started_at: u64,
    pub now: i64,
    pub git_branch: String,
    pub git_commit: String,
    pub tests_passed: Option<bool>,
    pub cas_object_count: usize,
    pub agent_count: usize,
    pub tag_count: usize,
    pub kg_node_count: usize,
    pub kg_edge_count: usize,
    pub event_count: usize,
    pub pending_suggestions: usize,
    pub phases: Vec<PhaseStatus>,
    pub modules: Vec<ModuleStatus>,
    pub soul_alignment: SoulAlignment,
    pub examples: Vec<ExampleCoverage>,
    pub next_steps: Vec<NextStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseStatus {
    pub name: String,
    pub percent: u8,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleStatus {
    pub name: String,
    pub path: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulAlignment {
    pub principles: Vec<PrincipleStatus>,
    pub overall_percent: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrincipleStatus {
    pub number: u8,
    pub title: String,
    pub description: String,
    pub aligned: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExampleCoverage {
    pub name: String,
    pub reasoning_chain: Vec<ChainStep>,
    pub execution_chain: Vec<ChainStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainStep {
    pub name: String,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NextStep {
    pub order: u8,
    pub title: String,
    pub description: String,
    pub priority: String,
}

impl ApiResponse {
    pub fn ok() -> Self {
        Self {
            ok: true,
            cid: None,
            data: None,
            results: None,
            agent_id: None,
            agents: None,
            memory: None,
            tags: None,
            neighbors: None,
            deleted: None,
            events: None,
            observations: None,
            user_facts: None,
            suggestions: None,
            project_status: None,
            error: None,
        }
    }

    pub fn with_cid(cid: String) -> Self {
        Self {
            ok: true,
            cid: Some(cid),
            data: None,
            results: None,
            agent_id: None,
            agents: None,
            memory: None,
            tags: None,
            neighbors: None,
            deleted: None,
            events: None,
            observations: None,
            user_facts: None,
            suggestions: None,
            project_status: None,
            error: None,
        }
    }

    pub fn with_data(data: String) -> Self {
        Self {
            ok: true,
            cid: None,
            data: Some(data),
            results: None,
            agent_id: None,
            agents: None,
            memory: None,
            tags: None,
            neighbors: None,
            deleted: None,
            events: None,
            observations: None,
            user_facts: None,
            suggestions: None,
            project_status: None,
            error: None,
        }
    }

    pub fn with_events(events: Vec<EventSummary>) -> Self {
        Self {
            ok: true,
            cid: None,
            data: None,
            results: None,
            agent_id: None,
            agents: None,
            memory: None,
            tags: None,
            neighbors: None,
            deleted: None,
            events: Some(events),
            observations: None,
            user_facts: None,
            suggestions: None,
            project_status: None,
            error: None,
        }
    }

    pub fn with_observations(observations: Vec<String>) -> Self {
        Self {
            ok: true,
            cid: None,
            data: None,
            results: None,
            agent_id: None,
            agents: None,
            memory: None,
            tags: None,
            neighbors: None,
            deleted: None,
            events: None,
            observations: Some(observations),
            user_facts: None,
            suggestions: None,
            project_status: None,
            error: None,
        }
    }

    pub fn with_user_facts(facts: Vec<UserFact>) -> Self {
        Self {
            ok: true,
            cid: None,
            data: None,
            results: None,
            agent_id: None,
            agents: None,
            memory: None,
            tags: None,
            neighbors: None,
            deleted: None,
            events: None,
            observations: None,
            user_facts: Some(facts),
            suggestions: None,
            project_status: None,
            error: None,
        }
    }

    pub fn with_suggestions(suggestions: Vec<ActionSuggestion>) -> Self {
        Self {
            ok: true,
            cid: None,
            data: None,
            results: None,
            agent_id: None,
            agents: None,
            memory: None,
            tags: None,
            neighbors: None,
            deleted: None,
            events: None,
            observations: None,
            user_facts: None,
            suggestions: Some(suggestions),
            project_status: None,
            error: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            cid: None,
            data: None,
            results: None,
            agent_id: None,
            agents: None,
            memory: None,
            tags: None,
            neighbors: None,
            deleted: None,
            events: None,
            observations: None,
            user_facts: None,
            suggestions: None,
            project_status: None,
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
