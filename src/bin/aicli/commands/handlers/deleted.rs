//! Deleted / recycle bin commands.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiResponse, DeletedDto};
use super::extract_arg;

pub fn cmd_deleted(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let entries = kernel.list_deleted(&agent_id);
    let dto: Vec<DeletedDto> = entries.into_iter().map(|e| DeletedDto {
        cid: e.cid, tags: e.original_meta.tags, deleted_at: e.deleted_at,
    }).collect();
    let mut r = ApiResponse::ok();
    r.deleted = Some(dto);
    r
}

pub fn cmd_restore(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.restore_deleted(&cid, &agent_id) {
        Ok(()) => ApiResponse::ok_with_message(format!("Restored: {} from recycle bin", cid)),
        Err(e) => ApiResponse::error_with_diagnosis(
            e.to_string(),
            "RESTORE_FAILED",
            "Check CID and try again",
            vec!["plico(action=\"deleted\")".to_string()],
        ),
    }
}
