//! CAS CRUD + versioning + batch create handlers.

use crate::api::semantic::{ApiRequest, ApiResponse, SearchResultDto, DeletedDto, ContentEncoding};
use crate::DEFAULT_TENANT;
use super::super::ops;

fn decode_content(content: &str, encoding: &ContentEncoding) -> Result<Vec<u8>, String> {
    crate::api::semantic::decode_content(content, encoding)
}

impl super::super::AIKernel {
    pub(crate) fn handle_cas(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::Create { content, content_encoding, tags, agent_id, agent_token, intent, .. } => {
                if let Err(e) = self.key_store.verify_agent_token(&agent_id, agent_token.as_deref()) {
                    return ApiResponse::error(e);
                }
                let bytes = match decode_content(&content, &content_encoding) {
                    Ok(b) => b,
                    Err(e) => return ApiResponse::error(e),
                };
                match self.semantic_create(bytes, tags, &agent_id, intent) {
                    Ok(cid) => {
                        self.maybe_persist_search_index();
                        ApiResponse::with_cid(cid)
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::Read { cid, agent_id, agent_token, tenant_id, .. } => {
                if let Err(e) = self.key_store.verify_agent_token(&agent_id, agent_token.as_deref()) {
                    return ApiResponse::error(e);
                }
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                match self.get_object(&cid, &agent_id, &tenant) {
                    Ok(obj) => ApiResponse::with_data(String::from_utf8_lossy(&obj.data).to_string()),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::Search { query, agent_id, agent_token, tenant_id, limit, offset, require_tags, exclude_tags, since, until, intent_context } => {
                if let Err(e) = self.key_store.verify_agent_token(&agent_id, agent_token.as_deref()) {
                    return ApiResponse::error(e);
                }
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                let lim = limit.unwrap_or(10);
                let off = offset.unwrap_or(0);
                let results = if intent_context.is_some() {
                    match self.semantic_search_with_intent(
                        ops::fs::SearchQuery {
                            query: &query, agent_id: &agent_id, tenant_id: &tenant,
                            limit: lim + off, require_tags, exclude_tags,
                        },
                        intent_context,
                    ) {
                        Ok(r) => r,
                        Err(e) => return ApiResponse::error(e.to_string()),
                    }
                } else {
                    match self.semantic_search_with_time(
                        ops::fs::SearchQuery {
                            query: &query, agent_id: &agent_id, tenant_id: &tenant,
                            limit: lim + off, require_tags, exclude_tags,
                        },
                        since, until,
                    ) {
                        Ok(r) => r,
                        Err(e) => return ApiResponse::error(e.to_string()),
                    }
                };
                let total = results.len();
                let page: Vec<SearchResultDto> = results.into_iter().skip(off).take(lim).map(|r| {
                    let snippet = r.snippet.clone();
                    SearchResultDto {
                        cid: r.cid, relevance: r.relevance, tags: r.meta.tags.clone(),
                        snippet, content_type: r.meta.content_type.to_string(), created_at: r.meta.created_at,
                    }
                }).collect();
                let mut r = ApiResponse::ok();
                r.total_count = Some(total);
                r.has_more = Some(off + page.len() < total);
                r.results = Some(page);
                r
            }
            ApiRequest::Update { cid, content, content_encoding, new_tags, agent_id, agent_token, tenant_id, .. } => {
                if let Err(e) = self.key_store.verify_agent_token(&agent_id, agent_token.as_deref()) {
                    return ApiResponse::error(e);
                }
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                let bytes = match decode_content(&content, &content_encoding) {
                    Ok(b) => b,
                    Err(e) => return ApiResponse::error(e),
                };
                match self.semantic_update(&cid, bytes, new_tags, &agent_id, &tenant) {
                    Ok(new_cid) => {
                        self.maybe_persist_search_index();
                        ApiResponse::with_cid(new_cid)
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::Delete { cid, agent_id, agent_token, tenant_id, .. } => {
                if let Err(e) = self.key_store.verify_agent_token(&agent_id, agent_token.as_deref()) {
                    return ApiResponse::error(e);
                }
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                match self.semantic_delete(&cid, &agent_id, &tenant) {
                    Ok(()) => {
                        self.maybe_persist_search_index();
                        ApiResponse::ok()
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::ListDeleted { agent_id } => {
                let entries = self.list_deleted(&agent_id);
                let dto: Vec<DeletedDto> = entries.into_iter().map(|e| DeletedDto {
                    cid: e.cid, deleted_at: e.deleted_at, tags: e.original_meta.tags,
                }).collect();
                let mut r = ApiResponse::ok();
                r.deleted = Some(dto);
                r
            }
            ApiRequest::Restore { cid, agent_id } => {
                match self.restore_deleted(&cid, &agent_id) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::History { cid, agent_id } => {
                let chain = self.version_history(&cid, &agent_id);
                let mut r = ApiResponse::ok();
                r.data = Some(serde_json::to_string(&chain).unwrap_or_default());
                r
            }
            ApiRequest::Rollback { cid, agent_id } => {
                match self.rollback(&cid, &agent_id) {
                    Ok(new_cid) => {
                        self.maybe_persist_search_index();
                        ApiResponse::with_cid(new_cid)
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::BatchCreate { items, agent_id, tenant_id } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                let batch_results = self.handle_batch_create(items, &agent_id, &tenant);
                let mut r = ApiResponse::ok();
                r.batch_create = Some(batch_results);
                r
            }
            _ => unreachable!("non-CAS request routed to handle_cas"),
        }
    }
}
