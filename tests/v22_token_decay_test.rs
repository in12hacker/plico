//! Intent-cache behavior in stub mode.
//!
//! The former token-decay test derived a session token metric from a field that
//! was not part of the durable session contract. Session watermark invariants
//! now live with the session store tests.

use plico::api::semantic::ApiRequest;
use plico::kernel::AIKernel;

fn make_kernel() -> (std::sync::Arc<AIKernel>, tempfile::TempDir) {
    std::env::set_var("EMBEDDING_BACKEND", "stub");
    std::env::set_var("LLM_BACKEND", "stub");
    let dir = tempfile::tempdir().unwrap();
    let kernel = AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
    (kernel, dir)
}

fn intent_cache_stats(kernel: &AIKernel) -> (usize, u64) {
    let response = kernel.handle_api_request(ApiRequest::IntentCacheStats);
    response
        .intent_cache_stats
        .map(|stats| (stats.entries, stats.hits))
        .unwrap_or_default()
}

#[test]
fn intent_cache_remains_empty_with_stub_embedding() {
    let (kernel, _dir) = make_kernel();
    let registered = kernel.handle_api_request(ApiRequest::RegisterAgent {
        name: "cache-test-agent".to_string(),
    });
    assert!(registered.ok);
    let agent_id = registered.agent_id.unwrap();

    for intent in ["fix auth bug", "fix auth bug", "improve performance"] {
        let response = kernel.handle_api_request(ApiRequest::DeclareIntent {
            agent_id: agent_id.clone(),
            intent: intent.to_string(),
            related_cids: vec![],
            budget_tokens: 4096,
        });
        assert!(response.ok, "DeclareIntent failed: {:?}", response.error);
    }

    let (entries, hits) = intent_cache_stats(&kernel);
    assert_eq!(entries, 0, "stub embedding must not populate the intent cache");
    assert_eq!(hits, 0, "an empty intent cache cannot report hits");
}
