use std::sync::Arc;
use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse};
use plico::fs::embedding::StubEmbeddingProvider;
use plico::llm::StubProvider;
use tempfile::tempdir;

#[tokio::test]
async fn test_active_entity_linking() {
    std::env::set_var("PLICO_KG_AUTO_EXTRACT", "true");
    let dir = tempdir().unwrap();
    let embedding = Arc::new(StubEmbeddingProvider::new());
    
    // Simulate LLM extraction for the first document
    let llm_resp1 = r#"{"triples": [{"subject":"CEO of Plico","predicate":"is","object":"Leo","type":"related_to"}], "preferences": []}"#;
    let llm = Arc::new(StubProvider::new(llm_resp1));
    
    let kernel = AIKernel::with_providers(dir.path().to_path_buf(), embedding.clone(), llm.clone()).unwrap();
    
    let content1 = "The CEO of Plico is Leo.";
    kernel.handle_api_request(ApiRequest::CoreCreate {
        variant: None,
        data: serde_json::Value::String(content1.to_string()),
        tags: vec!["session1".into()],
        agent_id: "test".into(),
    });
    
    // Use the new flush action
    kernel.handle_api_request(ApiRequest::CoreState {
        action: Some("flush".into()),
        agent_id: "test".into(),
    });
    
    // Verify first node created
    let resp = kernel.handle_api_request(ApiRequest::CoreList {
        variant: Some("node".into()),
        filter: None,
        limit: Some(100),
        offset: None,
        agent_id: "test".into(),
    });
    let nodes = resp.nodes.as_ref().expect("Nodes field should be populated");
    println!("Nodes after first ingestion: {:?}", nodes);
    assert!(nodes.iter().any(|n| n.label.to_lowercase() == "ceo of plico"));
    
    // Now simulate second ingestion with different name but same entity
    // We update the stub LLM (if we could, but here we'll just create a new kernel or use a more dynamic stub)
    // For now, let's use a dynamic stub if available or just re-initialize with new stub for second call
    // Actually, AIKernel stores the provider in a HotSwap wrapper!
    
    let llm_resp2 = r#"{"triples": [{"subject":"Leo","predicate":"lives in","object":"Zurich","type":"related_to"}], "preferences": []}"#;
    kernel.llm_provider().swap(Arc::new(StubProvider::new(llm_resp2)));
    
    let content2 = "Leo lives in Zurich.";
    kernel.handle_api_request(ApiRequest::CoreCreate {
        variant: None,
        data: serde_json::Value::String(content2.to_string()),
        tags: vec!["session2".into()],
        agent_id: "test".into(),
    });
    
    kernel.handle_api_request(ApiRequest::CoreState {
        action: Some("flush".into()),
        agent_id: "test".into(),
    });
    
    // 3. Inspect KG nodes to see if "Leo" was linked to "CEO of Plico"
    let resp_final = kernel.handle_api_request(ApiRequest::CoreList {
        variant: Some("node".into()),
        filter: None,
        limit: Some(100),
        offset: None,
        agent_id: "test".into(),
    });
    
    let nodes_final = resp_final.nodes.as_ref().unwrap();
    println!("KG Nodes after linking: {:?}", nodes_final);
    
    // If linking worked, "Leo" and "CEO of Plico" should be merged or linked.
    // In our implementation, we reused the ID "ent:CEO of Plico" for "Leo" because of label match.
    // Wait, "Leo" != "CEO of Plico". 
    // Ah, my resolve_entity currently only does exact label match OR alias match.
    // Since "Leo" is not yet an alias of "CEO of Plico" in the first step, it will create a new node "ent:Leo".
    
    let resp_edges = kernel.handle_api_request(ApiRequest::CoreList {
        variant: Some("edge".into()),
        filter: None,
        limit: Some(100),
        offset: None,
        agent_id: "test".into(),
    });
    
    let edges = resp_edges.edges.as_ref().expect("Edges field should be populated");
    println!("KG Edges: {:?}", edges);
    
    assert!(edges.iter().any(|e| format!("{:?}", e.edge_type).to_lowercase() == "isaliasof"));
}
