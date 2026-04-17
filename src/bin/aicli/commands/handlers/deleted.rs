//! Deleted / recycle bin commands.

use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use super::extract_arg;

pub fn cmd_deleted(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let entries = kernel.list_deleted(&agent_id);
    if entries.is_empty() {
        println!("Recycle bin is empty.");
    } else {
        println!("Recycle bin ({} items):", entries.len());
        for entry in &entries {
            println!("  CID: {}", entry.cid);
            println!("    Tags: {:?}", entry.original_meta.tags);
            println!("    Deleted at: {}", entry.deleted_at);
        }
    }
    ApiResponse::ok()
}

pub fn cmd_restore(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.restore_deleted(&cid, &agent_id) {
        Ok(()) => {
            println!("Restored: {}", cid);
            ApiResponse::ok()
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}
