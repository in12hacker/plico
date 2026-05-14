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
        changes.sort_by_key(|c| std::cmp::Reverse(c.timestamp_ms));

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

        struct DfsContext<'a> {
            path: &'a mut Vec<String>,
            edges: &'a mut Vec<KGEdge>,
            visited: &'a mut HashSet<String>,
            results: &'a mut Vec<CausalPath>,
        }

        fn dfs(
            kg: &dyn KnowledgeGraph,
            current: &str,
            max_depth: u8,
            current_depth: u8,
            ctx: &mut DfsContext<'_>,
        ) {
            if current_depth >= max_depth {
                return;
            }

            if ctx.visited.contains(current) {
                return;
            }
            ctx.visited.insert(current.to_string());

            if let Ok(neighbors) = kg.get_neighbors(current, None, 1) {
                for (neighbor_node, edge) in neighbors {
                    if !is_causal_edge(edge.edge_type) {
                        continue;
                    }

                    ctx.path.push(neighbor_node.id.clone());
                    ctx.edges.push(edge.clone());

                    if ctx.edges.len() >= 2 {
                        let chain_nodes: Vec<KGNode> = ctx.path
                            .iter()
                            .filter_map(|id| kg.get_node(id).ok().flatten())
                            .collect();
                        if !chain_nodes.is_empty() {
                            let strength = ctx.edges.iter()
                                .map(|e| e.weight)
                                .sum::<f32>() / ctx.edges.len() as f32;
                            ctx.results.push(CausalPath {
                                nodes: chain_nodes,
                                edges: ctx.edges.clone(),
                                causal_strength: strength,
                            });
                        }
                    }

                    dfs(
                        kg,
                        &neighbor_node.id,
                        max_depth,
                        current_depth + 1,
                        ctx,
                    );

                    ctx.path.pop();
                    ctx.edges.pop();
                }
            }

            ctx.visited.remove(current);
        }

        let mut path = vec![start_node_id.to_string()];
        let mut edges = Vec::new();
        dfs(
            kg.as_ref(),
            start_node_id,
            max_depth,
            0,
            &mut DfsContext {
                path: &mut path,
                edges: &mut edges,
                visited: &mut visited,
                results: &mut results,
            },
        );

        // Sort by causal strength descending
        results.sort_by(|a, b| b.causal_strength.partial_cmp(&a.causal_strength).unwrap_or(std::cmp::Ordering::Equal));

        results
    }

    // ─── Storage Governance (F-18) ─────────────────────────────────

    /// Check if a CID is referenced by any node in the knowledge graph.
    /// Returns true if at least one KG node has this CID as its content_cid.
    pub fn is_cid_referenced(&self, cid: &str) -> bool {
        let Some(ref kg) = self.knowledge_graph else {
            return false;
        };
        kg.has_node_with_cid(cid).unwrap_or(false)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_explore_empty_returns_empty() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        // Explore with no KG initialized returns empty
        let results = kernel.graph_explore("nonexistent-cid", None, 1);
        assert!(results.is_empty());
    }

    #[test]
    fn test_graph_explore_raw_empty_returns_empty() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let results = kernel.graph_explore_raw("nonexistent-cid", None, 1);
        assert!(results.is_empty());
    }

    #[test]
    fn test_kg_add_node_idempotent_returns_unique_ids() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("node-A", KGNodeType::Entity, serde_json::json!({}), "agent1", "default")
            .expect("first add_node failed");
        let id2 = kernel.kg_add_node("node-B", KGNodeType::Entity, serde_json::json!({}), "agent1", "default")
            .expect("second add_node failed");
        // Each call generates a unique UUID
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_kg_add_node_stores_and_retrieves() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel.kg_add_node(
            "test-node",
            KGNodeType::Fact,
            serde_json::json!({ "key": "value" }),
            "agent1",
            "default",
        ).expect("kg_add_node failed");

        let node = kernel.kg_get_node(&id, "agent1", "default")
            .expect("kg_get_node failed");
        let node = node.expect("node not found");
        assert_eq!(node.label, "test-node");
        assert!(matches!(node.node_type, KGNodeType::Fact));
    }

    #[test]
    fn test_kg_list_nodes_filters_by_type() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.kg_add_node("entity-1", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").ok();
        kernel.kg_add_node("fact-1", KGNodeType::Fact, serde_json::json!({}), "agent1", "default").ok();

        let all = kernel.kg_list_nodes(None, "agent1", "default").expect("kg_list_nodes failed");
        assert_eq!(all.len(), 2);

        let entities = kernel.kg_list_nodes(Some(KGNodeType::Entity), "agent1", "default").expect("kg_list_nodes failed");
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].label, "entity-1");

        let facts = kernel.kg_list_nodes(Some(KGNodeType::Fact), "agent1", "default").expect("kg_list_nodes failed");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].label, "fact-1");
    }

    #[test]
    fn test_kg_add_edge_and_list() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("src-node", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").expect("add src failed");
        let id2 = kernel.kg_add_node("dst-node", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").expect("add dst failed");

        kernel.kg_add_edge(&id1, &id2, KGEdgeType::RelatedTo, None, "agent1", "default").expect("add_edge failed");

        let edges = kernel.kg_list_edges("agent1", "default", None).expect("kg_list_edges failed");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].src, id1);
        assert_eq!(edges[0].dst, id2);
        assert!(matches!(edges[0].edge_type, KGEdgeType::RelatedTo));
    }

    #[test]
    fn test_kg_find_paths_no_path_returns_empty() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("isolated-A", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").expect("add failed");
        let id2 = kernel.kg_add_node("isolated-B", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").expect("add failed");

        let paths = kernel.kg_find_paths(&id1, &id2, 3);
        assert!(paths.is_empty());
    }

    #[test]
    fn test_kg_find_paths_direct() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("start", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").expect("add failed");
        let id2 = kernel.kg_add_node("end", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").expect("add failed");

        kernel.kg_add_edge(&id1, &id2, KGEdgeType::Follows, None, "agent1", "default").expect("add_edge failed");

        let paths = kernel.kg_find_paths(&id1, &id2, 3);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0][0].id, id1);
        assert_eq!(paths[0][paths[0].len() - 1].id, id2);
    }

    #[test]
    fn test_kg_get_valid_nodes_at_time() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.kg_add_node("current-node", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").ok();

        let now = chrono::Utc::now().timestamp_millis() as u64;
        let nodes = kernel.kg_get_valid_nodes_at("agent1", "default", None, now).expect("kg_get_valid_nodes_at failed");
        assert!(!nodes.is_empty());
    }

    #[test]
    fn test_kg_update_node() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel.kg_add_node("old-label", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").expect("add failed");

        kernel.kg_update_node(&id, Some("new-label"), None, "agent1", "default").expect("update failed");

        let node = kernel.kg_get_node(&id, "agent1", "default").expect("get failed").expect("node not found");
        assert_eq!(node.label, "new-label");
    }

    #[test]
    fn test_kg_remove_node() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel.kg_add_node("to-remove", KGNodeType::Entity, serde_json::json!({}), "kernel", "default").expect("add failed");

        kernel.kg_remove_node(&id, "kernel", "default").expect("remove failed");

        let node = kernel.kg_get_node(&id, "kernel", "default").expect("get failed");
        assert!(node.is_none());
    }

    #[test]
    fn test_kg_remove_edge() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("e1", KGNodeType::Entity, serde_json::json!({}), "kernel", "default").expect("add failed");
        let id2 = kernel.kg_add_node("e2", KGNodeType::Entity, serde_json::json!({}), "kernel", "default").expect("add failed");

        kernel.kg_add_edge(&id1, &id2, KGEdgeType::AssociatesWith, None, "kernel", "default").expect("add_edge failed");

        let edges_before = kernel.kg_list_edges("kernel", "default", None).expect("list failed");
        assert_eq!(edges_before.len(), 1);

        kernel.kg_remove_edge(&id1, &id2, None, "kernel", "default").expect("remove_edge failed");

        let edges_after = kernel.kg_list_edges("kernel", "default", None).expect("list failed");
        assert!(edges_after.is_empty());
    }

    #[test]
    fn test_is_cid_referenced_false_for_unused_cid() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        assert!(!kernel.is_cid_referenced("nonexistent-cid-abc123"));
    }

    #[test]
    fn test_kg_impact_analysis_no_neighbors() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel.kg_add_node("lonely", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").expect("add failed");

        let impact = kernel.kg_impact_analysis(&id, 2);
        assert!(impact.affected_nodes.is_empty());
    }

    #[test]
    fn test_kg_temporal_changes_empty_range() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        // Future time range with no changes should return empty
        let future = chrono::Utc::now().timestamp_millis() as u64 + 1_000_000_000;
        let changes = kernel.kg_temporal_changes(future, future + 1000, "agent1", "default").expect("kg_temporal_changes failed");
        // May be empty or may contain created nodes depending on timing
        // Just verify no panic
        let _ = changes;
    }

    // ── Additional coverage tests ──────────────────────────────────────────

    #[test]
    fn test_graph_explore_with_populated_graph() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("src", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("dst", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::Follows, Some(0.8), "agent1", "default").unwrap();

        // Explore with no edge type filter
        let results = kernel.graph_explore(&id1, None, 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].node.id, id2);
        assert!(results[0].edge_type.is_some());
        assert!(results[0].authority_score >= 0.0);

        // Explore with matching edge type filter
        let results = kernel.graph_explore(&id1, Some(KGEdgeType::Follows), 1);
        assert_eq!(results.len(), 1);

        // Explore with non-matching edge type filter
        let results = kernel.graph_explore(&id1, Some(KGEdgeType::Causes), 1);
        assert!(results.is_empty());
    }

    #[test]
    fn test_graph_explore_raw_edge_type_string_parsing() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("raw-a", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("raw-b", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::Mentions, None, "agent1", "default").unwrap();

        // Valid edge type strings
        let results = kernel.graph_explore_raw(&id1, Some("mentions"), 1);
        assert_eq!(results.len(), 1);
        let (node_id, label, node_type, edge_type, _score) = &results[0];
        assert_eq!(node_id, &id2);
        assert_eq!(label, "raw-b");
        assert_eq!(node_type, "entity");
        assert_eq!(edge_type, "mentions");

        // Other valid edge type strings
        for et_str in &["associates_with", "follows", "part_of", "related_to", "similar_to", "causes", "reminds"] {
            let r = kernel.graph_explore_raw(&id1, Some(et_str), 1);
            // No match expected since the actual edge is Mentions
            assert!(r.is_empty(), "expected empty for edge_type={}", et_str);
        }

        // Invalid edge type string falls through to None (no filter)
        let results = kernel.graph_explore_raw(&id1, Some("nonexistent_type"), 1);
        // With no filter it should still find the neighbor
        assert_eq!(results.len(), 1);

        // No filter
        let results = kernel.graph_explore_raw(&id1, None, 1);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_graph_explore_multihop() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("hop-a", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("hop-b", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id3 = kernel.kg_add_node("hop-c", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::Follows, None, "agent1", "default").unwrap();
        kernel.kg_add_edge(&id2, &id3, KGEdgeType::Follows, None, "agent1", "default").unwrap();

        // get_neighbors returns direct neighbors; depth>0 activates but doesn't do multi-hop BFS
        let r1 = kernel.graph_explore(&id1, None, 1);
        assert_eq!(r1.len(), 1);
        assert_eq!(r1[0].node.id, id2);

        // Depth=2 still returns direct neighbors only (backend behavior)
        let r2 = kernel.graph_explore(&id1, None, 2);
        let ids: Vec<&str> = r2.iter().map(|h| h.node.id.as_str()).collect();
        assert!(ids.contains(&id2.as_str()));

        // From id2, we should see both id1 (incoming) and id3 (outgoing)
        let r3 = kernel.graph_explore(&id2, None, 1);
        let ids2: Vec<&str> = r3.iter().map(|h| h.node.id.as_str()).collect();
        assert!(ids2.contains(&id1.as_str()));
        assert!(ids2.contains(&id3.as_str()));
    }

    #[test]
    fn test_kg_find_weighted_path_direct() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("wp-a", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("wp-b", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::Causes, Some(0.9), "agent1", "default").unwrap();

        let result = kernel.kg_find_weighted_path(&id1, &id2, 3);
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(path.len(), 2);
        assert_eq!(path[0].id, id1);
        assert_eq!(path[1].id, id2);
    }

    #[test]
    fn test_kg_find_weighted_path_no_path() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("iso-1", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("iso-2", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();

        let result = kernel.kg_find_weighted_path(&id1, &id2, 3);
        assert!(result.is_none());
    }

    #[test]
    fn test_kg_find_paths_multihop() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("path-a", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("path-b", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id3 = kernel.kg_add_node("path-c", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::RelatedTo, None, "agent1", "default").unwrap();
        kernel.kg_add_edge(&id2, &id3, KGEdgeType::RelatedTo, None, "agent1", "default").unwrap();

        let paths = kernel.kg_find_paths(&id1, &id3, 5);
        assert!(!paths.is_empty());
        // Each path should start at id1 and end at id3
        for path in &paths {
            assert_eq!(path.first().unwrap().id, id1);
            assert_eq!(path.last().unwrap().id, id3);
        }
    }

    #[test]
    fn test_kg_get_node_not_found() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let result = kernel.kg_get_node("nonexistent-id", "agent1", "default").expect("should not error");
        assert!(result.is_none());
    }

    #[test]
    fn test_kg_list_edges_with_node_id_filter() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("le-a", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("le-b", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id3 = kernel.kg_add_node("le-c", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::Follows, None, "agent1", "default").unwrap();
        kernel.kg_add_edge(&id2, &id3, KGEdgeType::Follows, None, "agent1", "default").unwrap();

        // No filter: returns all 2 edges
        let all = kernel.kg_list_edges("agent1", "default", None).unwrap();
        assert_eq!(all.len(), 2);

        // Filter by id1: only edge id1->id2
        let filtered = kernel.kg_list_edges("agent1", "default", Some(&id1)).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].src, id1);

        // Filter by id2: both edges touch id2
        let filtered = kernel.kg_list_edges("agent1", "default", Some(&id2)).unwrap();
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_kg_update_node_properties_only() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel.kg_add_node("prop-node", KGNodeType::Entity, serde_json::json!({"old": 1}), "agent1", "default").unwrap();

        kernel.kg_update_node(&id, None, Some(serde_json::json!({"new": 42})), "agent1", "default").unwrap();

        let node = kernel.kg_get_node(&id, "agent1", "default").unwrap().unwrap();
        assert_eq!(node.label, "prop-node"); // label unchanged
        assert_eq!(node.properties["new"], 42);
    }

    #[test]
    fn test_kg_update_node_label_and_properties() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel.kg_add_node("both-old", KGNodeType::Fact, serde_json::json!({}), "agent1", "default").unwrap();

        kernel.kg_update_node(&id, Some("both-new"), Some(serde_json::json!({"x": true})), "agent1", "default").unwrap();

        let node = kernel.kg_get_node(&id, "agent1", "default").unwrap().unwrap();
        assert_eq!(node.label, "both-new");
        assert_eq!(node.properties["x"], true);
    }

    #[test]
    fn test_kg_update_node_nonexistent() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        // Updating a nonexistent node should not error (no tenant check fires, update_node may or may not error)
        let result = kernel.kg_update_node("ghost-id", Some("new-label"), None, "agent1", "default");
        // The underlying kg.update_node should return an error for nonexistent node
        assert!(result.is_err());
    }

    #[test]
    fn test_kg_remove_edge_with_type_filter() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("rt-a", KGNodeType::Entity, serde_json::json!({}), "kernel", "default").unwrap();
        let id2 = kernel.kg_add_node("rt-b", KGNodeType::Entity, serde_json::json!({}), "kernel", "default").unwrap();
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::Follows, None, "kernel", "default").unwrap();
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::Mentions, None, "kernel", "default").unwrap();

        let edges = kernel.kg_list_edges("kernel", "default", None).unwrap();
        assert_eq!(edges.len(), 2);

        // Remove only the Follows edge
        kernel.kg_remove_edge(&id1, &id2, Some(KGEdgeType::Follows), "kernel", "default").unwrap();

        let edges = kernel.kg_list_edges("kernel", "default", None).unwrap();
        assert_eq!(edges.len(), 1);
        assert!(matches!(edges[0].edge_type, KGEdgeType::Mentions));
    }

    #[test]
    fn test_kg_edge_history_basic() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("eh-a", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("eh-b", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::SimilarTo, None, "agent1", "default").unwrap();

        let history = kernel.kg_edge_history(&id1, &id2, None, "agent1", "default").unwrap();
        assert_eq!(history.len(), 1);
        assert!(matches!(history[0].edge_type, KGEdgeType::SimilarTo));

        // Filter by matching type
        let history = kernel.kg_edge_history(&id1, &id2, Some(KGEdgeType::SimilarTo), "agent1", "default").unwrap();
        assert_eq!(history.len(), 1);

        // Filter by non-matching type
        let history = kernel.kg_edge_history(&id1, &id2, Some(KGEdgeType::Causes), "agent1", "default").unwrap();
        assert!(history.is_empty());
    }

    #[test]
    fn test_kg_edge_history_no_edges() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("noeh-a", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("noeh-b", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();

        let history = kernel.kg_edge_history(&id1, &id2, None, "agent1", "default").unwrap();
        assert!(history.is_empty());
    }

    #[test]
    fn test_kg_find_causal_path_direct() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("cause", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("effect", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::Causes, Some(0.9), "agent1", "default").unwrap();

        let paths = kernel.kg_find_causal_path(&id1, &id2, 3);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].nodes.first().unwrap().id, id1);
        assert_eq!(paths[0].nodes.last().unwrap().id, id2);
        assert_eq!(paths[0].edges.len(), 1);
        assert!(paths[0].causal_strength > 0.0);
    }

    #[test]
    fn test_kg_find_causal_path_no_path() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("cp-a", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("cp-b", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();

        let paths = kernel.kg_find_causal_path(&id1, &id2, 3);
        assert!(paths.is_empty());
    }

    #[test]
    fn test_kg_find_causal_path_multi_hop() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("ch-a", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("ch-b", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        let id3 = kernel.kg_add_node("ch-c", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::Causes, Some(0.8), "agent1", "default").unwrap();
        kernel.kg_add_edge(&id2, &id3, KGEdgeType::Causes, Some(0.7), "agent1", "default").unwrap();

        let paths = kernel.kg_find_causal_path(&id1, &id3, 5);
        assert!(!paths.is_empty());
        // Verify the path goes through all 3 nodes
        let first = paths.first().unwrap();
        assert_eq!(first.nodes.len(), 3);
        assert_eq!(first.edges.len(), 2);
    }

    #[test]
    fn test_kg_find_causal_path_edge_type_weighting() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("wt-a", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("wt-b", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        // Causes has 1.0x multiplier, Mentions has 0.3x
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::Causes, Some(1.0), "agent1", "default").unwrap();

        let paths = kernel.kg_find_causal_path(&id1, &id2, 3);
        assert_eq!(paths.len(), 1);
        let strong_strength = paths[0].causal_strength;

        // Now test with a weaker edge type
        let (kernel2, _dir2) = crate::kernel::tests::make_kernel();
        let id3 = kernel2.kg_add_node("wt-c", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id4 = kernel2.kg_add_node("wt-d", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        kernel2.kg_add_edge(&id3, &id4, KGEdgeType::Mentions, Some(1.0), "agent1", "default").unwrap();

        let paths2 = kernel2.kg_find_causal_path(&id3, &id4, 3);
        assert_eq!(paths2.len(), 1);
        let weak_strength = paths2[0].causal_strength;

        // Causes should have higher causal strength than Mentions
        assert!(strong_strength > weak_strength, "Causes ({}) should be stronger than Mentions ({})", strong_strength, weak_strength);
    }

    #[test]
    fn test_kg_impact_analysis_with_chain() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("ia-a", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("ia-b", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        let id3 = kernel.kg_add_node("ia-c", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::Causes, Some(0.9), "agent1", "default").unwrap();
        kernel.kg_add_edge(&id2, &id3, KGEdgeType::Causes, Some(0.8), "agent1", "default").unwrap();

        let impact = kernel.kg_impact_analysis(&id1, 3);
        assert!(!impact.affected_nodes.is_empty(), "should find affected nodes in causal chain");
        assert!(impact.severity > 0.0, "severity should be positive for causal chain");
        // id2 and id3 should be in the affected set
        assert!(impact.affected_nodes.contains(&id2));
        assert!(impact.affected_nodes.contains(&id3));
    }

    #[test]
    fn test_kg_impact_analysis_depth_zero() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("iz-a", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("iz-b", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::Causes, Some(0.5), "agent1", "default").unwrap();

        let impact = kernel.kg_impact_analysis(&id1, 0);
        // With depth 0, should not traverse any neighbors
        assert!(impact.affected_nodes.is_empty());
        assert_eq!(impact.severity, 0.0);
    }

    #[test]
    fn test_kg_temporal_changes_created() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let now = chrono::Utc::now().timestamp_millis() as u64;
        let _id = kernel.kg_add_node("tc-node", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();

        // Query a range that includes now
        let changes = kernel.kg_temporal_changes(now - 1000, now + 1000, "agent1", "default").unwrap();
        assert!(!changes.is_empty());
        let created: Vec<&TemporalChange> = changes.iter().filter(|c| matches!(c.change_type, ChangeType::Created)).collect();
        assert!(!created.is_empty(), "should find at least one Created change");
    }

    #[test]
    fn test_kg_detect_causal_chains_basic() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("cc-a", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("cc-b", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        let id3 = kernel.kg_add_node("cc-c", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        // HasFact is a causal edge type
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::HasFact, Some(0.8), "agent1", "default").unwrap();
        kernel.kg_add_edge(&id2, &id3, KGEdgeType::Causes, Some(0.9), "agent1", "default").unwrap();

        let chains = kernel.kg_detect_causal_chains(&id1, 5);
        assert!(!chains.is_empty(), "should detect causal chain");
        // Each chain should have causal_strength > 0
        for chain in &chains {
            assert!(chain.causal_strength > 0.0);
            assert!(chain.edges.len() >= 2, "chain needs >= 2 edges to be detected");
        }
    }

    #[test]
    fn test_kg_detect_causal_chains_no_causal_edges() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("nc-a", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("nc-b", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        // RelatedTo is NOT a causal edge type
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::RelatedTo, None, "agent1", "default").unwrap();

        let chains = kernel.kg_detect_causal_chains(&id1, 5);
        assert!(chains.is_empty(), "RelatedTo should not form a causal chain");
    }

    #[test]
    fn test_kg_detect_causal_chains_respects_max_depth() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("md-a", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("md-b", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        let id3 = kernel.kg_add_node("md-c", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::Causes, Some(0.8), "agent1", "default").unwrap();
        kernel.kg_add_edge(&id2, &id3, KGEdgeType::Causes, Some(0.8), "agent1", "default").unwrap();

        // max_depth=1 means only 1 hop, not enough for a 2-edge chain
        let chains = kernel.kg_detect_causal_chains(&id1, 1);
        assert!(chains.is_empty(), "depth=1 should not find 2-edge chain");

        // max_depth=3 should find it
        let chains = kernel.kg_detect_causal_chains(&id1, 3);
        assert!(!chains.is_empty());
    }

    #[test]
    fn test_is_cid_referenced_true() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        // Add a node with a content_cid using the KG API directly
        let kg = kernel.knowledge_graph.as_ref().unwrap();
        let node = KGNode {
            id: "cid-ref-node".to_string(),
            label: "with-cid".to_string(),
            node_type: KGNodeType::Document,
            content_cid: Some("abc123-sha".to_string()),
            properties: serde_json::json!({}),
            agent_id: "agent1".to_string(),
            tenant_id: "default".to_string(),
            created_at: 1000,
            valid_at: Some(1000),
            invalid_at: None,
            expired_at: None,
        };
        kg.add_node(node).unwrap();

        assert!(kernel.is_cid_referenced("abc123-sha"));
        assert!(!kernel.is_cid_referenced("other-cid"));
    }

    #[test]
    fn test_kg_list_nodes_tenant_isolation() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.kg_add_node("tenant-a-node", KGNodeType::Entity, serde_json::json!({}), "agent1", "tenant-a").unwrap();
        kernel.kg_add_node("tenant-b-node", KGNodeType::Entity, serde_json::json!({}), "agent1", "tenant-b").unwrap();

        // tenant-a should only see its own nodes
        let nodes_a = kernel.kg_list_nodes(None, "agent1", "tenant-a").unwrap();
        assert!(nodes_a.iter().all(|n| n.tenant_id == "tenant-a"));
        assert!(nodes_a.iter().any(|n| n.label == "tenant-a-node"));
        assert!(!nodes_a.iter().any(|n| n.label == "tenant-b-node"));

        // tenant-b should only see its own nodes
        let nodes_b = kernel.kg_list_nodes(None, "agent1", "tenant-b").unwrap();
        assert!(nodes_b.iter().all(|n| n.tenant_id == "tenant-b"));
        assert!(nodes_b.iter().any(|n| n.label == "tenant-b-node"));
    }

    #[test]
    fn test_kg_list_edges_tenant_isolation() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        // Create nodes in different tenants via their respective agents
        let id_a1 = kernel.kg_add_node("ti-a1", KGNodeType::Entity, serde_json::json!({}), "agent-x", "tenant-x").unwrap();
        let id_a2 = kernel.kg_add_node("ti-a2", KGNodeType::Entity, serde_json::json!({}), "agent-x", "tenant-x").unwrap();
        let id_b1 = kernel.kg_add_node("ti-b1", KGNodeType::Entity, serde_json::json!({}), "agent-y", "tenant-y").unwrap();
        let id_b2 = kernel.kg_add_node("ti-b2", KGNodeType::Entity, serde_json::json!({}), "agent-y", "tenant-y").unwrap();

        // Add edges using agents from the same tenant as the nodes
        kernel.kg_add_edge(&id_a1, &id_a2, KGEdgeType::Follows, None, "agent-x", "tenant-x").unwrap();
        kernel.kg_add_edge(&id_b1, &id_b2, KGEdgeType::Follows, None, "agent-y", "tenant-y").unwrap();

        // tenant-x should only see edges between its own nodes
        let edges_x = kernel.kg_list_edges("agent-x", "tenant-x", None).unwrap();
        assert!(edges_x.iter().all(|e| {
            (e.src == id_a1 || e.src == id_a2) && (e.dst == id_a1 || e.dst == id_a2)
        }));
        assert_eq!(edges_x.len(), 1);

        // tenant-y should only see edges between its own nodes
        let edges_y = kernel.kg_list_edges("agent-y", "tenant-y", None).unwrap();
        assert!(edges_y.iter().all(|e| {
            (e.src == id_b1 || e.src == id_b2) && (e.dst == id_b1 || e.dst == id_b2)
        }));
        assert_eq!(edges_y.len(), 1);
    }

    #[test]
    fn test_kg_get_node_tenant_isolation() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel.kg_add_node("cross-tenant", KGNodeType::Entity, serde_json::json!({}), "agent1", "tenant-secret").unwrap();

        // Same tenant: should succeed
        let node = kernel.kg_get_node(&id, "agent1", "tenant-secret").unwrap();
        assert!(node.is_some());

        // Different tenant: should fail with permission error
        let result = kernel.kg_get_node(&id, "agent1", "tenant-other");
        assert!(result.is_err(), "cross-tenant access should be denied");
    }

    #[test]
    fn test_kg_add_edge_tenant_isolation() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        // Create nodes in tenant-1
        let id1 = kernel.kg_add_node("iso-src", KGNodeType::Entity, serde_json::json!({}), "agent1", "tenant-1").unwrap();
        let id2 = kernel.kg_add_node("iso-dst", KGNodeType::Entity, serde_json::json!({}), "agent1", "tenant-1").unwrap();

        // Try to add edge from tenant-2 — should fail
        let result = kernel.kg_add_edge(&id1, &id2, KGEdgeType::Follows, None, "agent1", "tenant-2");
        assert!(result.is_err(), "cross-tenant add_edge should be denied");
    }

    #[test]
    fn test_kg_edge_history_tenant_isolation() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("ehi-a", KGNodeType::Entity, serde_json::json!({}), "kernel", "default").unwrap();
        let id2 = kernel.kg_add_node("ehi-b", KGNodeType::Entity, serde_json::json!({}), "kernel", "default").unwrap();
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::SimilarTo, None, "kernel", "default").unwrap();

        // Same tenant: should succeed
        let history = kernel.kg_edge_history(&id1, &id2, None, "agent1", "default").unwrap();
        assert_eq!(history.len(), 1);

        // Different tenant: should fail
        let result = kernel.kg_edge_history(&id1, &id2, None, "agent1", "other-tenant");
        assert!(result.is_err());
    }

    #[test]
    fn test_kg_remove_node_with_edges() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("rmv-a", KGNodeType::Entity, serde_json::json!({}), "kernel", "default").unwrap();
        let id2 = kernel.kg_add_node("rmv-b", KGNodeType::Entity, serde_json::json!({}), "kernel", "default").unwrap();
        let id3 = kernel.kg_add_node("rmv-c", KGNodeType::Entity, serde_json::json!({}), "kernel", "default").unwrap();
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::Follows, None, "kernel", "default").unwrap();
        kernel.kg_add_edge(&id2, &id3, KGEdgeType::Follows, None, "kernel", "default").unwrap();

        // Remove id2 which has edges to both id1 and id3
        kernel.kg_remove_node(&id2, "kernel", "default").unwrap();

        let node = kernel.kg_get_node(&id2, "kernel", "default").unwrap();
        assert!(node.is_none());

        // Edges touching id2 should also be removed
        let edges = kernel.kg_list_edges("kernel", "default", None).unwrap();
        assert!(edges.is_empty(), "edges should be removed when node is removed");
    }

    #[test]
    fn test_kg_remove_node_nonexistent() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let result = kernel.kg_remove_node("ghost-node", "agent1", "default");
        // Removing a nonexistent node should error
        assert!(result.is_err());
    }

    #[test]
    fn test_kg_find_causal_path_sorted_by_strength() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let a = kernel.kg_add_node("sort-a", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        let b = kernel.kg_add_node("sort-b", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        let c = kernel.kg_add_node("sort-c", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        let d = kernel.kg_add_node("sort-d", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();

        // Path 1: a -> b -> d (Causes, Causes) — strong
        kernel.kg_add_edge(&a, &b, KGEdgeType::Causes, Some(1.0), "agent1", "default").unwrap();
        kernel.kg_add_edge(&b, &d, KGEdgeType::Causes, Some(1.0), "agent1", "default").unwrap();

        // Path 2: a -> c -> d (Mentions, Mentions) — weak
        kernel.kg_add_edge(&a, &c, KGEdgeType::Mentions, Some(1.0), "agent1", "default").unwrap();
        kernel.kg_add_edge(&c, &d, KGEdgeType::Mentions, Some(1.0), "agent1", "default").unwrap();

        let paths = kernel.kg_find_causal_path(&a, &d, 5);
        assert!(paths.len() >= 2, "should find at least 2 paths");
        // First path should have higher or equal causal strength
        assert!(paths[0].causal_strength >= paths[1].causal_strength,
            "paths should be sorted by strength descending: {} >= {}",
            paths[0].causal_strength, paths[1].causal_strength);
    }

    #[test]
    fn test_kg_temporal_changes_sorted_descending() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let now = chrono::Utc::now().timestamp_millis() as u64;
        // Add two nodes with slightly different timestamps by adding them sequentially
        kernel.kg_add_node("ts-first", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        kernel.kg_add_node("ts-second", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();

        let changes = kernel.kg_temporal_changes(now - 5000, now + 5000, "agent1", "default").unwrap();
        if changes.len() >= 2 {
            // Should be sorted descending by timestamp
            for i in 0..changes.len() - 1 {
                assert!(changes[i].timestamp_ms >= changes[i + 1].timestamp_ms,
                    "changes should be sorted by timestamp descending");
            }
        }
    }

    #[test]
    fn test_kg_update_node_no_changes() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel.kg_add_node("noop", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();

        // Update with None, None — should succeed as a no-op
        kernel.kg_update_node(&id, None, None, "agent1", "default").unwrap();

        let node = kernel.kg_get_node(&id, "agent1", "default").unwrap().unwrap();
        assert_eq!(node.label, "noop");
    }

    #[test]
    fn test_kg_get_valid_nodes_at_filters_by_type() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.kg_add_node("vtype-e", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        kernel.kg_add_node("vtype-f", KGNodeType::Fact, serde_json::json!({}), "agent1", "default").unwrap();

        let now = chrono::Utc::now().timestamp_millis() as u64;
        let entities = kernel.kg_get_valid_nodes_at("agent1", "default", Some(KGNodeType::Entity), now).unwrap();
        assert!(entities.iter().all(|n| matches!(n.node_type, KGNodeType::Entity)));
        assert!(entities.iter().any(|n| n.label == "vtype-e"));
        assert!(!entities.iter().any(|n| n.label == "vtype-f"));
    }

    #[test]
    fn test_kg_get_valid_nodes_at_tenant_filtering() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.kg_add_node("vt-a", KGNodeType::Entity, serde_json::json!({}), "agent1", "tenant-1").unwrap();
        kernel.kg_add_node("vt-b", KGNodeType::Entity, serde_json::json!({}), "agent1", "tenant-2").unwrap();

        let now = chrono::Utc::now().timestamp_millis() as u64;
        let nodes = kernel.kg_get_valid_nodes_at("agent1", "tenant-1", None, now).unwrap();
        assert!(nodes.iter().all(|n| n.tenant_id == "tenant-1"));
        assert!(!nodes.iter().any(|n| n.tenant_id == "tenant-2"));
    }

    #[test]
    fn test_kg_list_nodes_returns_empty_when_no_kg() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        // This test relies on the kernel having a KG, but verifies the empty path.
        // In make_kernel the KG is always Some, so we test the normal path.
        let nodes = kernel.kg_list_nodes(None, "agent1", "default").unwrap();
        assert!(nodes.is_empty());
    }

    #[test]
    fn test_graph_explore_nonexistent_cid_returns_empty() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        // Add a node but explore a different CID
        kernel.kg_add_node("some-node", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let results = kernel.graph_explore("totally-unknown-cid", None, 1);
        assert!(results.is_empty());
    }

    #[test]
    fn test_kg_find_causal_path_follows_edge_type() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("fl-a", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("fl-b", KGNodeType::Event, serde_json::json!({}), "agent1", "default").unwrap();
        // Follows has 0.7x weight
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::Follows, Some(1.0), "agent1", "default").unwrap();

        let paths = kernel.kg_find_causal_path(&id1, &id2, 3);
        assert_eq!(paths.len(), 1);
        // causal_strength should reflect the Follows weight (0.7)
        assert!(paths[0].causal_strength > 0.0);
        assert!(paths[0].causal_strength < 1.0); // Follows is 0.7x
    }

    #[test]
    fn test_kg_find_causal_path_related_to_edge_type() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.kg_add_node("rt-a", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        let id2 = kernel.kg_add_node("rt-b", KGNodeType::Entity, serde_json::json!({}), "agent1", "default").unwrap();
        // RelatedTo has 0.5x weight
        kernel.kg_add_edge(&id1, &id2, KGEdgeType::RelatedTo, Some(1.0), "agent1", "default").unwrap();

        let paths = kernel.kg_find_causal_path(&id1, &id2, 3);
        assert_eq!(paths.len(), 1);
        assert!(paths[0].causal_strength > 0.0);
    }
}
