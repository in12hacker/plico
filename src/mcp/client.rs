//! MCP Client implementation — spawns and communicates with MCP servers.

use std::io::{BufRead, BufReader, BufWriter, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::tool::{ToolDescriptor, ToolHandler, ToolRegistry, ToolResult};

const PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Debug)]
pub enum McpError {
    Spawn(std::io::Error),
    Protocol(String),
    Io(std::io::Error),
    ServerError { code: i64, message: String },
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpError::Spawn(e) => write!(f, "failed to spawn MCP server: {e}"),
            McpError::Protocol(msg) => write!(f, "MCP protocol error: {msg}"),
            McpError::Io(e) => write!(f, "MCP I/O error: {e}"),
            McpError::ServerError { code, message } => write!(f, "MCP server error {code}: {message}"),
        }
    }
}

impl std::error::Error for McpError {}

#[derive(Debug, Clone)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

struct McpTransport {
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpTransport {
    fn send_request(&mut self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.next_id;
        self.next_id += 1;

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        writeln!(self.stdin, "{}", serde_json::to_string(&request).unwrap())
            .map_err(McpError::Io)?;
        self.stdin.flush().map_err(McpError::Io)?;

        let mut line = String::new();
        self.stdout.read_line(&mut line).map_err(McpError::Io)?;

        let resp: Value = serde_json::from_str(line.trim())
            .map_err(|e| McpError::Protocol(format!("invalid JSON response: {e}")))?;

        if let Some(err) = resp.get("error") {
            return Err(McpError::ServerError {
                code: err["code"].as_i64().unwrap_or(-1),
                message: err["message"].as_str().unwrap_or("").to_string(),
            });
        }

        Ok(resp)
    }

    fn send_notification(&mut self, method: &str, params: Value) -> Result<(), McpError> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });

        writeln!(self.stdin, "{}", serde_json::to_string(&notification).unwrap())
            .map_err(McpError::Io)?;
        self.stdin.flush().map_err(McpError::Io)?;
        Ok(())
    }

    fn call_tool(&mut self, name: &str, arguments: &Value) -> Result<String, McpError> {
        let resp = self.send_request("tools/call", serde_json::json!({
            "name": name,
            "arguments": arguments
        }))?;

        let result = &resp["result"];
        if result.get("isError") == Some(&Value::Bool(true)) {
            let text = result["content"][0]["text"].as_str().unwrap_or("unknown error");
            return Err(McpError::ServerError { code: -1, message: text.to_string() });
        }

        result["content"][0]["text"].as_str()
            .map(String::from)
            .ok_or_else(|| McpError::Protocol("tools/call response missing content text".into()))
    }
}

pub struct McpClient {
    _child: Child,
    transport: Arc<Mutex<McpTransport>>,
    server_info: ServerInfo,
    tools: Vec<McpToolDef>,
}

impl McpClient {
    pub fn new(
        command: &str,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> Result<Self, McpError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in envs {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn().map_err(McpError::Spawn)?;
        let stdin = BufWriter::new(child.stdin.take().ok_or_else(|| {
            McpError::Protocol("failed to open stdin".into())
        })?);
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| {
            McpError::Protocol("failed to open stdout".into())
        })?);

        let transport = Arc::new(Mutex::new(McpTransport {
            stdin,
            stdout,
            next_id: 1,
        }));

        let server_info = {
            let mut t = transport.lock().unwrap();
            let resp = t.send_request("initialize", serde_json::json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "plico-mcp-client", "version": "1.0.0" }
            }))?;
            let result = &resp["result"];
            let info = ServerInfo {
                name: result["serverInfo"]["name"].as_str().unwrap_or("unknown").to_string(),
                version: result["serverInfo"]["version"].as_str().unwrap_or("0.0.0").to_string(),
            };
            t.send_notification("notifications/initialized", serde_json::json!({}))?;
            info
        };

        let tools = {
            let mut t = transport.lock().unwrap();
            let resp = t.send_request("tools/list", serde_json::json!({}))?;
            let tools_arr = resp["result"]["tools"].as_array()
                .ok_or_else(|| McpError::Protocol("tools/list did not return tools array".into()))?;
            tools_arr.iter().map(|t| McpToolDef {
                name: t["name"].as_str().unwrap_or("").to_string(),
                description: t["description"].as_str().unwrap_or("").to_string(),
                input_schema: t["inputSchema"].clone(),
            }).collect()
        };

        Ok(Self {
            _child: child,
            transport,
            server_info,
            tools,
        })
    }

    pub fn server_info(&self) -> &ServerInfo {
        &self.server_info
    }

    pub fn tools(&self) -> &[McpToolDef] {
        &self.tools
    }

    pub fn call_tool(&self, name: &str, arguments: &Value) -> Result<String, McpError> {
        self.transport.lock().unwrap().call_tool(name, arguments)
    }

    pub fn register_tools(&self, registry: &ToolRegistry, prefix: &str) {
        for tool in &self.tools {
            let qualified_name = format!("{}.{}", prefix, tool.name);
            let desc = ToolDescriptor {
                name: qualified_name,
                description: format!("[MCP:{}] {}", self.server_info.name, tool.description),
                schema: tool.input_schema.clone(),
            };
            let handler = McpToolHandler {
                transport: Arc::clone(&self.transport),
                tool_name: tool.name.clone(),
            };
            registry.register_with_handler(desc, Arc::new(handler));
        }
    }
}

struct McpToolHandler {
    transport: Arc<Mutex<McpTransport>>,
    tool_name: String,
}

impl ToolHandler for McpToolHandler {
    fn execute(&self, params: &serde_json::Value, _agent_id: &str) -> ToolResult {
        match self.transport.lock() {
            Ok(mut t) => match t.call_tool(&self.tool_name, params) {
                Ok(text) => ToolResult::ok(serde_json::json!({"text": text})),
                Err(e) => ToolResult::error(e.to_string()),
            },
            Err(e) => ToolResult::error(format!("lock poisoned: {e}")),
        }
    }
}
