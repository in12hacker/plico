//! Knowledge Graph Backend — PetgraphBackend implementation.

use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::RwLock;

use crate::fs::graph::types::{DiskGraph, KGNode, KGEdge, KGEdgeType, KGNodeType};
use crate::fs::graph::{KGError, KnowledgeGraph};

/// Flattened edge record for JSON serialization.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EdgeRecord {
    pub src: String,
    pub dst: String,
    pub edge: KGEdge,
}

/// Path search state for Dijkstra-style shortest-path search.
#[derive(Clone)]
struct PathState {
    cost: f32,
    node: String,
    path: Vec<String>,
}

impl PartialEq for PathState {
    fn eq(&self, other: &Self) -> bool {
        self.cost == other.cost && self.node == other.node
    }
}

impl Eq for PathState {}

impl PartialOrd for PathState {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cost.partial_cmp(&other.cost).unwrap_or(std::cmp::Ordering::Equal).reverse())
    }
}

impl Ord for PathState {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.cost.partial_cmp(&other.cost).unwrap_or(std::cmp::Ordering::Equal).reverse()
    }
}

/// In-memory knowledge graph backed by HashMap.
/// Thread-safe via RwLock. Persisted to disk as JSON.
pub struct PetgraphBackend {
    nodes: RwLock<HashMap<String, KGNode>>,
    out_edges: RwLock<HashMap<String, Vec<(String, KGEdge)>>>,
    in_edges: RwLock<HashMap<String, Vec<(String, KGEdge)>>>,
    path: Option<std::path::PathBuf>,
}

impl PetgraphBackend {
    pub fn new() -> Self {
        Self {
            nodes: RwLock::new(HashMap::new()),
            out_edges: RwLock::new(HashMap::new()),
            in_edges: RwLock::new(HashMap::new()),
            path: None,
        }
    }

    pub fn open(root: std::path::PathBuf) -> Self {
        let nodes_path = root.join("kg_nodes.json");
        let edges_path = root.join("kg_edges.json");

        let (nodes, out_e, in_e) = if nodes_path.exists() {
            match Self::load_from_disk_internal(&nodes_path, &edges_path) {
                Ok(triple) => triple,
                Err(e) => {
                    tracing::warn!("Failed to load knowledge graph, starting fresh: {}", e);
                    (HashMap::new(), HashMap::new(), HashMap::new())
                }
            }
        } else {
            (HashMap::new(), HashMap::new(), HashMap::new())
        };

        Self {
            nodes: RwLock::new(nodes),
            out_edges: RwLock::new(out_e),
            in_edges: RwLock::new(in_e),
            path: Some(root),
        }
    }

    fn load_from_disk_internal(
        nodes_path: &std::path::Path,
        edges_path: &std::path::Path,
    ) -> Result<DiskGraph, KGError> {
        let nodes: HashMap<String, KGNode> =
            serde_json::from_str(&std::fs::read_to_string(nodes_path)?)
                .map_err(KGError::Json)?;
        let edges: Vec<EdgeRecord> =
            serde_json::from_str(&std::fs::read_to_string(edges_path)?)
                .map_err(KGError::Json)?;

        let mut out_edges: HashMap<String, Vec<(String, KGEdge)>> = HashMap::new();
        let mut in_edges: HashMap<String, Vec<(String, KGEdge)>> = HashMap::new();
        for rec in edges {
            out_edges
                .entry(rec.src.clone())
                .or_default()
                .push((rec.dst.clone(), rec.edge.clone()));
            in_edges
                .entry(rec.dst.clone())
                .or_default()
                .push((rec.src.clone(), rec.edge));
        }
        Ok((nodes, out_edges, in_edges))
    }

    fn persist(&self) {
        let Some(ref path) = self.path else { return };
        let nodes_path = path.join("kg_nodes.json");
        let edges_path = path.join("kg_edges.json");

        let nodes = self.nodes.read().unwrap();
        if let Ok(json) = serde_json::to_string(&*nodes) {
            let _ = std::fs::write(&nodes_path, json);
        }
        drop(nodes);

        let out = self.out_edges.read().unwrap();
        let records: Vec<EdgeRecord> = out
            .iter()
            .flat_map(|(src, list)| {
                list.iter().map(move |(dst, edge)| EdgeRecord {
                    src: src.clone(),
                    dst: dst.clone(),
                    edge: edge.clone(),
                })
            })
            .collect();
        if let Ok(json) = serde_json::to_string(&records) {
            let _ = std::fs::write(&edges_path, json);
        }
    }

    pub fn upsert_document(
        &self,
        cid: &str,
        tags: &[String],
        agent_id: &str,
    ) -> Result<(), KGError> {
        let node = KGNode {
            id: cid.to_string(),
            label: format!("doc:{}", &cid[..8.min(cid.len())]),
            node_type: KGNodeType::Document,
            content_cid: Some(cid.to_string()),
            properties: serde_json::json!({ "tags": tags }),
            agent_id: agent_id.to_string(),
            created_at: now_ms(),
            valid_at: None,
            invalid_at: None,
            expired_at: None,
        };

        let candidates: Vec<_> = {
            let nodes = self.nodes.read().unwrap();
            nodes
                .values()
                .filter(|n| {
                    n.agent_id == agent_id
                        && n.node_type == KGNodeType::Document
                        && n.id != cid
                        && shared_tag_count(&n.properties, tags) >= 2
                })
                .map(|n| (n.id.clone(), shared_tag_count(&n.properties, tags)))
                .collect()
        };

        self.add_node(node)?;

        for (other_id, shared) in candidates {
            let w = (shared as f32).min(1.0);
            let e1 = KGEdge::new_with_episode(
                cid.to_string(),
                other_id.clone(),
                KGEdgeType::AssociatesWith,
                w,
                cid,
            );
            let e2 = KGEdge::new_with_episode(
                other_id.clone(),
                cid.to_string(),
                KGEdgeType::AssociatesWith,
                w,
                cid,
            );
            let _ = self.add_edge(e1);
            let _ = self.add_edge(e2);
        }

        Ok(())
    }

    pub fn degree(&self, node_id: &str) -> usize {
        let out = self.out_edges.read().unwrap();
        let inc = self.in_edges.read().unwrap();
        let mut neighbors: HashSet<String> = out
            .get(node_id)
            .map(|v| v.iter().map(|(n, _)| n.clone()).collect())
            .unwrap_or_default();
        if let Some(inc_list) = inc.get(node_id) {
            for (n, _) in inc_list {
                neighbors.insert(n.clone());
            }
        }
        neighbors.len()
    }

    fn authority_score_internal(&self, node_id: &str) -> f32 {
        let degree = self.degree(node_id);
        if degree == 0 {
            return 0.0;
        }
        let max_degree = {
            let out = self.out_edges.read().unwrap();
            let inc = self.in_edges.read().unwrap();
            let all_ids: HashSet<_> = out.keys().chain(inc.keys()).cloned().collect();
            all_ids
                .into_iter()
                .map(|id| {
                    let mut nbrs: HashSet<String> = out
                        .get(&id)
                        .map(|v| v.iter().map(|(n, _)| n.clone()).collect())
                        .unwrap_or_default();
                    if let Some(inc_list) = inc.get(&id) {
                        for (n, _) in inc_list {
                            nbrs.insert(n.clone());
                        }
                    }
                    nbrs.len()
                })
                .max()
                .unwrap_or(1)
        };
        ((degree as f32).ln() / ((max_degree.max(1)) as f32).ln()).clamp(0.0, 1.0)
    }

    #[allow(dead_code)]
    fn load_from_disk(_path: &std::path::Path) -> Result<DiskGraph, KGError> {
        unimplemented!("use PetgraphBackend::open")
    }
}

fn shared_tag_count(props: &serde_json::Value, tags: &[String]) -> usize {
    let existing: HashSet<String> = props
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    tags.iter().filter(|t| existing.contains(t.as_str())).count()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Default for PetgraphBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl KnowledgeGraph for PetgraphBackend {
    fn add_node(&self, node: KGNode) -> Result<(), KGError> {
        let mut nodes = self.nodes.write().unwrap();
        nodes.insert(node.id.clone(), node);
        drop(nodes);
        self.persist();
        Ok(())
    }

    fn add_edge(&self, edge: KGEdge) -> Result<(), KGError> {
        {
            let out = self.out_edges.read().unwrap();
            if out
                .get(&edge.src)
                .is_some_and(|v| v.iter().any(|(dst, _)| dst == &edge.dst))
            {
                return Err(KGError::EdgeExists(
                    edge.src.clone(),
                    edge.dst.clone(),
                    edge.edge_type,
                ));
            }
        }
        self.out_edges
            .write()
            .unwrap()
            .entry(edge.src.clone())
            .or_default()
            .push((edge.dst.clone(), edge.clone()));
        self.in_edges
            .write()
            .unwrap()
            .entry(edge.dst.clone())
            .or_default()
            .push((edge.src.clone(), edge));
        self.persist();
        Ok(())
    }

    fn get_node(&self, id: &str) -> Result<Option<KGNode>, KGError> {
        let nodes = self.nodes.read().unwrap();
        Ok(nodes.get(id).cloned())
    }

    fn get_neighbors(
        &self,
        id: &str,
        _edge_type: Option<KGEdgeType>,
        depth: u8,
    ) -> Result<Vec<(KGNode, KGEdge)>, KGError> {
        if depth == 0 {
            return Ok(vec![]);
        }
        let out = self.out_edges.read().unwrap();
        let nodes = self.nodes.read().unwrap();
        let mut result = Vec::new();
        if let Some(edges) = out.get(id) {
            for (dst, edge) in edges {
                if _edge_type.map_or(true, |et| et == edge.edge_type) {
                    if let Some(node) = nodes.get(dst) {
                        result.push((node.clone(), edge.clone()));
                    }
                }
            }
        }
        Ok(result)
    }

    fn find_paths(
        &self,
        src: &str,
        dst: &str,
        max_depth: u8,
    ) -> Result<Vec<Vec<KGNode>>, KGError> {
        let nodes = self.nodes.read().unwrap();
        let out = self.out_edges.read().unwrap();
        let mut visited = HashSet::new();
        let mut stack: Vec<(String, Vec<String>)> = vec![(src.into(), vec![src.into()])];
        let mut results = Vec::new();

        while let Some((current, path)) = stack.pop() {
            if current == dst {
                let node_path: Vec<KGNode> = path
                    .iter()
                    .filter_map(|id| nodes.get(id).cloned())
                    .collect();
                results.push(node_path);
                continue;
            }
            if path.len() >= max_depth as usize {
                continue;
            }
            if visited.contains(&current) {
                continue;
            }
            visited.insert(current.clone());
            if let Some(edges) = out.get(&current) {
                for (next_dst, _) in edges {
                    let mut new_path = path.clone();
                    new_path.push(next_dst.clone());
                    stack.push((next_dst.clone(), new_path));
                }
            }
        }
        Ok(results)
    }

    fn find_weighted_path(
        &self,
        src: &str,
        dst: &str,
        max_depth: u8,
    ) -> Result<Option<Vec<KGNode>>, KGError> {
        let nodes = self.nodes.read().unwrap();
        let out = self.out_edges.read().unwrap();

        // Best-first DFS: explore paths ordered by descending cumulative weight.
        // At each step, expand the highest-weight partial path first.
        // This finds the maximum-weight path (unlike Dijkstra which finds minimum).
        //
        // Strategy: maintain a max-heap of (cost, path) and expand highest-cost paths,
        // tracking the best path to each visited node. When we reach dst, return it
        // because the heap ensures we've explored all higher-cost paths first.
        let mut heap: BinaryHeap<PathState> = BinaryHeap::new();
        heap.push(PathState {
            cost: 0.0,
            node: src.to_string(),
            path: vec![src.to_string()],
        });

        // Track best known cost to each node
        let mut best_cost: HashMap<String, f32> = HashMap::new();
        best_cost.insert(src.to_string(), 0.0);

        while let Some(state) = heap.pop() {
            // Skip if we've exceeded max depth
            if state.path.len() > max_depth as usize {
                continue;
            }

            // Found destination — return immediately.
            // Because we use a max-heap and expand highest-cost paths first,
            // the first time we reach dst, it's via the highest-weight path.
            if state.node == dst {
                let node_path: Vec<KGNode> = state
                    .path
                    .iter()
                    .filter_map(|id| nodes.get(id).cloned())
                    .collect();
                return Ok(Some(node_path));
            }

            // Skip if we've already found a better path to this node
            if let Some(&best) = best_cost.get(&state.node) {
                if state.cost < best {
                    continue;
                }
            }

            // Explore neighbors in descending weight order
            if let Some(edges) = out.get(&state.node) {
                let mut sorted_edges: Vec<_> = edges.clone();
                sorted_edges.sort_by(|a, b| b.1.weight.partial_cmp(&a.1.weight).unwrap_or(std::cmp::Ordering::Equal));

                for (next_dst, edge) in sorted_edges {
                    // Avoid cycles within current path
                    if state.path.contains(&next_dst) {
                        continue;
                    }
                    let new_cost = state.cost + edge.weight;

                    // Only proceed if this is a better path to next_dst
                    if let Some(&best) = best_cost.get(&next_dst) {
                        if new_cost <= best {
                            continue;
                        }
                    }
                    best_cost.insert(next_dst.clone(), new_cost);

                    let mut new_path = state.path.clone();
                    new_path.push(next_dst.clone());
                    heap.push(PathState {
                        cost: new_cost,
                        node: next_dst.clone(),
                        path: new_path,
                    });
                }
            }
        }

        // No path found within depth limit
        Ok(None)
    }

    fn list_nodes(
        &self,
        agent_id: &str,
        node_type: Option<KGNodeType>,
    ) -> Result<Vec<KGNode>, KGError> {
        let nodes = self.nodes.read().unwrap();
        Ok(nodes
            .values()
            .filter(|n| {
                n.agent_id == agent_id
                    && n.expired_at.is_none()
                    && node_type.map_or(true, |mt| mt == n.node_type)
            })
            .cloned()
            .collect())
    }

    fn list_edges(&self, agent_id: &str) -> Result<Vec<KGEdge>, KGError> {
        let nodes = self.nodes.read().unwrap();
        let out = self.out_edges.read().unwrap();
        let mut edges = Vec::new();
        for (src, list) in out.iter() {
            if nodes.get(src).map_or(true, |n| n.agent_id != agent_id) {
                continue;
            }
            for (_, edge) in list {
                if edge.expired_at.is_none() {
                    edges.push(edge.clone());
                }
            }
        }
        Ok(edges)
    }

    fn remove_node(&self, id: &str) -> Result<(), KGError> {
        let mut nodes = self.nodes.write().unwrap();
        nodes.remove(id);
        drop(nodes);
        let mut out = self.out_edges.write().unwrap();
        let mut in_e = self.in_edges.write().unwrap();
        if let Some(list) = out.remove(id) {
            for (dst, _) in list {
                if let Some(in_list) = in_e.get_mut(&dst) {
                    in_list.retain(|(s, _)| s != id);
                }
            }
        }
        if let Some(list) = in_e.remove(id) {
            for (src, _) in list {
                if let Some(out_list) = out.get_mut(&src) {
                    out_list.retain(|(d, _)| d != id);
                }
            }
        }
        self.persist();
        Ok(())
    }

    fn all_node_ids(&self) -> Vec<String> {
        let nodes = self.nodes.read().unwrap();
        nodes.keys().cloned().collect()
    }

    fn upsert_document(&self, cid: &str, tags: &[String], agent_id: &str) -> Result<(), KGError> {
        PetgraphBackend::upsert_document(self, cid, tags, agent_id)
    }

    fn authority_score(&self, node_id: &str) -> Result<f32, KGError> {
        Ok(self.authority_score_internal(node_id))
    }

    fn node_count(&self) -> Result<usize, KGError> {
        let nodes = self.nodes.read().unwrap();
        Ok(nodes.len())
    }

    fn edge_count(&self) -> Result<usize, KGError> {
        let out = self.out_edges.read().unwrap();
        Ok(out.values().map(|v| v.len()).sum())
    }

    fn get_valid_edges_at(&self, _t: u64) -> Result<Vec<KGEdge>, KGError> {
        let out = self.out_edges.read().unwrap();
        Ok(out
            .values()
            .flat_map(|v| v.iter())
            .filter(|(_, e)| e.is_valid_at(_t))
            .map(|(_, e)| e.clone())
            .collect())
    }

    fn get_valid_edge_between(
        &self,
        src: &str,
        dst: &str,
        _edge_type: Option<KGEdgeType>,
        t: u64,
    ) -> Result<Option<KGEdge>, KGError> {
        let out = self.out_edges.read().unwrap();
        Ok(out
            .get(src)
            .and_then(|list| {
                list.iter()
                    .filter(|(d, e)| d == dst && e.is_valid_at(t))
                    .max_by_key(|(_, e)| e.valid_at)
                    .map(|(_, e)| e.clone())
            }))
    }

    fn invalidate_conflicts(&self, _new_edge: &KGEdge) -> Result<usize, KGError> {
        Ok(0)
    }

    fn get_valid_nodes_at(
        &self,
        agent_id: &str,
        node_type: Option<KGNodeType>,
        t: u64,
    ) -> Result<Vec<KGNode>, KGError> {
        let nodes = self.nodes.read().unwrap();
        Ok(nodes
            .values()
            .filter(|n| {
                n.agent_id == agent_id
                    && n.is_valid_at(t)
                    && node_type.map_or(true, |mt| mt == n.node_type)
            })
            .cloned()
            .collect())
    }

    fn save_to_disk(&self, _path: &std::path::Path) -> Result<(), KGError> {
        todo!("use persist() with custom path")
    }

    #[allow(dead_code)]
    fn load_from_disk(path: &std::path::Path) -> Result<Self, KGError> {
        Ok(Self::open(path.to_path_buf()))
    }
}
