//! Context loading commands (L0/L1/L2) and context budget assembly.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiResponse, LoadedContextDto};
use plico::fs::ContextLayer;
use super::extract_arg;

pub fn cmd_context(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    match args.get(1).map(|s| s.as_str()) {
        Some("assemble") => cmd_context_assemble(kernel, args),
        _ => cmd_context_load(kernel, args),
    }
}

fn cmd_context_load(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_default();
    let layer_str = extract_arg(args, "--layer").unwrap_or_else(|| "L2".to_string());
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    if cid.is_empty() {
        return ApiResponse::error("Missing --cid argument");
    }

    let layer = match ContextLayer::parse_layer(&layer_str) {
        Some(l) => l,
        None => return ApiResponse::error(format!("Invalid layer '{}'. Use L0, L1, or L2.", layer_str)),
    };

    match kernel.context_load(&cid, layer, &agent_id) {
        Ok(loaded) => {
            let mut r = ApiResponse::ok();
            r.context_data = Some(LoadedContextDto {
                cid: loaded.cid.clone(),
                layer: loaded.layer.name().to_string(),
                content: loaded.content,
                tokens_estimate: loaded.tokens_estimate,
                actual_layer: loaded.actual_layer.map(|l| l.name().to_string()),
                degraded: loaded.degraded,
                degradation_reason: loaded.degradation_reason,
            });
            r
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
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

    let candidates: Vec<plico::fs::context_budget::ContextCandidate> = cids_str
        .split(',')
        .enumerate()
        .map(|(i, cid)| plico::fs::context_budget::ContextCandidate {
            cid: cid.trim().to_string(),
            relevance: 1.0 - i as f32 * 0.05,
        })
        .collect();

    match kernel.context_assemble(&candidates, budget, &agent_id) {
        Ok(allocation) => {
            let mut r = ApiResponse::ok();
            r.context_assembly = Some(allocation);
            r
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}
