//! Knowledge Graph tool handlers.

use crate::fs::{KGNodeType, KGEdgeType};
use crate::kernel::AIKernel;
use crate::tool::ToolResult;
use serde_json::json;

pub(in crate::kernel) fn handle(kernel: &AIKernel, name: &str, params: &serde_json::Value, agent_id: &str) -> ToolResult {
    match name {
        "kg.add_node" => {
            let label = params.get("label").and_then(|v| v.as_str()).unwrap_or("");
            let type_str = params.get("type").and_then(|v| v.as_str()).unwrap_or("entity");
            let node_type = match type_str {
                "fact" => KGNodeType::Fact,
                "document" => KGNodeType::Document,
                "agent" => KGNodeType::Agent,
                "memory" => KGNodeType::Memory,
                _ => KGNodeType::Entity,
            };
            let props = params.get("properties").cloned().unwrap_or(serde_json::Value::Null);
            match kernel.kg_add_node(label, node_type, props, agent_id, "default") {
                Ok(id) => ToolResult::ok(json!({"node_id": id})),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        "kg.add_edge" => {
            let src = params.get("src").and_then(|v| v.as_str()).unwrap_or("");
            let dst = params.get("dst").and_then(|v| v.as_str()).unwrap_or("");
            let type_str = params.get("type").and_then(|v| v.as_str()).unwrap_or("related_to");
            let edge_type = parse_edge_type(type_str);
            let weight = params.get("weight").and_then(|v| v.as_f64()).map(|w| w as f32);
            match kernel.kg_add_edge(src, dst, edge_type, weight, agent_id, "default") {
                Ok(()) => ToolResult::ok(json!({"created": true})),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        "kg.explore" => {
            let cid = params.get("cid").and_then(|v| v.as_str()).unwrap_or("");
            let edge_type = params.get("edge_type").and_then(|v| v.as_str());
            let depth = params.get("depth").and_then(|v| v.as_u64()).unwrap_or(1) as u8;
            let raw = kernel.graph_explore_raw(cid, edge_type, depth.min(3));
            let neighbors: Vec<serde_json::Value> = raw.into_iter()
                .map(|(id, label, ntype, etype, auth)| json!({
                    "node_id": id, "label": label, "node_type": ntype,
                    "edge_type": etype, "authority_score": auth,
                }))
                .collect();
            ToolResult::ok(json!({"neighbors": neighbors}))
        }
        "kg.paths" => {
            let src = params.get("src").and_then(|v| v.as_str()).unwrap_or("");
            let dst = params.get("dst").and_then(|v| v.as_str()).unwrap_or("");
            let depth = params.get("depth").and_then(|v| v.as_u64()).unwrap_or(3) as u8;
            let paths = kernel.kg_find_paths(src, dst, depth.min(5));
            let dto: Vec<Vec<serde_json::Value>> = paths.into_iter()
                .map(|p| p.into_iter().map(|n| json!({"id": n.id, "label": n.label})).collect())
                .collect();
            ToolResult::ok(json!({"paths": dto}))
        }
        "kg.get_node" => {
            let node_id = params.get("node_id").and_then(|v| v.as_str()).unwrap_or("");
            match kernel.kg_get_node(node_id, agent_id, "default") {
                Ok(Some(n)) => ToolResult::ok(json!({
                    "id": n.id, "label": n.label, "node_type": format!("{:?}", n.node_type),
                    "properties": n.properties, "agent_id": n.agent_id, "created_at": n.created_at,
                })),
                Ok(None) => ToolResult::error(format!("node not found: {}", node_id)),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        "kg.list_edges" => {
            let node_id = params.get("node_id").and_then(|v| v.as_str());
            match kernel.kg_list_edges(agent_id, "default", node_id) {
                Ok(edges) => {
                    let dto: Vec<serde_json::Value> = edges.into_iter().map(|e| json!({
                        "src": e.src, "dst": e.dst, "edge_type": format!("{:?}", e.edge_type),
                        "weight": e.weight, "created_at": e.created_at,
                    })).collect();
                    ToolResult::ok(json!({"edges": dto}))
                }
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        "kg.remove_node" => {
            let node_id = params.get("node_id").and_then(|v| v.as_str()).unwrap_or("");
            match kernel.kg_remove_node(node_id, agent_id, "default") {
                Ok(()) => ToolResult::ok(json!({"removed": node_id})),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        "kg.remove_edge" => {
            let src = params.get("src").and_then(|v| v.as_str()).unwrap_or("");
            let dst = params.get("dst").and_then(|v| v.as_str()).unwrap_or("");
            let edge_type = params.get("type").and_then(|v| v.as_str()).map(parse_edge_type);
            match kernel.kg_remove_edge(src, dst, edge_type, agent_id, "default") {
                Ok(()) => ToolResult::ok(json!({"removed": true})),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        "kg.update_node" => {
            let node_id = params.get("node_id").and_then(|v| v.as_str()).unwrap_or("");
            let label = params.get("label").and_then(|v| v.as_str());
            let properties = params.get("properties").cloned();
            match kernel.kg_update_node(node_id, label, properties, agent_id, "default") {
                Ok(()) => ToolResult::ok(json!({"updated": node_id})),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        _ => ToolResult::error(format!("unknown KG tool: {}", name)),
    }
}

fn parse_edge_type(s: &str) -> KGEdgeType {
    match s {
        "associates_with" => KGEdgeType::AssociatesWith,
        "mentions" => KGEdgeType::Mentions,
        "follows" => KGEdgeType::Follows,
        "causes" => KGEdgeType::Causes,
        "part_of" => KGEdgeType::PartOf,
        "similar_to" => KGEdgeType::SimilarTo,
        _ => KGEdgeType::RelatedTo,
    }
}
