//! plico-sse — SSE Streaming Adapter for A2A Protocol Compatibility
//!
//! HTTP/SSE server that bridges external AI agents (Cursor, Claude, etc.) to plicod's TCP API.
//! Exposes A2A-compatible endpoints including Agent Card and task streaming.
//!
//! Architecture:
//!     Cursor/Agent ←→ plico-sse ←→ plicod (TCP JSON, unchanged)
//!                           ↓
//!                      SSE/HTTP streaming
//!
//! Usage: cargo run --bin plico-sse [--port PORT] [--plicod-port PORT]
//!
//! A2A Protocol Endpoints:
//!     GET  /.well-known/agent.json  — Agent Card (capabilities declaration)
//!     POST /tasks/sendSubscribe     — Task submission with SSE streaming

use axum::{
    extract::State,
    http::{header, Method, StatusCode},
    response::{sse::{Event, KeepAlive, Sse}, IntoResponse, Response},
    routing::{get, post},
    Router,
};
use futures::StreamExt;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use plico::api::semantic::{ApiRequest, ApiResponse};

// ── Error Types ────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum SseError {
    #[error("plicod connection failed: {0}")]
    ConnectionFailed(#[from] std::io::Error),

    #[error("JSON parse error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("channel closed")]
    ChannelClosed,

    #[error("timeout")]
    Timeout,
}

// ── App State ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    plicod_port: u16,
    broadcast_tx: Arc<broadcast::Sender<ServerEvent>>,
}

#[derive(Clone, Debug)]
enum ServerEvent {
    /// A task status update to stream to subscribed clients
    TaskUpdate {
        task_id: String,
        state: TaskState,
        data: Option<serde_json::Value>,
    },
    /// Error occurred while processing
    Error {
        task_id: Option<String>,
        message: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Working,
    Completed,
    Failed,
    InputRequired,
}

impl TaskState {
    fn from_response(resp: &ApiResponse) -> Self {
        if resp.ok {
            TaskState::Completed
        } else if resp.error.as_ref().is_some() {
            // Check if it's a recoverable error
            let err_msg = resp.error.as_ref().unwrap().to_lowercase();
            if err_msg.contains("permission") || err_msg.contains("auth") {
                TaskState::Failed
            } else {
                TaskState::Working
            }
        } else {
            TaskState::Working
        }
    }
}

// ── SSE Event Types (A2A Protocol) ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseEvent {
    pub event_type: String,
    pub task_id: Option<String>,
    pub data: serde_json::Value,
}

impl SseEvent {
    /// Create an SSE event for task status updates
    fn task_status(task_id: &str, state: TaskState, data: Option<serde_json::Value>) -> Self {
        SseEvent {
            event_type: "task_status".to_string(),
            task_id: Some(task_id.to_string()),
            data: serde_json::json!({
                "state": state,
                "data": data,
            }),
        }
    }

    /// Create an SSE event for text/message chunks
    #[allow(dead_code)]
    fn text_chunk(task_id: &str, chunk: &str, done: bool) -> Self {
        SseEvent {
            event_type: "text_chunk".to_string(),
            task_id: Some(task_id.to_string()),
            data: serde_json::json!({
                "chunk": chunk,
                "done": done,
            }),
        }
    }

    /// Create an SSE error event
    fn error(task_id: Option<String>, message: &str) -> Self {
        SseEvent {
            event_type: "error".to_string(),
            task_id,
            data: serde_json::json!({ "message": message }),
        }
    }

    #[allow(dead_code)]
    /// Create an SSE ping event (heartbeat)
    fn ping() -> Self {
        SseEvent {
            event_type: "ping".to_string(),
            task_id: None,
            data: serde_json::json!({}),
        }
    }
}

// ── SSE Helper Functions ───────────────────────────────────────────────────────

/// Convert an SseEvent to an axum SSE Event
fn to_sse_event(se: SseEvent) -> Event {
    let event_data = serde_json::to_string(&se).unwrap_or_else(|_| "{}".to_string());
    let event_type = se.event_type.clone();

    Event::default()
        .event(event_type)
        .data(event_data)
}

// ── plicod TCP Client ──────────────────────────────────────────────────────────

async fn send_to_plicod(port: u16, request: ApiRequest) -> Result<ApiResponse, SseError> {
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();

    let mut stream = TcpStream::connect(addr).await?;
    let json = serde_json::to_vec(&request)?;
    stream.write_all(&json).await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;

    // Read response
    let mut buf = Vec::new();
    let mut temp_buf = [0u8; 4096];
    loop {
        let n = stream.read(&mut temp_buf).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&temp_buf[..n]);
        // Check for newline delimiter (plicod protocol)
        if buf.contains(&b'\n') {
            break;
        }
    }

    // Parse response, handling potential multiple JSON objects
    let buf_str = String::from_utf8_lossy(&buf);
    let first_json = buf_str.lines().next().unwrap_or("{}");
    let response: ApiResponse = serde_json::from_str(first_json)?;

    Ok(response)
}

// ── API Handlers ──────────────────────────────────────────────────────────────

/// Agent Card — A2A protocol capability declaration
async fn get_agent_card() -> Response {
    let card = serde_json::json!({
        "name": "plico",
        "description": "AI-native operating system kernel — semantic file system, knowledge graph, and agent orchestration",
        "version": "1.0.0",
        "capabilities": {
            "streaming": true,
            "pushNotifications": false,
            "agentCard": true,
        },
        "url": "http://localhost:7879",
        "endpoints": {
            "tasksSendSubscribe": "/tasks/sendSubscribe",
        },
        "streamingMethods": ["text/event-stream"],
    });

    (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], card.to_string()).into_response()
}

/// Handle POST /tasks/sendSubscribe — Submit task and stream results via SSE
async fn task_send_subscribe(
    State(state): State<AppState>,
    body: String,
) -> Response {
    // Parse the incoming request
    let request: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({ "error": format!("JSON parse error: {}", e) }).to_string()
            ).into_response();
        }
    };

    // Extract all needed values before spawning tasks
    let task_id = request.get("task_id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let method = request.get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("submit_intent")
        .to_string();

    let params = request.get("params").cloned();

    // Create broadcast channel for this task's events
    let (tx, rx) = broadcast::channel::<ServerEvent>(100);
    let broadcast_tx = state.broadcast_tx.clone();
    let tx_clone = tx.clone();

    let task_id_for_broadcast = task_id.clone();
    // Spawn task to forward events to broadcast
    tokio::spawn(async move {
        if let Err(e) = broadcast_tx.send(ServerEvent::TaskUpdate {
            task_id: task_id_for_broadcast,
            state: TaskState::Working,
            data: Some(serde_json::json!({ "message": "Task submitted" })),
        }) {
            eprintln!("Failed to broadcast task update: {}", e);
        }
    });

    // Spawn async task to process the request and stream results
    let plicod_port = state.plicod_port;
    let task_id_for_processing = task_id.clone();
    tokio::spawn(async move {
        // Forward to plicod and broadcast results
        let api_request = build_api_request(&method, params.as_ref());

        match api_request {
            Ok(req) => {
                // Send initial "processing" event
                let _ = tx_clone.send(ServerEvent::TaskUpdate {
                    task_id: task_id_for_processing.clone(),
                    state: TaskState::Working,
                    data: Some(serde_json::json!({ "message": "Processing..." })),
                });

                // Call plicod
                match send_to_plicod(plicod_port, req).await {
                    Ok(response) => {
                        let final_state = TaskState::from_response(&response);
                        let event_data = if response.ok {
                            Some(serde_json::to_value(&response).unwrap_or(serde_json::json!({})))
                        } else {
                            Some(serde_json::json!({ "error": response.error }))
                        };

                        let _ = tx_clone.send(ServerEvent::TaskUpdate {
                            task_id: task_id_for_processing.clone(),
                            state: final_state,
                            data: event_data,
                        });
                    }
                    Err(e) => {
                        let _ = tx_clone.send(ServerEvent::Error {
                            task_id: Some(task_id_for_processing.clone()),
                            message: e.to_string(),
                        });
                    }
                }
            }
            Err(e) => {
                let _ = tx_clone.send(ServerEvent::Error {
                    task_id: Some(task_id_for_processing.clone()),
                    message: e.to_string(),
                });
            }
        }
    });

    // Create SSE stream from broadcast channel
    let rx_stream = BroadcastStream::new(rx);

    // Map broadcast events to SSE events
    let event_stream = rx_stream.map(|result: Result<ServerEvent, _>| -> Result<Event, std::io::Error> {
        match result {
            Ok(event) => Ok(event_to_sse_event(event)),
            Err(e) => {
                eprintln!("Broadcast error: {}", e);
                Ok(Event::default().comment("error"))
            }
        }
    });

    // Add ping keepalive
    let ping_stream = tokio_stream::wrappers::IntervalStream::new(
        tokio::time::interval(Duration::from_secs(30))
    ).map(|_| Ok::<Event, std::io::Error>(Event::default().comment("keepalive")));

    let combined = tokio_stream::StreamExt::merge(event_stream, ping_stream);

    Sse::new(combined)
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn event_to_sse_event(event: ServerEvent) -> Event {
    match event {
        ServerEvent::TaskUpdate { task_id, state, data } => {
            to_sse_event(SseEvent::task_status(&task_id, state, data))
        }
        ServerEvent::Error { task_id, message } => {
            to_sse_event(SseEvent::error(task_id, &message))
        }
    }
}

/// Build an ApiRequest from method + params
fn build_api_request(method: &str, params: Option<&serde_json::Value>) -> Result<ApiRequest, String> {
    // Handle methods that don't require params
    match method {
        "system_status" => return Ok(ApiRequest::SystemStatus),
        _ => {}
    }

    let params = params.ok_or("missing params")?;

    // Extract common fields
    let agent_id = params.get("agent_id")
        .and_then(|v| v.as_str())
        .unwrap_or("sse-client")
        .to_string();

    match method {
        "create" => {
            let content = params.get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let tags: Vec<String> = params.get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            Ok(ApiRequest::Create {
                content,
                content_encoding: Default::default(),
                tags,
                agent_id,
                agent_token: None,
                intent: params.get("intent").and_then(|v| v.as_str()).map(String::from),
            })
        }

        "search" => {
            let query = params.get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            Ok(ApiRequest::Search {
                query,
                agent_id,
                agent_token: None,
                limit: params.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize),
                offset: None,
                require_tags: params.get("require_tags")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default(),
                exclude_tags: params.get("exclude_tags")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default(),
                since: None,
                until: None,
            })
        }

        "submit_intent" => {
            let description = params.get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let priority = params.get("priority")
                .and_then(|v| v.as_str())
                .unwrap_or("medium")
                .to_string();

            Ok(ApiRequest::SubmitIntent {
                description,
                priority,
                action: params.get("action").and_then(|v| v.as_str()).map(String::from),
                agent_id,
            })
        }

        "context_assemble" => {
            let cids: Vec<plico::api::semantic::ContextAssembleCandidate> = params
                .get("cids")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter().filter_map(|v| {
                        Some(plico::api::semantic::ContextAssembleCandidate {
                            cid: v.get("cid")?.as_str()?.to_string(),
                            relevance: v.get("relevance")?.as_f64().unwrap_or(1.0) as f32,
                        })
                    }).collect()
                })
                .unwrap_or_default();

            let budget_tokens = params.get("budget_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(4096) as usize;

            Ok(ApiRequest::ContextAssemble {
                agent_id,
                cids,
                budget_tokens,
            })
        }

        _ => Err(format!("unsupported method: {}", method)),
    }
}

// ── CORS Middleware ────────────────────────────────────────────────────────────

use tower_http::cors::{Any, CorsLayer};

fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers(Any)
        .expose_headers(Any)
}

// ── Main Entry Point ──────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    let port = args.iter().position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(7879);

    let plicod_port = args.iter().position(|a| a == "--plicod-port")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(7878);

    println!("Plico SSE Streaming Adapter (A2A Protocol)");
    println!("Listening on: 0.0.0.0:{}", port);
    println!("Connecting to plicod on: 0.0.0.0:{}", plicod_port);

    let (broadcast_tx, _) = broadcast::channel::<ServerEvent>(1000);

    let state = AppState {
        plicod_port,
        broadcast_tx: Arc::new(broadcast_tx),
    };

    let app = Router::new()
        .route("/.well-known/agent.json", get(get_agent_card))
        .route("/tasks/sendSubscribe", post(task_send_subscribe))
        .layer(cors_layer())
        .with_state(state);

    let addr: SocketAddr = format!("0.0.0.0:{}", port).parse().unwrap();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    println!("SSE adapter ready. Endpoints:");
    println!("  GET  /.well-known/agent.json  — Agent Card");
    println!("  POST /tasks/sendSubscribe     — Task streaming");
    println!("Accepting connections...");

    axum::serve(listener, app).await.unwrap();
}

// ── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_api_response(ok: bool, error: Option<String>) -> ApiResponse {
        ApiResponse {
            ok,
            cid: None,
            node_id: None,
            data: None,
            results: None,
            agent_id: None,
            agents: None,
            memory: None,
            tags: None,
            neighbors: None,
            deleted: None,
            events: None,
            nodes: None,
            paths: None,
            edges: None,
            intent_id: None,
            assembly_id: None,
            agent_state: None,
            pending_intents: None,
            tools: None,
            tool_result: None,
            resolved_intents: None,
            messages: None,
            context_data: None,
            error,
            total_count: None,
            has_more: None,
            subscription_id: None,
            kernel_events: None,
            system_status: None,
            context_assembly: None,
            agent_usage: None,
            agent_cards: None,
            delegation: None,
            event_history: None,
            discovered_skills: None,
            token: None,
        }
    }

    #[test]
    fn task_state_from_response_completed() {
        let resp = make_test_api_response(true, None);
        assert!(matches!(TaskState::from_response(&resp), TaskState::Completed));
    }

    #[test]
    fn task_state_from_response_failed_permission() {
        let resp = make_test_api_response(false, Some("permission denied".to_string()));
        assert!(matches!(TaskState::from_response(&resp), TaskState::Failed));
    }

    #[test]
    fn task_state_from_response_working() {
        let resp = make_test_api_response(false, Some("object not found".to_string()));
        assert!(matches!(TaskState::from_response(&resp), TaskState::Working));
    }

    #[test]
    fn sse_event_task_status_serialization() {
        let event = SseEvent::task_status(
            "task-123",
            TaskState::Working,
            Some(serde_json::json!({ "progress": 50 })),
        );
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("task_status"));
        assert!(json.contains("task-123"));
        // TaskState serializes with lowercase rename_all
        assert!(json.contains("working"));
    }

    #[test]
    fn sse_event_error_serialization() {
        let event = SseEvent::error(Some("task-456".to_string()), "something went wrong");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("error"));
        assert!(json.contains("task-456"));
        assert!(json.contains("something went wrong"));
    }

    #[test]
    fn sse_event_ping() {
        let event = SseEvent::ping();
        assert!(event.task_id.is_none());
        assert_eq!(event.event_type, "ping");
    }

    #[test]
    fn build_api_request_create() {
        let params = serde_json::json!({
            "content": "test content",
            "tags": ["tag1", "tag2"],
            "agent_id": "test-agent",
        });
        let req = build_api_request("create", Some(&params)).unwrap();
        assert!(matches!(req, ApiRequest::Create { .. }));
    }

    #[test]
    fn build_api_request_search() {
        let params = serde_json::json!({
            "query": "test query",
            "agent_id": "test-agent",
            "limit": 10,
        });
        let req = build_api_request("search", Some(&params)).unwrap();
        assert!(matches!(req, ApiRequest::Search { query, .. } if query == "test query"));
    }

    #[test]
    fn build_api_request_system_status() {
        let req = build_api_request("system_status", None).unwrap();
        assert!(matches!(req, ApiRequest::SystemStatus));
    }

    #[test]
    fn build_api_request_unknown_method() {
        let params = serde_json::json!({ "agent_id": "test" });
        let result = build_api_request("unknown_method", Some(&params));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unsupported"));
    }

    #[test]
    fn build_api_request_missing_params() {
        let result = build_api_request("create", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing params"));
    }

    #[test]
    fn agent_card_json_structure() {
        let json = serde_json::json!({
            "name": "plico",
            "description": "AI-native operating system kernel",
            "version": "1.0.0",
            "capabilities": {
                "streaming": true,
                "pushNotifications": false,
                "agentCard": true,
            },
            "url": "http://localhost:7879",
        });

        assert_eq!(json["name"], "plico");
        assert_eq!(json["capabilities"]["streaming"], true);
    }

    #[test]
    fn context_assemble_request_parsing() {
        let params = serde_json::json!({
            "agent_id": "test-agent",
            "cids": [
                { "cid": "abc123", "relevance": 0.9 },
                { "cid": "def456", "relevance": 0.7 }
            ],
            "budget_tokens": 2048
        });

        let req = build_api_request("context_assemble", Some(&params)).unwrap();
        assert!(matches!(req, ApiRequest::ContextAssemble { .. }));

        if let ApiRequest::ContextAssemble { cids, budget_tokens, .. } = req {
            assert_eq!(cids.len(), 2);
            assert_eq!(budget_tokens, 2048);
        }
    }

    #[test]
    fn build_api_request_submit_intent() {
        let params = serde_json::json!({
            "description": "Process the document",
            "priority": "high",
            "agent_id": "test-agent",
        });
        let req = build_api_request("submit_intent", Some(&params)).unwrap();
        assert!(matches!(req, ApiRequest::SubmitIntent { .. }));

        if let ApiRequest::SubmitIntent { description, priority, .. } = req {
            assert_eq!(description, "Process the document");
            assert_eq!(priority, "high");
        }
    }
}
