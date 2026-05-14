//! Knowledge Graph handlers.

use crate::api::semantic::{ApiRequest, ApiResponse, KGNodeDto, NeighborDto};
use crate::DEFAULT_TENANT;

impl super::super::AIKernel {
    pub(crate) fn handle_graph(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::Explore { cid, edge_type, depth, agent_id: _ } => {
                let depth = depth.unwrap_or(1).min(3);
                let raw = self.graph_explore_raw(&cid, edge_type.as_deref(), depth);
                let dto: Vec<NeighborDto> = raw.into_iter().map(|(node_id, label, node_type, edge_type, authority_score)| {
                    NeighborDto { node_id, label, node_type, edge_type, authority_score }
                }).collect();
                let mut r = ApiResponse::ok();
                r.neighbors = Some(dto);
                r
            }
            ApiRequest::AddNode { label, node_type, properties, agent_id, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                match self.kg_add_node(&label, node_type, properties, &agent_id, &tenant) {
                    Ok(id) => ApiResponse::with_node_id(id),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::AddEdge { src_id, dst_id, edge_type, weight, agent_id, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                match self.kg_add_edge(&src_id, &dst_id, edge_type, weight, &agent_id, &tenant) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::ListNodes { node_type, agent_id, tenant_id, limit, offset, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                let nodes = match self.kg_list_nodes(node_type, &agent_id, &tenant) {
                    Ok(n) => n,
                    Err(e) => return ApiResponse::error(e.to_string()),
                };
                let total = nodes.len();
                let off = offset.unwrap_or(0);
                let lim = limit.unwrap_or(total);
                let dto: Vec<KGNodeDto> = nodes.into_iter().skip(off).take(lim).map(|n| KGNodeDto {
                    id: n.id, label: n.label, node_type: n.node_type,
                    content_cid: n.content_cid, properties: n.properties,
                    agent_id: n.agent_id, created_at: n.created_at,
                }).collect();
                let mut r = ApiResponse::with_nodes(dto.clone());
                r.total_count = Some(total);
                r.has_more = Some(off + dto.len() < total);
                r
            }
            ApiRequest::ListNodesAtTime { node_type, agent_id, tenant_id, t, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                let nodes = match self.kg_get_valid_nodes_at(&agent_id, &tenant, node_type, t) {
                    Ok(n) => n,
                    Err(e) => return ApiResponse::error(e.to_string()),
                };
                let dto: Vec<KGNodeDto> = nodes.into_iter().map(|n| KGNodeDto {
                    id: n.id, label: n.label, node_type: n.node_type,
                    content_cid: n.content_cid, properties: n.properties,
                    agent_id: n.agent_id, created_at: n.created_at,
                }).collect();
                ApiResponse::with_nodes(dto)
            }
            ApiRequest::FindPaths { src_id, dst_id, max_depth, weighted, agent_id: _, .. } => {
                let depth = max_depth.unwrap_or(3).min(5);
                let dto: Vec<Vec<KGNodeDto>> = if weighted {
                    if let Some(path) = self.kg_find_weighted_path(&src_id, &dst_id, depth) {
                        vec![path.into_iter().map(|n| KGNodeDto {
                            id: n.id, label: n.label, node_type: n.node_type,
                            content_cid: n.content_cid, properties: n.properties,
                            agent_id: n.agent_id, created_at: n.created_at,
                        }).collect()]
                    } else {
                        vec![]
                    }
                } else {
                    let paths = self.kg_find_paths(&src_id, &dst_id, depth);
                    paths.into_iter().map(|path| {
                        path.into_iter().map(|n| KGNodeDto {
                            id: n.id, label: n.label, node_type: n.node_type,
                            content_cid: n.content_cid, properties: n.properties,
                            agent_id: n.agent_id, created_at: n.created_at,
                        }).collect()
                    }).collect()
                };
                ApiResponse::with_paths(dto)
            }
            ApiRequest::GetNode { node_id, agent_id, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                match self.kg_get_node(&node_id, &agent_id, &tenant) {
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
            ApiRequest::ListEdges { agent_id, node_id, tenant_id, limit, offset, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                match self.kg_list_edges(&agent_id, &tenant, node_id.as_deref()) {
                    Ok(edges) => {
                        let total = edges.len();
                        let off = offset.unwrap_or(0);
                        let lim = limit.unwrap_or(total);
                        let dto: Vec<crate::api::semantic::KGEdgeDto> = edges.into_iter().skip(off).take(lim).map(|e| {
                            crate::api::semantic::KGEdgeDto {
                                src: e.src, dst: e.dst, edge_type: e.edge_type,
                                weight: e.weight, created_at: e.created_at,
                            }
                        }).collect();
                        let mut r = ApiResponse::ok();
                        r.edges = Some(dto.clone());
                        r.total_count = Some(total);
                        r.has_more = Some(off + dto.len() < total);
                        r
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::RemoveNode { node_id, agent_id, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                match self.kg_remove_node(&node_id, &agent_id, &tenant) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::RemoveEdge { src_id, dst_id, edge_type, agent_id, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                match self.kg_remove_edge(&src_id, &dst_id, edge_type, &agent_id, &tenant) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::UpdateNode { node_id, label, properties, agent_id, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                match self.kg_update_node(&node_id, label.as_deref(), properties, &agent_id, &tenant) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::EdgeHistory { src_id, dst_id, edge_type, agent_id, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                match self.kg_edge_history(&src_id, &dst_id, edge_type, &agent_id, &tenant) {
                    Ok(edges) => {
                        let dtos: Vec<crate::api::semantic::KGEdgeDto> = edges.iter().map(|e| {
                            crate::api::semantic::KGEdgeDto {
                                src: e.src.clone(), dst: e.dst.clone(), edge_type: e.edge_type,
                                weight: e.weight, created_at: e.created_at,
                            }
                        }).collect();
                        let mut r = ApiResponse::ok();
                        r.edges = Some(dtos);
                        r
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::KGCausalPath { source_id, target_id, max_depth, agent_id: _, tenant_id: _ } => {
                let depth = max_depth.clamp(1, 5);
                let paths = self.kg_find_causal_path(&source_id, &target_id, depth);
                let dtos: Vec<crate::api::semantic::CausalPathDto> = paths.into_iter().map(|p| {
                    crate::api::semantic::CausalPathDto {
                        nodes: p.nodes.into_iter().map(|n| KGNodeDto {
                            id: n.id, label: n.label, node_type: n.node_type,
                            content_cid: n.content_cid, properties: n.properties,
                            agent_id: n.agent_id, created_at: n.created_at,
                        }).collect(),
                        edges: p.edges.into_iter().map(|e| crate::api::semantic::KGEdgeDto {
                            src: e.src, dst: e.dst, edge_type: e.edge_type,
                            weight: e.weight, created_at: e.created_at,
                        }).collect(),
                        causal_strength: p.causal_strength,
                    }
                }).collect();
                let mut r = ApiResponse::ok();
                r.causal_paths = Some(dtos);
                r
            }
            ApiRequest::KGImpactAnalysis { node_id, propagation_depth, agent_id, tenant_id } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                let depth = propagation_depth.clamp(1, 5);
                if let Ok(Some(node)) = self.kg_get_node(&node_id, &agent_id, &tenant) {
                    let ctx = crate::api::permission::PermissionContext::new(agent_id.clone(), tenant);
                    if let Err(e) = self.permissions.check_tenant_access(&ctx, &node.tenant_id) {
                        return ApiResponse::error(e.to_string());
                    }
                }
                let impact = self.kg_impact_analysis(&node_id, depth);
                let mut r = ApiResponse::ok();
                r.impact_analysis = Some(crate::api::semantic::ImpactAnalysisDto {
                    affected_nodes: impact.affected_nodes,
                    propagation_depth: impact.propagation_depth,
                    severity: impact.severity,
                });
                r
            }
            ApiRequest::KGTemporalChanges { from_ms, to_ms, agent_id, tenant_id } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                match self.kg_temporal_changes(from_ms, to_ms, &agent_id, &tenant) {
                    Ok(changes) => {
                        let dtos: Vec<crate::api::semantic::TemporalChangeDto> = changes.into_iter().map(|c| {
                            crate::api::semantic::TemporalChangeDto {
                                before: c.before.map(|n| KGNodeDto {
                                    id: n.id, label: n.label, node_type: n.node_type,
                                    content_cid: n.content_cid, properties: n.properties,
                                    agent_id: n.agent_id, created_at: n.created_at,
                                }),
                                after: c.after.map(|n| KGNodeDto {
                                    id: n.id, label: n.label, node_type: n.node_type,
                                    content_cid: n.content_cid, properties: n.properties,
                                    agent_id: n.agent_id, created_at: n.created_at,
                                }),
                                change_type: match c.change_type {
                                    crate::kernel::ops::graph::ChangeType::Created => "created".to_string(),
                                    crate::kernel::ops::graph::ChangeType::Modified => "modified".to_string(),
                                    crate::kernel::ops::graph::ChangeType::Deleted => "deleted".to_string(),
                                },
                                timestamp_ms: c.timestamp_ms,
                            }
                        }).collect();
                        let mut r = ApiResponse::ok();
                        r.temporal_changes = Some(dtos);
                        r
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            _ => unreachable!("non-graph request routed to handle_graph"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::kernel::tests::make_kernel;
    use crate::api::semantic::ApiRequest;
    use crate::fs::graph::{KGNodeType, KGEdgeType};

    #[test]
    fn test_add_node_and_get_node() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::AddNode {
            label: "test_entity".to_string(),
            node_type: KGNodeType::Entity,
            properties: serde_json::json!({"key": "value"}),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        };
        let resp = kernel.handle_api_request(req);
        assert!(resp.ok, "AddNode should succeed: {:?}", resp.error);
        let node_id = resp.node_id.clone().expect("should return node_id");

        let req = ApiRequest::GetNode {
            node_id,
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        };
        let resp = kernel.handle_api_request(req);
        assert!(resp.ok, "GetNode should succeed: {:?}", resp.error);
        assert!(resp.nodes.is_some());
        let nodes = resp.nodes.unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].label, "test_entity");
        assert_eq!(nodes[0].node_type, KGNodeType::Entity);
    }

    #[test]
    fn test_get_node_not_found() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::GetNode {
            node_id: "nonexistent_id".to_string(),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        };
        let resp = kernel.handle_api_request(req);
        assert!(!resp.ok, "GetNode for missing node should fail");
        assert!(resp.error.unwrap().contains("not found"));
    }

    #[test]
    fn test_add_edge_and_list_edges() {
        let (kernel, _dir) = make_kernel();
        // Create two nodes
        let resp1 = kernel.handle_api_request(ApiRequest::AddNode {
            label: "node_a".to_string(),
            node_type: KGNodeType::Entity,
            properties: serde_json::json!({}),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        let id_a = resp1.node_id.unwrap();
        let resp2 = kernel.handle_api_request(ApiRequest::AddNode {
            label: "node_b".to_string(),
            node_type: KGNodeType::Entity,
            properties: serde_json::json!({}),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        let id_b = resp2.node_id.unwrap();

        // Add edge
        let resp = kernel.handle_api_request(ApiRequest::AddEdge {
            src_id: id_a.clone(),
            dst_id: id_b.clone(),
            edge_type: KGEdgeType::RelatedTo,
            weight: Some(0.8),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        assert!(resp.ok, "AddEdge should succeed: {:?}", resp.error);

        // List edges
        let resp = kernel.handle_api_request(ApiRequest::ListEdges {
            agent_id: "test_agent".to_string(),
            tenant_id: None,
            node_id: None,
            limit: None,
            offset: None,
        });
        assert!(resp.ok, "ListEdges should succeed: {:?}", resp.error);
        let edges = resp.edges.unwrap();
        assert!(!edges.is_empty());
    }

    #[test]
    fn test_list_nodes_with_limit() {
        let (kernel, _dir) = make_kernel();
        // Create multiple nodes
        for i in 0..3 {
            kernel.handle_api_request(ApiRequest::AddNode {
                label: format!("node_{}", i),
                node_type: KGNodeType::Entity,
                properties: serde_json::json!({}),
                agent_id: "test_agent".to_string(),
                tenant_id: None,
            });
        }

        let resp = kernel.handle_api_request(ApiRequest::ListNodes {
            node_type: None,
            agent_id: "test_agent".to_string(),
            tenant_id: None,
            limit: Some(2),
            offset: None,
        });
        assert!(resp.ok, "ListNodes should succeed: {:?}", resp.error);
        let nodes = resp.nodes.unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(resp.total_count, Some(3));
        assert_eq!(resp.has_more, Some(true));
    }

    #[test]
    fn test_list_nodes_with_type_filter() {
        let (kernel, _dir) = make_kernel();
        kernel.handle_api_request(ApiRequest::AddNode {
            label: "entity1".to_string(),
            node_type: KGNodeType::Entity,
            properties: serde_json::json!({}),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        kernel.handle_api_request(ApiRequest::AddNode {
            label: "fact1".to_string(),
            node_type: KGNodeType::Fact,
            properties: serde_json::json!({}),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });

        let resp = kernel.handle_api_request(ApiRequest::ListNodes {
            node_type: Some(KGNodeType::Fact),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
            limit: None,
            offset: None,
        });
        assert!(resp.ok);
        let nodes = resp.nodes.unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].label, "fact1");
    }

    #[test]
    fn test_remove_node() {
        use crate::api::permission::PermissionAction;
        let (kernel, _dir) = make_kernel();
        kernel.permission_grant("test_agent", PermissionAction::Delete, None, None);
        let resp = kernel.handle_api_request(ApiRequest::AddNode {
            label: "to_delete".to_string(),
            node_type: KGNodeType::Entity,
            properties: serde_json::json!({}),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        let node_id = resp.node_id.unwrap();

        let resp = kernel.handle_api_request(ApiRequest::RemoveNode {
            node_id: node_id.clone(),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        assert!(resp.ok, "RemoveNode should succeed: {:?}", resp.error);

        // Verify removed
        let resp = kernel.handle_api_request(ApiRequest::GetNode {
            node_id,
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        assert!(!resp.ok, "GetNode after delete should fail");
    }

    #[test]
    fn test_update_node() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::AddNode {
            label: "original_label".to_string(),
            node_type: KGNodeType::Entity,
            properties: serde_json::json!({"a": 1}),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        let node_id = resp.node_id.unwrap();

        let resp = kernel.handle_api_request(ApiRequest::UpdateNode {
            node_id: node_id.clone(),
            label: Some("updated_label".to_string()),
            properties: Some(serde_json::json!({"b": 2})),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        assert!(resp.ok, "UpdateNode should succeed: {:?}", resp.error);

        let resp = kernel.handle_api_request(ApiRequest::GetNode {
            node_id,
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        assert!(resp.ok);
        let nodes = resp.nodes.unwrap();
        assert_eq!(nodes[0].label, "updated_label");
    }

    #[test]
    fn test_remove_edge() {
        use crate::api::permission::PermissionAction;
        let (kernel, _dir) = make_kernel();
        kernel.permission_grant("test_agent", PermissionAction::Delete, None, None);
        let r1 = kernel.handle_api_request(ApiRequest::AddNode {
            label: "ea".to_string(),
            node_type: KGNodeType::Entity,
            properties: serde_json::json!({}),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        let r2 = kernel.handle_api_request(ApiRequest::AddNode {
            label: "eb".to_string(),
            node_type: KGNodeType::Entity,
            properties: serde_json::json!({}),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        let (id_a, id_b) = (r1.node_id.unwrap(), r2.node_id.unwrap());

        kernel.handle_api_request(ApiRequest::AddEdge {
            src_id: id_a.clone(),
            dst_id: id_b.clone(),
            edge_type: KGEdgeType::Causes,
            weight: None,
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });

        let resp = kernel.handle_api_request(ApiRequest::RemoveEdge {
            src_id: id_a,
            dst_id: id_b,
            edge_type: Some(KGEdgeType::Causes),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        assert!(resp.ok, "RemoveEdge should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_explore_neighbors() {
        let (kernel, _dir) = make_kernel();
        let r1 = kernel.handle_api_request(ApiRequest::AddNode {
            label: "center".to_string(),
            node_type: KGNodeType::Entity,
            properties: serde_json::json!({}),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        let r2 = kernel.handle_api_request(ApiRequest::AddNode {
            label: "neighbor".to_string(),
            node_type: KGNodeType::Entity,
            properties: serde_json::json!({}),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        let (center_id, neighbor_id) = (r1.node_id.unwrap(), r2.node_id.unwrap());

        kernel.handle_api_request(ApiRequest::AddEdge {
            src_id: center_id.clone(),
            dst_id: neighbor_id,
            edge_type: KGEdgeType::AssociatesWith,
            weight: None,
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });

        let resp = kernel.handle_api_request(ApiRequest::Explore {
            cid: center_id,
            edge_type: None,
            depth: Some(1),
            agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok, "Explore should succeed: {:?}", resp.error);
        let neighbors = resp.neighbors.unwrap();
        assert!(!neighbors.is_empty());
    }

    #[test]
    fn test_find_paths() {
        let (kernel, _dir) = make_kernel();
        let r1 = kernel.handle_api_request(ApiRequest::AddNode {
            label: "path_start".to_string(),
            node_type: KGNodeType::Entity,
            properties: serde_json::json!({}),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        let r2 = kernel.handle_api_request(ApiRequest::AddNode {
            label: "path_mid".to_string(),
            node_type: KGNodeType::Entity,
            properties: serde_json::json!({}),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        let r3 = kernel.handle_api_request(ApiRequest::AddNode {
            label: "path_end".to_string(),
            node_type: KGNodeType::Entity,
            properties: serde_json::json!({}),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        let (n1, n2, n3) = (r1.node_id.unwrap(), r2.node_id.unwrap(), r3.node_id.unwrap());

        kernel.handle_api_request(ApiRequest::AddEdge {
            src_id: n1.clone(), dst_id: n2.clone(),
            edge_type: KGEdgeType::Follows, weight: None,
            agent_id: "test_agent".to_string(), tenant_id: None,
        });
        kernel.handle_api_request(ApiRequest::AddEdge {
            src_id: n2, dst_id: n3.clone(),
            edge_type: KGEdgeType::Follows, weight: None,
            agent_id: "test_agent".to_string(), tenant_id: None,
        });

        let resp = kernel.handle_api_request(ApiRequest::FindPaths {
            src_id: n1,
            dst_id: n3,
            max_depth: Some(3),
            weighted: false,
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        assert!(resp.ok, "FindPaths should succeed: {:?}", resp.error);
        let paths = resp.paths.unwrap();
        assert!(!paths.is_empty());
    }

    #[test]
    fn test_kg_impact_analysis() {
        let (kernel, _dir) = make_kernel();
        let r1 = kernel.handle_api_request(ApiRequest::AddNode {
            label: "impact_src".to_string(),
            node_type: KGNodeType::Entity,
            properties: serde_json::json!({}),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        let r2 = kernel.handle_api_request(ApiRequest::AddNode {
            label: "impact_dst".to_string(),
            node_type: KGNodeType::Entity,
            properties: serde_json::json!({}),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        let (src, dst) = (r1.node_id.unwrap(), r2.node_id.unwrap());
        kernel.handle_api_request(ApiRequest::AddEdge {
            src_id: src.clone(), dst_id: dst,
            edge_type: KGEdgeType::Causes, weight: None,
            agent_id: "test_agent".to_string(), tenant_id: None,
        });

        let resp = kernel.handle_api_request(ApiRequest::KGImpactAnalysis {
            node_id: src,
            propagation_depth: 2,
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        assert!(resp.ok, "KGImpactAnalysis should succeed: {:?}", resp.error);
        assert!(resp.impact_analysis.is_some());
    }
}
