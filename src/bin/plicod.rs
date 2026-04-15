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
use plico::api::semantic::{ApiRequest, ApiResponse, SearchResultDto, AgentDto, NeighborDto, decode_content};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

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
        .unwrap_or_else(|| PathBuf::from("/var/plico"));

    println!("Plico AI-Native OS Daemon");
    println!("Storage root: {:?}", root);
    println!("Listening on: 0.0.0.0:{}", port);

    // Initialize kernel
    let kernel = match AIKernel::new(root) {
        Ok(k) => Arc::new(k),
        Err(e) => {
            eprintln!("Failed to initialize kernel: {}", e);
            std::process::exit(1);
        }
    };

    // Spawn the agent execution dispatch loop via kernel (no direct subsystem imports).
    let _dispatch = kernel.start_dispatch_loop();

    println!("Agent dispatch loop started.");

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
    match req {
        ApiRequest::Create { content, content_encoding, tags, agent_id, intent } => {
            let bytes = match decode_content(&content, &content_encoding) {
                Ok(b) => b,
                Err(e) => return ApiResponse::error(e),
            };
            match kernel.semantic_create(bytes, tags, &agent_id, intent) {
                Ok(cid) => ApiResponse::with_cid(cid),
                Err(e) => ApiResponse::error(e.to_string()),
            }
        }

        ApiRequest::Read { cid, agent_id: _ } => {
            match kernel.get_object(&cid, "kernel") {
                Ok(obj) => ApiResponse::with_data(String::from_utf8_lossy(&obj.data).to_string()),
                Err(e) => ApiResponse::error(e.to_string()),
            }
        }

        ApiRequest::Search { query, agent_id, limit } => {
            let results = kernel.semantic_search(&query, &agent_id, limit.unwrap_or(10));
            let dto: Vec<SearchResultDto> = results.into_iter().map(|r| SearchResultDto {
                cid: r.cid,
                relevance: r.relevance,
                tags: r.meta.tags,
            }).collect();
            ApiResponse { ok: true, cid: None, data: None, results: Some(dto), agent_id: None, agents: None, memory: None, tags: None, neighbors: None, error: None }
        }

        ApiRequest::Update { cid, content, content_encoding, new_tags, agent_id } => {
            let bytes = match decode_content(&content, &content_encoding) {
                Ok(b) => b,
                Err(e) => return ApiResponse::error(e),
            };
            match kernel.semantic_update(&cid, bytes, new_tags, &agent_id) {
                Ok(new_cid) => ApiResponse::with_cid(new_cid),
                Err(e) => ApiResponse::error(e.to_string()),
            }
        }

        ApiRequest::Delete { cid, agent_id } => {
            match kernel.semantic_delete(&cid, &agent_id) {
                Ok(()) => ApiResponse::ok(),
                Err(e) => ApiResponse::error(e.to_string()),
            }
        }

        ApiRequest::RegisterAgent { name } => {
            let id = kernel.register_agent(name);
            ApiResponse { ok: true, cid: None, data: None, results: None, agent_id: Some(id), agents: None, memory: None, tags: None, neighbors: None, error: None }
        }

        ApiRequest::ListAgents => {
            let agents: Vec<AgentDto> = kernel.list_agents().into_iter().map(|a| AgentDto {
                id: a.id,
                name: a.name,
                state: format!("{:?}", a.state),
            }).collect();
            ApiResponse { ok: true, cid: None, data: None, results: None, agent_id: None, agents: Some(agents), memory: None, tags: None, neighbors: None, error: None }
        }

        ApiRequest::Remember { agent_id, content } => {
            kernel.remember(&agent_id, content);
            ApiResponse::ok()
        }

        ApiRequest::Recall { agent_id } => {
            let memories: Vec<String> = kernel.recall(&agent_id)
                .into_iter()
                .filter_map(|m| match m.content {
                    plico::memory::MemoryContent::Text(t) => Some(t),
                    _ => None,
                })
                .collect();
            ApiResponse { ok: true, cid: None, data: None, results: None, agent_id: None, agents: None, memory: Some(memories), tags: None, neighbors: None, error: None }
        }

        ApiRequest::Explore { cid, edge_type, depth, agent_id: _ } => {
            let depth = depth.unwrap_or(1).min(3);
            let raw = kernel.graph_explore_raw(&cid, edge_type.as_deref(), depth);
            let dto: Vec<NeighborDto> = raw.into_iter().map(|(node_id, label, node_type, edge_type, authority_score)| {
                NeighborDto { node_id, label, node_type, edge_type, authority_score }
            }).collect();
            ApiResponse { ok: true, cid: None, data: None, results: None, agent_id: None, agents: None, memory: None, tags: None, neighbors: Some(dto), error: None }
        }
    }
}
