//! Deleted / recycle bin commands — route through handle_api_request.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse};
use super::extract_arg;

pub fn cmd_deleted(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    kernel.handle_api_request(ApiRequest::ListDeleted { agent_id })
}

pub fn cmd_restore(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    kernel.handle_api_request(ApiRequest::Restore { cid, agent_id })
}
