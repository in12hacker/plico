//! Knowledge graph / KG commands.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiResponse, NeighborDto, KGNodeDto, KGEdgeDto};
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

    let dto: Vec<NeighborDto> = neighbors.into_iter().map(|(node_id, label, node_type, edge_str, auth)| {
        NeighborDto { node_id, label, node_type, edge_type: edge_str, authority_score: auth }
    }).collect();
    let mut r = ApiResponse::ok();
    r.neighbors = Some(dto);
    r
}

pub fn cmd_add_node(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let label = extract_arg(args, "--label").unwrap_or_default();
    let node_type = parse_node_type(&extract_arg(args, "--type").unwrap_or_else(|| "entity".to_string()));
    let props = extract_arg(args, "--props")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.kg_add_node(&label, node_type, props, &agent_id, "default") {
        Ok(id) => ApiResponse::with_node_id(id),
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_add_edge(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let src = extract_arg(args, "--src").unwrap_or_default();
    let dst = extract_arg(args, "--dst").unwrap_or_default();
    let edge_type = parse_edge_type(&extract_arg(args, "--type").unwrap_or_else(|| "related_to".to_string()));
    let weight = extract_arg(args, "--weight").and_then(|s| s.parse().ok());
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.kg_add_edge(&src, &dst, edge_type, weight, &agent_id, "default") {
        Ok(()) => ApiResponse::ok_with_message(format!("Edge created: {} --[{:?}]--> {}", src, edge_type, dst)),
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_list_nodes(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let node_type = extract_arg(args, "--type").map(|s| parse_node_type(&s));
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    let nodes = if let Some(at_str) = extract_arg(args, "--at-time") {
        let t: u64 = at_str.parse().unwrap_or(0);
        match kernel.kg_get_valid_nodes_at(&agent_id, "default", node_type, t) {
            Ok(n) => n,
            Err(e) => return ApiResponse::error(e.to_string()),
        }
    } else {
        match kernel.kg_list_nodes(node_type, &agent_id, "default") {
            Ok(n) => n,
            Err(e) => return ApiResponse::error(e.to_string()),
        }
    };
    let dto: Vec<KGNodeDto> = nodes.into_iter().map(|n| KGNodeDto {
        id: n.id, label: n.label, node_type: n.node_type,
        content_cid: n.content_cid, properties: n.properties,
        agent_id: n.agent_id, created_at: n.created_at,
    }).collect();
    ApiResponse::with_nodes(dto)
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
        match kernel.kg_find_weighted_path(&src, &dst, depth) {
            Some(path) => {
                let dto: Vec<KGNodeDto> = path.into_iter().map(|n| KGNodeDto {
                    id: n.id, label: n.label, node_type: n.node_type,
                    content_cid: n.content_cid, properties: n.properties,
                    agent_id: n.agent_id, created_at: n.created_at,
                }).collect();
                ApiResponse::with_paths(vec![dto])
            }
            None => ApiResponse::with_paths(vec![]),
        }
    } else {
        let paths = kernel.kg_find_paths(&src, &dst, depth);
        let dto: Vec<Vec<KGNodeDto>> = paths.into_iter().map(|path| {
            path.into_iter().map(|n| KGNodeDto {
                id: n.id, label: n.label, node_type: n.node_type,
                content_cid: n.content_cid, properties: n.properties,
                agent_id: n.agent_id, created_at: n.created_at,
            }).collect()
        }).collect();
        ApiResponse::with_paths(dto)
    }
}

pub fn cmd_get_node(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let node_id = extract_arg(args, "--id").unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.kg_get_node(&node_id, &agent_id, "default") {
        Ok(Some(n)) => {
            let dto = KGNodeDto {
                id: n.id, label: n.label, node_type: n.node_type,
                content_cid: n.content_cid, properties: n.properties,
                agent_id: n.agent_id, created_at: n.created_at,
            };
            ApiResponse::with_nodes(vec![dto])
        }
        Ok(None) => ApiResponse::error(format!("node not found: {}", node_id)),
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_list_edges(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let node_id = extract_arg(args, "--node");

    match kernel.kg_list_edges(&agent_id, "default", node_id.as_deref()) {
        Ok(edges) => {
            let dto: Vec<KGEdgeDto> = edges.into_iter().map(|e| KGEdgeDto {
                src: e.src, dst: e.dst, edge_type: e.edge_type,
                weight: e.weight, created_at: e.created_at,
            }).collect();
            let mut r = ApiResponse::ok();
            r.edges = Some(dto);
            r
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_rm_node(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let node_id = extract_arg(args, "--id").unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.kg_remove_node(&node_id, &agent_id, "default") {
        Ok(()) => ApiResponse::ok_with_message(format!("Node removed: {}", node_id)),
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_rm_edge(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let src = extract_arg(args, "--src").unwrap_or_default();
    let dst = extract_arg(args, "--dst").unwrap_or_default();
    let edge_type = extract_arg(args, "--type").map(|s| parse_edge_type(&s));
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.kg_remove_edge(&src, &dst, edge_type, &agent_id, "default") {
        Ok(()) => ApiResponse::ok_with_message(format!("Edge removed: {} --> {}", src, dst)),
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_update_node(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let node_id = extract_arg(args, "--id").unwrap_or_default();
    let label = extract_arg(args, "--label");
    let properties = extract_arg(args, "--props")
        .and_then(|s| serde_json::from_str(&s).ok());
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.kg_update_node(&node_id, label.as_deref(), properties, &agent_id, "default") {
        Ok(()) => ApiResponse::ok_with_message(format!("Node updated: {}", node_id)),
        Err(e) => ApiResponse::error(e.to_string()),
    }
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
        "has_participant" => KGEdgeType::HasParticipant,
        "has_artifact" => KGEdgeType::HasArtifact,
        "has_recording" => KGEdgeType::HasRecording,
        "has_resolution" => KGEdgeType::HasResolution,
        "has_fact" => KGEdgeType::HasFact,
        "supersedes" => KGEdgeType::Supersedes,
        _ => KGEdgeType::RelatedTo,
    }
}

pub fn cmd_edge_history(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let src = extract_arg(args, "--src").unwrap_or_default();
    let dst = extract_arg(args, "--dst").unwrap_or_default();
    let edge_type = extract_arg(args, "--edge-type").map(|s| parse_edge_type(&s));
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    if src.is_empty() || dst.is_empty() {
        return ApiResponse::error("Missing --src or --dst argument");
    }

    match kernel.kg_edge_history(&src, &dst, edge_type, &agent_id, "default") {
        Ok(edges) => {
            let dtos: Vec<KGEdgeDto> = edges.iter().map(|e| {
                KGEdgeDto {
                    src: e.src.clone(),
                    dst: e.dst.clone(),
                    edge_type: e.edge_type,
                    weight: e.weight,
                    created_at: e.created_at,
                }
            }).collect();
            let mut r = ApiResponse::ok();
            r.edges = Some(dtos);
            r
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}
