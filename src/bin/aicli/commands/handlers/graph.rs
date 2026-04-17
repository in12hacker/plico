//! Knowledge graph / KG commands.

use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use plico::fs::{KGNodeType, KGEdgeType};
use super::extract_arg;

pub fn cmd_explore(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_default();
    let edge_type = extract_arg(args, "--edge-type");
    let depth: u8 = extract_arg(args, "--depth")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_u8)
        .min(3);

    let neighbors = kernel.graph_explore_raw(&cid, edge_type.as_deref(), depth);

    if neighbors.is_empty() {
        println!("No graph neighbors for: {}", cid);
    } else {
        println!("Graph neighbors of {} (depth {}):", cid, depth);
        for (i, (node_id, label, node_type, edge_str, auth)) in neighbors.iter().enumerate() {
            println!("{}. [auth={:.3}] {} ({}) {} \"{}\"", i + 1, auth, node_id, node_type, edge_str, label);
        }
    }
    ApiResponse::ok()
}

pub fn cmd_add_node(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let label = extract_arg(args, "--label").unwrap_or_default();
    let node_type = parse_node_type(&extract_arg(args, "--type").unwrap_or_else(|| "entity".to_string()));
    let props = extract_arg(args, "--props")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.kg_add_node(&label, node_type, props, &agent_id) {
        Ok(id) => {
            println!("Node created: {} (type={}, label=\"{}\")", id, node_type, label);
            ApiResponse::with_node_id(id)
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_add_edge(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let src = extract_arg(args, "--src").unwrap_or_default();
    let dst = extract_arg(args, "--dst").unwrap_or_default();
    let edge_type = parse_edge_type(&extract_arg(args, "--type").unwrap_or_else(|| "related_to".to_string()));
    let weight = extract_arg(args, "--weight").and_then(|s| s.parse().ok());
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.kg_add_edge(&src, &dst, edge_type, weight, &agent_id) {
        Ok(()) => {
            println!("Edge created: {} --[{}]--> {}", src, edge_type, dst);
            ApiResponse::ok()
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_list_nodes(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let node_type = extract_arg(args, "--type").map(|s| parse_node_type(&s));
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    let nodes = kernel.kg_list_nodes(node_type, &agent_id);
    if nodes.is_empty() {
        println!("No KG nodes found.");
    } else {
        println!("KG nodes ({} total):", nodes.len());
        for n in &nodes {
            println!("  {} [{}] \"{}\"", n.id, n.node_type, n.label);
        }
    }
    ApiResponse::ok()
}

pub fn cmd_find_paths(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let src = extract_arg(args, "--src").unwrap_or_default();
    let dst = extract_arg(args, "--dst").unwrap_or_default();
    let depth: u8 = extract_arg(args, "--depth")
        .and_then(|s| s.parse().ok())
        .unwrap_or(3)
        .min(5);
    let weighted = args.iter().any(|a| a == "--weighted");

    if weighted {
        // Find highest-weight path
        match kernel.kg_find_weighted_path(&src, &dst, depth) {
            Some(path) => {
                let labels: Vec<&str> = path.iter().map(|n| n.label.as_str()).collect();
                println!("Best path (weighted): {}", labels.join(" → "));
            }
            None => {
                println!("No path from {} to {} (depth {}, weighted)", src, dst, depth);
            }
        }
    } else {
        // Find all paths
        let paths = kernel.kg_find_paths(&src, &dst, depth);
        if paths.is_empty() {
            println!("No paths from {} to {} (depth {})", src, dst, depth);
        } else {
            println!("Paths from {} to {} ({} found):", src, dst, paths.len());
            for (i, path) in paths.iter().enumerate() {
                let labels: Vec<&str> = path.iter().map(|n| n.label.as_str()).collect();
                println!("  {}: {}", i + 1, labels.join(" → "));
            }
        }
    }
    ApiResponse::ok()
}

pub fn parse_node_type(s: &str) -> KGNodeType {
    match s {
        "entity" => KGNodeType::Entity,
        "fact" => KGNodeType::Fact,
        "document" => KGNodeType::Document,
        "agent" => KGNodeType::Agent,
        "memory" => KGNodeType::Memory,
        _ => KGNodeType::Entity,
    }
}

pub fn parse_edge_type(s: &str) -> KGEdgeType {
    match s {
        "associates_with" => KGEdgeType::AssociatesWith,
        "follows" => KGEdgeType::Follows,
        "mentions" => KGEdgeType::Mentions,
        "causes" => KGEdgeType::Causes,
        "reminds" => KGEdgeType::Reminds,
        "part_of" => KGEdgeType::PartOf,
        "similar_to" => KGEdgeType::SimilarTo,
        "related_to" => KGEdgeType::RelatedTo,
        "has_attendee" => KGEdgeType::HasAttendee,
        "has_document" => KGEdgeType::HasDocument,
        "has_media" => KGEdgeType::HasMedia,
        "has_decision" => KGEdgeType::HasDecision,
        "has_preference" => KGEdgeType::HasPreference,
        "suggests_action" => KGEdgeType::SuggestsAction,
        "motivated_by" => KGEdgeType::MotivatedBy,
        _ => KGEdgeType::RelatedTo,
    }
}
