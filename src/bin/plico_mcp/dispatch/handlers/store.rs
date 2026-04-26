//! plico_store action handler — CAS put/read operations.

use plico::api::semantic::ApiRequest;
use plico::kernel::AIKernel;
use serde_json::Value;

use crate::dispatch::{DEFAULT_AGENT, STORE_WRITE_ACTIONS, is_read_only_mode};
use crate::format::format_response;

pub(in crate::dispatch) fn dispatch_plico_store(args: &Value, kernel: &AIKernel) -> Result<String, String> {
    let agent = args.get("agent_id").and_then(|a| a.as_str()).unwrap_or(DEFAULT_AGENT);
    let store_action = args.get("action")
        .and_then(|a| a.as_str())
        .ok_or("plico_store requires action")?;

    if is_read_only_mode() && STORE_WRITE_ACTIONS.contains(&store_action) {
        return Err(format!("read_only: action '{}' is a write operation. Set PLICO_READ_ONLY=false to allow writes.", store_action));
    }

    match store_action {
        "put" => {
            let content = args.get("content")
                .and_then(|c| c.as_str())
                .ok_or("put requires content")?;
            let tags: Vec<String> = args.get("tags")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let req = ApiRequest::Create {
                api_version: None,
                content: content.to_string(),
                content_encoding: Default::default(),
                tags,
                agent_id: agent.to_string(),
                tenant_id: None,
                agent_token: None,
                intent: None,
            };
            format_response(kernel.handle_api_request(req))
        }

        "read" => {
            let cid = args.get("cid")
                .and_then(|c| c.as_str())
                .ok_or("read requires cid")?;
            let req = ApiRequest::Read {
                cid: cid.to_string(),
                agent_id: agent.to_string(),
                tenant_id: None,
                agent_token: None,
            };
            format_response(kernel.handle_api_request(req))
        }

        _ => Err(format!("unknown store action: {}", store_action)),
    }
}

pub(in crate::dispatch) fn dispatch_plico_store_remote(args: &Value, client: &dyn plico::client::KernelClient) -> Result<String, String> {
    let agent = args.get("agent_id").and_then(|a| a.as_str()).unwrap_or(DEFAULT_AGENT);
    let store_action = args.get("action").and_then(|a| a.as_str()).ok_or("plico_store requires action")?;

    let req = match store_action {
        "put" => {
            let content = args.get("content").and_then(|c| c.as_str()).ok_or("put requires content")?;
            let tags: Vec<String> = args.get("tags")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            ApiRequest::Create { api_version: None, content: content.to_string(), content_encoding: Default::default(), tags, agent_id: agent.to_string(), tenant_id: None, agent_token: None, intent: None }
        }
        "read" => {
            let cid = args.get("cid").and_then(|c| c.as_str()).ok_or("read requires cid")?;
            ApiRequest::Read { cid: cid.to_string(), agent_id: agent.to_string(), tenant_id: None, agent_token: None }
        }
        _ => return Err(format!("unknown store action: {}", store_action)),
    };

    let resp = client.request(req);
    format_response(resp)
}
