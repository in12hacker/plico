//! Context loading commands (L0/L1/L2) and context budget assembly.
//! Routes through handle_api_request.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse, ContextAssembleCandidate};
use super::extract_arg;

pub fn cmd_context(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    match args.get(1).map(|s| s.as_str()) {
        Some("assemble") => cmd_context_assemble(kernel, args),
        _ => cmd_context_load(kernel, args),
    }
}

fn cmd_context_load(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_default();
    let intent = extract_arg(args, "--intent");
    let layer = extract_arg(args, "--layer").unwrap_or_else(|| "L2".to_string());
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let budget = extract_arg(args, "--budget")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(4000);

    if let Some(intent_text) = intent {
        let search_resp = kernel.handle_api_request(ApiRequest::Search {
            query: intent_text.clone(),
            agent_id: agent_id.clone(),
            tenant_id: None,
            agent_token: None,
            limit: Some(5),
            offset: None,
            require_tags: vec![],
            exclude_tags: vec![],
            since: None,
            until: None,
        });
        let results = search_resp.results.unwrap_or_default();
        if results.is_empty() {
            return ApiResponse::error(format!("No content found for intent: {}", intent_text));
        }
        let cids: Vec<ContextAssembleCandidate> = results.iter().enumerate()
            .map(|(i, sr)| ContextAssembleCandidate {
                cid: sr.cid.clone(),
                relevance: if sr.relevance > 0.0 { sr.relevance } else { 1.0 - i as f32 * 0.1 },
            })
            .collect();
        return kernel.handle_api_request(ApiRequest::ContextAssemble {
            agent_id, cids, budget_tokens: budget,
        });
    }

    if cid.is_empty() {
        return ApiResponse::error("Missing --cid or --intent argument");
    }

    kernel.handle_api_request(ApiRequest::LoadContext {
        cid, layer, agent_id, tenant_id: None,
    })
}

fn cmd_context_assemble(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let budget = extract_arg(args, "--budget")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(4000);
    let cids_str = extract_arg(args, "--cids").unwrap_or_default();

    if cids_str.is_empty() {
        return ApiResponse::error("Missing --cids argument (comma-separated CIDs)");
    }

    let cids: Vec<ContextAssembleCandidate> = cids_str
        .split(',')
        .enumerate()
        .map(|(i, cid)| ContextAssembleCandidate {
            cid: cid.trim().to_string(),
            relevance: 1.0 - i as f32 * 0.05,
        })
        .collect();

    kernel.handle_api_request(ApiRequest::ContextAssemble {
        agent_id, cids, budget_tokens: budget,
    })
}
