//! End-to-end checks for the typed MCP stdio adapter.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use base64::Engine;
use plico::api::public::PUBLIC_OPERATIONS;
use serde_json::{json, Value};

struct McpProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _root: tempfile::TempDir,
}

impl McpProcess {
    fn spawn() -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_plico-mcp"))
            .env("PLICO_ROOT", root.path())
            .env("EMBEDDING_BACKEND", "stub")
            .env("LLM_BACKEND", "stub")
            .env("RUST_LOG", "error")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn plico-mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            _root: root,
        }
    }

    fn request(&mut self, request: Value) -> Value {
        serde_json::to_writer(&mut self.stdin, &request).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();

        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(line.trim()).expect("valid JSON-RPC response")
    }

    fn initialize(&mut self) -> Value {
        self.request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "0.1" },
            },
        }))
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn typed_tool_response(response: &Value) -> Value {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("typed response text");
    serde_json::from_str(text).expect("typed plico.personal.v2 response")
}

#[test]
fn mcp_catalog_is_the_exact_public_catalog() {
    let mut mcp = McpProcess::spawn();
    let initialize = mcp.initialize();
    assert_eq!(initialize["result"]["capabilities"], json!({ "tools": {} }));

    let response = mcp.request(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
    }));
    let names: Vec<&str> = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, PUBLIC_OPERATIONS);
}

#[test]
fn mcp_object_put_get_roundtrip_uses_typed_tools() {
    let mut mcp = McpProcess::spawn();
    mcp.initialize();

    let put = mcp.request(json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "object.put",
            "arguments": {
                "content": "personal memory object",
                "tags": ["memory", "mcp-test"],
            },
        },
    }));
    let put = typed_tool_response(&put);
    assert_eq!(put["protocol"], "plico.personal.v2");
    assert_eq!(put["ok"], true);
    assert_eq!(put["data"]["operation"], "object.put");
    let cid = put["data"]["result"]["cid"].as_str().unwrap();

    let get = mcp.request(json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "object.get",
            "arguments": { "cid": cid },
        },
    }));
    let get = typed_tool_response(&get);
    assert_eq!(get["ok"], true);
    assert_eq!(get["data"]["operation"], "object.get");
    let encoded = get["data"]["result"]["content_base64"].as_str().unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(encoded).unwrap(),
        b"personal memory object"
    );
}

#[test]
fn removed_composite_tool_is_a_protocol_error_and_server_stays_live() {
    let mut mcp = McpProcess::spawn();
    mcp.initialize();

    let unknown = mcp.request(json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "plico",
            "arguments": { "action": "status", "agent_id": "forged" },
        },
    }));
    assert_eq!(unknown["error"]["code"], -32602);

    let ping = mcp.request(json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "ping",
    }));
    assert_eq!(ping["result"], json!({}));
}
