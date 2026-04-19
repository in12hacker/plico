//! Knowledge graph operations — explore, node/edge CRUD.

use std::collections::HashSet;

use crate::fs::{KGNodeType, KGNode, KGEdgeType, KGEdge, KGSearchHit, KnowledgeGraph};
use super::observability::{OpType, OperationTimer};

// ── Causal Reasoning Types ───────────────────────────────────────────────────

/// Change type for temporal queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeType {
    Created,
    Modified,
    Deleted,
}

/// Causal path with strength score.
#[derive(Debug, Clone)]
pub struct CausalPath {
    pub nodes: Vec<KGNode>,
    pub edges: Vec<KGEdge>,
    pub causal_strength: f32,
}

/// Impact analysis result.
#[derive(Debug, Clone)]
pub struct ImpactResult {
    pub affected_nodes: Vec<String>,
    pub propagation_depth: u8,
    pub severity: f32,
}

/// Temporal change record.
#[derive(Debug, Clone)]
pub struct TemporalChange {
    pub before: Option<KGNode>,
    pub after: Option<KGNode>,
    pub change_type: ChangeType,
    pub timestamp_ms: u64,
}

impl crate::kernel::AIKernel {
    /// Explore graph neighbors of a CID at a given depth.
    pub fn graph_explore(&self, cid: &str, edge_type: Option<KGEdgeType>, depth: u8) -> Vec<KGSearchHit> {
        let Some(ref kg) = self.knowledge_graph else {
            return Vec::new();
        };
        let neighbors = match kg.get_neighbors(cid, edge_type, depth) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("graph_explore failed for {}: {}", cid, e);
                return Vec::new();
            }
        };
        neighbors
            .into_iter()
            .map(|(node, edge)| KGSearchHit {
                node,
                edge_type: Some(edge.edge_type),
                vector_score: 0.0,
                authority_score: kg.authority_score(cid).unwrap_or(0.0),
                combined_score: 0.0,
            })
            .collect()
    }

    /// Explore graph neighbors, returning plain strings (no fs types) for API layer.
    ///
    /// Returns `Vec<(node_id, label, node_type, edge_type, authority_score)>`.
    pub fn graph_explore_raw(
        &self,
        cid: &str,
        edge_type_str: Option<&str>,
        depth: u8,
    ) -> Vec<(String, String, String, String, f32)> {
        let edge_filter = edge_type_str.and_then(|s| match s {
            "associates_with" => Some(KGEdgeType::AssociatesWith),
            "mentions"        => Some(KGEdgeType::Mentions),
            "follows"         => Some(KGEdgeType::Follows),
            "part_of"         => Some(KGEdgeType::PartOf),
            "related_to"      => Some(KGEdgeType::RelatedTo),
            "similar_to"      => Some(KGEdgeType::SimilarTo),
            "causes"          => Some(KGEdgeType::Causes),
            "reminds"         => Some(KGEdgeType::Reminds),
            _ => None,
        });
        self.graph_explore(cid, edge_filter, depth)
            .into_iter()
            .map(|hit| {
                let node_type = format!("{:?}", hit.node.node_type).to_lowercase();
                let edge_type = hit.edge_type
                    .map(|et| format!("{:?}", et).to_lowercase())
                    .unwrap_or_default();
                (hit.node.id, hit.node.label, node_type, edge_type, hit.authority_score)
            })
            .collect()
    }

    /// Create an arbitrary KG node (Entity, Fact, Document, Agent, Memory).
    pub fn kg_add_node(
        &self,
        label: &str,
        node_type: KGNodeType,
        properties: serde_json::Value,
        agent_id: &str,
        tenant_id: &str,
    ) -> std::io::Result<String> {
        let _timer = OperationTimer::new(&self.metrics, OpType::KgAddNode);
        let span = tracing::info_span!(
            "kg_add_node",
            operation = "kg_add_node",
            label = %label,
            node_type = ?node_type,
            agent_id = %agent_id,
            tenant_id = %tenant_id,
        );
        let _guard = span.enter();

        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Write)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Err(std::io::Error::other("knowledge graph not available"));
        };
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp_millis() as u64;
        let node = KGNode {
            id: id.clone(),
            label: label.to_string(),
            node_type,
            content_cid: None,
            properties,
            agent_id: agent_id.to_string(),
            tenant_id: tenant_id.to_string(),
            created_at: now,
            valid_at: Some(now),
            invalid_at: None,
            expired_at: None,
        };
        kg.add_node(node)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        tracing::info!(node_id = %id, "KG node added");
        Ok(id)
    }

    /// Create an edge between two KG nodes.
    pub fn kg_add_edge(
        &self,
        src: &str,
        dst: &str,
        edge_type: KGEdgeType,
        weight: Option<f32>,
        agent_id: &str,
        tenant_id: &str,
    ) -> std::io::Result<()> {
        let _timer = OperationTimer::new(&self.metrics, OpType::KgAddEdge);
        let span = tracing::info_span!(
            "kg_add_edge",
            operation = "kg_add_edge",
            src = %src,
            dst = %dst,
            edge_type = ?edge_type,
            weight = ?weight,
            agent_id = %agent_id,
            tenant_id = %tenant_id,
        );
        let _guard = span.enter();

        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Write)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Err(std::io::Error::other("knowledge graph not available"));
        };
        // Verify tenant isolation for both nodes
        if let Ok(Some(src_node)) = kg.get_node(src) {
            self.permissions.check_tenant_access(&ctx, &src_node.tenant_id)?;
        }
        if let Ok(Some(dst_node)) = kg.get_node(dst) {
            self.permissions.check_tenant_access(&ctx, &dst_node.tenant_id)?;
        }
        let now = chrono::Utc::now().timestamp_millis() as u64;
        let edge = KGEdge {
            src: src.to_string(),
            dst: dst.to_string(),
            edge_type,
            weight: weight.unwrap_or(1.0),
            evidence_cid: None,
            created_at: now,
            valid_at: Some(now),
            invalid_at: None,
            expired_at: None,
            episode: None,
        };
        kg.add_edge(edge)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        tracing::info!(src = %src, dst = %dst, "KG edge added");
        Ok(())
    }

    /// List KG nodes, optionally filtered by type.
    /// Returns only nodes belonging to the caller's tenant.
    pub fn kg_list_nodes(
        &self,
        node_type: Option<KGNodeType>,
        agent_id: &str,
        tenant_id: &str,
    ) -> std::io::Result<Vec<KGNode>> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Read)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Ok(Vec::new());
        };
        kg.list_nodes(agent_id, node_type)
            .map_err(|e| std::io::Error::other(e.to_string()))
            .map(|mut nodes| {
                // Filter by tenant isolation
                nodes.retain(|n| n.tenant_id == tenant_id);
                nodes
            })
    }

    /// Find all paths between two KG nodes up to a given depth.
    pub fn kg_find_paths(
        &self,
        src: &str,
        dst: &str,
        max_depth: u8,
    ) -> Vec<Vec<KGNode>> {
        let _timer = OperationTimer::new(&self.metrics, OpType::KgFindPaths);
        let span = tracing::info_span!(
            "kg_find_paths",
            operation = "kg_find_paths",
            src = %src,
            dst = %dst,
            max_depth = %max_depth,
        );
        let _guard = span.enter();

        let Some(ref kg) = self.knowledge_graph else {
            return Vec::new();
        };
        let paths = kg.find_paths(src, dst, max_depth).unwrap_or_default();
        tracing::info!(path_count = paths.len(), "KG paths found");
        paths
    }

    /// Find the highest-weighted path between two KG nodes using best-first search.
    /// Returns `None` if no path exists within max_depth hops.
    pub fn kg_find_weighted_path(
        &self,
        src: &str,
        dst: &str,
        max_depth: u8,
    ) -> Option<Vec<KGNode>> {
        let kg = self.knowledge_graph.as_ref()?;
        kg.find_weighted_path(src, dst, max_depth).unwrap_or_default()
    }

    /// Query nodes valid at a specific point in time.
    ///
    /// Returns nodes where:
    /// - `valid_at <= t`
    /// - `invalid_at.is_none() || invalid_at > t`
    /// - `expired_at.is_none()` (soft-deleted nodes excluded)
    /// - tenant_id matches caller's tenant
    pub fn kg_get_valid_nodes_at(
        &self,
        agent_id: &str,
        tenant_id: &str,
        node_type: Option<KGNodeType>,
        t: u64,
    ) -> std::io::Result<Vec<KGNode>> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Read)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Ok(Vec::new());
        };
        kg.get_valid_nodes_at(agent_id, node_type, t)
            .map_err(|e| std::io::Error::other(e.to_string()))
            .map(|mut nodes| {
                // Filter by tenant isolation
                nodes.retain(|n| n.tenant_id == tenant_id);
                nodes
            })
    }

    /// Get a single KG node by ID.
    pub fn kg_get_node(&self, node_id: &str, agent_id: &str, tenant_id: &str) -> std::io::Result<Option<KGNode>> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Read)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Ok(None);
        };
        let node = kg.get_node(node_id).map_err(|e| std::io::Error::other(e.to_string()))?;
        // Check tenant isolation
        if let Some(ref n) = node {
            self.permissions.check_tenant_access(&ctx, &n.tenant_id)?;
        }
        Ok(node)
    }

    /// List edges, optionally filtered by a node they touch.
    /// Only returns edges where both source and destination nodes belong to the caller's tenant.
    pub fn kg_list_edges(&self, agent_id: &str, tenant_id: &str, node_id: Option<&str>) -> std::io::Result<Vec<KGEdge>> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Read)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Ok(Vec::new());
        };
        let edges = kg.list_edges(agent_id)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        // Filter edges by tenant isolation (both src and dst nodes must be in same tenant)
        let filtered: Vec<KGEdge> = edges.into_iter().filter(|e| {
            if let (Ok(Some(src_node)), Ok(Some(dst_node))) = (kg.get_node(&e.src), kg.get_node(&e.dst)) {
                src_node.tenant_id == tenant_id && dst_node.tenant_id == tenant_id
            } else {
                false
            }
        }).collect();
        if let Some(nid) = node_id {
            Ok(filtered.into_iter().filter(|e| e.src == nid || e.dst == nid).collect())
        } else {
            Ok(filtered)
        }
    }

    /// Remove an edge between two KG nodes.
    pub fn kg_remove_edge(
        &self,
        src: &str,
        dst: &str,
        edge_type: Option<KGEdgeType>,
        agent_id: &str,
        tenant_id: &str,
    ) -> std::io::Result<()> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Delete)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Err(std::io::Error::other("knowledge graph not available"));
        };
        // Verify tenant isolation for both nodes
        if let Ok(Some(src_node)) = kg.get_node(src) {
            self.permissions.check_tenant_access(&ctx, &src_node.tenant_id)?;
        }
        if let Ok(Some(dst_node)) = kg.get_node(dst) {
            self.permissions.check_tenant_access(&ctx, &dst_node.tenant_id)?;
        }
        kg.remove_edge(src, dst, edge_type)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    /// Update an existing KG node's label and/or properties.
    pub fn kg_update_node(
        &self,
        node_id: &str,
        label: Option<&str>,
        properties: Option<serde_json::Value>,
        agent_id: &str,
        tenant_id: &str,
    ) -> std::io::Result<()> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Write)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Err(std::io::Error::other("knowledge graph not available"));
        };
        // Check tenant isolation before update
        if let Ok(Some(node)) = kg.get_node(node_id) {
            self.permissions.check_tenant_access(&ctx, &node.tenant_id)?;
        }
        kg.update_node(node_id, label, properties)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    /// Remove a KG node and all its edges.
    pub fn kg_remove_node(&self, node_id: &str, agent_id: &str, tenant_id: &str) -> std::io::Result<()> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Delete)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Err(std::io::Error::other("knowledge graph not available"));
        };
        // Check tenant isolation before delete
        if let Ok(Some(node)) = kg.get_node(node_id) {
            self.permissions.check_tenant_access(&ctx, &node.tenant_id)?;
        }
        kg.remove_node(node_id)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    /// Get full edge history (including invalidated edges) between two nodes.
    pub fn kg_edge_history(
        &self,
        src: &str,
        dst: &str,
        edge_type: Option<KGEdgeType>,
        agent_id: &str,
        tenant_id: &str,
    ) -> std::io::Result<Vec<KGEdge>> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Read)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Ok(Vec::new());
        };
        // Check tenant isolation for both nodes
        if let Ok(Some(src_node)) = kg.get_node(src) {
            self.permissions.check_tenant_access(&ctx, &src_node.tenant_id)?;
        }
        if let Ok(Some(dst_node)) = kg.get_node(dst) {
            self.permissions.check_tenant_access(&ctx, &dst_node.tenant_id)?;
        }
        kg.edge_history(src, dst, edge_type)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    /// Find causal paths between two KG nodes up to a given depth.
    ///
    /// Unlike standard path finding which focuses on connectivity, causal path
    /// analysis weights paths by causal edge strength and favors edges of type
    /// `Causes` or similar causal relationships.
    pub fn kg_find_causal_path(
        &self,
        src: &str,
        dst: &str,
        max_depth: u8,
    ) -> Vec<CausalPath> {
        let _timer = OperationTimer::new(&self.metrics, OpType::KgFindPaths);
        let Some(ref kg) = self.knowledge_graph else {
            return Vec::new();
        };

        // BFS to find all simple paths up to max_depth
        let mut results = Vec::new();
        let mut visited = HashSet::new();
        let mut stack: Vec<(String, Vec<String>, Vec<KGEdge>, f32)> = vec![(
            src.to_string(),
            vec![src.to_string()],
            vec![],
            1.0,
        )];

        while let Some((current, path, path_edges, cumulative_strength)) = stack.pop() {
            if current == dst && !path.is_empty() {
                // Collect actual nodes and edges for this path
                let causal_nodes: Vec<KGNode> = path
                    .iter()
                    .filter_map(|id| kg.get_node(id).ok().flatten())
                    .collect();
                let causal_strength = if path_edges.is_empty() {
                    1.0
                } else {
                    cumulative_strength / path_edges.len() as f32
                };
                if !causal_nodes.is_empty() {
                    results.push(CausalPath {
                        nodes: causal_nodes,
                        edges: path_edges.clone(),
                        causal_strength,
                    });
                }
                continue;
            }
            if path.len() >= max_depth as usize {
                continue;
            }
            if visited.contains(&current) {
                continue;
            }
            visited.insert(current.clone());

            // Get outgoing edges (causal direction)
            if let Ok(neighbors) = kg.get_neighbors(&current, None, 1) {
                for (neighbor_node, edge) in neighbors {
                    let next_node_id = neighbor_node.id.clone();
                    if path.contains(&next_node_id) {
                        continue;
                    }
                    // Weight causal strength by edge type and weight
                    let edge_causal_weight = match edge.edge_type {
                        KGEdgeType::Causes => edge.weight * 1.0,
                        KGEdgeType::HasFact => edge.weight * 0.9,
                        KGEdgeType::Follows => edge.weight * 0.7,
                        KGEdgeType::RelatedTo => edge.weight * 0.5,
                        KGEdgeType::AssociatesWith => edge.weight * 0.4,
                        KGEdgeType::Mentions => edge.weight * 0.3,
                        _ => edge.weight * 0.2,
                    };
                    let mut new_path = path.clone();
                    new_path.push(next_node_id.clone());
                    let mut new_edges = path_edges.clone();
                    new_edges.push(edge);
                    let new_strength = cumulative_strength * edge_causal_weight;
                    stack.push((next_node_id, new_path, new_edges, new_strength));
                }
            }
        }

        // Sort by causal strength descending
        results.sort_by(|a, b| b.causal_strength.partial_cmp(&a.causal_strength).unwrap_or(std::cmp::Ordering::Equal));
        tracing::info!(path_count = results.len(), "KG causal paths found");
        results
    }

    /// Analyze the impact of modifying or removing a node.
    ///
    /// Computes the transitive closure of effects up to `propagation_depth`
    /// by following outgoing causal edges.
    pub fn kg_impact_analysis(
        &self,
        node_id: &str,
        propagation_depth: u8,
    ) -> ImpactResult {
        let _timer = OperationTimer::new(&self.metrics, OpType::KgFindPaths);
        let Some(ref kg) = self.knowledge_graph else {
            return ImpactResult {
                affected_nodes: Vec::new(),
                propagation_depth: 0,
                severity: 0.0,
            };
        };

        let mut affected: HashSet<String> = HashSet::new();
        let mut frontier: Vec<(String, u8)> = vec![(node_id.to_string(), 0)];
        let mut visited: HashSet<String> = HashSet::new();
        let mut total_severity: f32 = 0.0;

        while let Some((current, depth)) = frontier.pop() {
            if visited.contains(&current) {
                continue;
            }
            visited.insert(current.clone());

            // Add to affected if not the source node
            if current != node_id {
                affected.insert(current.clone());
                // Severity increases with closer proximity to the source
                let depth_factor = if propagation_depth > 0 {
                    1.0 - (depth as f32 / propagation_depth as f32)
                } else {
                    0.0
                };
                total_severity += depth_factor;
            }

            // Only explore neighbors if we haven't reached max propagation depth
            if depth >= propagation_depth {
                continue;
            }

            // Follow outgoing causal edges
            if let Ok(neighbors) = kg.get_neighbors(&current, None, 1) {
                for (neighbor_node, edge) in neighbors {
                    let neighbor_id = neighbor_node.id.clone();
                    if !visited.contains(&neighbor_id) {
                        // Weight by edge causal strength
                        let edge_strength = match edge.edge_type {
                            KGEdgeType::Causes => edge.weight,
                            KGEdgeType::HasFact => edge.weight * 0.9,
                            KGEdgeType::Follows => edge.weight * 0.7,
                            KGEdgeType::RelatedTo => edge.weight * 0.5,
                            KGEdgeType::AssociatesWith => edge.weight * 0.4,
                            _ => edge.weight * 0.2,
                        };
                        if edge_strength > 0.1 {
                            frontier.push((neighbor_id, depth + 1));
                        }
                    }
                }
            }
        }

        let mut affected_list: Vec<String> = affected.into_iter().collect();
        affected_list.sort();
        let max_depth_reached = visited.len().min(propagation_depth as usize) as u8;
        let severity = (total_severity / affected_list.len().max(1) as f32).min(1.0);

        ImpactResult {
            affected_nodes: affected_list,
            propagation_depth: max_depth_reached,
            severity,
        }
    }

    /// Get temporal changes (created, modified, deleted nodes) between two timestamps.
    ///
    /// Uses valid_at/invalid_at timestamps to reconstruct the state at different points.
    pub fn kg_temporal_changes(
        &self,
        from_ms: u64,
        to_ms: u64,
        agent_id: &str,
        tenant_id: &str,
    ) -> std::io::Result<Vec<TemporalChange>> {
        let _timer = OperationTimer::new(&self.metrics, OpType::KgFindPaths);
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Read)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Ok(Vec::new());
        };

        let mut changes = Vec::new();

        // Get all nodes for the tenant
        let all_nodes = kg.list_nodes(agent_id, None)
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        for node in all_nodes {
            if node.tenant_id != tenant_id {
                continue;
            }

            let valid_at = node.valid_at.unwrap_or(node.created_at);
            let invalid_at = node.invalid_at;
            let expired_at = node.expired_at;

            // Node created in the time range: valid_at is within [from_ms, to_ms]
            if valid_at >= from_ms && valid_at <= to_ms {
                changes.push(TemporalChange {
                    before: None,
                    after: Some(node.clone()),
                    change_type: ChangeType::Created,
                    timestamp_ms: valid_at,
                });
            }

            // Node modified (re-activated after invalid_at) or deleted (expired_at set)
            if let Some(exp) = expired_at {
                if exp >= from_ms && exp <= to_ms {
                    changes.push(TemporalChange {
                        before: Some(node.clone()),
                        after: None,
                        change_type: ChangeType::Deleted,
                        timestamp_ms: exp,
                    });
                }
            }

            if let Some(inv) = invalid_at {
                if inv >= from_ms && inv <= to_ms {
                    changes.push(TemporalChange {
                        before: Some(node.clone()),
                        after: None,
                        change_type: ChangeType::Modified,
                        timestamp_ms: inv,
                    });
                }
            }
        }

        // Sort by timestamp descending (most recent first)
        changes.sort_by(|a, b| b.timestamp_ms.cmp(&a.timestamp_ms));

        Ok(changes)
    }

    /// Detect causal chains in the knowledge graph.
    ///
    /// A causal chain is a sequence of nodes connected by causal edges
    /// (Causes, HasFact, Follows) where each edge represents a cause-effect relationship.
    pub fn kg_detect_causal_chains(
        &self,
        start_node_id: &str,
        max_depth: u8,
    ) -> Vec<CausalPath> {
        let _timer = OperationTimer::new(&self.metrics, OpType::KgFindPaths);
        let Some(ref kg) = self.knowledge_graph else {
            return Vec::new();
        };

        let mut results = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();

        // Causal edge types that form chains
        fn is_causal_edge(et: KGEdgeType) -> bool {
            matches!(et, KGEdgeType::Causes | KGEdgeType::HasFact | KGEdgeType::Follows)
        }

        // DFS to find causal chains
        fn dfs(
            kg: &dyn KnowledgeGraph,
            current: &str,
            max_depth: u8,
            current_depth: u8,
            path: &mut Vec<String>,
            edges: &mut Vec<KGEdge>,
            visited: &mut HashSet<String>,
            results: &mut Vec<CausalPath>,
        ) {
            if current_depth >= max_depth {
                return;
            }

            if visited.contains(current) {
                return;
            }
            visited.insert(current.to_string());

            if let Ok(neighbors) = kg.get_neighbors(current, None, 1) {
                for (neighbor_node, edge) in neighbors {
                    if !is_causal_edge(edge.edge_type) {
                        continue;
                    }

                    path.push(neighbor_node.id.clone());
                    edges.push(edge.clone());

                    // Found a chain of at least 2 edges
                    if edges.len() >= 2 {
                        let chain_nodes: Vec<KGNode> = path
                            .iter()
                            .filter_map(|id| kg.get_node(id).ok().flatten())
                            .collect();
                        if !chain_nodes.is_empty() {
                            let strength = edges.iter()
                                .map(|e| e.weight)
                                .sum::<f32>() / edges.len() as f32;
                            results.push(CausalPath {
                                nodes: chain_nodes,
                                edges: edges.clone(),
                                causal_strength: strength,
                            });
                        }
                    }

                    dfs(
                        kg,
                        &neighbor_node.id,
                        max_depth,
                        current_depth + 1,
                        path,
                        edges,
                        visited,
                        results,
                    );

                    path.pop();
                    edges.pop();
                }
            }

            visited.remove(current);
        }

        let mut path = vec![start_node_id.to_string()];
        let mut edges = Vec::new();
        dfs(
            kg.as_ref(),
            start_node_id,
            max_depth,
            0,
            &mut path,
            &mut edges,
            &mut visited,
            &mut results,
        );

        // Sort by causal strength descending
        results.sort_by(|a, b| b.causal_strength.partial_cmp(&a.causal_strength).unwrap_or(std::cmp::Ordering::Equal));

        results
    }
}
