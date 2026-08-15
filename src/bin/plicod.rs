//! plicod — Plico AI-Native OS Daemon
//!
//! Long-running daemon exposing the semantic API over TCP and Unix Domain Socket.
//! Also runs the agent execution dispatch loop in the background.
//!
//! Usage:
//!   plicod [start] [--port PORT] [--root PATH] [--no-uds]   Start daemon (default)
//!   plicod stop    [--root PATH]                             Stop running daemon
//!   plicod status  [--root PATH]                             Show daemon status (JSON)
//!
//! # Protocol
//!
//! Length-prefixed JSON framing over TCP/UDS:
//!   [4-byte big-endian length][JSON payload]
//!
//! # Daemon Lifecycle
//!
//! On startup: checks for existing daemon (multi-instance protection),
//!   writes PID to `<root>/plicod.pid`, creates UDS at `<root>/plico.sock`.
//! On shutdown: persists state, removes PID file and socket.

use plico::api::public::{PublicError, PublicErrorCode, PublicRequest, PublicRequestHead, PublicResponse};
use plico::kernel::{AIKernel, PublicRequestContext, PublicTransport};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time;
use tracing_subscriber::util::SubscriberInitExt;

const MAX_MESSAGE_SIZE: u32 = 16 * 1024 * 1024; // 16 MiB

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransportKind {
    Tcp,
    #[cfg(unix)]
    Uds,
}

impl TransportKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            #[cfg(unix)]
            Self::Uds => "uds",
        }
    }
}

// ── Subcommand Dispatch ─────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = resolve_root(&args);

    match detect_subcommand(&args) {
        Subcommand::Start => cmd_start(args, root).await,
        Subcommand::Stop => cmd_stop(&root),
        Subcommand::Status => cmd_status(&root),
    }
}

enum Subcommand {
    Start,
    Stop,
    Status,
}

fn detect_subcommand(args: &[String]) -> Subcommand {
    for arg in args.iter().skip(1) {
        match arg.as_str() {
            "stop" => return Subcommand::Stop,
            "status" => return Subcommand::Status,
            "start" => return Subcommand::Start,
            _ if arg.starts_with("--") => continue,
            _ => continue,
        }
    }
    Subcommand::Start
}

fn resolve_root(args: &[String]) -> PathBuf {
    extract_opt(args, "--root")
        .map(PathBuf::from)
        .or_else(|| std::env::var("PLICO_ROOT").ok().map(PathBuf::from))
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(std::env::temp_dir).join(".plico"))
}

// ── PID Management ──────────────────────────────────────────────────

fn pid_path(root: &Path) -> PathBuf {
    root.join("plicod.pid")
}
fn sock_path(root: &Path) -> PathBuf {
    root.join("plico.sock")
}

/// Read PID file and check if the process is still alive.
/// Returns `Some(pid)` if daemon is running, `None` otherwise.
fn check_existing_daemon(root: &Path) -> Option<u32> {
    let path = pid_path(root);
    let pid_str = std::fs::read_to_string(&path).ok()?;
    let pid: u32 = pid_str.trim().parse().ok()?;
    if pid == 0 {
        return None;
    }
    // Check if process exists via /proc on Linux, or kill(0) on Unix
    #[cfg(unix)]
    {
        let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
        if alive {
            Some(pid)
        } else {
            None
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

fn write_pid_file(path: &Path) {
    if let Err(e) = std::fs::write(path, std::process::id().to_string()) {
        eprintln!("Warning: failed to write plicod.pid: {e}");
    }
}

// ── cmd_stop ────────────────────────────────────────────────────────

fn cmd_stop(root: &Path) {
    match check_existing_daemon(root) {
        Some(pid) => {
            #[cfg(unix)]
            {
                let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                if rc == 0 {
                    println!("{{\"ok\":true,\"message\":\"SIGTERM sent to plicod (PID {})\"}}", pid);
                    // Wait briefly for process to exit, clean up stale PID if it does
                    std::thread::sleep(Duration::from_millis(500));
                    if check_existing_daemon(root).is_none() {
                        let _ = std::fs::remove_file(pid_path(root));
                        let _ = std::fs::remove_file(sock_path(root));
                    }
                } else {
                    eprintln!("{{\"ok\":false,\"error\":\"Failed to send SIGTERM to PID {}\"}}", pid);
                    std::process::exit(1);
                }
            }
            #[cfg(not(unix))]
            {
                eprintln!("{{\"ok\":false,\"error\":\"stop not supported on this platform\"}}");
                std::process::exit(1);
            }
        }
        None => {
            eprintln!("{{\"ok\":false,\"error\":\"plicod is not running (no live plicod.pid)\"}}");
            std::process::exit(1);
        }
    }
}

// ── cmd_status ──────────────────────────────────────────────────────

fn cmd_status(root: &Path) {
    let pp = pid_path(root);
    let sp = sock_path(root);
    match check_existing_daemon(root) {
        Some(pid) => {
            let sock_exists = sp.exists();
            println!(
                "{{\"ok\":true,\"running\":true,\"pid\":{},\"pid_file\":\"plicod.pid\",\"socket\":\"plico.sock\",\"socket_exists\":{}}}",
                pid,
                sock_exists,
            );
        }
        None => {
            let stale = pp.exists();
            if stale {
                let _ = std::fs::remove_file(&pp);
                let _ = std::fs::remove_file(&sp);
            }
            println!("{{\"ok\":true,\"running\":false,\"stale_pid_cleaned\":{}}}", stale,);
            std::process::exit(1);
        }
    }
}

// ── cmd_start (main daemon logic) ───────────────────────────────────

async fn cmd_start(args: Vec<String>, root: PathBuf) {
    let config = plico::config::PlicoConfig::load(Some(root.clone()));
    let host = extract_opt(&args, "--host").unwrap_or(config.network.host.clone());
    let port = extract_opt(&args, "--port")
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(config.network.daemon_port);
    let no_uds = args.iter().any(|a| a == "--no-uds") || config.network.disable_uds;

    let env = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    tracing_subscriber::fmt()
        .with_env_filter(&env)
        .with_writer(std::io::stderr)
        .finish()
        .try_init()
        .ok();

    // Multi-instance protection
    if let Some(existing_pid) = check_existing_daemon(&root) {
        eprintln!(
            "{{\"ok\":false,\"error\":\"plicod already running (PID {}). Use 'plicod stop' first.\"}}",
            existing_pid
        );
        std::process::exit(1);
    }

    // Clean stale PID/socket from crashed previous run
    let pp = pid_path(&root);
    let sp = sock_path(&root);
    if pp.exists() {
        let _ = std::fs::remove_file(&pp);
    }

    println!("Plico AI-Native OS Daemon");
    println!("Storage root: configured PLICO_ROOT");

    let kernel = match AIKernel::new(root.clone()) {
        Ok(kernel) => kernel,
        Err(e) => {
            eprintln!("Failed to initialize kernel: {}", e);
            std::process::exit(1);
        }
    };
    match kernel.ensure_personal_owner_credential() {
        Ok(_) => {}
        Err(error) => {
            eprintln!("Failed to initialize the TCP owner credential: {error}");
            std::process::exit(1);
        }
    }
    let tcp_addr: SocketAddr = match format!("{host}:{port}").parse() {
        Ok(address) => address,
        Err(error) => {
            eprintln!("Invalid configured TCP listen address: {error}");
            std::process::exit(1);
        }
    };
    let tcp_listener = match TcpListener::bind(tcp_addr).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("Failed to bind configured TCP listen address: {error}");
            std::process::exit(1);
        }
    };
    kernel.start_workers();
    println!("TCP owner credential: agent_tokens.json under configured PLICO_ROOT");

    write_pid_file(&pp);

    setup_shutdown_handler(Arc::clone(&kernel), pp.clone(), sp.clone());

    setup_periodic_persist(Arc::clone(&kernel), config.tuning.persist_interval_secs);

    let dispatch = kernel.start_dispatch_loop();
    let _result_consumer = kernel.start_result_consumer(&dispatch);
    println!("Agent dispatch loop + result consumer started.");

    println!("TCP listening on: {}:{}", host, port);

    // UDS listener (Unix only)
    #[cfg(unix)]
    let uds_listener = if !no_uds {
        let _ = std::fs::remove_file(&sp);
        match tokio::net::UnixListener::bind(&sp) {
            Ok(l) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&sp, std::fs::Permissions::from_mode(0o600));
                }
                println!("UDS listening on configured plico.sock");
                Some(l)
            }
            Err(e) => {
                eprintln!("Warning: failed to bind configured plico.sock: {e}");
                None
            }
        }
    } else {
        println!("UDS disabled (--no-uds)");
        None
    };

    println!("Daemon ready. PID file: plicod.pid under configured PLICO_ROOT");
    println!("Awaiting AI connections...");

    #[cfg(unix)]
    {
        if let Some(ref uds) = uds_listener {
            loop {
                tokio::select! {
                    result = tcp_listener.accept() => {
                        match result {
                            Ok((stream, peer)) => {
                                let _ = stream.set_nodelay(true);
                                let kernel = Arc::clone(&kernel);
                                tokio::spawn(async move {
                                    if let Err(e) = handle_connection(stream, kernel, TransportKind::Tcp).await {
                                        tracing::warn!("TCP connection error from {}: {}", peer, e);
                                    }
                                });
                            }
                            Err(e) => tracing::error!("TCP accept error: {}", e),
                        }
                    }
                    result = uds.accept() => {
                        match result {
                            Ok((stream, _addr)) => {
                                let kernel = Arc::clone(&kernel);
                                tokio::spawn(async move {
                                    if let Err(e) = handle_connection(stream, kernel, TransportKind::Uds).await {
                                        tracing::warn!("UDS connection error: {}", e);
                                    }
                                });
                            }
                            Err(e) => tracing::error!("UDS accept error: {}", e),
                        }
                    }
                }
            }
        } else {
            accept_tcp_only(tcp_listener, kernel).await;
        }
    }

    #[cfg(not(unix))]
    {
        accept_tcp_only(tcp_listener, kernel).await;
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn extract_opt(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn setup_shutdown_handler(kernel: Arc<AIKernel>, pid_path: PathBuf, sock_path: PathBuf) {
    #[cfg(unix)]
    {
        tokio::spawn(async move {
            let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
            let sigint = tokio::signal::ctrl_c();
            tokio::select! {
                _ = sigterm.recv() => {},
                _ = sigint => {},
            }
            tracing::info!("Shutdown signal received, stopping projection work...");
            kernel.shutdown_projection_worker();
            tracing::info!("Projection worker stopped; flushing canonical memory...");
            if kernel.flush_canonical_memory().is_err() {
                tracing::error!(error_category = "ledger_flush", "shutdown persistence failed");
            }
            kernel.persist_auxiliary_best_effort();
            let _ = std::fs::remove_file(&pid_path);
            let _ = std::fs::remove_file(&sock_path);
            tracing::info!("Cleanup complete. Exiting.");
            std::process::exit(0);
        });
    }
}

fn setup_periodic_persist(kernel: Arc<AIKernel>, interval_secs: u64) {
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            match kernel.flush_canonical_memory() {
                Ok(()) => tracing::debug!("Periodic canonical memory flush completed"),
                Err(_) => tracing::warn!(error_category = "ledger_flush", "periodic persistence failed"),
            }
            kernel.persist_auxiliary_best_effort();
        }
    });
}

// ── Length-Prefixed Framing ─────────────────────────────────────────

async fn read_frame<R: AsyncReadExt + Unpin>(reader: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    let mut header = [0u8; 4];
    match reader.read(&mut header[..1]).await {
        Ok(0) => return Ok(None),
        Ok(_) => {
            reader.read_exact(&mut header[1..]).await?;
        }
        Err(e) => return Err(e),
    }

    let len = u32::from_be_bytes(header);

    if len == 0 || len > MAX_MESSAGE_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame length {} exceeds max {}", len, MAX_MESSAGE_SIZE),
        ));
    }

    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload).await?;
    Ok(Some(payload))
}

async fn write_frame<W: AsyncWriteExt + Unpin>(writer: &mut W, payload: &[u8]) -> std::io::Result<()> {
    if payload.is_empty() || payload.len() > MAX_MESSAGE_SIZE as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("frame length {} is outside 1..={}", payload.len(), MAX_MESSAGE_SIZE),
        ));
    }
    let len = payload.len() as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(payload).await?;
    writer.flush().await?;
    Ok(())
}

// ── Connection Handler ──────────────────────────────────────────────

async fn handle_connection<S: AsyncReadExt + AsyncWriteExt + Unpin>(
    mut stream: S,
    kernel: Arc<AIKernel>,
    transport: TransportKind,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    loop {
        let frame = match read_frame(&mut stream).await? {
            Some(f) => f,
            None => return Ok(()),
        };
        let decoded = match decode_request_frame(&frame, transport, |bearer| {
            kernel.authenticate_public_bearer(bearer).map_err(|_| ())
        }) {
            Ok(decoded) => decoded,
            Err(error) => {
                tracing::warn!(
                    transport = transport.as_str(),
                    error_category = error.category(),
                    "public request frame rejected"
                );
                return Err(error.into());
            }
        };

        let trace = match &decoded {
            DecodedFrame::Response { trace, .. } | DecodedFrame::Dispatch { trace, .. } => trace,
        };
        let span = request_span(trace);
        let _guard = span.enter();
        let response = match decoded {
            DecodedFrame::Response { response, .. } => response,
            DecodedFrame::Dispatch { request, context, .. } => {
                let response = kernel.handle_public_request(&context, request.clone());
                if response.validate_for(&request).is_ok() {
                    response
                } else {
                    failure(
                        request.request_id,
                        PublicErrorCode::Internal,
                        "public service returned an invalid response",
                        false,
                        "invalid_service_response",
                    )
                }
            }
        };
        tracing::info!(
            outcome = if response.ok { "success" } else { "error" },
            "public transport request completed"
        );
        let json = serde_json::to_vec(&response)?;
        write_frame(&mut stream, &json).await?;
    }
}

#[derive(Debug)]
struct RequestTrace {
    request_id: uuid::Uuid,
    operation: String,
    transport: TransportKind,
    role_id: String,
}

fn request_span(trace: &RequestTrace) -> tracing::Span {
    tracing::info_span!(
        "public_transport_request",
        request_id = %trace.request_id,
        operation = trace.operation.as_str(),
        transport = trace.transport.as_str(),
        role_kind = if trace.role_id == plico::PERSONAL_OWNER_ROLE_ID { "personal_owner" } else { "agent_role" },
    )
}

enum DecodedFrame {
    Dispatch {
        trace: RequestTrace,
        request: PublicRequest,
        context: PublicRequestContext,
    },
    Response {
        trace: RequestTrace,
        response: PublicResponse,
    },
}

#[derive(Debug, thiserror::Error)]
enum RequestFrameError {
    #[error("request does not match the public protocol envelope")]
    Schema,
    #[error("request contains invalid public protocol metadata")]
    Metadata,
}

impl RequestFrameError {
    const fn category(&self) -> &'static str {
        match self {
            Self::Schema => "schema",
            Self::Metadata => "metadata",
        }
    }
}

fn decode_request_frame<F>(
    frame: &[u8],
    transport: TransportKind,
    authenticate: F,
) -> Result<DecodedFrame, RequestFrameError>
where
    F: FnOnce(&str) -> Result<String, ()>,
{
    let head: PublicRequestHead = serde_json::from_slice(frame).map_err(|_| RequestFrameError::Schema)?;
    head.validate_metadata().map_err(|_| RequestFrameError::Metadata)?;

    let (context, role_id) = match transport {
        TransportKind::Tcp => {
            let Some(auth) = &head.auth else {
                return Ok(typed_rejection(
                    &head,
                    transport,
                    "unauthenticated",
                    PublicErrorCode::Unauthenticated,
                    "TCP requests require a valid bearer credential",
                    "missing_bearer",
                ));
            };
            let role_id = match authenticate(&auth.bearer) {
                Ok(role_id) => role_id,
                Err(()) => {
                    return Ok(typed_rejection(
                        &head,
                        transport,
                        "unauthenticated",
                        PublicErrorCode::Unauthenticated,
                        "invalid bearer credential",
                        "invalid_bearer",
                    ));
                }
            };
            (
                PublicRequestContext::authenticated_role(role_id.clone(), PublicTransport::Tcp),
                role_id,
            )
        }
        #[cfg(unix)]
        TransportKind::Uds => {
            if head.auth.is_some() {
                return Ok(typed_rejection(
                    &head,
                    transport,
                    "local_owner",
                    PublicErrorCode::InvalidArgument,
                    "UDS requests must not contain payload authentication",
                    "transport_auth_forbidden",
                ));
            }
            let context = PublicRequestContext::local_owner(PublicTransport::Uds);
            let role_id = context.role_id().to_string();
            (context, role_id)
        }
    };

    if !head.operation_supported() {
        return Ok(typed_rejection(
            &head,
            transport,
            &role_id,
            PublicErrorCode::UnsupportedCapability,
            "operation is not supported by this public protocol",
            "unsupported_operation",
        ));
    }

    let mut request: PublicRequest = match serde_json::from_slice(frame) {
        Ok(request) => request,
        Err(_) => {
            return Ok(typed_rejection(
                &head,
                transport,
                &role_id,
                PublicErrorCode::InvalidArgument,
                "input does not match the operation schema",
                "invalid_operation_input",
            ));
        }
    };
    if request.validate().is_err() {
        return Ok(typed_rejection(
            &head,
            transport,
            &role_id,
            PublicErrorCode::InvalidArgument,
            "input failed public protocol validation",
            "invalid_operation_input",
        ));
    }
    // Credentials are transport metadata. The domain service receives only
    // the trusted context derived above, never the bearer itself.
    request.auth = None;

    Ok(DecodedFrame::Dispatch {
        trace: RequestTrace {
            request_id: request.request_id,
            operation: request.command.operation().to_string(),
            transport,
            role_id,
        },
        request,
        context,
    })
}

fn typed_rejection(
    head: &PublicRequestHead,
    transport: TransportKind,
    role_id: &str,
    code: PublicErrorCode,
    message: &'static str,
    category: &'static str,
) -> DecodedFrame {
    let operation = if head.operation_supported() {
        head.operation.clone()
    } else {
        "unsupported_operation".to_string()
    };
    DecodedFrame::Response {
        trace: RequestTrace {
            request_id: head.request_id,
            operation,
            transport,
            role_id: role_id.to_string(),
        },
        response: failure(head.request_id, code, message, false, category),
    }
}

fn failure(
    request_id: uuid::Uuid,
    code: PublicErrorCode,
    message: &'static str,
    retryable: bool,
    category: &'static str,
) -> PublicResponse {
    PublicResponse::failure(
        request_id,
        PublicError {
            code,
            message: message.to_string(),
            retryable,
            details: Some(serde_json::json!({ "category": category })),
        },
    )
}

async fn accept_tcp_only(listener: TcpListener, kernel: Arc<AIKernel>) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let _ = stream.set_nodelay(true);
                let kernel = Arc::clone(&kernel);
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, kernel, TransportKind::Tcp).await {
                        tracing::warn!("TCP connection error from {}: {}", peer, e);
                    }
                });
            }
            Err(e) => tracing::error!("TCP accept error: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use plico::api::public::{PERSONAL_PROTOCOL, PUBLIC_OPERATIONS};

    fn wire_request(
        request_id: uuid::Uuid,
        operation: &str,
        input: serde_json::Value,
        bearer: Option<&str>,
    ) -> Vec<u8> {
        let mut value = serde_json::json!({
            "protocol": PERSONAL_PROTOCOL,
            "request_id": request_id,
            "operation": operation,
            "input": input,
        });
        if let Some(bearer) = bearer {
            value["auth"] = serde_json::json!({ "bearer": bearer });
        }
        serde_json::to_vec(&value).unwrap()
    }

    fn valid_input(operation: &str) -> serde_json::Value {
        let id = uuid::Uuid::new_v4();
        match operation {
            "capabilities.describe" | "runtime.readiness" | "session.start" => serde_json::json!({}),
            "object.put" => serde_json::json!({ "content": "object" }),
            "object.get" => serde_json::json!({ "cid": "a".repeat(64) }),
            "object.search" => serde_json::json!({ "query": "object" }),
            "memory.create" => serde_json::json!({ "content": "memory" }),
            "memory.get" | "memory.delete" => {
                serde_json::json!({ "entry_id": id })
            }
            "memory.recall" => serde_json::json!({ "query": "memory" }),
            "projection.status" => {
                serde_json::json!({ "kind": "memory_embedding", "revision_id": id })
            }
            "projection.rebuild" => {
                serde_json::json!({ "kind": "memory_embedding", "selector": { "type": "all_eligible" } })
            }
            "memory.update" => serde_json::json!({ "entry_id": id, "content": "corrected" }),
            "session.end" => serde_json::json!({ "session_id": id }),
            other => panic!("missing representative input for {other}"),
        }
    }

    fn response_from(decoded: DecodedFrame) -> PublicResponse {
        match decoded {
            DecodedFrame::Response { response, .. } => response,
            DecodedFrame::Dispatch { .. } => panic!("expected a typed transport rejection"),
        }
    }

    #[tokio::test]
    async fn frame_round_trip_preserves_payload() {
        let (mut writer, mut reader) = tokio::io::duplex(64);
        let write = tokio::spawn(async move { write_frame(&mut writer, br#"{"ok":true}"#).await });

        let payload = read_frame(&mut reader).await.unwrap().unwrap();
        write.await.unwrap().unwrap();

        assert_eq!(payload, br#"{"ok":true}"#);
    }

    #[tokio::test]
    async fn clean_eof_is_not_an_invalid_frame() {
        let mut empty = &[][..];
        assert!(read_frame(&mut empty).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn truncated_header_fails_closed() {
        let mut truncated = &[0_u8, 0_u8][..];
        let error = read_frame(&mut truncated).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn invalid_frame_lengths_are_rejected() {
        let zero_header = 0_u32.to_be_bytes();
        let mut zero = zero_header.as_slice();
        let zero_error = read_frame(&mut zero).await.unwrap_err();
        assert_eq!(zero_error.kind(), std::io::ErrorKind::InvalidData);

        let oversized_header = (MAX_MESSAGE_SIZE + 1).to_be_bytes();
        let mut oversized = oversized_header.as_slice();
        let oversized_error = read_frame(&mut oversized).await.unwrap_err();
        assert_eq!(oversized_error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn writer_rejects_empty_payload() {
        let (mut writer, _reader) = tokio::io::duplex(8);
        let error = write_frame(&mut writer, &[]).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn transport_kind_is_explicit() {
        assert_eq!(TransportKind::Tcp.as_str(), "tcp");
        #[cfg(unix)]
        assert_eq!(TransportKind::Uds.as_str(), "uds");
    }

    #[test]
    fn tcp_authenticates_before_dispatch_for_every_public_operation() {
        for operation in PUBLIC_OPERATIONS {
            let frame = wire_request(
                uuid::Uuid::new_v4(),
                operation,
                valid_input(operation),
                Some("valid-secret"),
            );
            let authenticated = Cell::new(false);
            let decoded = decode_request_frame(&frame, TransportKind::Tcp, |bearer| {
                assert_eq!(bearer, "valid-secret");
                authenticated.set(true);
                Ok("personal-owner".to_string())
            })
            .unwrap();

            assert!(authenticated.get(), "authentication was skipped for {operation}");
            let DecodedFrame::Dispatch {
                request,
                context,
                trace,
            } = decoded
            else {
                panic!("valid {operation} request was not dispatchable");
            };
            assert_eq!(request.command.operation(), operation);
            assert!(request.auth.is_none(), "bearer crossed the transport boundary");
            assert_eq!(context.transport, PublicTransport::Tcp);
            assert_eq!(context.role_id(), "personal-owner");
            assert_eq!(trace.role_id, "personal-owner");
        }
    }

    #[test]
    fn tcp_missing_or_invalid_bearer_is_typed_and_never_dispatches() {
        let id = uuid::Uuid::new_v4();
        let missing = wire_request(id, "runtime.readiness", serde_json::json!({}), None);
        let response = response_from(
            decode_request_frame(&missing, TransportKind::Tcp, |_| {
                panic!("missing bearer must not call authenticator")
            })
            .unwrap(),
        );
        assert_eq!(response.error.unwrap().code, PublicErrorCode::Unauthenticated);

        let invalid = wire_request(id, "runtime.readiness", serde_json::json!({}), Some("invalid"));
        let response = response_from(decode_request_frame(&invalid, TransportKind::Tcp, |_| Err(())).unwrap());
        assert_eq!(response.error.unwrap().code, PublicErrorCode::Unauthenticated);
    }

    #[test]
    fn tcp_authenticates_before_classifying_unknown_operation() {
        let authenticated = Cell::new(false);
        let frame = wire_request(
            uuid::Uuid::new_v4(),
            "legacy.delete_everything",
            serde_json::json!({}),
            Some("valid-secret"),
        );
        let response = response_from(
            decode_request_frame(&frame, TransportKind::Tcp, |_| {
                authenticated.set(true);
                Ok("personal-owner".to_string())
            })
            .unwrap(),
        );

        assert!(authenticated.get());
        assert_eq!(response.error.unwrap().code, PublicErrorCode::UnsupportedCapability);
    }

    #[test]
    fn known_operation_with_bad_input_is_typed_invalid_argument() {
        let frame = wire_request(
            uuid::Uuid::new_v4(),
            "object.get",
            serde_json::json!({ "cid": "not-a-cid" }),
            Some("valid-secret"),
        );
        let response = response_from(
            decode_request_frame(&frame, TransportKind::Tcp, |_| Ok("personal-owner".to_string())).unwrap(),
        );

        assert_eq!(response.error.unwrap().code, PublicErrorCode::InvalidArgument);
    }

    #[cfg(unix)]
    #[test]
    fn uds_rejects_payload_auth_and_injects_local_owner_context() {
        let id = uuid::Uuid::new_v4();
        let with_auth = wire_request(
            id,
            "runtime.readiness",
            serde_json::json!({}),
            Some("must-not-cross-uds"),
        );
        let response = response_from(
            decode_request_frame(&with_auth, TransportKind::Uds, |_| {
                panic!("UDS must never authenticate payload credentials")
            })
            .unwrap(),
        );
        assert_eq!(response.error.unwrap().code, PublicErrorCode::InvalidArgument);

        let without_auth = wire_request(id, "runtime.readiness", serde_json::json!({}), None);
        let DecodedFrame::Dispatch { context, trace, .. } =
            decode_request_frame(&without_auth, TransportKind::Uds, |_| {
                panic!("UDS must never call the bearer authenticator")
            })
            .unwrap()
        else {
            panic!("valid local request was not dispatchable");
        };
        assert_eq!(context.transport, PublicTransport::Uds);
        assert_eq!(context.role_id(), "personal-owner");
        assert_eq!(trace.role_id, "personal-owner");
    }

    #[test]
    fn legacy_envelope_without_request_id_is_a_schema_error() {
        let legacy = br#"{"method":"delete","params":{"id":"anything"}}"#;
        assert!(matches!(
            decode_request_frame(legacy, TransportKind::Tcp, |_| {
                panic!("legacy envelopes must fail before authentication")
            }),
            Err(RequestFrameError::Schema)
        ));
    }

    #[test]
    fn complete_v1_envelope_is_rejected_before_authentication_or_dispatch() {
        let authenticated = Cell::new(0_u32);
        let frame = serde_json::to_vec(&serde_json::json!({
            "protocol": "plico.personal.v1",
            "request_id": uuid::Uuid::new_v4(),
            "operation": "runtime.readiness",
            "input": {},
            "auth": { "bearer": "must-not-be-read" }
        }))
        .unwrap();
        assert!(matches!(
            decode_request_frame(&frame, TransportKind::Tcp, |_| {
                authenticated.set(authenticated.get() + 1);
                Ok("personal-owner".to_string())
            }),
            Err(RequestFrameError::Metadata)
        ));
        assert_eq!(authenticated.get(), 0);
    }

    #[test]
    fn request_span_has_a_fixed_transport_event_name() {
        let trace = RequestTrace {
            request_id: uuid::Uuid::new_v4(),
            operation: "memory.create".to_string(),
            transport: TransportKind::Tcp,
            role_id: "personal-owner".to_string(),
        };
        tracing::subscriber::with_default(tracing_subscriber::registry(), || {
            assert_eq!(
                request_span(&trace).metadata().unwrap().name(),
                "public_transport_request"
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn unsupported_operation_trace_never_contains_raw_operation() {
        let canary = "unknown.operation.PRIVATE_CONTROL_CANARY";
        let frame = serde_json::to_vec(&serde_json::json!({
            "protocol": plico::api::public::PERSONAL_PROTOCOL,
            "request_id": uuid::Uuid::new_v4(),
            "operation": canary,
            "input": {},
        }))
        .unwrap();
        let DecodedFrame::Response { trace, .. } = decode_request_frame(&frame, TransportKind::Uds, |_| {
            panic!("UDS decoding must not authenticate a payload bearer")
        })
        .unwrap() else {
            panic!("unsupported operation must be rejected")
        };
        assert_eq!(trace.operation, "unsupported_operation");
        assert!(!format!("{trace:?}").contains(canary));
    }
}
