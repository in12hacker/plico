//! plicod — Plico AI-Native OS Daemon
//!
//! Long-running TCP server exposing the semantic API for external AI programs.
//!
//! Usage: cargo run --bin plicod [--port PORT] [--root PATH]
//!
//! # Protocol
//!
//! JSON messages over TCP. Connect, send ApiRequest as JSON, receive ApiResponse.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse, SearchResultDto, AgentDto};
use plico::memory::MemoryContent;
use std::net::{TcpListener, TcpStream};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

fn main() {
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

    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).expect("Failed to bind port");
    println!("Daemon ready. Awaiting AI connections...");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let kernel = Arc::clone(&kernel);
                std::thread::spawn(|| handle_connection(stream, kernel));
            }
            Err(e) => {
                eprintln!("Connection error: {}", e);
            }
        }
    }
}

fn handle_connection(mut stream: TcpStream, kernel: Arc<AIKernel>) {
    let mut buf = [0u8; 65536];
    loop {
        let n = match stream.read(&mut buf) {
            Ok(0) => return, // Connection closed
            Ok(n) => n,
            Err(e) => {
                eprintln!("Read error: {}", e);
                return;
            }
        };

        let request: ApiRequest = match serde_json::from_slice(&buf[..n]) {
            Ok(r) => r,
            Err(e) => {
                let _ = send_response(&mut stream, ApiResponse::error(format!("parse error: {}", e)));
                return;
            }
        };

        let response = handle_request(&kernel, request);
        if let Err(e) = send_response(&mut stream, response) {
            eprintln!("Write error: {}", e);
            return;
        }
    }
}

fn send_response(stream: &mut TcpStream, response: ApiResponse) -> std::io::Result<()> {
    let json = serde_json::to_vec(&response).unwrap();
    stream.write_all(&json)?;
    stream.write_all(b"\n")?;
    stream.flush()
}

fn handle_request(kernel: &AIKernel, req: ApiRequest) -> ApiResponse {
    match req {
        ApiRequest::Create { content, tags, agent_id, intent } => {
            match kernel.semantic_create(
                content.into_bytes(),
                tags,
                &agent_id,
                intent,
            ) {
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
            ApiResponse { ok: true, cid: None, data: None, results: Some(dto), agent_id: None, agents: None, memory: None, tags: None, error: None }
        }

        ApiRequest::Update { cid, content, new_tags, agent_id } => {
            match kernel.semantic_update(&cid, content.into_bytes(), new_tags, &agent_id) {
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
            ApiResponse { ok: true, cid: None, data: None, results: None, agent_id: Some(id), agents: None, memory: None, tags: None, error: None }
        }

        ApiRequest::ListAgents => {
            let agents: Vec<AgentDto> = kernel.list_agents().into_iter().map(|a| AgentDto {
                id: a.id,
                name: a.name,
                state: format!("{:?}", a.state),
            }).collect();
            ApiResponse { ok: true, cid: None, data: None, results: None, agent_id: None, agents: Some(agents), memory: None, tags: None, error: None }
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
            ApiResponse { ok: true, cid: None, data: None, results: None, agent_id: None, agents: None, memory: Some(memories), tags: None, error: None }
        }
    }
}
