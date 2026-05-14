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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::tests::make_kernel;

    #[test]
    fn test_kg_add_node_entity() {
        let (kernel, _tmp) = make_kernel();
        let params = json!({"label": "test_node", "type": "entity"});
        let result = handle(&*kernel, "kg.add_node", &params, "test");
        assert!(result.error.is_none(), "add_node should succeed: {:?}", result.error);
        let data = result.output;
        assert!(data.get("node_id").is_some());
    }

    #[test]
    fn test_kg_add_node_fact() {
        let (kernel, _tmp) = make_kernel();
        let params = json!({"label": "fact_node", "type": "fact"});
        let result = handle(&*kernel, "kg.add_node", &params, "test");
        assert!(result.error.is_none());
    }

    #[test]
    fn test_kg_add_edge() {
        let (kernel, _tmp) = make_kernel();
        let r1 = handle(&*kernel, "kg.add_node", &json!({"label": "a"}), "test");
        let r2 = handle(&*kernel, "kg.add_node", &json!({"label": "b"}), "test");
        let id_a = r1.output["node_id"].as_str().unwrap().to_string();
        let id_b = r2.output["node_id"].as_str().unwrap().to_string();

        let params = json!({"src": id_a, "dst": id_b, "type": "related_to"});
        let result = handle(&*kernel, "kg.add_edge", &params, "test");
        assert!(result.error.is_none(), "add_edge should succeed: {:?}", result.error);
    }

    #[test]
    fn test_kg_get_node() {
        let (kernel, _tmp) = make_kernel();
        let r1 = handle(&*kernel, "kg.add_node", &json!({"label": "my_node"}), "test");
        let node_id = r1.output["node_id"].as_str().unwrap().to_string();

        let result = handle(&*kernel, "kg.get_node", &json!({"node_id": node_id}), "test");
        assert!(result.error.is_none(), "get_node should succeed: {:?}", result.error);
        let data = result.output;
        assert_eq!(data["label"], "my_node");
    }

    #[test]
    fn test_kg_get_node_not_found() {
        let (kernel, _tmp) = make_kernel();
        let result = handle(&*kernel, "kg.get_node", &json!({"node_id": "nonexistent"}), "test");
        assert!(result.error.is_some());
    }

    #[test]
    fn test_kg_explore() {
        let (kernel, _tmp) = make_kernel();
        let r1 = handle(&*kernel, "kg.add_node", &json!({"label": "center"}), "test");
        let r2 = handle(&*kernel, "kg.add_node", &json!({"label": "neighbor"}), "test");
        let id1 = r1.output["node_id"].as_str().unwrap().to_string();
        let id2 = r2.output["node_id"].as_str().unwrap().to_string();
        handle(&*kernel, "kg.add_edge", &json!({"src": id1, "dst": id2}), "test");

        let result = handle(&*kernel, "kg.explore", &json!({"cid": id1}), "test");
        assert!(result.error.is_none());
        let neighbors = result.output["neighbors"].as_array().unwrap();
        assert!(!neighbors.is_empty());
    }

    #[test]
    fn test_kg_list_edges() {
        let (kernel, _tmp) = make_kernel();
        let r1 = handle(&*kernel, "kg.add_node", &json!({"label": "a"}), "test");
        let r2 = handle(&*kernel, "kg.add_node", &json!({"label": "b"}), "test");
        let id_a = r1.output["node_id"].as_str().unwrap().to_string();
        let id_b = r2.output["node_id"].as_str().unwrap().to_string();
        handle(&*kernel, "kg.add_edge", &json!({"src": id_a, "dst": id_b}), "test");

        let result = handle(&*kernel, "kg.list_edges", &json!({"node_id": id_a}), "test");
        assert!(result.error.is_none());
        let edges = result.output["edges"].as_array().unwrap();
        assert!(!edges.is_empty());
    }

    #[test]
    fn test_kg_remove_node() {
        use crate::api::permission::PermissionAction;
        let (kernel, _tmp) = make_kernel();
        kernel.permission_grant("test", PermissionAction::Delete, None, None);
        let r1 = handle(&*kernel, "kg.add_node", &json!({"label": "to_remove"}), "test");
        let node_id = r1.output["node_id"].as_str().unwrap().to_string();

        let result = handle(&*kernel, "kg.remove_node", &json!({"node_id": node_id}), "test");
        assert!(result.error.is_none(), "remove_node failed: {:?}", result.error);
    }

    #[test]
    fn test_kg_update_node() {
        let (kernel, _tmp) = make_kernel();
        let r1 = handle(&*kernel, "kg.add_node", &json!({"label": "original"}), "test");
        let node_id = r1.output["node_id"].as_str().unwrap().to_string();

        let result = handle(&*kernel, "kg.update_node", &json!({"node_id": node_id, "label": "updated"}), "test");
        assert!(result.error.is_none());

        let get_result = handle(&*kernel, "kg.get_node", &json!({"node_id": node_id}), "test");
        assert_eq!(get_result.output["label"], "updated");
    }

    #[test]
    fn test_kg_unknown_tool() {
        let (kernel, _tmp) = make_kernel();
        let result = handle(&*kernel, "kg.nonexistent", &json!({}), "test");
        assert!(result.error.is_some());
    }

    #[test]
    fn test_parse_edge_types() {
        assert!(matches!(parse_edge_type("associates_with"), KGEdgeType::AssociatesWith));
        assert!(matches!(parse_edge_type("mentions"), KGEdgeType::Mentions));
        assert!(matches!(parse_edge_type("follows"), KGEdgeType::Follows));
        assert!(matches!(parse_edge_type("causes"), KGEdgeType::Causes));
        assert!(matches!(parse_edge_type("part_of"), KGEdgeType::PartOf));
        assert!(matches!(parse_edge_type("similar_to"), KGEdgeType::SimilarTo));
        assert!(matches!(parse_edge_type("unknown"), KGEdgeType::RelatedTo));
    }
}
