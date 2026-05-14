use std::sync::Arc;
use plico::kernel::AIKernel;
use plico::api::semantic::ApiRequest;
use plico::fs::embedding::StubEmbeddingProvider;
use plico::llm::StubProvider;
use plico::kernel::event_bus::KernelEvent;
use tempfile::tempdir;

#[tokio::test]
async fn test_cognitive_conflict_detection() {
    std::env::set_var("PLICO_KG_AUTO_EXTRACT", "true");
    let dir = tempdir().unwrap();
    let embedding = Arc::new(StubEmbeddingProvider::new());
    
    let llm_resp1 = r#"{"triples": [{"subject":"CEO of Plico","predicate":"is","object":"Leo","type":"related_to"}], "preferences": []}"#;
    let llm = Arc::new(StubProvider::new(llm_resp1));
    
    let kernel = AIKernel::with_providers(dir.path().to_path_buf(), embedding.clone(), llm.clone()).unwrap();
    
    // Subscribe to event bus
    let sub = kernel.event_bus().subscribe();
    
    kernel.handle_api_request(ApiRequest::CoreCreate {
        variant: None,
        data: serde_json::Value::String("According to the latest company report, the CEO of Plico is Leo.".to_string()),
        tags: vec!["t1".into()],
        agent_id: "test".into(),
    });
    
    kernel.handle_api_request(ApiRequest::CoreState {
        action: Some("flush".into()),
        agent_id: "test".into(),
    });
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    
    // Ingest conflict
    let llm_resp2 = r#"{"triples": [{"subject":"CEO of Plico","predicate":"is","object":"Max","type":"related_to"}], "preferences": []}"#;
    kernel.llm_provider().swap(Arc::new(StubProvider::new(llm_resp2)));
    
    kernel.handle_api_request(ApiRequest::CoreCreate {
        variant: None,
        data: serde_json::Value::String("However, an internal memo reveals that the CEO of Plico is actually Max.".to_string()),
        tags: vec!["t2".into()],
        agent_id: "test".into(),
    });
    
    kernel.handle_api_request(ApiRequest::CoreState {
        action: Some("flush".into()),
        agent_id: "test".into(),
    });
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    
    // Check for VerificationFailed event
    let events = kernel.event_bus().poll(&sub).unwrap();
    println!("Polled events: {:?}", events);
    let conflict_found = events.iter().any(|e| matches!(e, KernelEvent::VerificationFailed { operation, .. } if operation == "ConflictDetection"));
    
    assert!(conflict_found, "Cognitive conflict should have been detected and emitted as an event");
}
