//! plicod — Plico AI-Native OS Daemon
//!
//! Long-running daemon exposing the semantic API over TCP and Unix Domain Socket.
//! Also runs the agent execution dispatch loop in the background.
//!
//! Usage: cargo run --bin plicod [--port PORT] [--root PATH] [--no-uds]
//!
//! # Protocol
//!
//! Length-prefixed JSON framing over TCP/UDS:
//!   [4-byte big-endian length][JSON payload]
//!
//! # Daemon Lifecycle
//!
//! On startup: writes PID to `<root>/plicod.pid`, creates UDS at `<root>/plico.sock`.
//! On shutdown: persists state, removes PID file and socket.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time;
use tracing_subscriber::util::SubscriberInitExt;

const MAX_MESSAGE_SIZE: u32 = 16 * 1024 * 1024; // 16 MiB

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let port = extract_opt(&args, "--port")
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(7878);
    let no_uds = args.iter().any(|a| a == "--no-uds");
    let root = extract_opt(&args, "--root")
        .map(PathBuf::from)
        .or_else(|| std::env::var("PLICO_ROOT").ok().map(PathBuf::from))
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".plico")
        });

    let env = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    tracing_subscriber::fmt()
        .with_env_filter(&env)
        .with_writer(std::io::stderr)
        .finish()
        .try_init()
        .ok();

    println!("Plico AI-Native OS Daemon");
    println!("Storage root: {:?}", root);

    let kernel = match AIKernel::new(root.clone()) {
        Ok(k) => Arc::new(k),
        Err(e) => {
            eprintln!("Failed to initialize kernel: {}", e);
            std::process::exit(1);
        }
    };

    let pid_path = root.join("plicod.pid");
    let sock_path = root.join("plico.sock");

    write_pid_file(&pid_path);

    // Graceful shutdown: persist state, remove PID file and socket
    setup_shutdown_handler(
        Arc::clone(&kernel),
        pid_path.clone(),
        sock_path.clone(),
    );

    // Periodic persistence
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
        // Remove stale socket if present
        let _ = std::fs::remove_file(&sock_path);
        match tokio::net::UnixListener::bind(&sock_path) {
            Ok(l) => {
                // Restrict socket permissions to owner only
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&sock_path,
                        std::fs::Permissions::from_mode(0o600));
                }
                println!("UDS listening on: {:?}", sock_path);
                Some(l)
            }
            Err(e) => {
                eprintln!("Warning: failed to bind UDS at {:?}: {}", sock_path, e);
                None
            }
        }
    } else {
        println!("UDS disabled (--no-uds)");
        None
    };

    println!("Daemon ready. PID file: {:?}", pid_path);
    println!("Awaiting AI connections...");

    #[cfg(unix)]
    {
        if let Some(ref uds) = uds_listener {
            loop {
                tokio::select! {
                    result = tcp_listener.accept() => {
                        match result {
                            Ok((stream, peer)) => {
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

// ── Helpers ──────────────────────────────────────────────────────────

fn extract_opt(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn write_pid_file(path: &PathBuf) {
    if let Err(e) = std::fs::write(path, std::process::id().to_string()) {
        eprintln!("Warning: failed to write PID file {:?}: {}", path, e);
    }
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

// ── Length-Prefixed Framing ──────────────────────────────────────────

/// Read one length-prefixed frame: [4-byte big-endian length][JSON payload].
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

/// Write a length-prefixed frame.
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
