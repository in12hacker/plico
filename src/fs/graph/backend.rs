//! Knowledge Graph Backend — PetgraphBackend implementation.

use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::RwLock;
use std::path::PathBuf;

use redb::{Database, ReadableDatabase, TableDefinition};

use crate::fs::graph::types::{DiskGraph, KGNode, KGEdge, KGEdgeType, KGNodeType};
use crate::fs::graph::{KGError, KnowledgeGraph};

const KG_NODES: TableDefinition<&str, &[u8]> = TableDefinition::new("kg_nodes");
const KG_EDGES: TableDefinition<&str, &[u8]> = TableDefinition::new("kg_edges");

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
        Some(self.cmp(other))
    }
}

impl Ord for PathState {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.cost.partial_cmp(&other.cost).unwrap_or(std::cmp::Ordering::Equal).reverse()
    }
}

/// In-memory knowledge graph backed by HashMap.
/// Thread-safe via RwLock. Persisted to disk via redb 4.0 with JSON fallback.
pub struct PetgraphBackend {
    nodes: RwLock<HashMap<String, KGNode>>,
    out_edges: RwLock<HashMap<String, Vec<(String, KGEdge)>>>,
    in_edges: RwLock<HashMap<String, Vec<(String, KGEdge)>>>,
    root: Option<PathBuf>,
    db: Option<std::sync::Arc<Database>>,
}

impl PetgraphBackend {
    pub fn new() -> Self {
        Self {
            nodes: RwLock::new(HashMap::new()),
            out_edges: RwLock::new(HashMap::new()),
            in_edges: RwLock::new(HashMap::new()),
            root: None,
            db: None,
        }
    }

    pub fn open(root: std::path::PathBuf) -> Self {
        let db_path = root.join("kg.redb");
        let nodes_path = root.join("kg_nodes.json");

        // Determine if we need migration from JSON to redb
        let needs_migration = nodes_path.exists() && !db_path.exists();

        // Use a static map to track open databases per path, wrapped in Arc for sharing
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex, OnceLock};
        static OPEN_DATABASES: OnceLock<Mutex<HashMap<std::path::PathBuf, Arc<Database>>>> = OnceLock::new();

        let db_map = OPEN_DATABASES.get_or_init(|| Mutex::new(HashMap::new()));
        let db = {
            let mut db_guard = db_map.lock().unwrap();
            if let Some(existing) = db_guard.get(&db_path) {
                // Return existing database (Arc clone is cheap)
                Some(Arc::clone(existing))
            } else {
                let db = if db_path.exists() {
                    Database::open(&db_path).expect("Failed to open redb database")
                } else {
                    Database::create(&db_path).expect("Failed to create redb database")
                };
                let arc_db = Arc::new(db);
                db_guard.insert(db_path.clone(), Arc::clone(&arc_db));
                Some(arc_db)
            }
        };

        let (nodes, out_e, in_e) = if needs_migration {
            // Migrate from JSON to redb
            match Self::load_from_json(&root) {
                Ok((nodes, out_edges, in_edges)) => {
                    tracing::info!("Migrating KG from JSON to redb...");
                    if let Some(ref database) = db {
                        // Persist all nodes to redb
                        {
                            let write_txn = database.begin_write().expect("Failed to begin write txn");
                            let mut table = write_txn.open_table(KG_NODES).expect("Failed to open nodes table");
                            for (node_id, node) in &nodes {
                                let data = serde_json::to_vec(node).expect("Failed to serialize node");
                                table.insert(node_id.as_str(), data.as_slice()).expect("Failed to insert node");
                            }
                            drop(table);
                            write_txn.commit().expect("Failed to commit nodes migration");
                        }
                        // Persist all edges to redb
                        {
                            let write_txn = database.begin_write().expect("Failed to begin write txn");
                            let mut table = write_txn.open_table(KG_EDGES).expect("Failed to open edges table");
                            for (src, list) in &out_edges {
                                for (dst, edge) in list {
                                    let key = Self::edge_key(src, dst, &edge.edge_type);
                                    let data = serde_json::to_vec(edge).expect("Failed to serialize edge");
                                    table.insert(key.as_str(), data.as_slice()).expect("Failed to insert edge");
                                }
                            }
                            drop(table);
                            write_txn.commit().expect("Failed to commit edges migration");
                        }
                        tracing::info!("KG migration to redb complete");
                    }
                    (nodes, out_edges, in_edges)
                }
                Err(e) => {
                    tracing::warn!("Failed to load KG from JSON for migration, starting fresh: {}", e);
                    (HashMap::new(), HashMap::new(), HashMap::new())
                }
            }
        } else if let Some(ref database) = db {
            match Self::load_from_redb(database) {
                Ok(triple) => triple,
                Err(_) => {
                    // Try JSON fallback
                    match Self::load_from_json(&root) {
                        Ok(triple) => triple,
                        Err(e) => {
                            tracing::warn!("Failed to load KG, starting fresh: {}", e);
                            (HashMap::new(), HashMap::new(), HashMap::new())
                        }
                    }
                }
            }
        } else {
            (HashMap::new(), HashMap::new(), HashMap::new())
        };

        Self {
            nodes: RwLock::new(nodes),
            out_edges: RwLock::new(out_e),
            in_edges: RwLock::new(in_e),
            root: Some(root),
            db,
        }
    }

    fn edge_key(src: &str, dst: &str, edge_type: &KGEdgeType) -> String {
        format!("{}|{}|{:?}", src, dst, edge_type)
    }

    fn persist_node(&self, node_id: &str, node: &KGNode) {
        let Some(ref db) = self.db else { return };
        let write_txn = db.begin_write().expect("Failed to begin write txn");
        {
            let mut table = write_txn.open_table(KG_NODES).expect("Failed to open nodes table");
            let data = serde_json::to_vec(node).expect("Failed to serialize node");
            table.insert(node_id, data.as_slice()).expect("Failed to insert node");
        }
        write_txn.commit().expect("Failed to commit node");
    }

    fn persist_edge(&self, src: &str, dst: &str, edge: &KGEdge) {
        let Some(ref db) = self.db else { return };
        let key = Self::edge_key(src, dst, &edge.edge_type);
        let write_txn = db.begin_write().expect("Failed to begin write txn");
        {
            let mut table = write_txn.open_table(KG_EDGES).expect("Failed to open edges table");
            let data = serde_json::to_vec(edge).expect("Failed to serialize edge");
            table.insert(key.as_str(), data.as_slice()).expect("Failed to insert edge");
        }
        write_txn.commit().expect("Failed to commit edge");
    }

    fn remove_node_from_db(&self, node_id: &str) {
        let Some(ref db) = self.db else { return };
        let write_txn = db.begin_write().expect("Failed to begin write txn");
        {
            let mut table = write_txn.open_table(KG_NODES).expect("Failed to open nodes table");
            table.remove(node_id).ok();
        }
        write_txn.commit().expect("Failed to commit node removal");
    }

    fn remove_edge_from_db(&self, src: &str, dst: &str, edge_type: &KGEdgeType) {
        let Some(ref db) = self.db else { return };
        let key = Self::edge_key(src, dst, edge_type);
        let write_txn = db.begin_write().expect("Failed to begin write txn");
        {
            let mut table = write_txn.open_table(KG_EDGES).expect("Failed to open edges table");
            table.remove(key.as_str()).ok();
        }
        write_txn.commit().expect("Failed to commit edge removal");
    }

    fn load_from_redb(
        database: &std::sync::Arc<Database>,
    ) -> Result<(HashMap<String, KGNode>, HashMap<String, Vec<(String, KGEdge)>>, HashMap<String, Vec<(String, KGEdge)>>), KGError> {
        let read_txn = database.begin_read().expect("Failed to begin read txn");

        let mut nodes = HashMap::new();
        let mut out_edges: HashMap<String, Vec<(String, KGEdge)>> = HashMap::new();
        let mut in_edges: HashMap<String, Vec<(String, KGEdge)>> = HashMap::new();

        // Load nodes - use range() to iterate all entries
        if let Ok(table) = read_txn.open_table(KG_NODES) {
            // Use unbounded range .. to get all entries (K=&str, so &str implements RangeBounds<&str>)
            if let Ok(iter) = table.range::<&str>(..) {
                for item in iter {
                    if let Ok((node_id, value)) = item {
                        // AccessGuard with &str key returns &str directly
                        let node_id_str = node_id.value();
                        if let Ok(node) = serde_json::from_slice::<KGNode>(value.value()) {
                            nodes.insert(node_id_str.to_string(), node);
                        }
                    }
                }
            }
        }

        // Load edges - use range() to iterate all entries
        if let Ok(table) = read_txn.open_table(KG_EDGES) {
            if let Ok(iter) = table.range::<&str>(..) {
                for item in iter {
                    if let Ok((key, value)) = item {
                        // AccessGuard with &str key returns &str directly
                        let key_str = key.value();
                        if let Ok(edge) = serde_json::from_slice::<KGEdge>(value.value()) {
                            let parts: Vec<&str> = key_str.split('|').collect();
                            if parts.len() == 3 {
                                let src = parts[0].to_string();
                                let dst = parts[1].to_string();
                                out_edges.entry(src.clone()).or_default().push((dst.clone(), edge.clone()));
                                in_edges.entry(dst).or_default().push((src, edge));
                            }
                        }
                    }
                }
            }
        }

        Ok((nodes, out_edges, in_edges))
    }

    fn load_from_json(root: &std::path::Path) -> Result<(HashMap<String, KGNode>, HashMap<String, Vec<(String, KGEdge)>>, HashMap<String, Vec<(String, KGEdge)>>), KGError> {
        let nodes_path = root.join("kg_nodes.json");
        let edges_path = root.join("kg_edges.json");
        Self::load_from_disk_internal(&nodes_path, &edges_path)
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
        // Legacy JSON persist kept for backward compatibility when db is None
        let Some(ref path) = self.root else { return };
        let nodes_path = path.join("kg_nodes.json");
        let edges_path = path.join("kg_edges.json");

        let nodes = self.nodes.read().unwrap();
        if let Ok(json) = serde_json::to_string(&*nodes) {
            let tmp = nodes_path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &nodes_path);
            }
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
            let tmp = edges_path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &edges_path);
            }
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
            tenant_id: "default".to_string(),
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
                        && shared_tag_count(&n.properties, tags) >= 1
                })
                .map(|n| (n.id.clone(), jaccard_weight(&n.properties, tags)))
                .collect()
        };

        self.add_node(node)?;

        for (other_id, w) in candidates {
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
        if max_degree <= 1 {
            return if degree > 0 { 1.0 } else { 0.0 };
        }
        ((degree as f32).ln() / (max_degree as f32).ln()).clamp(0.0, 1.0)
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

fn total_tag_count(props: &serde_json::Value) -> usize {
    props
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| arr.len())
        .unwrap_or(0)
}

fn jaccard_weight(props: &serde_json::Value, tags: &[String]) -> f32 {
    let shared = shared_tag_count(props, tags) as f32;
    let total_a = total_tag_count(props).max(1) as f32;
    let total_b = tags.len().max(1) as f32;
    shared / total_a.max(total_b)
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
        let node_id = node.id.clone();
        let node_clone = node.clone();
        let mut nodes = self.nodes.write().unwrap();
        nodes.insert(node_id.clone(), node);
        drop(nodes);
        self.persist_node(&node_id, &node_clone);
        Ok(())
    }

    fn add_edge(&self, edge: KGEdge) -> Result<(), KGError> {
        self.invalidate_conflicts(&edge)?;
        let edge_clone = edge.clone();
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
        self.persist_edge(&edge_clone.src, &edge_clone.dst, &edge_clone);
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
        let in_e = self.in_edges.read().unwrap();
        let nodes = self.nodes.read().unwrap();
        let mut result = Vec::new();
        if let Some(edges) = out.get(id) {
            for (dst, edge) in edges {
                if _edge_type.is_none_or(|et| et == edge.edge_type) {
                    if let Some(node) = nodes.get(dst) {
                        result.push((node.clone(), edge.clone()));
                    }
                }
            }
        }
        if let Some(edges) = in_e.get(id) {
            for (src, edge) in edges {
                if _edge_type.is_none_or(|et| et == edge.edge_type) {
                    if let Some(node) = nodes.get(src) {
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
                    && node_type.is_none_or(|mt| mt == n.node_type)
            })
            .cloned()
            .collect())
    }

    fn list_edges(&self, agent_id: &str) -> Result<Vec<KGEdge>, KGError> {
        let nodes = self.nodes.read().unwrap();
        let out = self.out_edges.read().unwrap();
        let mut edges = Vec::new();
        for (src, list) in out.iter() {
            if nodes.get(src).is_none_or(|n| n.agent_id != agent_id) {
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
        drop(out);
        drop(in_e);
        self.remove_node_from_db(id);
        Ok(())
    }

    fn remove_edge(&self, src: &str, dst: &str, edge_type: Option<KGEdgeType>) -> Result<(), KGError> {
        let mut out = self.out_edges.write().unwrap();
        let mut in_e = self.in_edges.write().unwrap();

        // Track which edge type to remove for redb
        let mut removed_type: Option<KGEdgeType> = None;
        let removed = if let Some(list) = out.get_mut(src) {
            let before = list.len();
            list.retain(|(d, e)| {
                let keep = !(d == dst && edge_type.is_none_or(|et| et == e.edge_type));
                if !keep && removed_type.is_none() {
                    removed_type = Some(e.edge_type);
                }
                keep
            });
            before - list.len()
        } else {
            0
        };
        if let Some(list) = in_e.get_mut(dst) {
            list.retain(|(s, e)| !(s == src && edge_type.is_none_or(|et| et == e.edge_type)));
        }
        drop(out);
        drop(in_e);
        if removed == 0 {
            return Err(KGError::NodeNotFound(format!("edge {}→{} not found", src, dst)));
        }
        if let Some(et) = removed_type {
            self.remove_edge_from_db(src, dst, &et);
        }
        Ok(())
    }

    fn update_node(&self, id: &str, label: Option<&str>, properties: Option<serde_json::Value>) -> Result<(), KGError> {
        let mut nodes = self.nodes.write().unwrap();
        let node = nodes.get_mut(id).ok_or_else(|| KGError::NodeNotFound(id.to_string()))?;
        if let Some(l) = label {
            node.label = l.to_string();
        }
        if let Some(props) = properties {
            if let (Some(existing), Some(new)) = (node.properties.as_object_mut(), props.as_object()) {
                for (k, v) in new {
                    existing.insert(k.clone(), v.clone());
                }
            } else {
                node.properties = props;
            }
        }
        let node_clone = node.clone();
        drop(nodes);
        self.persist_node(&id, &node_clone);
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

    fn invalidate_conflicts(&self, new_edge: &KGEdge) -> Result<usize, KGError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut count = 0;
        {
            let mut out = self.out_edges.write().unwrap();
            if let Some(list) = out.get_mut(&new_edge.src) {
                for (dst, edge) in list.iter_mut() {
                    if *dst == new_edge.dst
                        && edge.edge_type == new_edge.edge_type
                        && edge.invalid_at.is_none()
                    {
                        edge.invalid_at = Some(now);
                        count += 1;
                    }
                }
            }
        }
        {
            let mut inc = self.in_edges.write().unwrap();
            if let Some(list) = inc.get_mut(&new_edge.dst) {
                for (src, edge) in list.iter_mut() {
                    if *src == new_edge.src
                        && edge.edge_type == new_edge.edge_type
                        && edge.invalid_at.is_none()
                    {
                        edge.invalid_at = Some(now);
                    }
                }
            }
        }
        Ok(count)
    }

    fn edge_history(
        &self,
        src: &str,
        dst: &str,
        edge_type: Option<KGEdgeType>,
    ) -> Result<Vec<KGEdge>, KGError> {
        let out = self.out_edges.read().unwrap();
        let edges: Vec<KGEdge> = out
            .get(src)
            .map(|list| {
                list.iter()
                    .filter(|(d, e)| {
                        d == dst && edge_type.as_ref().is_none_or(|et| *et == e.edge_type)
                    })
                    .map(|(_, e)| e.clone())
                    .collect()
            })
            .unwrap_or_default();
        Ok(edges)
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
                    && node_type.is_none_or(|mt| mt == n.node_type)
            })
            .cloned()
            .collect())
    }

    fn save_to_disk(&self, path: &std::path::Path) -> Result<(), KGError> {
        std::fs::create_dir_all(path)?;
        let nodes_path = path.join("kg_nodes.json");
        let edges_path = path.join("kg_edges.json");

        let nodes = self.nodes.read().unwrap();
        let json = serde_json::to_string_pretty(&*nodes)?;
        let tmp = nodes_path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &nodes_path)?;
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
        let json = serde_json::to_string_pretty(&records)?;
        let tmp = edges_path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &edges_path)?;
        Ok(())
    }

    fn load_from_disk(&self, path: &std::path::Path) -> Result<(), KGError> {
        let nodes_path = path.join("kg_nodes.json");
        let edges_path = path.join("kg_edges.json");
        let (new_nodes, new_out, new_in) = Self::load_from_disk_internal(&nodes_path, &edges_path)?;
        *self.nodes.write().unwrap() = new_nodes;
        *self.out_edges.write().unwrap() = new_out;
        *self.in_edges.write().unwrap() = new_in;
        Ok(())
    }
}
