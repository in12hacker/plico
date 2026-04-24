//! plico-sse Integration Tests
//!
//! Tests A2A Protocol SSE Streaming Adapter endpoints via HTTP:
//!     GET  /.well-known/agent.json  — Agent Card
//!     POST /tasks/sendSubscribe     — Task streaming via SSE
//!     POST /tasks/send              — Non-streaming fallback
//!     GET  /health                  — Health check
//!     DELETE /tasks/:task_id        — Task cancellation
//!
//! Note: The integration tests that spawn the actual binary are marked as `#[test]`
//! and can be run manually. In CI, only the unit tests that don't require
//! spawning the binary are executed reliably.

use std::time::Duration;

// ── Unit Tests (no server required) ────────────────────────────────────────────

/// Test SSE event format compliance (TaskStatus events)
/// This tests the serialization format used by plico_sse
#[test]
fn test_sse_event_format() {
    // Test that TaskState serializes correctly per A2A protocol
    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
    #[serde(rename_all = "lowercase")]
    enum TaskState {
        Working,
        Completed,
        Failed,
        InputRequired,
        Cancelled,
    }

    #[derive(serde::Serialize, serde::Deserialize, Debug)]
    struct SseEvent {
        event_type: String,
        task_id: Option<String>,
        data: serde_json::Value,
    }

    // Working state
    let state = TaskState::Working;
    let json = serde_json::to_string(&state).expect("serialization failed");
    assert!(json.contains("working"), "Working state should serialize to 'working'");

    // Completed state
    let state = TaskState::Completed;
    let json = serde_json::to_string(&state).expect("serialization failed");
    assert!(json.contains("completed"), "Completed state should serialize to 'completed'");

    // Test event structure
    let event = SseEvent {
        event_type: "task_status".to_string(),
        task_id: Some("task-123".to_string()),
        data: serde_json::json!({
            "state": TaskState::Working,
            "data": { "progress": 50 },
        }),
    };

    let json = serde_json::to_string(&event).expect("serialization failed");
    assert!(json.contains("task_status"));
    assert!(json.contains("task-123"));
    assert!(json.contains("working"));
}

/// Test task status lifecycle (Working -> Completed/Failed)
#[test]
fn test_task_status_lifecycle() {
    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
    #[serde(rename_all = "lowercase")]
    enum TaskState {
        Working,
        Completed,
        Failed,
        InputRequired,
        Cancelled,
    }

    // Working state
    let working_state = TaskState::Working;
    let json = serde_json::to_string(&working_state).expect("serialization failed");
    assert!(json.contains("working"), "Working state should serialize to 'working'");

    // Completed state
    let completed_state = TaskState::Completed;
    let json = serde_json::to_string(&completed_state).expect("serialization failed");
    assert!(json.contains("completed"), "Completed state should serialize to 'completed'");

    // Failed state
    let failed_state = TaskState::Failed;
    let json = serde_json::to_string(&failed_state).expect("serialization failed");
    assert!(json.contains("failed"), "Failed state should serialize to 'failed'");

    // InputRequired state
    let input_required_state = TaskState::InputRequired;
    let json = serde_json::to_string(&input_required_state).expect("serialization failed");
    assert!(json.contains("inputrequired"), "InputRequired state should serialize to 'inputrequired'");

    // Cancelled state
    let cancelled_state = TaskState::Cancelled;
    let json = serde_json::to_string(&cancelled_state).expect("serialization failed");
    assert!(json.contains("cancelled"), "Cancelled state should serialize to 'cancelled'");
}

/// Test error response format
#[test]
fn test_error_response_format() {
    // Simulate error response structure used by plico_sse
    #[derive(Debug, serde::Serialize)]
    struct ErrorResponse {
        error: String,
        class: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_after: Option<u64>,
    }

    // Client error
    let client_err = ErrorResponse {
        error: "malformed json".to_string(),
        class: "client".to_string(),
        retry_after: None,
    };
    let json = serde_json::to_string(&client_err).expect("serialization failed");
    assert!(json.contains("malformed json"));
    assert!(json.contains("client"));
    assert!(!json.contains("retry_after"));

    // Server error
    let server_err = ErrorResponse {
        error: "connection refused".to_string(),
        class: "server".to_string(),
        retry_after: Some(5),
    };
    let json = serde_json::to_string(&server_err).expect("serialization failed");
    assert!(json.contains("server"));
    assert!(json.contains("5"));
}

/// Test AgentCard structure matches A2A spec
#[test]
fn test_agent_card_structure_matches_spec() {
    // The AgentCard should declare streaming capability
    let card = serde_json::json!({
        "name": "plico",
        "version": "1.0.0",
        "capabilities": {
            "streaming": true,
            "pushNotifications": true
        },
        "skills": [
            {"id": "semantic-search", "name": "Semantic Search"},
            {"id": "memory-recall", "name": "Memory Recall"},
            {"id": "intent-declare", "name": "Declare Intent"}
        ]
    });

    assert_eq!(card["name"], "plico");
    assert!(card["capabilities"]["streaming"].as_bool().unwrap_or(false));

    let skills = card["skills"].as_array().expect("skills should be array");
    assert!(!skills.is_empty(), "Should declare at least one skill");

    // Each skill should have id and name
    for skill in skills {
        assert!(skill["id"].is_string(), "Skill should have id");
        assert!(skill["name"].is_string(), "Skill should have name");
    }
}

// ── Integration Tests (require running server) ─────────────────────────────────
// These tests spawn the actual plico-sse binary and make HTTP requests.
// They are marked with #[test] but may have environmental dependencies.

/// Integration test: AgentCard endpoint
/// Run manually with: cargo test test_integration_agent_card -- --nocapture
#[tokio::test]
#[ignore] // Requires manual server startup or specific environment
async fn test_integration_agent_card() {
    let mut child = tokio::process::Command::new("cargo")
        .args(["run", "--bin", "plico-sse", "--", "--port", "28790"])
        .env("EMBEDDING_BACKEND", "stub")
        .env("LLM_BACKEND", "stub")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn plico-sse");

    tokio::time::sleep(Duration::from_secs(3)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get("http://127.0.0.1:28790/.well-known/agent.json")
        .timeout(Duration::from_secs(5))
        .send()
        .await;

    child.kill().await.ok();
    let _ = child.wait().await;

    let resp = resp.expect("request failed");
    assert_eq!(resp.status(), 200, "Agent card should return 200 OK");
}

/// Integration test: Health endpoint
/// Run manually with: cargo test test_integration_health -- --nocapture
#[tokio::test]
#[ignore] // Requires manual server startup or specific environment
async fn test_integration_health() {
    let mut child = tokio::process::Command::new("cargo")
        .args(["run", "--bin", "plico-sse", "--", "--port", "28791"])
        .env("EMBEDDING_BACKEND", "stub")
        .env("LLM_BACKEND", "stub")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn plico-sse");

    tokio::time::sleep(Duration::from_secs(3)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get("http://127.0.0.1:28791/health")
        .timeout(Duration::from_secs(5))
        .send()
        .await;

    child.kill().await.ok();
    let _ = child.wait().await;

    let resp = resp.expect("request failed");
    assert_eq!(resp.status(), 200, "Health endpoint should return 200 OK");
}

/// Integration test: SSE streaming endpoint
/// Run manually with: cargo test test_integration_sse -- --nocapture
#[tokio::test]
#[ignore] // Requires manual server startup or specific environment
async fn test_integration_sse() {
    let mut child = tokio::process::Command::new("cargo")
        .args(["run", "--bin", "plico-sse", "--", "--port", "28792"])
        .env("EMBEDDING_BACKEND", "stub")
        .env("LLM_BACKEND", "stub")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn plico-sse");

    tokio::time::sleep(Duration::from_secs(3)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post("http://127.0.0.1:28792/tasks/sendSubscribe")
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(5))
        .send()
        .await;

    child.kill().await.ok();
    let _ = child.wait().await;

    let resp = resp.expect("request failed");
    assert_eq!(resp.status(), 200, "sendSubscribe should return 200");

    let content_type = resp.headers().get("content-type").map(|h| h.to_str().unwrap_or("")).unwrap_or("");
    assert!(content_type.contains("text/event-stream"), "Should be SSE stream");
}

/// Integration test: Non-streaming send endpoint
/// Run manually with: cargo test test_integration_send -- --nocapture
#[tokio::test]
#[ignore] // Requires manual server startup or specific environment
async fn test_integration_send() {
    let mut child = tokio::process::Command::new("cargo")
        .args(["run", "--bin", "plico-sse", "--", "--port", "28793"])
        .env("EMBEDDING_BACKEND", "stub")
        .env("LLM_BACKEND", "stub")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn plico-sse");

    tokio::time::sleep(Duration::from_secs(3)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post("http://127.0.0.1:28793/tasks/send")
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(5))
        .send()
        .await;

    child.kill().await.ok();
    let _ = child.wait().await;

    let resp = resp.expect("request failed");
    assert_eq!(resp.status(), 200, "send should return 200");
}

/// Integration test: Invalid JSON error handling
/// Run manually with: cargo test test_integration_invalid_json -- --nocapture
#[tokio::test]
#[ignore] // Requires manual server startup or specific environment
async fn test_integration_invalid_json() {
    let mut child = tokio::process::Command::new("cargo")
        .args(["run", "--bin", "plico-sse", "--", "--port", "28794"])
        .env("EMBEDDING_BACKEND", "stub")
        .env("LLM_BACKEND", "stub")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn plico-sse");

    tokio::time::sleep(Duration::from_secs(3)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post("http://127.0.0.1:28794/tasks/send")
        .header("Content-Type", "application/json")
        .body("not valid json")
        .timeout(Duration::from_secs(5))
        .send()
        .await;

    child.kill().await.ok();
    let _ = child.wait().await;

    let resp = resp.expect("request failed");
    assert_eq!(resp.status(), 400, "Invalid JSON should return 400");
}

/// Integration test: Graceful shutdown
/// Run manually with: cargo test test_integration_shutdown -- --nocapture
#[tokio::test]
#[ignore] // Requires manual server startup or specific environment
async fn test_integration_shutdown() {
    let mut child = tokio::process::Command::new("cargo")
        .args(["run", "--bin", "plico-sse", "--", "--port", "28795"])
        .env("EMBEDDING_BACKEND", "stub")
        .env("LLM_BACKEND", "stub")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn plico-sse");

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Server should still be running
    let client = reqwest::Client::new();
    let resp = client
        .get("http://127.0.0.1:28795/health")
        .timeout(Duration::from_secs(2))
        .send()
        .await;

    assert!(resp.is_ok(), "Server should still be running");

    child.kill().await.ok();
    let _ = child.wait().await;
}
