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
    // Parse args
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

    // Initialize structured logging (reads RUST_LOG env var; defaults to INFO)
    // Use fmt().finish().try_init() instead of fmt::init() to avoid background
    // worker threads that prevent the process from exiting cleanly.
    let env = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    tracing_subscriber::fmt()
        .with_env_filter(&env)
        .with_writer(std::io::stderr)
        .finish()
        .try_init()
        .ok();

    // Initialize kernel
    let kernel = match AIKernel::new(root) {
        Ok(k) => Arc::new(k),
        Err(e) => {
            eprintln!("Failed to initialize kernel: {}", e);
            std::process::exit(1);
        }
    };

    // Spawn the agent execution dispatch loop via kernel (no direct subsystem imports).
    let dispatch = kernel.start_dispatch_loop();

    // Spawn result consumer — drains execution results into memory for autonomous learning.
    let _result_consumer = kernel.start_result_consumer(&dispatch);

    println!("Agent dispatch loop + result consumer started.");

    let addr: SocketAddr = format!("0.0.0.0:{}", port).parse().unwrap();
    let listener = TcpListener::bind(addr).await.expect("Failed to bind port");
    println!("Daemon ready. Awaiting AI connections...");

    // Start HTTP dashboard server on port 7879
    let dashboard_kernel = Arc::clone(&kernel);
    tokio::spawn(async move {
        if let Err(e) = run_dashboard_server(dashboard_kernel).await {
            eprintln!("Dashboard HTTP server error: {}", e);
        }
    });
    println!("Dashboard HTTP server: http://127.0.0.1:7879/api/status");

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
            return Ok(()); // Connection closed
        }

        let request: ApiRequest = match serde_json::from_slice(&buf[..n]) {
            Ok(r) => r,
            Err(e) => {
                send_error(&mut stream, format!("parse error: {}", e)).await?;
                return Ok(());
            }
        };

        let response = handle_request(kernel, request);
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

fn handle_request(kernel: &AIKernel, req: ApiRequest) -> ApiResponse {
    kernel.handle_api_request(req)
}

// ─── HTTP Dashboard Server ────────────────────────────────────────────────────────


async fn run_dashboard_server(kernel: Arc<AIKernel>) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:7879").await?;
    println!("Dashboard HTTP server listening on http://127.0.0.1:7879");

    loop {
        let (stream, peer) = listener.accept().await?;
        let kernel = Arc::clone(&kernel);
        tokio::spawn(async move {
            if let Err(e) = handle_dashboard_http(stream, &kernel).await {
                eprintln!("Dashboard HTTP error from {}: {}", peer, e);
            }
        });
    }
}

async fn handle_dashboard_http(
    mut stream: tokio::net::TcpStream,
    kernel: &Arc<AIKernel>,
) -> std::io::Result<()> {
    let mut buf = vec![0u8; 65536];
    let n = stream.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }

    let request = String::from_utf8_lossy(&buf[..n]);

    let first_line = request.lines().next().unwrap_or("");
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    let method = parts.first().unwrap_or(&"");
    let path = parts.get(1).unwrap_or(&"/");

    let (status, body) = match (*method, *path) {
        ("GET", "/api/status") | ("GET", "/") => {
            let metrics = kernel.dashboard_status();
            let json = serde_json::to_string(&metrics).unwrap_or_else(|_| r#"{"error":"serialization failed"}"#.to_string());
            (200, json)
        }
        ("GET", "/health") => {
            (200, r#"{"ok":true}"#.to_string())
        }
        ("OPTIONS", "/api") => {
            let resp = "HTTP/1.1 204 No Content\r\n\
                 Access-Control-Allow-Origin: *\r\n\
                 Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
                 Access-Control-Allow-Headers: Content-Type\r\n\
                 Content-Length: 0\r\n\
                 Connection: close\r\n\
                 \r\n";
            stream.write_all(resp.as_bytes()).await?;
            stream.flush().await?;
            return Ok(());
        }
        ("POST", "/api") => {
            let http_body = request.find("\r\n\r\n")
                .map(|pos| &request[pos + 4..])
                .unwrap_or("");
            if http_body.is_empty() {
                (400, r#"{"ok":false,"error":"empty request body"}"#.to_string())
            } else {
                match serde_json::from_str::<ApiRequest>(http_body) {
                    Ok(api_req) => {
                        let api_resp = kernel.handle_api_request(api_req);
                        let json = serde_json::to_string(&api_resp)
                            .unwrap_or_else(|_| r#"{"ok":false,"error":"serialization failed"}"#.to_string());
                        (if api_resp.ok { 200 } else { 400 }, json)
                    }
                    Err(e) => {
                        (400, format!(r#"{{"ok":false,"error":"parse error: {}"}}"#, e))
                    }
                }
            }
        }
        _ => {
            (404, r#"{"error":"not found"}"#.to_string())
        }
    };

    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "OK",
    };
    let response = format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         Access-Control-Allow-Origin: *\r\n\
         \r\n\
         {}",
        status,
        status_text,
        body.len(),
        body
    );

    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}
