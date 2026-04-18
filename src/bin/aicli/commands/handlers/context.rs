//! Context loading commands (L0/L1/L2).

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiResponse, LoadedContextDto};
use plico::fs::ContextLayer;
use super::extract_arg;

pub fn cmd_context(kernel: &AIKernel, args: &[String]) -> ApiResponse {
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
                cid: loaded.cid,
                layer: loaded.layer.name().to_string(),
                content: loaded.content,
                tokens_estimate: loaded.tokens_estimate,
            });
            r
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}
