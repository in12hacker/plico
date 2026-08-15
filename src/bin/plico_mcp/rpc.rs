//! MCP JSON-RPC decoding and typed public-command dispatch.

use plico::api::public::{PublicCommand, PublicRequest};
use plico::client::KernelClient;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{tools, MCP_PROTOCOL_VERSION, SERVER_NAME, SERVER_VERSION};

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolCallParams {
    name: String,
    #[serde(default = "empty_arguments")]
    arguments: Value,
}

pub fn process_line(line: &str, client: &dyn KernelClient) -> Option<Value> {
    let message = match serde_json::from_str::<Value>(line) {
        Ok(message) => message,
        Err(_) => return Some(rpc_error(Value::Null, PARSE_ERROR, "Parse error")),
    };
    dispatch_message(&message, client)
}

fn dispatch_message(message: &Value, client: &dyn KernelClient) -> Option<Value> {
    let Some(object) = message.as_object() else {
        return Some(rpc_error(Value::Null, INVALID_REQUEST, "Invalid Request"));
    };
    let id = object.get("id").cloned();
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || object.get("method").and_then(Value::as_str).is_none()
        || id.as_ref().is_some_and(invalid_rpc_id)
    {
        return Some(rpc_error(id.unwrap_or(Value::Null), INVALID_REQUEST, "Invalid Request"));
    }
    let id = id?;
    let method = object["method"].as_str().expect("method was validated above");
    let params = object.get("params").cloned().unwrap_or_else(|| json!({}));

    Some(match method {
        "initialize" => initialize_response(id),
        "tools/list" => rpc_result(id, json!({ "tools": tools::tool_definitions() })),
        "tools/call" => handle_tool_call(id, params, client),
        "ping" => rpc_result(id, json!({})),
        _ => rpc_error(id, METHOD_NOT_FOUND, "Method not found"),
    })
}

fn invalid_rpc_id(id: &Value) -> bool {
    !matches!(id, Value::Null | Value::String(_) | Value::Number(_))
}

fn initialize_response(id: Value) -> Value {
    rpc_result(
        id,
        json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION,
            },
        }),
    )
}

fn handle_tool_call(id: Value, params: Value, client: &dyn KernelClient) -> Value {
    let params = match serde_json::from_value::<ToolCallParams>(params) {
        Ok(params) if params.arguments.is_object() => params,
        _ => return rpc_error(id, INVALID_PARAMS, "Invalid tool call parameters"),
    };
    let argument_count = params.arguments.as_object().map_or(0, serde_json::Map::len);
    let command = match tools::command_from_tool(&params.name, params.arguments) {
        Ok(command) => command,
        Err(tools::ToolInputError::UnknownTool) => return rpc_error(id, INVALID_PARAMS, "Unknown tool"),
        Err(tools::ToolInputError::InvalidArguments) => return rpc_error(id, INVALID_PARAMS, "Invalid tool arguments"),
    };

    record_tool_call(&command, argument_count);
    let request = PublicRequest::new(uuid::Uuid::new_v4(), None, command);
    let response = match client.request(request) {
        Ok(response) => response,
        Err(_) => return rpc_error(id, INTERNAL_ERROR, "Plico transport request failed"),
    };
    let is_domain_error = !response.ok;
    let text = match serde_json::to_string(&response) {
        Ok(text) => text,
        Err(_) => return rpc_error(id, INTERNAL_ERROR, "Plico response encoding failed"),
    };
    let mut result = json!({
        "content": [{ "type": "text", "text": text }]
    });
    if is_domain_error {
        result["isError"] = Value::Bool(true);
    }
    rpc_result(id, result)
}

fn safe_tool_call_metadata(command: &PublicCommand, argument_count: usize) -> (&'static str, usize) {
    (command.operation(), argument_count)
}

fn record_tool_call(command: &PublicCommand, argument_count: usize) {
    let (operation, argument_count) = safe_tool_call_metadata(command, argument_count);
    tracing::debug!(operation, argument_count, "MCP tool call received");
}

fn empty_arguments() -> Value {
    json!({})
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i64, message: &'static str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use plico::api::public::{PublicError, PublicErrorCode, PublicResponse};

    use super::*;

    struct DomainFailureClient {
        calls: AtomicUsize,
    }

    impl DomainFailureClient {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl KernelClient for DomainFailureClient {
        fn request(&self, request: PublicRequest) -> Result<PublicResponse, plico::client::ClientError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(PublicResponse::failure(
                request.request_id,
                PublicError {
                    code: PublicErrorCode::NotFound,
                    message: "not found".to_string(),
                    retryable: false,
                    details: None,
                },
            ))
        }
    }

    #[test]
    fn initialize_advertises_only_tools() {
        let response = initialize_response(json!(1));
        assert_eq!(response["result"]["capabilities"], json!({ "tools": {} }));
        assert!(response["result"]["capabilities"].get("resources").is_none());
        assert!(response["result"]["capabilities"].get("prompts").is_none());
    }

    #[test]
    fn unknown_tool_is_invalid_params_without_client_invocation() {
        let client = DomainFailureClient::new();
        let response = dispatch_message(
            &json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": { "name": "plico", "arguments": {} },
            }),
            &client,
        )
        .unwrap();

        assert_eq!(response["error"]["code"], INVALID_PARAMS);
        assert_eq!(client.calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn domain_failure_is_a_typed_tool_error() {
        let client = DomainFailureClient::new();
        let response = dispatch_message(
            &json!({
                "jsonrpc": "2.0",
                "id": 8,
                "method": "tools/call",
                "params": {
                    "name": "object.get",
                    "arguments": { "cid": "a".repeat(64) },
                },
            }),
            &client,
        )
        .unwrap();

        assert_eq!(response["result"]["isError"], true);
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        let public_response: PublicResponse = serde_json::from_str(text).unwrap();
        assert!(!public_response.ok);
        assert_eq!(public_response.error.unwrap().code, PublicErrorCode::NotFound);
        assert_eq!(client.calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn invalid_json_and_methods_have_distinct_protocol_errors() {
        let client = DomainFailureClient::new();
        assert_eq!(process_line("{", &client).unwrap()["error"]["code"], PARSE_ERROR);
        assert_eq!(
            dispatch_message(&json!({ "jsonrpc": "2.0", "id": 1 }), &client).unwrap()["error"]["code"],
            INVALID_REQUEST
        );
        assert_eq!(
            dispatch_message(
                &json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/list" }),
                &client,
            )
            .unwrap()["error"]["code"],
            METHOD_NOT_FOUND
        );
    }

    #[test]
    fn tool_log_metadata_never_contains_argument_values() {
        let command = tools::command_from_tool("object.search", json!({ "query": "private medical note" })).unwrap();
        let metadata = format!("{:?}", safe_tool_call_metadata(&command, 2));

        assert_eq!(metadata, "(\"object.search\", 2)");
        assert!(!metadata.contains("private medical note"));
        assert!(!metadata.contains("secret bearer"));
    }
}
