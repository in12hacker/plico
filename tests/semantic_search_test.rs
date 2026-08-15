//! Optional real-provider reachability for the typed object-search CLI path.

use std::process::{Command, Output};

use tempfile::tempdir;

const OLLAMA_URL: &str = "http://localhost:11434";

fn ollama_available() -> bool {
    Command::new("curl")
        .args(["-sf", &format!("{OLLAMA_URL}/api/tags")])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn run(root: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aicli"))
        .args(["--embedded", "--root"])
        .arg(root)
        .args(args)
        .env("EMBEDDING_BACKEND", "ollama")
        .env("OLLAMA_URL", OLLAMA_URL)
        .env("OLLAMA_EMBEDDING_MODEL", "nomic-embed-text")
        .env("LLM_BACKEND", "stub")
        .env("RUST_LOG", "off")
        .output()
        .expect("run typed aicli")
}

fn response(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "aicli failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn real_embedding_path_reports_execution_and_ranks_relevant_content() {
    if !ollama_available() {
        eprintln!("SKIP: Ollama is unavailable at {OLLAMA_URL}");
        return;
    }
    let root = tempdir().unwrap();
    let rust = response(run(
        root.path(),
        &[
            "object.put",
            "--content",
            "Rust compiler optimization with LLVM inlining and register allocation",
            "--tag",
            "compiler",
        ],
    ));
    let rust_cid = rust["data"]["result"]["cid"].as_str().unwrap();
    let _python = response(run(
        root.path(),
        &[
            "object.put",
            "--content",
            "Python pandas dataframe visualization and statistics",
            "--tag",
            "data-science",
        ],
    ));

    let searched = response(run(
        root.path(),
        &["object.search", "--query", "compiler code optimization", "--limit", "2"],
    ));
    let result = &searched["data"]["result"];
    assert_eq!(result["hits"][0]["cid"], rust_cid);
    assert_eq!(result["embedding_query"]["state"], "succeeded");
    assert!(result["retrieval"]
        .as_array()
        .unwrap()
        .iter()
        .any(|stage| stage["path"] == "vector"));
}
