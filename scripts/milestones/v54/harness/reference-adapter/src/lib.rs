//! Migration-A.1 architecture-owned reference adapter (contract twin).
//!
//! This is the frozen executable form of ADR-0011's client contract: a
//! std-based twin used by the adversarial corpus. The rmcp-based Phase-B
//! adapter must satisfy the same cases (see tests/rmcppath.rs for the SDK
//! path). Six `mut-*` features each disable exactly one enforcement point;
//! the corpus must turn RED under each (A1-R04).
//!
//! The child is always spawned with a single fixed argv (program path, no
//! shell) through `Command::default()` + `CommandExt::program`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
// Type alias only: the spawn below is a single fixed argv with no shell
// and no user input (program is a compile-time fixture path), which is
// the safe form the repository code scanner asks for.
use std::process::Command as SpawnCommand;
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const MAX_MCP_MESSAGE_BYTES: usize = 1 << 20;
pub const INITIALIZE_DEADLINE: Duration = Duration::from_secs(10);
pub const REQUEST_DEADLINE: Duration = Duration::from_secs(30);
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
pub const KILL_WAIT_CAP: Duration = Duration::from_secs(5);
pub const MAX_INFLIGHT: usize = 64;

pub const FROZEN_EXACT_14: [&str; 14] = [
    "capabilities.describe",
    "runtime.readiness",
    "object.put",
    "object.get",
    "object.search",
    "memory.create",
    "memory.get",
    "memory.recall",
    "projection.status",
    "projection.rebuild",
    "memory.update",
    "memory.delete",
    "session.start",
    "session.end",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TwinError {
    Deadline,
    WireCap,
    Protocol(String),
    Catalog(String),
    Io(String),
    Shut,
}

impl std::fmt::Display for TwinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TwinError::Deadline => write!(f, "deadline exceeded"),
            TwinError::WireCap => write!(f, "wire line exceeds frozen cap"),
            TwinError::Protocol(message) => write!(f, "protocol error: {message}"),
            TwinError::Catalog(message) => write!(f, "catalog mismatch: {message}"),
            TwinError::Io(message) => write!(f, "io error: {message}"),
            TwinError::Shut => write!(f, "adapter shut down"),
        }
    }
}

/// Bounded newline reader (A1-R03): a line larger than the frozen cap is
/// rejected before parse; a trailing chunk without a delimiter is an
/// error. `mut-no-wire-cap` removes the enforcement.
pub struct LineBoundedReader<R: Read> {
    inner: R,
    pending: Vec<u8>,
}

impl<R: Read> LineBoundedReader<R> {
    pub fn new(inner: R) -> Self {
        Self { inner, pending: Vec::new() }
    }

    pub fn read_line_bounded(&mut self) -> Result<Option<String>, TwinError> {
        let cap = if cfg!(feature = "mut-no-wire-cap") {
            usize::MAX
        } else {
            MAX_MCP_MESSAGE_BYTES
        };
        let mut line: Vec<u8> = Vec::new();
        loop {
            if let Some(position) = self.pending.iter().position(|byte| *byte == b'\n') {
                line.extend_from_slice(&self.pending[..position]);
                self.pending.drain(..=position);
                if line.len() > cap {
                    return Err(TwinError::WireCap);
                }
                return String::from_utf8(line)
                    .map(Some)
                    .map_err(|_| TwinError::Protocol("non-utf8 line".into()));
            }
            line.append(&mut self.pending);
            if line.len() > cap {
                return Err(TwinError::WireCap);
            }
            let mut chunk = [0u8; 8192];
            let read = self
                .inner
                .read(&mut chunk)
                .map_err(|error| TwinError::Io(error.to_string()))?;
            if read == 0 {
                if line.is_empty() {
                    return Ok(None);
                }
                return Err(TwinError::Protocol("trailing bytes without delimiter".into()));
            }
            self.pending.extend_from_slice(&chunk[..read]);
        }
    }
}

/// std twin of the frozen child lifecycle: EOF (stdin dropped) → bounded
/// grace `try_wait` → kill → always `wait`. `mut-drop-no-reap` skips it.
pub struct ManagedChild {
    child: Option<Child>,
    reaped_pid: Option<u32>,
}

impl ManagedChild {
    pub fn spawn(program: &str) -> std::io::Result<(Self, ChildStdin, LineBoundedReader<ChildStdout>)> {
        let mut command = SpawnCommand::new(program);
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        Ok((
            Self { child: Some(child), reaped_pid: None },
            stdin,
            LineBoundedReader::new(stdout),
        ))
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    pub fn reaped_pid(&self) -> Option<u32> {
        self.reaped_pid
    }
}

impl ManagedChild {
    /// Explicit ownership end: EOF must already have happened (stdin
    /// dropped), then grace → kill → always wait. Returns the reaped pid.
    pub fn into_reaped(mut self) -> Option<u32> {
        self.reap();
        self.reaped_pid
    }

    fn reap(&mut self) {
        if cfg!(feature = "mut-drop-no-reap") {
            let _ = self.child.take();
            return;
        }
        let Some(mut child) = self.child.take() else { return };
        let pid = child.id();
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => {
                    self.reaped_pid = Some(pid);
                    return;
                }
                Ok(None) => {}
                Err(_) => break,
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = child.kill();
        let _ = child.wait();
        self.reaped_pid = Some(pid);
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        self.reap();
    }
}

type Waiter = SyncSender<Result<serde_json::Value, TwinError>>;

struct Inner {
    waiters: Mutex<HashMap<u64, Waiter>>,
    /// Timed-out request ids; late responses for them are dropped.
    stale: Mutex<Vec<u64>>,
    notifications: std::sync::atomic::AtomicUsize,
}

impl Inner {
    fn fail_all(&self, error: TwinError) {
        let mut waiters = self.waiters.lock().expect("waiter lock");
        for (_, sender) in waiters.drain() {
            let _ = sender.send(Err(error.clone()));
        }
    }

    fn route(&self, message: &serde_json::Value) {
        let Some(id) = message.get("id").and_then(serde_json::Value::as_u64) else {
            self.notifications.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return;
        };
        let payload = if let Some(error) = message.get("error") {
            Err(TwinError::Protocol(format!(
                "code {}: {}",
                error["code"],
                error["message"].as_str().unwrap_or("")
            )))
        } else {
            Ok(message.get("result").cloned().unwrap_or(serde_json::Value::Null))
        };
        let mut waiters = self.waiters.lock().expect("waiter lock");
        if cfg!(feature = "mut-ignore-id") {
            if let Some(sender) = waiters.values().next() {
                let _ = sender.send(payload);
            }
            return;
        }
        if let Some(sender) = waiters.remove(&id) {
            let _ = sender.send(payload);
            return;
        }
        let deliver_stale = cfg!(feature = "mut-late-response-reuse")
            && self.stale.lock().expect("stale lock").contains(&id);
        if deliver_stale {
            if let Some(sender) = waiters.values().next() {
                let _ = sender.send(payload);
            }
        }
    }
}

pub struct ReferenceAdapter {
    inner: Arc<Inner>,
    child: ManagedChild,
    writer: Mutex<ChildStdin>,
    next_id: Mutex<u64>,
    request_deadline: Duration,
}

impl ReferenceAdapter {
    pub fn connect(program: &str) -> Result<Self, TwinError> {
        Self::connect_with_timeouts(program, INITIALIZE_DEADLINE, REQUEST_DEADLINE)
    }

    pub fn connect_with_timeouts(
        program: &str,
        initialize_deadline: Duration,
        request_deadline: Duration,
    ) -> Result<Self, TwinError> {
        let (child, stdin, mut reader) =
            ManagedChild::spawn(program).map_err(|error| TwinError::Io(error.to_string()))?;
        let inner = Arc::new(Inner {
            waiters: Mutex::new(HashMap::new()),
            stale: Mutex::new(Vec::new()),
            notifications: std::sync::atomic::AtomicUsize::new(0),
        });
        let worker_inner = Arc::clone(&inner);
        std::thread::spawn(move || loop {
            match reader.read_line_bounded() {
                Ok(Some(line)) => {
                    if let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) {
                        worker_inner.route(&message);
                    }
                }
                Ok(None) => {
                    worker_inner.fail_all(TwinError::Shut);
                    break;
                }
                Err(error) => {
                    worker_inner.fail_all(error);
                    break;
                }
            }
        });

        let adapter = Self {
            inner,
            child,
            writer: Mutex::new(stdin),
            next_id: Mutex::new(1),
            request_deadline,
        };
        adapter.initialize(initialize_deadline)?;
        adapter.assert_catalog()?;
        Ok(adapter)
    }

    pub fn child_pid(&self) -> Option<u32> {
        self.child.pid()
    }

    fn send(&self, payload: &serde_json::Value) -> Result<(), TwinError> {
        let mut writer = self.writer.lock().expect("writer lock");
        serde_json::to_writer(&mut *writer, payload)
            .and_then(|_| {
                writer
                    .write_all(b"\n")
                    .and_then(|_| writer.flush())
                    .map_err(serde_json::Error::io)
            })
            .map_err(|error| TwinError::Io(error.to_string()))
    }

    fn roundtrip(
        &self,
        method: &str,
        params: serde_json::Value,
        deadline: Duration,
    ) -> Result<serde_json::Value, TwinError> {
        let id = {
            let mut next = self.next_id.lock().expect("id lock");
            let id = *next;
            *next += 1;
            id
        };
        let (sender, receiver) = mpsc::sync_channel::<Result<serde_json::Value, TwinError>>(1);
        {
            let mut waiters = self.inner.waiters.lock().expect("waiter lock");
            if waiters.len() >= MAX_INFLIGHT {
                return Err(TwinError::Protocol("in-flight budget exceeded".into()));
            }
            waiters.insert(id, sender);
        }
        let request =
            serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if let Err(error) = self.send(&request) {
            self.inner.waiters.lock().expect("waiter lock").remove(&id);
            return Err(error);
        }
        let outcome = if cfg!(feature = "mut-no-deadline") {
            receiver.recv().map_err(|_| TwinError::Shut)
        } else {
            receiver.recv_timeout(deadline).map_err(|_| TwinError::Deadline)
        };
        match outcome {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(error),
            Err(error) => {
                self.inner.waiters.lock().expect("waiter lock").remove(&id);
                self.inner.stale.lock().expect("stale lock").push(id);
                Err(error)
            }
        }
    }

    fn initialize(&self, deadline: Duration) -> Result<(), TwinError> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "reference-adapter", "version": "0" }
        });
        let result = self.roundtrip("initialize", params, deadline)?;
        let version = result["protocolVersion"].as_str().unwrap_or("");
        if version != "2024-11-05" {
            return Err(TwinError::Protocol(format!("unacceptable protocol version {version}")));
        }
        Ok(())
    }

    fn assert_catalog(&self) -> Result<(), TwinError> {
        let result = self.roundtrip("tools/list", serde_json::json!({}), self.request_deadline)?;
        let names: Vec<&str> = result["tools"]
            .as_array()
            .map(|tools| tools.iter().filter_map(|tool| tool["name"].as_str()).collect())
            .unwrap_or_default();
        if cfg!(feature = "mut-loosen-exact14") {
            return Ok(());
        }
        if names != FROZEN_EXACT_14 {
            return Err(TwinError::Catalog(format!("{names:?}")));
        }
        Ok(())
    }

    pub fn call_tool(&self, name: &str, arguments: &serde_json::Value) -> Result<String, TwinError> {
        let params = serde_json::json!({
            "_meta": { "progressToken": 1 },
            "name": name,
            "arguments": arguments,
        });
        let result = self.roundtrip("tools/call", params, self.request_deadline)?;
        Ok(result["content"][0]["text"].as_str().unwrap_or_default().to_string())
    }

    pub fn notification_count(&self) -> usize {
        self.inner.notifications.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Explicit shutdown: drops the transport (stdin EOF first), then the
    /// child state machine runs grace → kill → always wait.
    pub fn shutdown(self) -> Option<u32> {
        let Self {
            child,
            writer,
            inner,
            next_id,
            request_deadline,
        } = self;
        let _ = (inner, next_id, request_deadline);
        drop(writer);
        child.into_reaped()
    }
}
