//! Knowledge Agent — Example for Node4 MVP
//!
//! Demonstrates the knowledge query workflow for the knowledge base scenario:
//! - StartSession for session management
//! - HybridRetrieve for combined vector + KG retrieval
//! - GrowthReport for tracking learning progress
//! - Knowledge memory and sharing between agents
//!
//! This is SAMPLE CODE, not production code.

use plico::kernel::AIKernel;
use plico::api::semantic::{
    ApiRequest, ApiResponse, GrowthPeriod, HybridResult,
};
use plico::memory::MemoryTier;
use std::path::Path;

/// Helper to check API response and return error string if failed.
fn check_response(resp: ApiResponse, context: &str) -> Result<ApiResponse, String> {
    if resp.ok {
        Ok(resp)
    } else {
        Err(format!("{} failed: {:?}", context, resp.error))
    }
}

/// Knowledge Agent — processes user queries against the security knowledge base.
///
/// This agent demonstrates:
/// 1. StartSession to manage session lifecycle and restore context
/// 2. HybridRetrieve for combined vector + KG search (Graph-RAG)
/// 3. RememberLongTerm to store insights with sharing
/// 4. GrowthReport to track learning progress
/// 5. RecallVisible for cross-agent knowledge sharing
pub fn run_knowledge_agent(
    root: &Path,
    query: &str,
    agent_id: &str,
) -> Result<KnowledgeResponse, String> {
    let kernel = AIKernel::new(root.to_path_buf())
        .map_err(|e| format!("Failed to initialize kernel: {}", e))?;

    // ── Step 1: StartSession管理会话 ─────────────────────────────────────────
    // Restore working and long-term memory from previous sessions
    let session_req = ApiRequest::StartSession {
        agent_id: agent_id.to_string(),
        agent_token: None,
        intent_hint: Some(query.to_string()),
        load_tiers: vec![MemoryTier::Working, MemoryTier::LongTerm],
        last_seen_seq: None, // First session, or pass from previous EndSession
    };

    let session_response = check_response(kernel.handle_api_request(session_req), "StartSession")?;

    let session_id = session_response.session_started
        .as_ref()
        .map(|s| s.session_id.clone())
        .unwrap_or_else(|| "unknown".to_string());

    println!("[Session {}] Started", session_id);

    // Check if we restored any context from previous sessions
    if let Some(ref started) = session_response.session_started {
        if !started.changes_since_last.is_empty() {
            println!("  Restored {} changes from previous session", started.changes_since_last.len());
        }
        if let Some(ref warm_ctx) = started.warm_context {
            println!("  Warm context available: {}", warm_ctx);
        }
    }

    // ── Step 2: HybridRetrieve执行检索 ─────────────────────────────────────────
    // Query using combined vector search + knowledge graph traversal
    let hybrid_req = ApiRequest::HybridRetrieve {
        query_text: query.to_string(),
        seed_tags: extract_seed_tags(query),   // KG seed nodes based on query
        graph_depth: 2,                         // Traverse 2 hops in KG
        edge_types: vec![
            "causes".to_string(),
            "has_resolution".to_string(),
            "mentions".to_string(),
            "part_of".to_string(),
        ],
        max_results: 20,
        token_budget: Some(8000), // Limit context to ~8000 tokens
        agent_id: agent_id.to_string(),
        tenant_id: None,
    };

    let hybrid_response = check_response(kernel.handle_api_request(hybrid_req), "HybridRetrieve")?;

    let hybrid_result = hybrid_response.hybrid_result
        .ok_or("No hybrid_result in response")?;

    println!("\n[HybridRetrieve] Results:");
    println!("  Items returned: {}", hybrid_result.items.len());
    println!("  Vector hits: {}", hybrid_result.vector_hits);
    println!("  Graph hits: {}", hybrid_result.graph_hits);
    println!("  Causal paths: {}", hybrid_result.paths_found);
    println!("  Token estimate: {}", hybrid_result.token_estimate);

    // ── Step 3: Analyze results and store insights ──────────────────────────────
    // Agent's LLM would process these results to generate an answer.
    // Here we simulate storing a useful insight.

    let insight = format!(
        "Query '{}' returned {} results via hybrid search ({} vector, {} graph hits)",
        query,
        hybrid_result.items.len(),
        hybrid_result.vector_hits,
        hybrid_result.graph_hits
    );

    // Store the insight as a working memory
    let remember_req = ApiRequest::Remember {
        agent_id: agent_id.to_string(),
        content: insight,
        tenant_id: None,
    };
    let _ = kernel.handle_api_request(remember_req);

    // If the query was important, store as long-term with sharing
    if hybrid_result.items.len() > 5 {
        let long_term_req = ApiRequest::RememberLongTerm {
            agent_id: agent_id.to_string(),
            content: format!(
                "Successfully answered query '{}' using hybrid retrieval. \
                Found {} relevant documents with {} causal paths.",
                query,
                hybrid_result.items.len(),
                hybrid_result.paths_found
            ),
            tags: vec!["query".to_string(), "insight".to_string()],
            importance: 70,
            scope: Some("shared".to_string()),
            tenant_id: None,
        };
        let _ = kernel.handle_api_request(long_term_req);
        println!("[Memory] Stored insight as shared long-term memory");
    }

    // ── Step 4: Cross-agent knowledge sharing ──────────────────────────────────
    // Check if other agents have shared knowledge relevant to this query
    let shared_req = ApiRequest::RecallVisible {
        agent_id: agent_id.to_string(),
        groups: vec![], // Empty = all shared memories
    };

    let shared_response = check_response(kernel.handle_api_request(shared_req), "RecallVisible")?;

    let shared_memories: Vec<String> = shared_response.memory
        .unwrap_or_default();

    println!("\n[Shared Knowledge] Found {} relevant memories from other agents",
        shared_memories.len());

    // ── Step 5: GrowthReport追踪成长 ───────────────────────────────────────────
    let growth_req = ApiRequest::QueryGrowthReport {
        agent_id: agent_id.to_string(),
        period: GrowthPeriod::Last7Days,
    };

    let growth_response = check_response(kernel.handle_api_request(growth_req), "QueryGrowthReport")?;

    if let Some(report) = growth_response.growth_report {
        println!("\n[Growth Report] Last 7 days:");
        println!("  Sessions: {}", report.sessions_total);
        println!("  Token efficiency: {:.2} (lower is better)", report.token_efficiency_ratio);
        println!("  Intent cache hit rate: {:.2}", report.intent_cache_hit_rate);
        println!("  Memories stored: {}", report.memories_stored);
        println!("  Memories shared: {}", report.memories_shared);
        println!("  KG nodes created: {}", report.kg_nodes_created);
        println!("  KG edges created: {}", report.kg_edges_created);
    }

    // ── Step 6: EndSession ─────────────────────────────────────────────────────
    let end_req = ApiRequest::EndSession {
        agent_id: agent_id.to_string(),
        session_id: session_id.clone(),
        auto_checkpoint: true,
    };

    let end_response = check_response(kernel.handle_api_request(end_req), "EndSession")?;

    if let Some(ended) = end_response.session_ended {
        println!("\n[Session {}] Ended. Last seq: {}", session_id, ended.last_seq);
        if let Some(cp_id) = ended.checkpoint_id {
            println!("  Checkpoint created: {}", cp_id);
        }
    }

    Ok(KnowledgeResponse {
        session_id,
        hybrid_result,
        shared_memories_count: shared_memories.len(),
        query: query.to_string(),
    })
}

/// Response from a knowledge query.
pub struct KnowledgeResponse {
    pub session_id: String,
    pub hybrid_result: HybridResult,
    pub shared_memories_count: usize,
    pub query: String,
}

/// Extract KG seed tags from the query (simplified - production would use NER/LLM).
fn extract_seed_tags(query: &str) -> Vec<String> {
    let lowercase = query.to_lowercase();
    let mut tags = Vec::new();

    if lowercase.contains("sql") || lowercase.contains("injection") {
        tags.push("sql-injection".to_string());
    }
    if lowercase.contains("xss") || lowercase.contains("cross-site") {
        tags.push("xss".to_string());
    }
    if lowercase.contains("csrf") {
        tags.push("csrf".to_string());
    }
    if lowercase.contains("authentication") || lowercase.contains("auth") {
        tags.push("authentication".to_string());
    }
    if lowercase.contains("encryption") || lowercase.contains("crypto") {
        tags.push("encryption".to_string());
    }
    if lowercase.contains("vulnerability") || lowercase.contains("cve") {
        tags.push("vulnerability".to_string());
    }
    if lowercase.contains("defense") || lowercase.contains("protect") || lowercase.contains("mitigation") {
        tags.push("defense".to_string());
    }
    if lowercase.contains("attack") || lowercase.contains("threat") {
        tags.push("attack".to_string());
    }
    if lowercase.contains("web") || lowercase.contains("http") || lowercase.contains("api") {
        tags.push("web-security".to_string());
    }

    // If no specific tags found, use a general tag
    if tags.is_empty() {
        tags.push("security".to_string());
    }

    tags
}

/// Demonstrate knowledge sharing between agents.
///
/// Shows how agents can share insights through the shared memory scope.
pub fn demonstrate_knowledge_sharing(root: &Path, agent_a: &str, agent_b: &str) -> Result<(), String> {
    let kernel = AIKernel::new(root.to_path_buf())
        .map_err(|e| format!("Failed to initialize kernel: {}", e))?;

    println!("\n=== Knowledge Sharing Demo ===");
    println!("Agent A: {} | Agent B: {}", agent_a, agent_b);

    // Agent A stores a shared insight
    let a_store = ApiRequest::RememberLongTerm {
        agent_id: agent_a.to_string(),
        content: format!(
            "Key insight from {}: SQL injection can be prevented by using parameterized queries. \
            WAF alone is not sufficient protection.",
            agent_a
        ),
        tags: vec!["sql-injection".to_string(), "prevention".to_string(), "insight".to_string()],
        importance: 90,
        scope: Some("shared".to_string()),
        tenant_id: None,
    };

    check_response(kernel.handle_api_request(a_store), "Agent A RememberLongTerm")?;
    println!("[Agent A] Stored shared insight about SQL injection prevention");

    // Agent B queries the shared knowledge
    let b_query = ApiRequest::RecallVisible {
        agent_id: agent_b.to_string(),
        groups: vec![],
    };

    let b_response = check_response(kernel.handle_api_request(b_query), "Agent B RecallVisible")?;
    let memories_found = b_response.memory.map(|m| m.len()).unwrap_or(0);
    println!("[Agent B] Found {} shared memories", memories_found);

    println!("[Agent B] Found shared memories from other agents");

    Ok(())
}

/// Demonstrate growth tracking over multiple sessions.
///
/// Shows how the GrowthReport reflects agent learning and efficiency improvements.
pub fn demonstrate_growth_tracking(root: &Path, agent_id: &str) -> Result<(), String> {
    let kernel = AIKernel::new(root.to_path_buf())
        .map_err(|e| format!("Failed to initialize kernel: {}", e))?;

    println!("\n=== Growth Tracking Demo ===");
    println!("Agent: {}", agent_id);

    // Query growth reports for different time periods
    for period in &[GrowthPeriod::Last7Days, GrowthPeriod::Last30Days, GrowthPeriod::AllTime] {
        let req = ApiRequest::QueryGrowthReport {
            agent_id: agent_id.to_string(),
            period: *period,
        };

        let resp = check_response(kernel.handle_api_request(req), "QueryGrowthReport")?;

        if let Some(report) = resp.growth_report {
            println!("\n[{:?}]", period);
            println!("  Sessions: {}", report.sessions_total);
            println!("  Token efficiency (last5/first5): {:.3}", report.token_efficiency_ratio);
            println!("  Cache hit rate: {:.2}", report.intent_cache_hit_rate);
            println!("  Memories: {} stored, {} shared", report.memories_stored, report.memories_shared);
            println!("  KG: {} nodes, {} edges", report.kg_nodes_created, report.kg_edges_created);
            println!("  Procedures learned: {}", report.procedures_learned);
        }
    }

    Ok(())
}

/// Demonstrate declarative intent and context prefetch.
///
/// Shows how DeclareIntent + FetchAssembledContext enables proactive context warming.
pub fn demonstrate_intent_prefetch(root: &Path, agent_id: &str, intent: &str) -> Result<String, String> {
    let kernel = AIKernel::new(root.to_path_buf())
        .map_err(|e| format!("Failed to initialize kernel: {}", e))?;

    println!("\n=== Intent Prefetch Demo ===");
    println!("Intent: {}", intent);

    // Declare intent to trigger background prefetch
    let declare_req = ApiRequest::DeclareIntent {
        agent_id: agent_id.to_string(),
        intent: intent.to_string(),
        related_cids: vec![],
        budget_tokens: 4096,
    };

    let declare_resp = check_response(kernel.handle_api_request(declare_req), "DeclareIntent")?;

    let assembly_id = declare_resp.assembly_id
        .ok_or("No assembly_id in response")?;
    println!("Assembly ID: {}", assembly_id);

    // In production, agent would do other work here while context is being assembled
    // For demo, we fetch immediately
    let fetch_req = ApiRequest::FetchAssembledContext {
        agent_id: agent_id.to_string(),
        assembly_id: assembly_id.clone(),
    };

    let fetch_resp = check_response(kernel.handle_api_request(fetch_req), "FetchAssembledContext")?;

    if let Some(ctx) = fetch_resp.context_data {
        let preview_len = 100.min(ctx.content.len());
        println!("Fetched context:");
        println!("  CID: {}", ctx.cid);
        println!("  Layer: {}", ctx.layer);
        println!("  Tokens: {}", ctx.tokens_estimate);
        println!("  Preview: {}...", &ctx.content[..preview_len]);
    }

    Ok(assembly_id)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 3 {
        println!(r#"Usage: {} <root> <query> [agent_id]

Examples:
  # Run a knowledge query
  {} /tmp/plico "How to prevent SQL injection attacks?" knowledge-agent-1

  # Demonstrate knowledge sharing between agents
  {} /tmp/plico --share agent-a agent-b

  # Demonstrate growth tracking
  {} /tmp/plico --growth knowledge-agent-1

  # Demonstrate intent prefetch
  {} /tmp/plico --prefetch "SQL injection defense patterns" knowledge-agent-1
"#, args[0], args[0], args[0], args[0], args[0]);
        std::process::exit(1);
    }

    let root = Path::new(&args[1]);

    match args.get(2).map(|s| s.as_str()) {
        Some("--share") => {
            if args.len() < 5 {
                eprintln!("--share requires agent-a and agent-b");
                std::process::exit(1);
            }
            let agent_a = &args[3];
            let agent_b = &args[4];
            // Note: In this demo both use proper agent IDs
            if let Err(e) = demonstrate_knowledge_sharing(root, agent_a, agent_b) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Some("--growth") => {
            let agent_id = args.get(3).map(|s| s.as_str()).unwrap_or("knowledge-agent");
            if let Err(e) = demonstrate_growth_tracking(root, agent_id) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Some("--prefetch") => {
            if args.len() < 4 {
                eprintln!("--prefetch requires an intent description");
                std::process::exit(1);
            }
            let intent = &args[3];
            let agent_id = args.get(4).map(|s| s.as_str()).unwrap_or("knowledge-agent");
            match demonstrate_intent_prefetch(root, agent_id, intent) {
                Ok(asm_id) => println!("\nPrefetch complete. Assembly ID: {}", asm_id),
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        _ => {
            let query = &args[2];
            let agent_id = args.get(3).map(|s| s.as_str()).unwrap_or("knowledge-agent");
            match run_knowledge_agent(root, query, agent_id) {
                Ok(resp) => {
                    println!("\n=== Query Complete ===");
                    println!("Query: {}", resp.query);
                    println!("Session: {}", resp.session_id);
                    println!("Results: {} items ({} vector, {} graph)",
                        resp.hybrid_result.items.len(),
                        resp.hybrid_result.vector_hits,
                        resp.hybrid_result.graph_hits);
                    println!("Shared memories consulted: {}", resp.shared_memories_count);
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }
}
