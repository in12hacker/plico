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

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time;
use tracing_subscriber::util::SubscriberInitExt;

const MAX_MESSAGE_SIZE: u32 = 16 * 1024 * 1024; // 16 MiB

// ── Subcommand Dispatch ─────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = resolve_root(&args);

    match detect_subcommand(&args) {
        Subcommand::Start => cmd_start(args, root).await,
        Subcommand::Stop  => cmd_stop(&root),
        Subcommand::Status => cmd_status(&root),
    }
}

enum Subcommand { Start, Stop, Status }

fn detect_subcommand(args: &[String]) -> Subcommand {
    for arg in args.iter().skip(1) {
        match arg.as_str() {
            "stop"   => return Subcommand::Stop,
            "status" => return Subcommand::Status,
            "start"  => return Subcommand::Start,
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
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".plico")
        })
}

// ── PID Management ──────────────────────────────────────────────────

fn pid_path(root: &Path) -> PathBuf { root.join("plicod.pid") }
fn sock_path(root: &Path) -> PathBuf { root.join("plico.sock") }

/// Read PID file and check if the process is still alive.
/// Returns `Some(pid)` if daemon is running, `None` otherwise.
fn check_existing_daemon(root: &Path) -> Option<u32> {
    let path = pid_path(root);
    let pid_str = std::fs::read_to_string(&path).ok()?;
    let pid: u32 = pid_str.trim().parse().ok()?;
    if pid == 0 { return None; }
    // Check if process exists via /proc on Linux, or kill(0) on Unix
    #[cfg(unix)]
    {
        let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
        if alive { Some(pid) } else { None }
    }
    #[cfg(not(unix))]
    {
        std::fs::metadata(format!("/proc/{}", pid)).ok().map(|_| pid)
    }
}

fn write_pid_file(path: &Path) {
    if let Err(e) = std::fs::write(path, std::process::id().to_string()) {
        eprintln!("Warning: failed to write PID file {:?}: {}", path, e);
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
            eprintln!("{{\"ok\":false,\"error\":\"plicod is not running (no live PID in {:?})\"}}",
                pid_path(root));
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
                "{{\"ok\":true,\"running\":true,\"pid\":{},\"pid_file\":\"{}\",\"socket\":\"{}\",\"socket_exists\":{}}}",
                pid,
                pp.display(),
                sp.display(),
                sock_exists,
            );
        }
        None => {
            let stale = pp.exists();
            if stale {
                let _ = std::fs::remove_file(&pp);
                let _ = std::fs::remove_file(&sp);
            }
            println!(
                "{{\"ok\":true,\"running\":false,\"stale_pid_cleaned\":{}}}",
                stale,
            );
            std::process::exit(1);
        }
    }
}

// ── cmd_start (main daemon logic) ───────────────────────────────────

async fn cmd_start(args: Vec<String>, root: PathBuf) {
    let port = extract_opt(&args, "--port")
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(7878);
    let no_uds = args.iter().any(|a| a == "--no-uds");

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

    // Ensure root directory exists
    let _ = std::fs::create_dir_all(&root);

    println!("Plico AI-Native OS Daemon");
    println!("Storage root: {:?}", root);

    let kernel = match AIKernel::new(root.clone()) {
        Ok(k) => Arc::new(k),
        Err(e) => {
            eprintln!("Failed to initialize kernel: {}", e);
            std::process::exit(1);
        }
    };

    write_pid_file(&pp);

    setup_shutdown_handler(
        Arc::clone(&kernel),
        pp.clone(),
        sp.clone(),
    );

    setup_periodic_persist(Arc::clone(&kernel));

    let dispatch = kernel.start_dispatch_loop();
    let _result_consumer = kernel.start_result_consumer(&dispatch);
    println!("Agent dispatch loop + result consumer started.");

    // TCP listener
    let tcp_addr: SocketAddr = format!("0.0.0.0:{}", port).parse().unwrap();
    let tcp_listener = TcpListener::bind(tcp_addr).await.expect("Failed to bind TCP port");
    println!("TCP listening on: 0.0.0.0:{}", port);

    // UDS listener (Unix only)
    #[cfg(unix)]
    let uds_listener = if !no_uds {
        let _ = std::fs::remove_file(&sp);
        match tokio::net::UnixListener::bind(&sp) {
            Ok(l) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&sp,
                        std::fs::Permissions::from_mode(0o600));
                }
                println!("UDS listening on: {:?}", sp);
                Some(l)
            }
            Err(e) => {
                eprintln!("Warning: failed to bind UDS at {:?}: {}", sp, e);
                None
            }
        }
    } else {
        println!("UDS disabled (--no-uds)");
        None
    };

    println!("Daemon ready. PID file: {:?}", pp);
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
                                    if let Err(e) = handle_connection(stream, &kernel).await {
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
                                    if let Err(e) = handle_connection(stream, &kernel).await {
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
    args.iter().position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn setup_shutdown_handler(kernel: Arc<AIKernel>, pid_path: PathBuf, sock_path: PathBuf) {
    #[cfg(unix)]
    {
        tokio::spawn(async move {
            let mut sigterm = tokio::signal::unix::signal(
                tokio::signal::unix::SignalKind::terminate()
            ).unwrap();
            let sigint = tokio::signal::ctrl_c();
            tokio::select! {
                _ = sigterm.recv() => {},
                _ = sigint => {},
            }
            tracing::info!("Shutdown signal received, persisting all state...");
            kernel.persist_all();
            let _ = std::fs::remove_file(&pid_path);
            let _ = std::fs::remove_file(&sock_path);
            tracing::info!("Cleanup complete. Exiting.");
            std::process::exit(0);
        });
    }
}

fn setup_periodic_persist(kernel: Arc<AIKernel>) {
    let interval_secs: u64 = std::env::var("PLICO_PERSIST_INTERVAL_SECS")
        .unwrap_or_else(|_| "300".to_string())
        .parse()
        .unwrap_or(300);
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            kernel.persist_all();
            tracing::debug!("Periodic persist completed");
        }
    });
}

// ── Length-Prefixed Framing ─────────────────────────────────────────

async fn read_frame<R: AsyncReadExt + Unpin>(reader: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    let mut header = [0u8; 4];
    match reader.read_exact(&mut header).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
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
    let len = payload.len() as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(payload).await?;
    writer.flush().await?;
    Ok(())
}

// ── Connection Handler ──────────────────────────────────────────────

async fn handle_connection<S: AsyncReadExt + AsyncWriteExt + Unpin>(
    mut stream: S,
    kernel: &Arc<AIKernel>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    loop {
        let frame = match read_frame(&mut stream).await? {
            Some(f) => f,
            None => return Ok(()),
        };
        let request: ApiRequest = match serde_json::from_slice(&frame) {
            Ok(r) => r,
            Err(e) => {
                let err_resp = ApiResponse::error(format!("parse error: {}", e));
                let json = serde_json::to_vec(&err_resp)?;
                write_frame(&mut stream, &json).await?;
                return Ok(());
            }
        };
        let response = kernel.handle_api_request(request);
        let json = serde_json::to_vec(&response)?;
        write_frame(&mut stream, &json).await?;
    }
}

async fn accept_tcp_only(listener: TcpListener, kernel: Arc<AIKernel>) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let _ = stream.set_nodelay(true);
                let kernel = Arc::clone(&kernel);
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, &kernel).await {
                        tracing::warn!("TCP connection error from {}: {}", peer, e);
                    }
                });
            }
            Err(e) => tracing::error!("TCP accept error: {}", e),
        }
    }
}
