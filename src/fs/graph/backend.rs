//! Knowledge Graph Backend — PetgraphBackend implementation.
//!
//! Persistence strategy:
//! - Runtime: redb 4.0 ACID KV store — O(1) per write, crash-safe
//! - Export:  JSON via `save_to_disk`/`load_from_disk` — portable, human-readable
//!
//! Edge keys use 4-part format: `"src|dst|type_debug|created_at"` to preserve
//! full temporal history across restarts (system-v2.md axiom 8: causality).

use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::RwLock;

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
    db: Option<std::sync::Arc<Database>>,
}

impl PetgraphBackend {
    pub fn new() -> Self {
        Self {
            nodes: RwLock::new(HashMap::new()),
            out_edges: RwLock::new(HashMap::new()),
            in_edges: RwLock::new(HashMap::new()),
            db: None,
        }
    }

    pub fn open(root: std::path::PathBuf) -> Self {
        let db_path = root.join("kg.redb");

        use std::collections::HashMap;
        use std::sync::{Arc, Mutex, OnceLock};
        static OPEN_DATABASES: OnceLock<Mutex<HashMap<std::path::PathBuf, Arc<Database>>>> = OnceLock::new();

        let db_map = OPEN_DATABASES.get_or_init(|| Mutex::new(HashMap::new()));
        let db = {
            let mut db_guard = db_map.lock().unwrap();
            if let Some(existing) = db_guard.get(&db_path) {
                Some(Arc::clone(existing))
            } else {
                let database = if db_path.exists() {
                    Database::open(&db_path).expect("Failed to open redb database")
                } else {
                    Database::create(&db_path).expect("Failed to create redb database")
                };
                let arc_db = Arc::new(database);
                db_guard.insert(db_path.clone(), Arc::clone(&arc_db));
                Some(arc_db)
            }
        };

        let (nodes, out_e, in_e) = if let Some(ref database) = db {
            match Self::load_from_redb(database) {
                Ok(triple) => triple,
                Err(e) => {
                    tracing::warn!("Failed to load KG from redb, starting fresh: {}", e);
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
            db,
        }
    }

    // ── Key format ─────────────────────────────────────────────────────────

    /// v2 edge key: includes `created_at` so each temporal version is distinct.
    fn edge_key(src: &str, dst: &str, edge_type: &KGEdgeType, created_at: u64) -> String {
        format!("{}|{}|{:?}|{}", src, dst, edge_type, created_at)
    }

    // ── Single-record persistence ──────────────────────────────────────────

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

    // ── Atomic batch persistence ───────────────────────────────────────────

    /// Persist a new edge + all invalidated predecessors in one ACID transaction.
    fn persist_add_edge_atomic(&self, new_edge: &KGEdge, invalidated: &[KGEdge]) {
        let Some(ref db) = self.db else { return };
        let write_txn = db.begin_write().expect("Failed to begin write txn");
        {
            let mut table = write_txn.open_table(KG_EDGES).expect("Failed to open edges table");
            for edge in invalidated {
                let key = Self::edge_key(&edge.src, &edge.dst, &edge.edge_type, edge.created_at);
                let data = serde_json::to_vec(edge).expect("Failed to serialize edge");
                table.insert(key.as_str(), data.as_slice()).expect("Failed to insert edge");
            }
            let key = Self::edge_key(&new_edge.src, &new_edge.dst, &new_edge.edge_type, new_edge.created_at);
            let data = serde_json::to_vec(new_edge).expect("Failed to serialize edge");
            table.insert(key.as_str(), data.as_slice()).expect("Failed to insert edge");
        }
        write_txn.commit().expect("Failed to commit edge batch");
    }

    /// Batch-remove edges from redb in one transaction.
    fn remove_edges_from_db(&self, edges: &[KGEdge]) {
        let Some(ref db) = self.db else { return };
        if edges.is_empty() { return; }
        let write_txn = db.begin_write().expect("Failed to begin write txn");
        {
            let mut table = write_txn.open_table(KG_EDGES).expect("Failed to open edges table");
            for edge in edges {
                let key = Self::edge_key(&edge.src, &edge.dst, &edge.edge_type, edge.created_at);
                table.remove(key.as_str()).ok();
            }
        }
        write_txn.commit().expect("Failed to commit edge removal");
    }

    /// Remove a node + all its edges in one ACID transaction.
    fn remove_node_and_edges_from_db(&self, node_id: &str, edges: &[KGEdge]) {
        let Some(ref db) = self.db else { return };
        let write_txn = db.begin_write().expect("Failed to begin write txn");
        {
            let mut table = write_txn.open_table(KG_NODES).expect("Failed to open nodes table");
            table.remove(node_id).ok();
        }
        if !edges.is_empty() {
            let mut table = write_txn.open_table(KG_EDGES).expect("Failed to open edges table");
            for edge in edges {
                let key = Self::edge_key(&edge.src, &edge.dst, &edge.edge_type, edge.created_at);
                table.remove(key.as_str()).ok();
            }
        }
        write_txn.commit().expect("Failed to commit node removal");
    }

    // ── Conflict invalidation (private) ────────────────────────────────────

    /// In-memory invalidation: sets `invalid_at` on conflicting edges, returns clones.
    /// Does NOT persist — caller is responsible for persistence.
    fn invalidate_conflicts_internal(&self, new_edge: &KGEdge) -> Vec<KGEdge> {
        let now = now_ms();
        let mut invalidated = Vec::new();
        {
            let mut out = self.out_edges.write().unwrap();
            if let Some(list) = out.get_mut(&new_edge.src) {
                for (dst, edge) in list.iter_mut() {
                    if *dst == new_edge.dst
                        && edge.edge_type == new_edge.edge_type
                        && edge.invalid_at.is_none()
                    {
                        edge.invalid_at = Some(now);
                        invalidated.push(edge.clone());
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
        invalidated
    }

    // ── Load / Migration ───────────────────────────────────────────────────

    fn load_from_redb(
        database: &std::sync::Arc<Database>,
    ) -> Result<DiskGraph, KGError> {
        let read_txn = database.begin_read().expect("Failed to begin read txn");

        let mut nodes = HashMap::new();
        let mut out_edges: HashMap<String, Vec<(String, KGEdge)>> = HashMap::new();
        let mut in_edges: HashMap<String, Vec<(String, KGEdge)>> = HashMap::new();

        if let Ok(table) = read_txn.open_table(KG_NODES) {
            if let Ok(iter) = table.range::<&str>(..) {
                for (node_id, value) in iter.flatten() {
                    let node_id_str = node_id.value();
                    if let Ok(node) = serde_json::from_slice::<KGNode>(value.value()) {
                        nodes.insert(node_id_str.to_string(), node);
                    }
                }
            }
        }

        if let Ok(table) = read_txn.open_table(KG_EDGES) {
            if let Ok(iter) = table.range::<&str>(..) {
                for (key, value) in iter.flatten() {
                    let key_str = key.value();
                    if let Ok(edge) = serde_json::from_slice::<KGEdge>(value.value()) {
                        let parts: Vec<&str> = key_str.split('|').collect();
                        if parts.len() == 4 {
                            let src = parts[0].to_string();
                            let dst = parts[1].to_string();
                            out_edges.entry(src.clone()).or_default().push((dst.clone(), edge.clone()));
                            in_edges.entry(dst).or_default().push((src, edge));
                        }
                    }
                }
            }
        }

        Ok((nodes, out_edges, in_edges))
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

    // ── Domain logic ───────────────────────────────────────────────────────

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

// ── KnowledgeGraph trait implementation ────────────────────────────────────────

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
        let invalidated = self.invalidate_conflicts_internal(&edge);
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
        self.persist_add_edge_atomic(&edge_clone, &invalidated);
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

        let mut heap: BinaryHeap<PathState> = BinaryHeap::new();
        heap.push(PathState {
            cost: 0.0,
            node: src.to_string(),
            path: vec![src.to_string()],
        });

        let mut best_cost: HashMap<String, f32> = HashMap::new();
        best_cost.insert(src.to_string(), 0.0);

        while let Some(state) = heap.pop() {
            if state.path.len() > max_depth as usize {
                continue;
            }

            if state.node == dst {
                let node_path: Vec<KGNode> = state
                    .path
                    .iter()
                    .filter_map(|id| nodes.get(id).cloned())
                    .collect();
                return Ok(Some(node_path));
            }

            if let Some(&best) = best_cost.get(&state.node) {
                if state.cost < best {
                    continue;
                }
            }

            if let Some(edges) = out.get(&state.node) {
                let mut sorted_edges: Vec<_> = edges.clone();
                sorted_edges.sort_by(|a, b| b.1.weight.partial_cmp(&a.1.weight).unwrap_or(std::cmp::Ordering::Equal));

                for (next_dst, edge) in sorted_edges {
                    if state.path.contains(&next_dst) {
                        continue;
                    }
                    let new_cost = state.cost + edge.weight;

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
        let mut removed_edges: Vec<KGEdge> = Vec::new();
        if let Some(list) = out.remove(id) {
            for (dst, edge) in list {
                removed_edges.push(edge);
                if let Some(in_list) = in_e.get_mut(&dst) {
                    in_list.retain(|(s, _)| s != id);
                }
            }
        }
        if let Some(list) = in_e.remove(id) {
            for (src, edge) in list {
                removed_edges.push(edge);
                if let Some(out_list) = out.get_mut(&src) {
                    out_list.retain(|(d, _)| d != id);
                }
            }
        }
        drop(out);
        drop(in_e);
        self.remove_node_and_edges_from_db(id, &removed_edges);
        Ok(())
    }

    fn remove_edge(&self, src: &str, dst: &str, edge_type: Option<KGEdgeType>) -> Result<(), KGError> {
        let mut out = self.out_edges.write().unwrap();
        let mut in_e = self.in_edges.write().unwrap();

        let mut removed_edges: Vec<KGEdge> = Vec::new();
        if let Some(list) = out.get_mut(src) {
            let mut keep = Vec::new();
            for item in list.drain(..) {
                if item.0 == dst && edge_type.is_none_or(|et| et == item.1.edge_type) {
                    removed_edges.push(item.1);
                } else {
                    keep.push(item);
                }
            }
            *list = keep;
        }
        if let Some(list) = in_e.get_mut(dst) {
            list.retain(|(s, e)| !(s == src && edge_type.is_none_or(|et| et == e.edge_type)));
        }
        drop(out);
        drop(in_e);
        if removed_edges.is_empty() {
            return Err(KGError::NodeNotFound(format!("edge {}→{} not found", src, dst)));
        }
        self.remove_edges_from_db(&removed_edges);
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
        self.persist_node(id, &node_clone);
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
        let invalidated = self.invalidate_conflicts_internal(new_edge);
        let count = invalidated.len();
        if !invalidated.is_empty() {
            // Persist the invalidated edges so `invalid_at` survives restart
            let Some(ref db) = self.db else { return Ok(count); };
            let write_txn = db.begin_write().expect("Failed to begin write txn");
            {
                let mut table = write_txn.open_table(KG_EDGES).expect("Failed to open edges table");
                for edge in &invalidated {
                    let key = Self::edge_key(&edge.src, &edge.dst, &edge.edge_type, edge.created_at);
                    let data = serde_json::to_vec(edge).expect("Failed to serialize edge");
                    table.insert(key.as_str(), data.as_slice()).expect("Failed to insert edge");
                }
            }
            write_txn.commit().expect("Failed to commit invalidation");
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
