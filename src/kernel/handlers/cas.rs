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
            ApiRequest::Create { content, content_encoding, tags, agent_id, intent, .. } => {
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
            ApiRequest::Read { cid, agent_id, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                match self.get_object(&cid, &agent_id, &tenant) {
                    Ok(obj) => ApiResponse::with_data(String::from_utf8_lossy(&obj.data).to_string()),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::Search { query, agent_id, tenant_id, limit, offset, require_tags, exclude_tags, since, until, intent_context, .. } => {
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
            ApiRequest::Update { cid, content, content_encoding, new_tags, agent_id, tenant_id, .. } => {
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
            ApiRequest::Delete { cid, agent_id, tenant_id, .. } => {
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

#[cfg(test)]
mod tests {
    use crate::kernel::tests::make_kernel;
    use crate::api::semantic::ApiRequest;

    fn create_object(kernel: &crate::kernel::AIKernel, content: &str, tags: Vec<&str>) -> String {
        let resp = kernel.handle_api_request(ApiRequest::Create {
            api_version: None,
            content: content.to_string(),
            content_encoding: Default::default(),
            tags: tags.into_iter().map(String::from).collect(),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
            agent_token: None,
            intent: None,
        });
        assert!(resp.ok, "Create should succeed: {:?}", resp.error);
        resp.cid.unwrap()
    }

    #[test]
    fn test_create_and_read() {
        let (kernel, _dir) = make_kernel();
        let cid = create_object(&kernel, "hello world", vec!["test"]);
        let resp = kernel.handle_api_request(ApiRequest::Read {
            cid, agent_id: "test_agent".to_string(), tenant_id: None, agent_token: None,
        });
        assert!(resp.ok);
        assert_eq!(resp.data.unwrap(), "hello world");
    }

    #[test]
    fn test_read_not_found() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::Read {
            cid: "nonexistent".to_string(), agent_id: "test_agent".to_string(), tenant_id: None, agent_token: None,
        });
        assert!(!resp.ok);
    }

    #[test]
    fn test_search_basic() {
        let (kernel, _dir) = make_kernel();
        create_object(&kernel, "rust programming", vec!["code"]);
        let resp = kernel.handle_api_request(ApiRequest::Search {
            query: "rust".to_string(), agent_id: "test_agent".to_string(), tenant_id: None,
            limit: Some(10), offset: None, require_tags: vec![], exclude_tags: vec![],
            since: None, until: None, intent_context: None, agent_token: None,
        });
        assert!(resp.ok, "Search should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_search_with_tags() {
        let (kernel, _dir) = make_kernel();
        create_object(&kernel, "tagged content", vec!["important", "review"]);
        let resp = kernel.handle_api_request(ApiRequest::Search {
            query: "tagged".to_string(), agent_id: "test_agent".to_string(), tenant_id: None,
            limit: Some(10), offset: None, require_tags: vec!["important".to_string()], exclude_tags: vec![],
            since: None, until: None, intent_context: None, agent_token: None,
        });
        assert!(resp.ok);
    }

    #[test]
    fn test_search_with_pagination() {
        let (kernel, _dir) = make_kernel();
        for i in 0..5 {
            create_object(&kernel, &format!("item {i}"), vec!["batch"]);
        }
        let resp = kernel.handle_api_request(ApiRequest::Search {
            query: "item".to_string(), agent_id: "test_agent".to_string(), tenant_id: None,
            limit: Some(2), offset: Some(1), require_tags: vec![], exclude_tags: vec![],
            since: None, until: None, intent_context: None, agent_token: None,
        });
        assert!(resp.ok, "Search with pagination should succeed: {:?}", resp.error);
        // total_count depends on search backend (stub embedding may not find all)
        assert!(resp.total_count.is_some());
    }

    #[test]
    fn test_update() {
        let (kernel, _dir) = make_kernel();
        let cid = create_object(&kernel, "original", vec!["v1"]);
        let resp = kernel.handle_api_request(ApiRequest::Update {
            cid, content: "updated".to_string(), content_encoding: Default::default(),
            new_tags: Some(vec!["v2".to_string()]),
            agent_id: "test_agent".to_string(), tenant_id: None, agent_token: None,
        });
        assert!(resp.ok, "Update should succeed: {:?}", resp.error);
        assert!(resp.cid.is_some());
    }

    #[test]
    fn test_delete_and_list_deleted() {
        let (kernel, _dir) = make_kernel();
        // Grant Delete permission
        kernel.handle_api_request(ApiRequest::GrantPermission {
            agent_id: "test_agent".to_string(), action: "Delete".to_string(),
            scope: Some("*".to_string()), expires_at: None,
        });
        let cid = create_object(&kernel, "to be deleted", vec![]);
        let resp = kernel.handle_api_request(ApiRequest::Delete {
            cid: cid.clone(), agent_id: "test_agent".to_string(), tenant_id: None, agent_token: None,
        });
        assert!(resp.ok, "Delete should succeed: {:?}", resp.error);

        let resp = kernel.handle_api_request(ApiRequest::ListDeleted {
            agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok);
        let deleted = resp.deleted.unwrap();
        assert!(deleted.iter().any(|d| d.cid == cid));
    }

    #[test]
    fn test_restore() {
        let (kernel, _dir) = make_kernel();
        kernel.handle_api_request(ApiRequest::GrantPermission {
            agent_id: "test_agent".to_string(), action: "Delete".to_string(),
            scope: Some("*".to_string()), expires_at: None,
        });
        let cid = create_object(&kernel, "restore me", vec![]);
        kernel.handle_api_request(ApiRequest::Delete {
            cid: cid.clone(), agent_id: "test_agent".to_string(), tenant_id: None, agent_token: None,
        });
        let resp = kernel.handle_api_request(ApiRequest::Restore {
            cid: cid.clone(), agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok, "Restore should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_history() {
        let (kernel, _dir) = make_kernel();
        let cid = create_object(&kernel, "versioned content", vec!["history"]);
        let resp = kernel.handle_api_request(ApiRequest::History {
            cid, agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok, "History should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_rollback() {
        let (kernel, _dir) = make_kernel();
        let cid = create_object(&kernel, "original", vec![]);
        // Update to create a version chain
        let resp = kernel.handle_api_request(ApiRequest::Update {
            cid: cid.clone(), content: "v2".to_string(), content_encoding: Default::default(),
            new_tags: None, agent_id: "test_agent".to_string(), tenant_id: None, agent_token: None,
        });
        let new_cid = resp.cid.unwrap();
        // Rollback to original
        let resp = kernel.handle_api_request(ApiRequest::Rollback {
            cid: new_cid, agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok, "Rollback should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_batch_create() {
        use crate::api::dto::BatchCreateItem;
        use crate::api::semantic::ContentEncoding;
        let (kernel, _dir) = make_kernel();
        let items = vec![
            BatchCreateItem { content: "item1".to_string(), content_encoding: ContentEncoding::default(), tags: vec!["batch".to_string()], intent: None },
            BatchCreateItem { content: "item2".to_string(), content_encoding: ContentEncoding::default(), tags: vec!["batch".to_string()], intent: None },
        ];
        let resp = kernel.handle_api_request(ApiRequest::BatchCreate {
            items, agent_id: "test_agent".to_string(), tenant_id: None,
        });
        assert!(resp.ok, "BatchCreate should succeed: {:?}", resp.error);
        let batch = resp.batch_create.unwrap();
        assert_eq!(batch.successful, 2);
    }

    #[test]
    fn test_search_with_intent_context() {
        let (kernel, _dir) = make_kernel();
        create_object(&kernel, "intent context test", vec!["intent"]);
        let resp = kernel.handle_api_request(ApiRequest::Search {
            query: "intent".to_string(), agent_id: "test_agent".to_string(), tenant_id: None,
            limit: Some(10), offset: None, require_tags: vec![], exclude_tags: vec![],
            since: None, until: None, intent_context: Some("testing".to_string()), agent_token: None,
        });
        assert!(resp.ok, "Search with intent_context should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_search_with_time_range() {
        let (kernel, _dir) = make_kernel();
        create_object(&kernel, "time range test", vec![]);
        let resp = kernel.handle_api_request(ApiRequest::Search {
            query: "time range".to_string(), agent_id: "test_agent".to_string(), tenant_id: None,
            limit: Some(10), offset: None, require_tags: vec![], exclude_tags: vec![],
            since: Some(0), until: Some(u64::MAX as i64), intent_context: None, agent_token: None,
        });
        assert!(resp.ok);
    }
}
