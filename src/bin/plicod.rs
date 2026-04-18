//! plicod — Plico AI-Native OS Daemon
//!
//! Long-running TCP server exposing the semantic API for external AI programs.
//! Also runs the agent execution dispatch loop in the background.
//!
//! Usage: cargo run --bin plicod [--port PORT] [--root PATH]
//!
//! # Protocol
//!
//! JSON messages over TCP. Connect, send ApiRequest as JSON, receive ApiResponse.
//!
//! # System Status
//!
//! Query via the semantic API: `{"system_status": null}` — no separate HTTP dashboard.
//! This follows the soul principle: all interaction via agent-facing semantic APIs.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let port = args.iter().position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(7878);
    let root = args.iter().position(|a| a == "--root")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .or_else(|| std::env::var("PLICO_ROOT").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/tmp/plico"));

    println!("Plico AI-Native OS Daemon");
    println!("Storage root: {:?}", root);
    println!("Listening on: 0.0.0.0:{}", port);

    let env = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    tracing_subscriber::fmt()
        .with_env_filter(&env)
        .with_writer(std::io::stderr)
        .finish()
        .try_init()
        .ok();

    let kernel = match AIKernel::new(root) {
        Ok(k) => Arc::new(k),
        Err(e) => {
            eprintln!("Failed to initialize kernel: {}", e);
            std::process::exit(1);
        }
    };

    let dispatch = kernel.start_dispatch_loop();
    let _result_consumer = kernel.start_result_consumer(&dispatch);

    println!("Agent dispatch loop + result consumer started.");

    let addr: SocketAddr = format!("0.0.0.0:{}", port).parse().unwrap();
    let listener = TcpListener::bind(addr).await.expect("Failed to bind port");
    println!("Daemon ready. Awaiting AI connections...");

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let kernel = Arc::clone(&kernel);
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, &kernel).await {
                        eprintln!("Connection error from {}: {}", peer, e);
                    }
                });
            }
            Err(e) => {
                eprintln!("Accept error: {}", e);
            }
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    kernel: &Arc<AIKernel>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut buf = vec![0u8; 65536];
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }

        let request: ApiRequest = match serde_json::from_slice(&buf[..n]) {
            Ok(r) => r,
            Err(e) => {
                send_error(&mut stream, format!("parse error: {}", e)).await?;
                return Ok(());
            }
        };

        let response = kernel.handle_api_request(request);
        send_response(&mut stream, response).await?;
    }
}

async fn send_response(stream: &mut TcpStream, response: ApiResponse) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let json = serde_json::to_vec(&response).unwrap();
    stream.write_all(&json).await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;
    Ok(())
}

async fn send_error(stream: &mut TcpStream, msg: String) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    send_response(stream, ApiResponse::error(msg)).await
}
