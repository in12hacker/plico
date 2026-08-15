//! Trace query handlers — TraceList, TraceShow, TraceFailures.

use crate::api::semantic::{ApiRequest, ApiResponse};
use crate::kernel::trace::TraceStore;

impl super::super::AIKernel {
    pub(crate) fn handle_trace(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::TraceList {
                agent_id,
                since,
                until,
                tool_name,
                status,
                limit,
            } => {
                let store = TraceStore::new(self.root.clone());
                let files = store.list_files(agent_id.as_deref(), since.as_deref(), until.as_deref());
                let mut all_spans = Vec::new();
                for path in files {
                    if let Ok(spans) = store.read_spans(&path) {
                        all_spans.extend(spans);
                    }
                }
                // Filter by tool_name and status
                if let Some(ref t) = tool_name {
                    all_spans.retain(|s| &s.tool_name == t);
                }
                if let Some(ref s) = status {
                    let want = match s.as_str() {
                        "success" => crate::kernel::trace::SpanStatus::Success,
                        "error" => crate::kernel::trace::SpanStatus::Error,
                        "timeout" => crate::kernel::trace::SpanStatus::Timeout,
                        _ => crate::kernel::trace::SpanStatus::Success,
                    };
                    all_spans.retain(|sp| sp.status == want);
                }
                // Sort by timestamp desc
                all_spans.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
                if let Some(l) = limit {
                    all_spans.truncate(l);
                }
                let mut r = ApiResponse::ok();
                r.trace_list = Some(serde_json::json!({
                    "spans": all_spans,
                    "total": all_spans.len(),
                }));
                r
            }
            ApiRequest::TraceShow { trace_id } => {
                let store = TraceStore::new(self.root.clone());
                let spans = store.read_trace(&trace_id);
                let mut r = ApiResponse::ok();
                r.trace_show = Some(serde_json::json!({
                    "trace_id": trace_id,
                    "spans": spans,
                    "span_count": spans.len(),
                }));
                r
            }
            ApiRequest::TraceFailures { agent_id, since } => {
                let store = TraceStore::new(self.root.clone());
                let spans = store.read_failures(agent_id.as_deref(), since.as_deref());
                let mut r = ApiResponse::ok();
                r.trace_failures = Some(serde_json::json!({
                    "spans": spans,
                    "total": spans.len(),
                }));
                r
            }
            _ => ApiResponse::error("Unknown trace request"),
        }
    }
}
