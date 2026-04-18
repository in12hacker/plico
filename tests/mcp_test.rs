//! E2E integration test for plico-mcp (MCP Server over stdio).
//!
//! Spawns the plico-mcp binary, sends JSON-RPC requests via stdin,
//! reads JSON-RPC responses from stdout, verifies the full MCP lifecycle.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn send_and_recv(
    stdin: &mut std::process::ChildStdin,
    stdout: &mut BufReader<std::process::ChildStdout>,
    request: &serde_json::Value,
) -> serde_json::Value {
    let msg = serde_json::to_string(request).unwrap();
    writeln!(stdin, "{}", msg).expect("write to stdin");
    stdin.flush().expect("flush stdin");

    let mut line = String::new();
    stdout.read_line(&mut line).expect("read from stdout");
    serde_json::from_str(line.trim()).expect("parse JSON response")
}

#[test]
fn mcp_e2e_full_lifecycle() {
    let dir = tempfile::TempDir::new().unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_plico-mcp"))
        .env("PLICO_ROOT", dir.path())
        .env("EMBEDDING_BACKEND", "stub")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn plico-mcp");

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    // 1. Initialize
    let resp = send_and_recv(&mut stdin, &mut stdout, &serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0.1" }
        }
    }));
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(resp["result"]["serverInfo"]["name"], "plico-mcp");

    // 2. Send initialized notification (no response expected)
    writeln!(stdin, r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#).unwrap();
    stdin.flush().unwrap();

    // 3. List tools
    let resp = send_and_recv(&mut stdin, &mut stdout, &serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list"
    }));
    assert_eq!(resp["id"], 2);
    let tools = resp["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 7);

    // 4. Store content via plico_put
    let resp = send_and_recv(&mut stdin, &mut stdout, &serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "plico_put",
            "arguments": {
                "content": "MCP E2E test: protocol adapter architecture",
                "tags": ["plico:type:test", "plico:module:api"]
            }
        }
    }));
    assert_eq!(resp["id"], 3);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let put_resp: serde_json::Value = serde_json::from_str(text).unwrap();
    assert!(put_resp["ok"].as_bool().unwrap());
    let cid = put_resp["cid"].as_str().unwrap().to_string();

    // 5. Read back via plico_read
    let resp = send_and_recv(&mut stdin, &mut stdout, &serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "plico_read",
            "arguments": { "cid": cid }
        }
    }));
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let read_resp: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(read_resp["data"].as_str().unwrap(), "MCP E2E test: protocol adapter architecture");

    // 6. Search for the stored content
    let resp = send_and_recv(&mut stdin, &mut stdout, &serde_json::json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "plico_search",
            "arguments": { "query": "protocol adapter architecture" }
        }
    }));
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let search_resp: serde_json::Value = serde_json::from_str(text).unwrap();
    let results = search_resp["results"].as_array().unwrap();
    assert!(!results.is_empty(), "search should find stored content via BM25");

    // 7. List tags
    let resp = send_and_recv(&mut stdin, &mut stdout, &serde_json::json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tools/call",
        "params": {
            "name": "plico_tags",
            "arguments": {}
        }
    }));
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let tags: Vec<String> = serde_json::from_str(text).unwrap();
    assert!(tags.contains(&"plico:type:test".to_string()));

    // 8. List nodes (should succeed even if empty)
    let resp = send_and_recv(&mut stdin, &mut stdout, &serde_json::json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {
            "name": "plico_nodes",
            "arguments": {}
        }
    }));
    assert!(resp["result"]["content"][0]["text"].as_str().is_some());

    // Clean up
    drop(stdin);
    let _ = child.wait();
}
