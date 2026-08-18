//! MCP Client implementation — spawns and communicates with MCP servers.
//!
//! Implements `ExternalToolProvider` — the kernel never sees MCP protocol
//! details. If MCP is replaced by a new protocol, delete this file and
//! add a new one. The kernel's tool dispatch is unchanged.

use std::io::{BufRead, BufReader, BufWriter, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::tool::{ExternalToolProvider, ToolDescriptor, ToolResult};

const PROTOCOL_VERSION: &str = "2024-11-05";

/// How long shutdown waits for the server to exit on its own after stdin
/// closes (EOF) before force-killing it.
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const REAP_POLL_INTERVAL: Duration = Duration::from_millis(10);

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

        writeln!(self.stdin, "{}", serde_json::to_string(&request).unwrap()).map_err(McpError::Io)?;
        self.stdin.flush().map_err(McpError::Io)?;

        let mut line = String::new();
        self.stdout.read_line(&mut line).map_err(McpError::Io)?;

        let resp: Value =
            serde_json::from_str(line.trim()).map_err(|e| McpError::Protocol(format!("invalid JSON response: {e}")))?;

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

        writeln!(self.stdin, "{}", serde_json::to_string(&notification).unwrap()).map_err(McpError::Io)?;
        self.stdin.flush().map_err(McpError::Io)?;
        Ok(())
    }

    fn call_tool(&mut self, name: &str, arguments: &Value) -> Result<String, McpError> {
        let resp = self.send_request(
            "tools/call",
            serde_json::json!({
                "name": name,
                "arguments": arguments
            }),
        )?;

        let result = &resp["result"];
        if result.get("isError") == Some(&Value::Bool(true)) {
            let text = result["content"][0]["text"].as_str().unwrap_or("unknown error");
            return Err(McpError::ServerError {
                code: -1,
                message: text.to_string(),
            });
        }

        result["content"][0]["text"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| McpError::Protocol("tools/call response missing content text".into()))
    }
}

/// How the managed child left its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedChildOutcome {
    /// The server exited on its own within the grace period.
    ExitedDuringGrace,
    /// The grace period elapsed and the server was killed, then reaped.
    KilledAfterTimeout,
}

/// RAII owner of the MCP server subprocess. The `spawn` result moves in
/// immediately, so every later failure path reaps through the same state
/// machine: close stdin (EOF), bounded `try_wait` grace, `kill`, `wait`.
/// Dropping a bare `std::process::Child` never reaps; this type always does.
pub(crate) struct ManagedChild {
    child: Option<Child>,
    graceful_timeout: Duration,
}

impl ManagedChild {
    pub(crate) fn new(child: Child, graceful_timeout: Duration) -> Self {
        Self {
            child: Some(child),
            graceful_timeout,
        }
    }

    fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.as_mut()?.stdin.take()
    }

    fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.as_mut()?.stdout.take()
    }

    /// The single shutdown state machine for both the graceful EOF path and
    /// the forced kill path. It is bounded: after the grace deadline the
    /// child is killed, then always reaped with `wait`. Best effort — I/O
    /// errors are swallowed because shutdown must never panic.
    pub(crate) fn shutdown_and_reap(&mut self) -> Option<ManagedChildOutcome> {
        let child = self.child.as_mut()?;
        // Initiate EOF even when no transport ever existed (construction
        // failures); with the transport already gone this is a no-op.
        child.stdin.take();
        let deadline = Instant::now() + self.graceful_timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    self.child = None;
                    return Some(ManagedChildOutcome::ExitedDuringGrace);
                }
                Ok(None) => {}
                Err(_) => break,
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(REAP_POLL_INTERVAL);
        }
        let _ = child.kill();
        let _ = child.wait();
        self.child = None;
        Some(ManagedChildOutcome::KilledAfterTimeout)
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        // Backstop for construction failures and any path that did not run
        // the explicit shutdown: same state machine, best effort, no panic.
        let _ = self.shutdown_and_reap();
    }
}

/// MCP Client — connects to one MCP server subprocess.
///
/// Implements `ExternalToolProvider` so the kernel treats it identically
/// to any other external tool source. The MCP JSON-RPC protocol is fully
/// encapsulated here — nothing leaks to the kernel.
///
/// The server subprocess is owned by a [`ManagedChild`]: dropping the client
/// closes the transport first, then waits a bounded grace period for the
/// server's EOF exit, kills it if still running, and always reaps.
pub struct McpClient {
    child: ManagedChild,
    transport: Mutex<Option<McpTransport>>,
    server_info: ServerInfo,
    tools: Vec<McpToolDef>,
}

impl McpClient {
    pub fn new(command: &str, args: &[&str], envs: &[(&str, &str)]) -> Result<Self, McpError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in envs {
            cmd.env(k, v);
        }

        let child = cmd.spawn().map_err(McpError::Spawn)?;
        // Ownership transfers to the RAII owner immediately: every `?`
        // below reaps through the ManagedChild backstop, and the transport
        // (which closes stdin) is declared later so it drops first.
        let mut child = ManagedChild::new(child, GRACEFUL_SHUTDOWN_TIMEOUT);
        let stdin = BufWriter::new(
            child
                .take_stdin()
                .ok_or_else(|| McpError::Protocol("failed to open stdin".into()))?,
        );
        let stdout = BufReader::new(
            child
                .take_stdout()
                .ok_or_else(|| McpError::Protocol("failed to open stdout".into()))?,
        );

        let mut transport = McpTransport {
            stdin,
            stdout,
            next_id: 1,
        };

        let server_info = {
            let resp = transport.send_request(
                "initialize",
                serde_json::json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": "plico-mcp-client", "version": "1.0.0" }
                }),
            )?;
            let result = &resp["result"];
            let info = ServerInfo {
                name: result["serverInfo"]["name"].as_str().unwrap_or("unknown").to_string(),
                version: result["serverInfo"]["version"].as_str().unwrap_or("0.0.0").to_string(),
            };
            transport.send_notification("notifications/initialized", serde_json::json!({}))?;
            info
        };

        let tools = {
            let resp = transport.send_request("tools/list", serde_json::json!({}))?;
            let tools_arr = resp["result"]["tools"]
                .as_array()
                .ok_or_else(|| McpError::Protocol("tools/list did not return tools array".into()))?;
            tools_arr
                .iter()
                .map(|t| McpToolDef {
                    name: t["name"].as_str().unwrap_or("").to_string(),
                    description: t["description"].as_str().unwrap_or("").to_string(),
                    input_schema: t["inputSchema"].clone(),
                })
                .collect()
        };

        Ok(Self {
            child,
            transport: Mutex::new(Some(transport)),
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
        let mut guard = self.transport.lock().unwrap();
        match guard.as_mut() {
            Some(transport) => transport.call_tool(name, arguments),
            None => Err(McpError::Protocol("MCP client transport is shut down".into())),
        }
    }
}

/// Takes the value out of a `Mutex<Option<T>>` even when the mutex is
/// poisoned — shutdown must close the transport regardless of lock state
/// and must never panic.
pub(crate) fn take_even_if_poisoned<T>(cell: &Mutex<Option<T>>) -> Option<T> {
    match cell.lock() {
        Ok(mut guard) => guard.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    }
}

impl Drop for McpClient {
    /// Explicit shutdown order — never the default field-drop order:
    /// (a) take and drop the transport (closing stdin) even when the mutex
    /// is poisoned, (b) bounded grace period of `try_wait` for the server's
    /// EOF exit, (c) kill if still running, (d) always `wait`/reap.
    /// Never panics.
    fn drop(&mut self) {
        drop(take_even_if_poisoned(&self.transport));
        let _ = self.child.shutdown_and_reap();
    }
}

impl ExternalToolProvider for McpClient {
    fn provider_name(&self) -> &str {
        &self.server_info.name
    }

    fn discover_tools(&self) -> Vec<ToolDescriptor> {
        self.tools
            .iter()
            .map(|t| ToolDescriptor {
                name: t.name.clone(),
                description: t.description.clone(),
                schema: t.input_schema.clone(),
            })
            .collect()
    }

    fn call_tool(&self, name: &str, params: &serde_json::Value) -> ToolResult {
        let Ok(mut guard) = self.transport.lock() else {
            return ToolResult::error("MCP client transport lock is poisoned");
        };
        match guard.as_mut() {
            Some(transport) => match transport.call_tool(name, params) {
                Ok(text) => ToolResult::ok(serde_json::json!({ "text": text })),
                Err(e) => ToolResult::error(e.to_string()),
            },
            None => ToolResult::error("MCP client transport is shut down"),
        }
    }
}
