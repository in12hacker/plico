//! Hybrid retrieval — Graph-RAG combining vector search and KG traversal.

use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use super::extract_arg;

/// Perform hybrid retrieval combining vector search and knowledge graph traversal.
pub fn cmd_hybrid(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let query_text = extract_arg(args, "--query")
        .or_else(|| args.get(1).cloned())
        .unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let seed_tags: Vec<String> = extract_arg(args, "--seed-tags")
        .map(|s| s.split(',').map(String::from).collect())
        .unwrap_or_default();
    let graph_depth = extract_arg(args, "--depth")
        .and_then(|s| s.parse().ok())
        .unwrap_or(2);
    let edge_types: Vec<String> = extract_arg(args, "--edge-types")
        .map(|s| s.split(',').map(String::from).collect())
        .unwrap_or_default();
    let max_results = extract_arg(args, "--limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let token_budget = extract_arg(args, "--budget").and_then(|s| s.parse().ok());

    if query_text.is_empty() {
        return ApiResponse::error("hybrid requires a query. Use: hybrid <text> or: hybrid --query <text>");
    }

    let req = plico::api::semantic::ApiRequest::HybridRetrieve {
        query_text,
        seed_tags,
        graph_depth,
        edge_types,
        max_results,
        token_budget,
        agent_id,
        tenant_id: None,
    };
    kernel.handle_api_request(req)
}
