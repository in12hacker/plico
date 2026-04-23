//! Response formatting and shaping for MCP output.

use plico::api::semantic::ApiResponse;
use serde_json::Value;

pub fn format_response(resp: ApiResponse) -> Result<String, String> {
    if resp.ok {
        Ok(serde_json::to_string_pretty(&resp).unwrap_or_default())
    } else {
        Err(resp.error.unwrap_or_else(|| "unknown error".to_string()))
    }
}

pub fn format_plico_response(resp: ApiResponse, args: &Value) -> Result<String, String> {
    if !resp.ok {
        return Err(resp.error.unwrap_or_else(|| "unknown error".to_string()));
    }

    let mut response_json: Value = serde_json::to_value(&resp).map_err(|e| e.to_string())?;

    let action = args.get("action").and_then(|a| a.as_str()).unwrap_or("");
    if action == "search" || action == "hybrid" || action == "recall" || action == "recall_semantic" {
        apply_response_shaping(&mut response_json, args);
    }

    Ok(serde_json::to_string_pretty(&response_json).unwrap_or_default())
}

pub fn apply_response_shaping(response_json: &mut Value, args: &Value) {
    if let Some(preview) = args.get("preview").and_then(|p| p.as_u64()) {
        if preview > 0 {
            truncate_by_preview(response_json, preview as usize);
        }
    }

    if let Some(select) = args.get("select").and_then(|s| s.as_array()) {
        let fields: Vec<&str> = select.iter().filter_map(|v| v.as_str()).collect();
        if !fields.is_empty() {
            if let Some(results) = response_json.get_mut("results").and_then(|r| r.as_array_mut()) {
                for item in results.iter_mut() {
                    if let Some(obj) = item.as_object_mut() {
                        obj.retain(|key, _| fields.iter().any(|f| *f == key));
                    }
                }
            }
        }
    }
}

fn truncate_by_preview(value: &mut Value, preview: usize) {
    match value {
        Value::String(s) if s.len() > preview => {
            *s = format!("{}...", &s[..preview]);
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                truncate_by_preview(item, preview);
            }
        }
        Value::Object(obj) => {
            for (_, v) in obj.iter_mut() {
                truncate_by_preview(v, preview);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn format_response_ok() {
        let resp = ApiResponse::ok();
        let result = format_response(resp);
        assert!(result.is_ok());
        let json: Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["ok"], true);
    }

    #[test]
    fn format_response_error() {
        let resp = ApiResponse::error("bad request");
        let result = format_response(resp);
        assert_eq!(result.unwrap_err(), "bad request");
    }

    #[test]
    fn truncate_by_preview_shortens_strings() {
        let mut val = json!({"name": "hello world"});
        truncate_by_preview(&mut val, 5);
        assert_eq!(val["name"], "hello...");
    }

    #[test]
    fn truncate_by_preview_leaves_short_strings() {
        let mut val = json!({"name": "hi"});
        truncate_by_preview(&mut val, 10);
        assert_eq!(val["name"], "hi");
    }

    #[test]
    fn truncate_by_preview_works_on_nested_arrays() {
        let mut val = json!(["long string here"]);
        truncate_by_preview(&mut val, 4);
        assert_eq!(val[0], "long...");
    }

    #[test]
    fn apply_response_shaping_preview() {
        let mut resp = json!({
            "results": [{"snippet": "very long content here"}]
        });
        let args = json!({"action": "search", "preview": 4});
        apply_response_shaping(&mut resp, &args);
        assert_eq!(resp["results"][0]["snippet"], "very...");
    }

    #[test]
    fn apply_response_shaping_select_fields() {
        let mut resp = json!({
            "results": [{"cid": "abc", "snippet": "text", "score": 0.9}]
        });
        let args = json!({"action": "search", "select": ["cid", "score"]});
        apply_response_shaping(&mut resp, &args);
        let item = &resp["results"][0];
        assert_eq!(item["cid"], "abc");
        assert!(item.get("snippet").is_none());
    }

    #[test]
    fn apply_response_shaping_no_preview_no_select() {
        let mut resp = json!({"results": [{"cid": "abc"}]});
        let args = json!({"action": "search"});
        apply_response_shaping(&mut resp, &args);
        assert_eq!(resp["results"][0]["cid"], "abc");
    }
}
