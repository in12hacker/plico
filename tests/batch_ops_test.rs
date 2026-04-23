//! Batch operations API tests (v15.0)
//!
//! Tests for:
//! - BatchCreate: bulk object creation
//! - BatchMemoryStore: bulk memory storage
//! - BatchSubmitIntent: bulk intent submission
//! - BatchQuery: bulk querying

use plico::api::semantic::{
    ApiRequest, BatchCreateItem, BatchMemoryEntry, ContentEncoding, IntentSpec, QuerySpec,
};
use plico::kernel::AIKernel;
use tempfile::tempdir;

fn make_kernel() -> (AIKernel, tempfile::TempDir) {
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
    let dir = tempdir().unwrap();
    let kernel = AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
    (kernel, dir)
}

#[test]
fn test_batch_create_success() {
    let (kernel, _dir) = make_kernel();

    let items = vec![
        BatchCreateItem {
            content: "content1".to_string(),
            content_encoding: ContentEncoding::Utf8,
            tags: vec!["tag1".to_string()],
            intent: None,
        },
        BatchCreateItem {
            content: "content2".to_string(),
            content_encoding: ContentEncoding::Utf8,
            tags: vec!["tag2".to_string()],
            intent: None,
        },
        BatchCreateItem {
            content: "content3".to_string(),
            content_encoding: ContentEncoding::Utf8,
            tags: vec!["tag3".to_string()],
            intent: None,
        },
    ];

    let req = ApiRequest::BatchCreate {
        items,
        agent_id: "TestAgent".to_string(),
        tenant_id: Some("default".to_string()),
    };

    let response = kernel.handle_api_request(req);

    assert!(response.ok, "batch create should succeed");
    let batch = response.batch_create.expect("batch_create should be present");
    assert_eq!(batch.successful, 3, "all 3 items should succeed");
    assert_eq!(batch.failed, 0, "no items should fail");

    // Verify all CIDs are valid
    for result in &batch.results {
        let cid = result.as_ref().expect("item should succeed");
        assert!(!cid.is_empty(), "CID should not be empty");
        // Verify the object exists
        let obj = kernel
            .get_object(cid, "TestAgent", "default")
            .expect("object should exist");
        assert!(!obj.data.is_empty());
    }
}

#[test]
fn test_batch_create_partial_failure() {
    let (kernel, _dir) = make_kernel();

    // First create an object that we'll try to update with invalid data later
    let items = vec![
        BatchCreateItem {
            content: "valid content".to_string(),
            content_encoding: ContentEncoding::Utf8,
            tags: vec![],
            intent: None,
        },
        // Empty content is valid, but let's test with invalid base64
        BatchCreateItem {
            content: "not-valid-base64!!!".to_string(),
            content_encoding: ContentEncoding::Base64,
            tags: vec![],
            intent: None,
        },
        BatchCreateItem {
            content: "another valid".to_string(),
            content_encoding: ContentEncoding::Utf8,
            tags: vec!["test".to_string()],
            intent: None,
        },
    ];

    let req = ApiRequest::BatchCreate {
        items,
        agent_id: "TestAgent".to_string(),
        tenant_id: Some("default".to_string()),
    };

    let response = kernel.handle_api_request(req);

    assert!(response.ok, "batch create overall should succeed (partial)");
    let batch = response.batch_create.expect("batch_create should be present");
    assert_eq!(batch.successful, 2, "2 items should succeed");
    assert_eq!(batch.failed, 1, "1 item should fail");

    // First result should be Ok
    assert!(batch.results[0].is_ok());
    // Second result should be Err (invalid base64)
    assert!(batch.results[1].is_err());
    assert!(batch.results[1].as_ref().err().unwrap().contains("base64"));
    // Third result should be Ok
    assert!(batch.results[2].is_ok());
}

#[test]
fn test_batch_memory_store() {
    let (kernel, _dir) = make_kernel();

    // First register the agent to avoid permission issues
    kernel.handle_api_request(ApiRequest::RegisterAgent {
        name: "MemAgent".to_string(),
    });

    let entries = vec![
        BatchMemoryEntry {
            content: "memory 1".to_string(),
            tier: "working".to_string(),
            importance: 50,
            tags: vec!["test".to_string()],
        },
        BatchMemoryEntry {
            content: "memory 2".to_string(),
            tier: "working".to_string(),
            importance: 70,
            tags: vec!["important".to_string()],
        },
        BatchMemoryEntry {
            content: "memory 3".to_string(),
            tier: "working".to_string(),
            importance: 30,
            tags: vec![],
        },
    ];

    let req = ApiRequest::BatchMemoryStore {
        entries,
        agent_id: "MemAgent".to_string(),
        tenant_id: Some("default".to_string()),
    };

    let response = kernel.handle_api_request(req);

    assert!(response.ok, "batch memory store should succeed");
    let batch = response.batch_memory_store.expect("batch_memory_store should be present");
    assert_eq!(batch.successful, 3, "all 3 entries should succeed");
    assert_eq!(batch.failed, 0, "no entries should fail");

    // Verify memories can be recalled
    let recall_req = ApiRequest::Recall {
        agent_id: "MemAgent".to_string(),
        scope: None,
        query: None,
        limit: None,
    };
    let recall_response = kernel.handle_api_request(recall_req);
    assert!(recall_response.ok);
    let memories = recall_response.memory.expect("memory should be present");
    assert_eq!(memories.len(), 3, "should have 3 memories stored");
}

#[test]
fn test_batch_submit_intent() {
    let (kernel, _dir) = make_kernel();

    let intents = vec![
        IntentSpec {
            description: "intent 1".to_string(),
            priority: "high".to_string(),
            action: None,
        },
        IntentSpec {
            description: "intent 2".to_string(),
            priority: "medium".to_string(),
            action: None,
        },
        IntentSpec {
            description: "intent 3".to_string(),
            priority: "low".to_string(),
            action: None,
        },
    ];

    let req = ApiRequest::BatchSubmitIntent {
        intents,
        agent_id: "IntentAgent".to_string(),
    };

    let response = kernel.handle_api_request(req);

    assert!(response.ok, "batch submit intent should succeed");
    let batch = response.batch_submit_intent.expect("batch_submit_intent should be present");
    assert_eq!(batch.successful, 3, "all 3 intents should succeed");
    assert_eq!(batch.failed, 0, "no intents should fail");

    // Verify all intent IDs are valid
    for result in &batch.results {
        let intent_id = result.as_ref().expect("intent should succeed");
        assert!(!intent_id.is_empty(), "intent_id should not be empty");
    }
}

#[test]
fn test_batch_query() {
    let (kernel, _dir) = make_kernel();

    // First create some objects to query
    let cid1 = kernel
        .semantic_create(
            b"content about rust programming".to_vec(),
            vec!["rust".to_string(), "programming".to_string()],
            "QueryAgent",
            None,
        )
        .expect("create should succeed");

    let _cid2 = kernel
        .semantic_create(
            b"content about python programming".to_vec(),
            vec!["python".to_string(), "programming".to_string()],
            "QueryAgent",
            None,
        )
        .expect("create should succeed");

    let cid3 = kernel
        .semantic_create(
            b"content about web development".to_vec(),
            vec!["web".to_string(), "development".to_string()],
            "QueryAgent",
            None,
        )
        .expect("create should succeed");

    let queries = vec![
        // Read query for first object
        QuerySpec::Read {
            cid: cid1.clone(),
        },
        // Search query
        QuerySpec::Search {
            query: "programming".to_string(),
            limit: Some(10),
            require_tags: vec![],
            exclude_tags: vec![],
        },
        // Read query for third object
        QuerySpec::Read {
            cid: cid3.clone(),
        },
    ];

    let req = ApiRequest::BatchQuery {
        queries,
        agent_id: "QueryAgent".to_string(),
        tenant_id: Some("default".to_string()),
    };

    let response = kernel.handle_api_request(req);

    assert!(response.ok, "batch query should succeed");
    let batch = response.batch_query.expect("batch_query should be present");
    assert_eq!(batch.successful, 3, "all 3 queries should succeed");
    assert_eq!(batch.failed, 0, "no queries should fail");

    // First result: read should return the object content
    let first = batch.results[0].as_ref().expect("first query should succeed");
    let first_json: serde_json::Value = serde_json::from_str(
        &serde_json::to_string(first).unwrap()
    ).unwrap();
    assert_eq!(first_json["cid"], cid1);

    // Second result: search should return results
    let second = batch.results[1].as_ref().expect("second query should succeed");
    let second_json: serde_json::Value = serde_json::from_str(
        &serde_json::to_string(second).unwrap()
    ).unwrap();
    assert!(second_json["count"].as_i64().unwrap() >= 2); // at least rust and python

    // Third result: read should return the object content
    let third = batch.results[2].as_ref().expect("third query should succeed");
    let third_json: serde_json::Value = serde_json::from_str(
        &serde_json::to_string(third).unwrap()
    ).unwrap();
    assert_eq!(third_json["cid"], cid3);
}

#[test]
fn test_batch_query_with_failure() {
    let (kernel, _dir) = make_kernel();

    // Query with non-existent CID
    let queries = vec![
        QuerySpec::Read {
            cid: "non-existent-cid".to_string(),
        },
        QuerySpec::Read {
            cid: "also-non-existent".to_string(),
        },
    ];

    let req = ApiRequest::BatchQuery {
        queries,
        agent_id: "QueryAgent".to_string(),
        tenant_id: Some("default".to_string()),
    };

    let response = kernel.handle_api_request(req);

    // Batch query overall succeeds (partial)
    assert!(response.ok, "batch query overall should succeed");
    let batch = response.batch_query.expect("batch_query should be present");
    assert_eq!(batch.successful, 0, "no queries should succeed");
    assert_eq!(batch.failed, 2, "both queries should fail");

    // Both should be errors
    assert!(batch.results[0].is_err());
    assert!(batch.results[1].is_err());
}

#[test]
fn test_batch_intent_priority() {
    let (kernel, _dir) = make_kernel();

    let intents = vec![
        IntentSpec {
            description: "critical task".to_string(),
            priority: "critical".to_string(),
            action: None,
        },
        IntentSpec {
            description: "low priority task".to_string(),
            priority: "low".to_string(),
            action: None,
        },
        IntentSpec {
            description: "default medium task".to_string(),
            priority: "medium".to_string(),
            action: None,
        },
    ];

    let req = ApiRequest::BatchSubmitIntent {
        intents,
        agent_id: "PriorityAgent".to_string(),
    };

    let response = kernel.handle_api_request(req);

    assert!(response.ok);
    let batch = response.batch_submit_intent.expect("batch_submit_intent should be present");
    assert_eq!(batch.successful, 3);

    // All intents should have been submitted successfully regardless of priority
    for result in &batch.results {
        assert!(result.is_ok());
    }
}

#[test]
fn test_batch_empty_items() {
    let (kernel, _dir) = make_kernel();

    // Empty batch create
    let req = ApiRequest::BatchCreate {
        items: vec![],
        agent_id: "TestAgent".to_string(),
        tenant_id: Some("default".to_string()),
    };

    let response = kernel.handle_api_request(req);
    assert!(response.ok);
    let batch = response.batch_create.expect("batch_create should be present");
    assert_eq!(batch.successful, 0);
    assert_eq!(batch.failed, 0);
    assert!(batch.results.is_empty());

    // Empty batch memory store
    let req = ApiRequest::BatchMemoryStore {
        entries: vec![],
        agent_id: "TestAgent".to_string(),
        tenant_id: Some("default".to_string()),
    };

    let response = kernel.handle_api_request(req);
    assert!(response.ok);
    let batch = response.batch_memory_store.expect("batch_memory_store should be present");
    assert_eq!(batch.successful, 0);
    assert_eq!(batch.failed, 0);

    // Empty batch submit intent
    let req = ApiRequest::BatchSubmitIntent {
        intents: vec![],
        agent_id: "TestAgent".to_string(),
    };

    let response = kernel.handle_api_request(req);
    assert!(response.ok);
    let batch = response.batch_submit_intent.expect("batch_submit_intent should be present");
    assert_eq!(batch.successful, 0);
    assert_eq!(batch.failed, 0);

    // Empty batch query
    let req = ApiRequest::BatchQuery {
        queries: vec![],
        agent_id: "TestAgent".to_string(),
        tenant_id: Some("default".to_string()),
    };

    let response = kernel.handle_api_request(req);
    assert!(response.ok);
    let batch = response.batch_query.expect("batch_query should be present");
    assert_eq!(batch.successful, 0);
    assert_eq!(batch.failed, 0);
}
