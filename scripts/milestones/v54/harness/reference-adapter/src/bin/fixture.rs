//! Deterministic MCP fixture server for the Migration-A.1 corpus.
//!
//! Mode is chosen by argv[1]. Every mode speaks newline-delimited
//! JSON-RPC over stdio. Responses are minimal but shape-correct for the
//! frozen 2024-11-05 contract; `_meta` is tolerated and ignored in every
//! mode (MCP-B-R1 behavior model).

use std::io::{BufRead, BufReader, Read, Write};

const FROZEN_14: [&str; 14] = [
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

const SECRET: &str = "SK13-HARNESS-SECRET-do-not-log";

fn emit(value: &serde_json::Value) -> bool {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let sent = serde_json::to_writer(&mut lock, value).is_ok()
        && lock.write_all(b"\n").is_ok()
        && lock.flush().is_ok();
    sent
}

fn result(id: serde_json::Value, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn initialize_result(id: &serde_json::Value) -> serde_json::Value {
    result(
        id.clone(),
        serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "fixture", "version": "0" }
        }),
    )
}

fn tools_result_wide(id: &serde_json::Value) -> serde_json::Value {
    let mut tools: Vec<serde_json::Value> = FROZEN_14
        .iter()
        .map(|name| serde_json::json!({ "name": name, "description": format!("fixture {name}") }))
        .collect();
    tools.push(serde_json::json!({ "name": "extra.one", "description": "drift" }));
    tools.push(serde_json::json!({ "name": "extra.two", "description": "drift" }));
    result(id.clone(), serde_json::json!({ "tools": tools }))
}

fn tools_result(id: &serde_json::Value) -> serde_json::Value {
    let tools: Vec<serde_json::Value> = FROZEN_14
        .iter()
        .map(|name| serde_json::json!({ "name": name, "description": format!("fixture {name}") }))
        .collect();
    result(id.clone(), serde_json::json!({ "tools": tools }))
}

fn call_result(id: &serde_json::Value, tag: &str) -> serde_json::Value {
    result(
        id.clone(),
        serde_json::json!({ "content": [ { "type": "text", "text": format!("ok:{tag}") } ] }),
    )
}

fn read_request(reader: &mut impl BufRead) -> Option<serde_json::Value> {
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).ok()?;
        if read == 0 {
            return None;
        }
        if line.trim().is_empty() {
            continue;
        }
        return serde_json::from_str(&line).ok();
    }
}

fn main() {
    let mode = std::env::var("FIXTURE_MODE").unwrap_or_else(|_| "exact14".to_string());
    if mode == "silent" {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
    let call_ordinal = std::sync::atomic::AtomicU32::new(0);
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());

    while let Some(request) = read_request(&mut reader) {
        let method = request["method"].as_str().unwrap_or_default().to_string();
        let id = request["id"].clone();
        if mode == "no-delimiter" && method == "tools/list" {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            let _ = lock.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":");
            let _ = lock.flush();
            return;
        }
        let mut sent = true;
        match (method.as_str(), id.as_u64()) {
            ("initialize", Some(_)) => {
                sent = emit(&initialize_result(&id));
            }
            ("notifications/initialized", None) => {}
            ("tools/list", Some(_)) => {
                sent = if mode == "wide16" {
                    emit(&tools_result_wide(&id))
                } else {
                    emit(&tools_result(&id))
                };
            }
            ("ping", Some(_)) => {
                sent = emit(&result(id.clone(), serde_json::json!({})));
            }
            ("tools/call", Some(id_number)) => match mode.as_str() {
                "exact14" => sent = emit(&call_result(&id, "exact14")),
                "wrong-id" => {
                    sent = emit(&call_result(&serde_json::json!(id_number + 100), "wrong"));
                }
                "dup-id" => {
                    sent = emit(&call_result(&id, "dup"));
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    let _ = emit(&call_result(&id, "dup"));
                }
                "unknown-id" => sent = emit(&call_result(&serde_json::json!(9_999), "unknown")),
                "interleave" => {
                    let _ = emit(&serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/progress",
                        "params": { "progressToken": 1, "progress": 1 }
                    }));
                    sent = emit(&call_result(&id, "interleave"));
                }
                "never" => {
                    let mut sink = std::io::sink();
                    let _ = sink.write_all(&[]);
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(3600));
                    }
                }
                "late" => {
                    // Overlap-window choreography for A06: the first call's
                    // response is delayed to 400 ms (STALE tag), the second
                    // to 150 ms after its receipt (ok:fresh), so the stale
                    // arrival falls inside the fresh call's pending window.
                    let ordinal = call_ordinal.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let (delay, tag) = if ordinal == 0 {
                        (400, "stale")
                    } else {
                        (150, "fresh")
                    };
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                        let _ = emit(&call_result(&id, tag));
                    });
                }
                "oversized" => {
                    let pad = "x".repeat(64);
                    let line = format!(
                        "{}{}\n",
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "content": [ { "type": "text", "text": pad.repeat(40_000) } ] }
                        }),
                        ""
                    );
                    let stdout = std::io::stdout();
                    let mut lock = stdout.lock();
                    lock.write_all(line.as_bytes()).unwrap();
                    lock.write_all(format!("{}\n", serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "content": [ { "type": "text", "text": "ok:oversized" } ] }
                    })).as_bytes()).unwrap();
                    lock.flush().unwrap();
                }
                "no-delimiter" => {
                    let stdout = std::io::stdout();
                    let mut lock = stdout.lock();
                    lock.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}").unwrap();
                    lock.flush().unwrap();
                    let _ = std::io::stdout().flush();
                    break;
                }
                "secret-stderr" => { sent = true;
                    eprintln!("fixture stderr carries {SECRET}");
                    emit(&call_result(&id, "secret"));
                }
                "stubborn" => {
                    // Never reads stdin again, never exits: shutdown must kill.
                    let mut sink: Box<dyn Read> = Box::new(std::io::empty());
                    let _ = sink.read(&mut [0u8; 1]);
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(3600));
                    }
                }
                other => {
                    let _ = other;
                    emit(&serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": "unknown fixture mode" }
                    }));
                }
            },
            _ => {}
        }
        if !sent {
            return;
        }
    }
}
