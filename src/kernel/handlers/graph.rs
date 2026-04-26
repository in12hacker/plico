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
