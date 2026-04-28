//! Causal Memory Graph — tracks cause-effect chains and supersession between memories.
//!
//! Each `MemoryEntry` can optionally carry:
//! - `causal_parent`: the memory that causally led to its creation
//! - `supersedes`: the memory this one replaces (contradiction resolution)
//!
//! The `CausalGraph` provides efficient traversal: ancestors, descendants,
//! supersession chains, and root-cause analysis.

use std::collections::{HashMap, HashSet, VecDeque};
use crate::memory::layered::MemoryEntry;

/// Relationship type between two memories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CausalEdge {
    /// A caused B (A is causal_parent of B).
    Caused,
    /// B supersedes A (B.supersedes == A.id).
    Supersedes,
}

/// A lightweight in-memory causal graph built from `MemoryEntry` slices.
///
/// Construction is O(n), all traversals are O(reachable nodes).
/// The graph is immutable once built — rebuild after mutations.
#[derive(Debug)]
pub struct CausalGraph {
    children: HashMap<String, Vec<(String, CausalEdge)>>,
    parents: HashMap<String, Vec<(String, CausalEdge)>>,
    all_ids: HashSet<String>,
}

impl CausalGraph {
    /// Build a graph from a set of memory entries.
    pub fn build(entries: &[MemoryEntry]) -> Self {
        let mut children: HashMap<String, Vec<(String, CausalEdge)>> = HashMap::new();
        let mut parents: HashMap<String, Vec<(String, CausalEdge)>> = HashMap::new();
        let mut all_ids = HashSet::new();

        for entry in entries {
            all_ids.insert(entry.id.clone());

            if let Some(ref parent_id) = entry.causal_parent {
                children
                    .entry(parent_id.clone())
                    .or_default()
                    .push((entry.id.clone(), CausalEdge::Caused));
                parents
                    .entry(entry.id.clone())
                    .or_default()
                    .push((parent_id.clone(), CausalEdge::Caused));
            }

            if let Some(ref old_id) = entry.supersedes {
                children
                    .entry(old_id.clone())
                    .or_default()
                    .push((entry.id.clone(), CausalEdge::Supersedes));
                parents
                    .entry(entry.id.clone())
                    .or_default()
                    .push((old_id.clone(), CausalEdge::Supersedes));
            }
        }

        Self {
            children,
            parents,
            all_ids,
        }
    }

    /// Walk upward from `start_id` following *causal* edges only, returning
    /// the full ancestor chain (oldest first).
    pub fn ancestors(&self, start_id: &str) -> Vec<String> {
        self.walk_backward(start_id, Some(CausalEdge::Caused))
    }

    /// Walk upward from `start_id` following *supersedes* edges only,
    /// returning the full version chain (oldest first).
    pub fn supersession_chain(&self, start_id: &str) -> Vec<String> {
        self.walk_backward(start_id, Some(CausalEdge::Supersedes))
    }

    /// Walk downward from `start_id` to find all descendants (BFS, all edge types).
    pub fn descendants(&self, start_id: &str) -> Vec<String> {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut result = Vec::new();

        queue.push_back(start_id.to_string());
        visited.insert(start_id.to_string());

        while let Some(current) = queue.pop_front() {
            if let Some(kids) = self.children.get(&current) {
                for (kid_id, _) in kids {
                    if visited.insert(kid_id.clone()) {
                        result.push(kid_id.clone());
                        queue.push_back(kid_id.clone());
                    }
                }
            }
        }

        result
    }

    /// Find the causal root(s) — entries with no causal_parent.
    /// Walks backward from `start_id` following causal edges until no more parents.
    pub fn root_cause(&self, start_id: &str) -> String {
        let chain = self.ancestors(start_id);
        chain.into_iter().next().unwrap_or_else(|| start_id.to_string())
    }

    /// Get the latest version of a memory following supersession chains forward.
    pub fn latest_version(&self, start_id: &str) -> String {
        let mut current = start_id.to_string();
        let mut visited = HashSet::new();
        visited.insert(current.clone());

        loop {
            let next = self
                .children
                .get(&current)
                .and_then(|kids| {
                    kids.iter()
                        .find(|(_, edge)| *edge == CausalEdge::Supersedes)
                        .map(|(id, _)| id.clone())
                });
            match next {
                Some(id) if visited.insert(id.clone()) => current = id,
                _ => break,
            }
        }

        current
    }

    /// Check if an entry has been superseded by a newer version.
    pub fn is_superseded(&self, entry_id: &str) -> bool {
        self.children
            .get(entry_id)
            .map(|kids| kids.iter().any(|(_, e)| *e == CausalEdge::Supersedes))
            .unwrap_or(false)
    }

    /// Count all nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.all_ids.len()
    }

    /// Count all edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.children.values().map(|v| v.len()).sum()
    }

    /// Return the direct children of an entry with their edge types.
    pub fn direct_children(&self, entry_id: &str) -> Vec<(String, CausalEdge)> {
        self.children
            .get(entry_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Return the direct parents of an entry with their edge types.
    pub fn direct_parents(&self, entry_id: &str) -> Vec<(String, CausalEdge)> {
        self.parents
            .get(entry_id)
            .cloned()
            .unwrap_or_default()
    }

    /// BFS shortest path distance between two entries (undirected, all edge types).
    /// Returns None if no path exists.
    pub fn shortest_path_len(&self, from: &str, to: &str) -> Option<usize> {
        if from == to {
            return Some(0);
        }
        if !self.all_ids.contains(from) || !self.all_ids.contains(to) {
            return None;
        }
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back((from.to_string(), 0usize));
        visited.insert(from.to_string());

        while let Some((current, dist)) = queue.pop_front() {
            let neighbors = self.children.get(&current).into_iter().flatten()
                .chain(self.parents.get(&current).into_iter().flatten());
            for (neighbor_id, _) in neighbors {
                if neighbor_id == to {
                    return Some(dist + 1);
                }
                if visited.insert(neighbor_id.clone()) {
                    queue.push_back((neighbor_id.clone(), dist + 1));
                }
            }
        }
        None
    }

    /// Find common causal ancestors of two entries.
    pub fn common_ancestors(&self, a: &str, b: &str) -> Vec<String> {
        let ancestors_a: HashSet<String> = self.ancestors(a).into_iter().collect();
        let ancestors_b: HashSet<String> = self.ancestors(b).into_iter().collect();
        ancestors_a.intersection(&ancestors_b).cloned().collect()
    }

    fn walk_backward(&self, start_id: &str, filter: Option<CausalEdge>) -> Vec<String> {
        let mut chain = Vec::new();
        let mut current = start_id.to_string();
        let mut visited = HashSet::new();
        visited.insert(current.clone());

        loop {
            let next = self.parents.get(&current).and_then(|parents| {
                parents.iter().find_map(|(pid, edge)| {
                    if filter.is_none_or(|f| f == *edge) {
                        Some(pid.clone())
                    } else {
                        None
                    }
                })
            });
            match next {
                Some(pid) if visited.insert(pid.clone()) => {
                    chain.push(pid.clone());
                    current = pid;
                }
                _ => break,
            }
        }

        chain.reverse();
        chain
    }
}

/// Build a causal chain prompt for LLM context injection.
/// Given a target memory and its ancestor chain, produces a string
/// that explains the causal reasoning path.
pub fn causal_context_prompt(
    target: &MemoryEntry,
    ancestors: &[MemoryEntry],
) -> String {
    if ancestors.is_empty() {
        return format!(
            "Memory: {}",
            target.content.display()
        );
    }

    let mut lines = Vec::with_capacity(ancestors.len() + 2);
    lines.push("Causal chain (oldest → newest):".to_string());
    for (i, ancestor) in ancestors.iter().enumerate() {
        lines.push(format!("  {}. [{}] {}", i + 1, ancestor.id, ancestor.content.display()));
    }
    lines.push(format!(
        "  → Current [{}]: {}",
        target.id,
        target.content.display()
    ));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::layered::MemoryEntry;

    fn make_entry(id: &str, causal_parent: Option<&str>, supersedes: Option<&str>) -> MemoryEntry {
        let mut entry = MemoryEntry::ephemeral("test-agent", format!("content-{}", id));
        entry.id = id.to_string();
        entry.causal_parent = causal_parent.map(|s| s.to_string());
        entry.supersedes = supersedes.map(|s| s.to_string());
        entry
    }

    #[test]
    fn test_empty_graph() {
        let graph = CausalGraph::build(&[]);
        assert_eq!(graph.node_count(), 0);
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn test_single_causal_chain() {
        // A → B → C (causal parent chain)
        let entries = vec![
            make_entry("A", None, None),
            make_entry("B", Some("A"), None),
            make_entry("C", Some("B"), None),
        ];
        let graph = CausalGraph::build(&entries);

        assert_eq!(graph.node_count(), 3);
        assert_eq!(graph.edge_count(), 2);

        let ancestors = graph.ancestors("C");
        assert_eq!(ancestors, vec!["A", "B"]);

        let root = graph.root_cause("C");
        assert_eq!(root, "A");

        let descendants = graph.descendants("A");
        assert_eq!(descendants.len(), 2);
        assert!(descendants.contains(&"B".to_string()));
        assert!(descendants.contains(&"C".to_string()));
    }

    #[test]
    fn test_supersession_chain() {
        // V1 → V2 → V3 (supersession)
        let entries = vec![
            make_entry("V1", None, None),
            make_entry("V2", None, Some("V1")),
            make_entry("V3", None, Some("V2")),
        ];
        let graph = CausalGraph::build(&entries);

        let chain = graph.supersession_chain("V3");
        assert_eq!(chain, vec!["V1", "V2"]);

        let latest = graph.latest_version("V1");
        assert_eq!(latest, "V3");

        assert!(graph.is_superseded("V1"));
        assert!(graph.is_superseded("V2"));
        assert!(!graph.is_superseded("V3"));
    }

    #[test]
    fn test_mixed_causal_and_supersession() {
        // A causes B, B is superseded by C
        let entries = vec![
            make_entry("A", None, None),
            make_entry("B", Some("A"), None),
            make_entry("C", None, Some("B")),
        ];
        let graph = CausalGraph::build(&entries);

        assert_eq!(graph.ancestors("B"), vec!["A"]);
        assert_eq!(graph.supersession_chain("C"), vec!["B"]);
        assert!(graph.is_superseded("B"));
        assert_eq!(graph.latest_version("B"), "C");
    }

    #[test]
    fn test_branching_causal_tree() {
        // A → B, A → C (A has two effects)
        let entries = vec![
            make_entry("A", None, None),
            make_entry("B", Some("A"), None),
            make_entry("C", Some("A"), None),
        ];
        let graph = CausalGraph::build(&entries);

        let children = graph.direct_children("A");
        assert_eq!(children.len(), 2);

        let descendants = graph.descendants("A");
        assert_eq!(descendants.len(), 2);
    }

    #[test]
    fn test_root_cause_of_orphan() {
        let entries = vec![make_entry("X", None, None)];
        let graph = CausalGraph::build(&entries);
        assert_eq!(graph.root_cause("X"), "X");
        assert!(graph.ancestors("X").is_empty());
    }

    #[test]
    fn test_root_cause_nonexistent() {
        let graph = CausalGraph::build(&[]);
        assert_eq!(graph.root_cause("missing"), "missing");
    }

    #[test]
    fn test_cycle_protection() {
        // A → B → A (cycle): should not infinite-loop
        let mut a = make_entry("A", None, None);
        a.causal_parent = Some("B".to_string());
        let b = make_entry("B", Some("A"), None);
        let entries = vec![a, b];
        let graph = CausalGraph::build(&entries);

        let ancestors = graph.ancestors("A");
        assert!(ancestors.len() <= 2, "cycle should be broken by visited set");
    }

    #[test]
    fn test_causal_context_prompt_no_ancestors() {
        let entry = make_entry("X", None, None);
        let prompt = causal_context_prompt(&entry, &[]);
        assert!(prompt.starts_with("Memory:"));
    }

    #[test]
    fn test_causal_context_prompt_with_chain() {
        let a = make_entry("A", None, None);
        let b = make_entry("B", Some("A"), None);
        let c = make_entry("C", Some("B"), None);
        let prompt = causal_context_prompt(&c, &[a, b]);
        assert!(prompt.contains("Causal chain"));
        assert!(prompt.contains("[A]"));
        assert!(prompt.contains("[B]"));
        assert!(prompt.contains("Current [C]"));
    }
}
