//! Ingest Agent — Example for Node4 MVP
//!
//! Demonstrates the data ingestion workflow for the knowledge base scenario:
//! - Read Markdown files from a directory
//! - BatchCreate存入CAS
//! - KG nodes and edges construction
//! - remember_procedural() to store the workflow itself
//! - HybridRetrieve to verify ingestion results
//!
//! This is SAMPLE CODE, not production code.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse, BatchCreateItem, ProcedureStepDto};
use plico::fs::{KGNodeType, KGEdgeType};
use std::path::Path;
use std::fs;

/// Helper to check API response and return error string if failed.
fn check_response(resp: ApiResponse, context: &str) -> Result<ApiResponse, String> {
    if resp.ok {
        Ok(resp)
    } else {
        Err(format!("{} failed: {:?}", context, resp.error))
    }
}

/// Ingest a directory of Markdown files into Plico CAS + Knowledge Graph.
///
/// This function demonstrates:
/// 1. Reading markdown files from disk
/// 2. Batch creating objects in CAS
/// 3. Building KG nodes (Entity, Fact, Document)
/// 4. Building KG edges (Causes, HasResolution, Mentions, etc.)
/// 5. Storing the ingest workflow as a procedural memory
/// 6. Using HybridRetrieve to verify results
pub fn run_ingest_agent(root: &Path, dir: &Path, agent_id: &str) -> Result<(), String> {
    let kernel = AIKernel::new(root.to_path_buf())
        .map_err(|e| format!("Failed to initialize kernel: {}", e))?;

    // ── Step 1: Read Markdown files from directory ────────────────────────────
    let markdown_files = read_markdown_dir(dir)?;
    if markdown_files.is_empty() {
        println!("No markdown files found in {:?}", dir);
        return Ok(());
    }
    println!("Found {} markdown files to ingest", markdown_files.len());

    // ── Step 2: BatchCreate存入CAS ───────────────────────────────────────────
    let items: Vec<BatchCreateItem> = markdown_files
        .iter()
        .map(|(filename, content)| BatchCreateItem {
            content: content.clone(),
            content_encoding: Default::default(),
            tags: extract_tags_from_filename(filename),
            intent: Some(format!("Ingested from {}", filename)),
        })
        .collect();

    let batch_req = ApiRequest::BatchCreate {
        items,
        agent_id: agent_id.to_string(),
        tenant_id: None,
    };

    let batch_response = check_response(kernel.handle_api_request(batch_req), "BatchCreate")?;

    let results = batch_response.batch_create
        .ok_or("No batch_create result in response")?;
    println!("CAS storage: {} successful, {} failed",
        results.successful, results.failed);

    // Collect successful CIDs for KG operations
    let cids: Vec<(String, String)> = results.results
        .iter()
        .enumerate()
        .filter_map(|(i, r): (usize, &Result<String, String>)| match r {
            Ok(cid) => Some((cid.clone(), markdown_files.get(i).map(|(f, _)| f.clone()).unwrap_or_default())),
            Err(_) => None,
        })
        .collect();

    // ── Step 3: KG节点构建 ────────────────────────────────────────────────────
    // For each ingested document, create KG nodes:
    // - Document node: represents the article itself
    // - Entity nodes: extracted concepts (simplified - in production, use LLM)
    // - Fact nodes: claims made in the document
    for (cid, filename) in &cids {
        let cid_len = 8.min(cid.len());
        let doc_node_id = format!("doc_{}", &cid[..cid_len]);

        // Create Document node
        let add_doc_node = ApiRequest::AddNode {
            label: filename.clone(),
            node_type: KGNodeType::Document,
            properties: serde_json::json!({
                "source_cid": cid,
                "filename": filename,
            }),
            agent_id: agent_id.to_string(),
            tenant_id: None,
        };
        check_response(kernel.handle_api_request(add_doc_node), "AddNode (Document)")?;

        // Create Entity nodes for common security concepts (simplified example)
        let entities = extract_entities_from_filename(filename);
        for entity_label in entities {
            let entity_node_id = format!("entity_{}", entity_label.to_lowercase().replace(" ", "_"));

            let add_entity_node = ApiRequest::AddNode {
                label: entity_label.clone(),
                node_type: KGNodeType::Entity,
                properties: serde_json::json!({
                    "source_document": doc_node_id,
                }),
                agent_id: agent_id.to_string(),
                tenant_id: None,
            };
            let _ = kernel.handle_api_request(add_entity_node);

            // Create Mentions edge: Document --> Entity
            let mentions_edge = ApiRequest::AddEdge {
                src_id: doc_node_id.clone(),
                dst_id: entity_node_id,
                edge_type: KGEdgeType::Mentions,
                weight: None,
                agent_id: agent_id.to_string(),
                tenant_id: None,
            };
            let _ = kernel.handle_api_request(mentions_edge);
        }
    }

    // ── Step 4: KG边构建 ─────────────────────────────────────────────────────
    // Create causal relationships between entities (simplified)
    build_causal_edges(&kernel, agent_id)?;

    // ── Step 5: remember_procedural() 存储工作流 ──────────────────────────────
    let workflow_steps = vec![
        ProcedureStepDto {
            description: "Read markdown files from directory".to_string(),
            action: "read_dir(*.md)".to_string(),
            expected_outcome: Some("List of (filename, content) tuples".to_string()),
        },
        ProcedureStepDto {
            description: "BatchCreate each file into CAS".to_string(),
            action: "ApiRequest::BatchCreate".to_string(),
            expected_outcome: Some("Vector of (CID, filename) for successful inserts".to_string()),
        },
        ProcedureStepDto {
            description: "Create KG Document nodes".to_string(),
            action: "ApiRequest::AddNode(node_type=Document)".to_string(),
            expected_outcome: Some("Document nodes linked to CAS CIDs".to_string()),
        },
        ProcedureStepDto {
            description: "Create KG Entity nodes".to_string(),
            action: "ApiRequest::AddNode(node_type=Entity)".to_string(),
            expected_outcome: Some("Entity nodes for key concepts".to_string()),
        },
        ProcedureStepDto {
            description: "Create KG edges".to_string(),
            action: "ApiRequest::AddEdge".to_string(),
            expected_outcome: Some("Mentions, Causes, HasResolution edges".to_string()),
        },
        ProcedureStepDto {
            description: "Store workflow as procedural memory".to_string(),
            action: "ApiRequest::RememberProcedural".to_string(),
            expected_outcome: Some("Workflow stored in L3 Procedural tier".to_string()),
        },
        ProcedureStepDto {
            description: "Verify with HybridRetrieve".to_string(),
            action: "ApiRequest::HybridRetrieve".to_string(),
            expected_outcome: Some("Documents retrievable via hybrid search".to_string()),
        },
    ];

    let remember_req = ApiRequest::RememberProcedural {
        agent_id: agent_id.to_string(),
        name: "security_article_ingest".to_string(),
        description: "Ingest workflow for cybersecurity articles into CAS and Knowledge Graph".to_string(),
        steps: workflow_steps,
        learned_from: Some("design-node4-collaborative-ecosystem.md".to_string()),
        tags: vec!["verified".to_string(), "ingest-workflow".to_string(), "security".to_string()],
        scope: Some("shared".to_string()),
    };

    check_response(kernel.handle_api_request(remember_req), "RememberProcedural")?;
    println!("Workflow stored as procedural memory with scope=shared");

    // ── Step 6: HybridRetrieve验证摄入结果 ───────────────────────────────────
    let verify_query = ApiRequest::HybridRetrieve {
        query_text: "SQL injection attack defense".to_string(),
        seed_tags: vec!["sql-injection".to_string()],
        graph_depth: 2,
        edge_types: vec!["causes".to_string(), "has_resolution".to_string(), "mentions".to_string()],
        max_results: 10,
        token_budget: Some(4000),
        agent_id: agent_id.to_string(),
        tenant_id: None,
    };

    let verify_response = check_response(kernel.handle_api_request(verify_query), "HybridRetrieve")?;

    if let Some(hybrid_result) = verify_response.hybrid_result {
        println!("\n=== HybridRetrieve Verification ===");
        println!("Total items returned: {}", hybrid_result.items.len());
        println!("Vector hits: {}", hybrid_result.vector_hits);
        println!("Graph hits: {}", hybrid_result.graph_hits);
        println!("Paths found: {}", hybrid_result.paths_found);
        println!("Token estimate: {}", hybrid_result.token_estimate);

        for (i, hit) in hybrid_result.items.iter().take(5).enumerate() {
            let cid_preview = &hit.cid[..8.min(hit.cid.len())];
            println!("  [{}/{}] CID: {} score: {:.3}",
                i + 1,
                hybrid_result.items.len(),
                cid_preview,
                hit.combined_score
            );
        }
    }

    println!("\nIngest completed successfully!");
    Ok(())
}

/// Read all .md files from a directory.
fn read_markdown_dir(dir: &Path) -> Result<Vec<(String, String)>, String> {
    let mut files = Vec::new();

    if !dir.is_dir() {
        return Err(format!("{:?} is not a directory", dir));
    }

    for entry in fs::read_dir(dir)
        .map_err(|e| format!("Failed to read directory: {}", e))?
    {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry.path();

        if path.extension().map(|e| e == "md").unwrap_or(false) {
            let filename = path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown.md")
                .to_string();

            let content = fs::read_to_string(&path)
                .map_err(|e| format!("Failed to read {:?}: {}", path, e))?;

            files.push((filename, content));
        }
    }

    Ok(files)
}

/// Extract semantic tags from filename (simplified).
fn extract_tags_from_filename(filename: &str) -> Vec<String> {
    let lowercase = filename.to_lowercase();
    let mut tags = Vec::new();

    // Extract topic tags based on filename keywords
    if lowercase.contains("sql-injection") || lowercase.contains("sql_injection") {
        tags.push("sql-injection".to_string());
        tags.push("web-security".to_string());
    }
    if lowercase.contains("xss") {
        tags.push("xss".to_string());
        tags.push("web-security".to_string());
    }
    if lowercase.contains("csrf") {
        tags.push("csrf".to_string());
        tags.push("web-security".to_string());
    }
    if lowercase.contains("cve") {
        tags.push("vulnerability".to_string());
    }
    if lowercase.contains("pentest") || lowercase.contains("pen-test") {
        tags.push("penetration-testing".to_string());
    }
    if lowercase.contains("defense") || lowercase.contains("mitigation") {
        tags.push("defense".to_string());
    }

    // Always tag as security content
    tags.push("security".to_string());
    tags.push("article".to_string());

    tags
}

/// Extract entities from filename (simplified - production would use LLM).
fn extract_entities_from_filename(filename: &str) -> Vec<String> {
    let lowercase = filename.to_lowercase();
    let mut entities = Vec::new();

    if lowercase.contains("sql") {
        entities.push("SQL".to_string());
        entities.push("SQL Injection".to_string());
    }
    if lowercase.contains("injection") {
        entities.push("Injection Attack".to_string());
    }
    if lowercase.contains("xss") {
        entities.push("Cross-Site Scripting".to_string());
    }
    if lowercase.contains("csrf") {
        entities.push("CSRF".to_string());
    }
    if lowercase.contains("authentication") {
        entities.push("Authentication".to_string());
    }
    if lowercase.contains("authorization") || lowercase.contains("authz") {
        entities.push("Authorization".to_string());
    }
    if lowercase.contains("waf") {
        entities.push("WAF".to_string());
        entities.push("Web Application Firewall".to_string());
    }
    if lowercase.contains("encryption") || lowercase.contains("crypto") {
        entities.push("Encryption".to_string());
    }

    entities
}

/// Build causal edges between entities (simplified example).
fn build_causal_edges(kernel: &AIKernel, agent_id: &str) -> Result<(), String> {
    // Example causal relationships for cybersecurity domain
    let causal_relations = vec![
        // (source_entity, target_entity, edge_type, description)
        ("SQL Injection", "Data Breach", "causes", "SQL injection can lead to data exfiltration"),
        ("SQL Injection", "Parameterization", "has_resolution", "Use parameterized queries to prevent SQL injection"),
        ("XSS", "Session Hijacking", "causes", "XSS vulnerabilities can enable session hijacking"),
        ("XSS", "Input Validation", "has_resolution", "Sanitize user input to prevent XSS"),
        ("CSRF", "Unauthorized Actions", "causes", "CSRF can force users to perform unwanted actions"),
        ("CSRF", "Anti-CSRF Tokens", "has_resolution", "Use anti-CSRF tokens to prevent attacks"),
    ];

    for (src, dst, edge_type_str, _desc) in causal_relations {
        let edge_type = match edge_type_str {
            "causes" => KGEdgeType::Causes,
            "has_resolution" => KGEdgeType::HasResolution,
            _ => KGEdgeType::RelatedTo,
        };

        // Create source entity node if not exists
        let src_id = format!("entity_{}", src.to_lowercase().replace(" ", "_"));
        let add_src = ApiRequest::AddNode {
            label: src.to_string(),
            node_type: KGNodeType::Entity,
            properties: serde_json::json!({"concept": "security"}),
            agent_id: agent_id.to_string(),
            tenant_id: None,
        };
        let _ = kernel.handle_api_request(add_src);

        // Create destination entity node if not exists
        let dst_id = format!("entity_{}", dst.to_lowercase().replace(" ", "_"));
        let add_dst = ApiRequest::AddNode {
            label: dst.to_string(),
            node_type: KGNodeType::Entity,
            properties: serde_json::json!({"concept": "security"}),
            agent_id: agent_id.to_string(),
            tenant_id: None,
        };
        let _ = kernel.handle_api_request(add_dst);

        // Create the edge
        let add_edge = ApiRequest::AddEdge {
            src_id,
            dst_id,
            edge_type,
            weight: Some(0.8),
            agent_id: agent_id.to_string(),
            tenant_id: None,
        };
        let _ = kernel.handle_api_request(add_edge);
    }

    Ok(())
}

fn main() {
    // Example usage
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 3 {
        println!("Usage: {} <root> <markdown_dir> [agent_id]", args[0]);
        println!("Example: {} /tmp/plico ./articles ingest-agent-1", args[0]);
        std::process::exit(1);
    }

    let root = Path::new(&args[1]);
    let dir = Path::new(&args[2]);
    let agent_id = args.get(3).map(|s| s.as_str()).unwrap_or("ingest-agent");

    if let Err(e) = run_ingest_agent(root, dir, agent_id) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
