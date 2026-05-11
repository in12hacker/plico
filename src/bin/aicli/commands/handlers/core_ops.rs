//! aicli handlers for Plico Core Verbs.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse};
use crate::commands::extract_arg;
use crate::commands::extract_tags;

pub fn cmd_core_get(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let id = args.get(1).cloned().unwrap_or_default();
    let variant = extract_arg(args, "--variant");
    kernel.handle_api_request(ApiRequest::CoreGet {
        id,
        variant,
        agent_id: extract_agent_id(args),
    })
}

pub fn cmd_core_list(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let variant = extract_arg(args, "--variant");
    let limit = extract_arg(args, "--limit").and_then(|s| s.parse().ok());
    let offset = extract_arg(args, "--offset").and_then(|s| s.parse().ok());
    let filter = extract_arg(args, "--filter").and_then(|s| serde_json::from_str(&s).ok());
    kernel.handle_api_request(ApiRequest::CoreList {
        variant,
        filter,
        limit,
        offset,
        agent_id: extract_agent_id(args),
    })
}

pub fn cmd_core_search(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let query = extract_arg(args, "--query")
        .or_else(|| args.get(1).cloned().filter(|a| !a.starts_with("--")))
        .unwrap_or_default();
    let variant = extract_arg(args, "--variant");
    let limit = extract_arg(args, "--limit").and_then(|s| s.parse().ok());
    let filter = extract_arg(args, "--filter").and_then(|s| serde_json::from_str(&s).ok());
    kernel.handle_api_request(ApiRequest::CoreSearch {
        query,
        variant,
        filter,
        limit,
        agent_id: extract_agent_id(args),
    })
}

pub fn cmd_core_store(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let data_str = extract_arg(args, "--data")
        .or_else(|| args.get(1).cloned().filter(|a| !a.starts_with("--")))
        .unwrap_or_default();
    let data = serde_json::from_str(&data_str).unwrap_or(serde_json::Value::String(data_str));
    let variant = extract_arg(args, "--variant");
    let tags = extract_tags(args, "--tags");
    kernel.handle_api_request(ApiRequest::CoreCreate {
        variant,
        data,
        tags,
        agent_id: extract_agent_id(args),
    })
}

pub fn cmd_core_patch(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let id = extract_arg(args, "--id")
        .or_else(|| args.get(1).cloned().filter(|a| !a.starts_with("--")))
        .unwrap_or_default();
    let data_str = extract_arg(args, "--data").unwrap_or_default();
    let data = serde_json::from_str(&data_str).unwrap_or(serde_json::Value::String(data_str));
    let variant = extract_arg(args, "--variant");
    kernel.handle_api_request(ApiRequest::CoreUpdate {
        id,
        variant,
        data,
        agent_id: extract_agent_id(args),
    })
}

pub fn cmd_core_remove(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let id = args.get(1).cloned().unwrap_or_default();
    let variant = extract_arg(args, "--variant");
    kernel.handle_api_request(ApiRequest::CoreDelete {
        id,
        variant,
        agent_id: extract_agent_id(args),
    })
}

pub fn cmd_core_invoke(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let action = args.get(1).cloned().unwrap_or_default();
    let params_str = extract_arg(args, "--params").unwrap_or_else(|| "{}".to_string());
    let params = serde_json::from_str(&params_str).unwrap_or(serde_json::Value::Object(Default::default()));
    kernel.handle_api_request(ApiRequest::CoreExec {
        action,
        params,
        agent_id: extract_agent_id(args),
    })
}

pub fn cmd_core_inspect(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let variant = extract_arg(args, "--variant");
    kernel.handle_api_request(ApiRequest::CoreObserve {
        variant,
        agent_id: extract_agent_id(args),
    })
}

pub fn cmd_core_link(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let src = extract_arg(args, "--src").unwrap_or_default();
    let dst = extract_arg(args, "--dst").unwrap_or_default();
    let relation = extract_arg(args, "--relation");
    let weight = extract_arg(args, "--weight").and_then(|s| s.parse().ok());
    kernel.handle_api_request(ApiRequest::CoreLink {
        src,
        dst,
        relation,
        weight,
        agent_id: extract_agent_id(args),
    })
}

pub fn cmd_core_ask(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let query = extract_arg(args, "--query")
        .or_else(|| args.get(1).cloned().filter(|a| !a.starts_with("--")))
        .unwrap_or_default();
    let context_ids = extract_tags(args, "--context");
    kernel.handle_api_request(ApiRequest::CoreAsk {
        query,
        context_ids,
        agent_id: extract_agent_id(args),
    })
}

pub fn cmd_core_state(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let action = extract_arg(args, "--action");
    kernel.handle_api_request(ApiRequest::CoreState {
        action,
        agent_id: extract_agent_id(args),
    })
}

fn extract_agent_id(args: &[String]) -> String {
    extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string())
}
