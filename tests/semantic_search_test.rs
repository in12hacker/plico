//! Semantic Search Reachability Test
//!
//! Validates end-to-end semantic search: store content → embed via Ollama →
//! search by natural language → verify semantic ranking.
//!
//! **This test requires Ollama running at localhost:11434 with nomic-embed-text.**
//! Skipped automatically if Ollama is unavailable.
//!
//! Run:
//!   cargo test --test semantic_search_test -- --nocapture
//!
//! Setup:
//!   ollama serve &
//!   ollama pull nomic-embed-text

use std::process::{Command, Output};
use tempfile::tempdir;

const OLLAMA_URL: &str = "http://localhost:11434";
const EMBED_MODEL: &str = "nomic-embed-text";

/// Check that Ollama is running and nomic-embed-text is available.
/// Returns false if the test should be skipped.
fn ollama_available() -> bool {
    let resp = std::process::Command::new("curl")
        .args(["-sf", &format!("{}/api/tags", OLLAMA_URL)])
        .output();

    match resp {
        Ok(o) if o.status.success() => {
            let body = String::from_utf8_lossy(&o.stdout);
            body.contains("nomic-embed")
        }
        _ => false,
    }
}

fn aicli() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    format!("{}/target/debug/aicli", manifest_dir)
}

fn run_with_ollama(root: &std::path::Path, args: &[&str]) -> Output {
    Command::new(aicli())
        .arg("--root")
        .arg(root)
        .env("EMBEDDING_BACKEND", "ollama")
        .env("OLLAMA_URL", OLLAMA_URL)
        .env("OLLAMA_EMBEDDING_MODEL", EMBED_MODEL)
        .args(args)
        .output()
        .expect("failed to spawn aicli")
}

fn extract_cid(output: &Output) -> Option<String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find(|l| l.starts_with("CID:"))
        .map(|l| l.trim_start_matches("CID:").trim().to_string())
}

/// Parse search result CIDs in rank order.
///
/// aicli search output format:
///   "1. [relevance=0.84] <64-char CID>"
///   "   Tags: [...]"
fn extract_ranked_cids(output: &Output) -> Vec<String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut cids = Vec::new();
    for line in stdout.lines() {
        // Match lines like "1. [relevance=0.84] <cid>"
        if let Some(rest) = line.split("] ").last() {
            let candidate = rest.trim();
            // A CID is 64 hex chars
            if candidate.len() == 64 && candidate.chars().all(|c| c.is_ascii_hexdigit()) {
                cids.push(candidate.to_string());
            }
        }
    }
    cids
}

// ─── Tests ──────────────────────────────────────────────────────────────────

/// Basic smoke: Ollama embed returns 768-dim vector for nomic-embed-text.
#[test]
fn test_ollama_embed_endpoint() {
    if !ollama_available() {
        eprintln!("SKIP: Ollama not available at {}", OLLAMA_URL);
        return;
    }

    let body = serde_json::json!({
        "model": EMBED_MODEL,
        "prompt": "hello world"
    });

    let resp = std::process::Command::new("curl")
        .args([
            "-sf",
            "-X", "POST",
            &format!("{}/api/embeddings", OLLAMA_URL),
            "-H", "Content-Type: application/json",
            "-d", &body.to_string(),
        ])
        .output()
        .expect("curl failed");

    assert!(resp.status.success(), "Ollama /api/embeddings failed");

    let json: serde_json::Value =
        serde_json::from_slice(&resp.stdout).expect("parse embedding response");
    let embedding = json["embedding"].as_array().expect("embedding field missing");

    eprintln!("Dimension: {}", embedding.len());
    assert_eq!(
        embedding.len(),
        768,
        "nomic-embed-text should produce 768-dim vectors"
    );
    // All values should be finite floats
    for v in embedding {
        let f = v.as_f64().expect("embedding value not numeric");
        assert!(f.is_finite(), "embedding contains non-finite value");
    }
}

/// Core semantic reachability: store 3 docs, search returns semantically
/// relevant doc first (not just exact-string match).
///
/// Documents:
///   A: "Rust system programming meeting: discussed LLVM IR optimization passes"
///   B: "Python data science notebook: pandas DataFrame analysis"
///   C: "Cranelift compiler backend for Rust: MIR to CLIF lowering"
///
/// Query: "Rust compiler optimization"
/// Expected ranking: A and C before B (Rust/compiler content > Python/pandas)
#[test]
fn test_semantic_ranking_rust_vs_python() {
    if !ollama_available() {
        eprintln!("SKIP: Ollama not available at {}", OLLAMA_URL);
        return;
    }

    let root = tempdir().unwrap();

    // Store document A — Rust + LLVM (should rank high for "Rust compiler optimization")
    let out_a = run_with_ollama(root.path(), &[
        "put",
        "--content",
        "Rust system programming meeting: discussed LLVM IR optimization passes, inlining, loop unrolling",
        "--tags", "rust,compiler,optimization",
    ]);
    assert!(out_a.status.success(), "put A failed: {}", String::from_utf8_lossy(&out_a.stderr));
    let cid_a = extract_cid(&out_a).expect("no CID from put A");
    eprintln!("CID A (Rust+LLVM): {}", &cid_a[..8]);

    // Store document B — Python + data science (should rank LOW for "Rust compiler optimization")
    let out_b = run_with_ollama(root.path(), &[
        "put",
        "--content",
        "Python data science notebook: pandas DataFrame merge, numpy array broadcasting, matplotlib visualization",
        "--tags", "python,datascience,pandas",
    ]);
    assert!(out_b.status.success(), "put B failed: {}", String::from_utf8_lossy(&out_b.stderr));
    let cid_b = extract_cid(&out_b).expect("no CID from put B");
    eprintln!("CID B (Python): {}", &cid_b[..8]);

    // Store document C — Cranelift/MIR (should rank high for "Rust compiler optimization")
    let out_c = run_with_ollama(root.path(), &[
        "put",
        "--content",
        "Cranelift compiler backend for Rust: MIR to CLIF lowering, register allocation, code generation",
        "--tags", "rust,cranelift,compiler",
    ]);
    assert!(out_c.status.success(), "put C failed: {}", String::from_utf8_lossy(&out_c.stderr));
    let cid_c = extract_cid(&out_c).expect("no CID from put C");
    eprintln!("CID C (Cranelift): {}", &cid_c[..8]);

    // Search for "Rust compiler optimization"
    let search = run_with_ollama(root.path(), &[
        "search",
        "--query", "Rust compiler optimization",
        "--limit", "3",
    ]);
    assert!(
        search.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );

    let ranked = extract_ranked_cids(&search);
    eprintln!("Ranked results: {:?}", ranked.iter().map(|c| &c[..8.min(c.len())]).collect::<Vec<_>>());

    // Must return at least 2 results
    assert!(
        ranked.len() >= 2,
        "expected ≥2 search results, got {}: {}",
        ranked.len(),
        String::from_utf8_lossy(&search.stdout)
    );

    // Python doc (B) should NOT be first — Rust docs should outrank it
    if !ranked.is_empty() {
        assert_ne!(
            ranked[0], cid_b,
            "Python pandas doc should not be #1 for 'Rust compiler optimization' query"
        );
    }

    // At least one of A or C should appear before B
    let pos_a = ranked.iter().position(|c| c == &cid_a);
    let pos_b = ranked.iter().position(|c| c == &cid_b);
    let pos_c = ranked.iter().position(|c| c == &cid_c);

    eprintln!(
        "Positions — A(Rust+LLVM): {:?}, B(Python): {:?}, C(Cranelift): {:?}",
        pos_a, pos_b, pos_c
    );

    let rust_before_python = match (pos_a.or(pos_c), pos_b) {
        (Some(rust_pos), Some(python_pos)) => rust_pos < python_pos,
        (Some(_), None) => true,  // Python didn't appear at all → Rust wins
        _ => false,
    };

    assert!(
        rust_before_python,
        "At least one Rust/compiler doc (A or C) should rank before Python/pandas doc (B). \
        Ranked order: {:?}",
        ranked.iter().map(|c| &c[..8.min(c.len())]).collect::<Vec<_>>()
    );
}

/// Verify search returns the exact document we stored via its CID.
#[test]
fn test_semantic_search_finds_stored_document() {
    if !ollama_available() {
        eprintln!("SKIP: Ollama not available at {}", OLLAMA_URL);
        return;
    }

    let root = tempdir().unwrap();

    let out = run_with_ollama(root.path(), &[
        "put",
        "--content",
        "Memory-safe systems programming with ownership and borrow checker guarantees",
        "--tags", "rust,memory-safety",
    ]);
    assert!(out.status.success());
    let expected_cid = extract_cid(&out).expect("no CID");

    let search = run_with_ollama(root.path(), &[
        "search",
        "--query", "Rust ownership borrowing memory safety",
    ]);
    assert!(search.status.success(), "search failed: {}", String::from_utf8_lossy(&search.stderr));

    let ranked = extract_ranked_cids(&search);
    eprintln!("Search results: {:?}", ranked.iter().map(|c| &c[..8.min(c.len())]).collect::<Vec<_>>());

    assert!(
        ranked.iter().any(|c| c == &expected_cid),
        "Stored document (CID: {}) should appear in semantic search results. Got: {:?}",
        &expected_cid[..8],
        ranked.iter().map(|c| &c[..8.min(c.len())]).collect::<Vec<_>>()
    );
}

/// Verify that semantically similar texts produce closer results than dissimilar ones.
/// Put two closely related docs and one unrelated; first result for a specific
/// query should be the most semantically related doc.
#[test]
fn test_semantic_similarity_ordering() {
    if !ollama_available() {
        eprintln!("SKIP: Ollama not available at {}", OLLAMA_URL);
        return;
    }

    let root = tempdir().unwrap();

    // Near: async Rust
    let out_near = run_with_ollama(root.path(), &[
        "put", "--content",
        "Tokio async runtime: async/await, Future trait, task spawning, executor",
        "--tags", "rust,async,tokio",
    ]);
    let cid_near = extract_cid(&out_near).expect("no CID near");

    // Far: cooking recipe
    let out_far = run_with_ollama(root.path(), &[
        "put", "--content",
        "Classic French onion soup: caramelized onions, beef broth, gruyere cheese, baguette croutons",
        "--tags", "cooking,soup,french",
    ]);
    let cid_far = extract_cid(&out_far).expect("no CID far");

    // Query is closely related to async Rust
    let search = run_with_ollama(root.path(), &[
        "search",
        "--query", "Rust futures and async task scheduling",
    ]);
    assert!(search.status.success());
    let ranked = extract_ranked_cids(&search);

    eprintln!(
        "Near(async-rust): {}, Far(cooking): {}",
        &cid_near[..8], &cid_far[..8]
    );
    eprintln!(
        "Result order: {:?}",
        ranked.iter().map(|c| &c[..8.min(c.len())]).collect::<Vec<_>>()
    );

    // Async Rust doc should rank before cooking doc
    let pos_near = ranked.iter().position(|c| c == &cid_near);
    let pos_far = ranked.iter().position(|c| c == &cid_far);

    match (pos_near, pos_far) {
        (Some(n), Some(f)) => assert!(
            n < f,
            "Async Rust doc (pos {}) should rank before cooking doc (pos {})",
            n, f
        ),
        (Some(_), None) => {} // cooking not in results → async Rust wins
        _ => panic!("Async Rust doc not found in search results at all"),
    }
}
