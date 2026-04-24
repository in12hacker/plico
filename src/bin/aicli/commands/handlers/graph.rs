//! Knowledge graph commands — all operations route through handle_api_request.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse};
use plico::fs::{KGNodeType, KGEdgeType};
use super::extract_arg;

pub fn cmd_explore(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_default();
    let edge_type = extract_arg(args, "--edge-type");
    let depth: u8 = extract_arg(args, "--depth")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_u8)
        .min(3);

    kernel.handle_api_request(ApiRequest::Explore {
        cid,
        edge_type,
        depth: Some(depth),
        agent_id: extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string()),
    })
}

pub fn cmd_add_node(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let label = extract_arg(args, "--label").unwrap_or_default();
    let node_type = parse_node_type(&extract_arg(args, "--type").unwrap_or_else(|| "entity".to_string()));
    let props = extract_arg(args, "--props")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    kernel.handle_api_request(ApiRequest::AddNode {
        label, node_type, properties: props, agent_id, tenant_id: None,
    })
}

pub fn cmd_add_edge(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let src_id = extract_arg(args, "--src")
        .or_else(|| extract_arg(args, "--from"))
        .unwrap_or_default();
    let dst_id = extract_arg(args, "--dst")
        .or_else(|| extract_arg(args, "--to"))
        .unwrap_or_default();
    let edge_type_str = extract_arg(args, "--type").unwrap_or_else(|| "related_to".to_string());
    let edge_type = match parse_edge_type(&edge_type_str) {
        Ok(t) => t,
        Err(e) => return ApiResponse::error(e),
    };
    let weight = extract_arg(args, "--weight").and_then(|s| s.parse().ok());
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    kernel.handle_api_request(ApiRequest::AddEdge {
        src_id, dst_id, edge_type, weight, agent_id, tenant_id: None,
    })
}

pub fn cmd_list_nodes(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let node_type = extract_arg(args, "--type").map(|s| parse_node_type(&s));
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    if let Some(at_str) = extract_arg(args, "--at-time") {
        let t: u64 = at_str.parse().unwrap_or(0);
        kernel.handle_api_request(ApiRequest::ListNodesAtTime {
            node_type, agent_id, tenant_id: None, t,
        })
    } else {
        kernel.handle_api_request(ApiRequest::ListNodes {
            node_type, agent_id, tenant_id: None, limit: None, offset: None,
        })
    }
}

pub fn cmd_find_paths(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let src_id = extract_arg(args, "--src").unwrap_or_default();
    let dst_id = extract_arg(args, "--dst").unwrap_or_default();
    let max_depth: u8 = extract_arg(args, "--depth")
        .and_then(|s| s.parse().ok())
        .unwrap_or(3)
        .min(5);
    let weighted = args.iter().any(|a| a == "--weighted");

    kernel.handle_api_request(ApiRequest::FindPaths {
        src_id, dst_id, max_depth: Some(max_depth), weighted, agent_id: "cli".to_string(), tenant_id: None,
    })
}

pub fn cmd_get_node(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let node_id = extract_arg(args, "--id").unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    kernel.handle_api_request(ApiRequest::GetNode {
        node_id, agent_id, tenant_id: None,
    })
}

pub fn cmd_list_edges(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let node_id = extract_arg(args, "--node");

    kernel.handle_api_request(ApiRequest::ListEdges {
        agent_id, tenant_id: None, node_id, limit: None, offset: None,
    })
}

pub fn cmd_rm_node(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let node_id = extract_arg(args, "--id").unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    kernel.handle_api_request(ApiRequest::RemoveNode {
        node_id, agent_id, tenant_id: None,
    })
}

pub fn cmd_rm_edge(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let src_id = extract_arg(args, "--src").unwrap_or_default();
    let dst_id = extract_arg(args, "--dst").unwrap_or_default();
    let edge_type: Option<KGEdgeType> = extract_arg(args, "--type")
        .and_then(|s| parse_edge_type(&s).ok());
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    kernel.handle_api_request(ApiRequest::RemoveEdge {
        src_id, dst_id, edge_type, agent_id, tenant_id: None,
    })
}

pub fn cmd_update_node(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let node_id = extract_arg(args, "--id").unwrap_or_default();
    let label = extract_arg(args, "--label");
    let properties = extract_arg(args, "--props")
        .and_then(|s| serde_json::from_str(&s).ok());
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    kernel.handle_api_request(ApiRequest::UpdateNode {
        node_id, label, properties, agent_id, tenant_id: None,
    })
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

pub fn parse_edge_type(s: &str) -> Result<KGEdgeType, String> {
    match s {
        "associates_with" => Ok(KGEdgeType::AssociatesWith),
        "follows" => Ok(KGEdgeType::Follows),
        "mentions" => Ok(KGEdgeType::Mentions),
        "causes" => Ok(KGEdgeType::Causes),
        "reminds" => Ok(KGEdgeType::Reminds),
        "part_of" => Ok(KGEdgeType::PartOf),
        "similar_to" => Ok(KGEdgeType::SimilarTo),
        "related_to" => Ok(KGEdgeType::RelatedTo),
        "has_participant" => Ok(KGEdgeType::HasParticipant),
        "has_artifact" => Ok(KGEdgeType::HasArtifact),
        "has_recording" => Ok(KGEdgeType::HasRecording),
        "has_resolution" => Ok(KGEdgeType::HasResolution),
        "has_fact" => Ok(KGEdgeType::HasFact),
        "supersedes" => Ok(KGEdgeType::Supersedes),
        _ => Err(format!(
            "Unknown edge type: '{}'. Valid: associates_with, follows, mentions, causes, \
             reminds, part_of, similar_to, related_to, has_participant, has_artifact, \
             has_recording, has_resolution, has_fact, supersedes",
            s
        )),
    }
}

pub fn cmd_edge_history(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let src_id = extract_arg(args, "--src").unwrap_or_default();
    let dst_id = extract_arg(args, "--dst").unwrap_or_default();
    let edge_type: Option<KGEdgeType> = extract_arg(args, "--edge-type")
        .and_then(|s| parse_edge_type(&s).ok());
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    if src_id.is_empty() || dst_id.is_empty() {
        return ApiResponse::error("Missing --src or --dst argument");
    }

    kernel.handle_api_request(ApiRequest::EdgeHistory {
        src_id, dst_id, edge_type, agent_id, tenant_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_kernel() -> plico::kernel::AIKernel {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("EMBEDDING_BACKEND", "stub");
        plico::kernel::AIKernel::new(dir.path().to_path_buf()).expect("kernel")
    }

    #[test]
    fn test_parse_edge_type_valid_causes() {
        let result = parse_edge_type("causes");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), KGEdgeType::Causes);
    }

    #[test]
    fn test_parse_edge_type_invalid_caused_by_returns_error() {
        let result = parse_edge_type("caused_by");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Unknown edge type: 'caused_by'"));
    }

    #[test]
    fn test_parse_edge_type_error_lists_valid_types() {
        let result = parse_edge_type("invalid_type");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Valid:"));
        assert!(err.contains("associates_with"));
        assert!(err.contains("related_to"));
    }

    #[test]
    fn test_cmd_add_edge_valid_type() {
        let kernel = make_test_kernel();
        let args = vec!["edge".to_string(), "--from".to_string(), "node1".to_string(),
                         "--to".to_string(), "node2".to_string(),
                         "--type".to_string(), "causes".to_string()];
        let response = cmd_add_edge(&kernel, &args);
        if !response.ok {
            let err_msg = response.error.as_deref().unwrap_or("");
            assert!(!err_msg.contains("Unknown edge type"),
                "Unexpected invalid edge type error: {}", err_msg);
        }
    }

    #[test]
    fn test_cmd_add_edge_invalid_type_returns_error() {
        let kernel = make_test_kernel();
        let args = vec!["edge".to_string(), "--from".to_string(), "node1".to_string(),
                         "--to".to_string(), "node2".to_string(),
                         "--type".to_string(), "caused_by".to_string()];
        let response = cmd_add_edge(&kernel, &args);
        assert!(!response.ok, "cmd_add_edge should fail for invalid type 'caused_by'");
        let err_msg = response.error.as_deref().unwrap_or("");
        assert!(err_msg.contains("Unknown edge type"),
            "Expected 'Unknown edge type' error, got: {}", err_msg);
    }
}
