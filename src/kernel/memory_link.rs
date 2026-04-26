//! A-4: Memory Link Engine — create KG nodes for memories and link related ones.
//! Called after a successful remember_long_term storage.

use crate::fs::{KGEdgeType, KGNodeType};

impl super::AIKernel {
    /// Public method for CLI handlers to link a memory entry to KG.
    pub(crate) fn link_memory_to_kg(&self, entry_id: &str, agent_id: &str, tenant_id: &str, tags: &[String]) {
        let label = format!("mem:{}", &entry_id[..8.min(entry_id.len())]);
        let props = serde_json::json!({
            "memory_entry_id": entry_id,
            "tags": tags,
            "kind": "memory",
        });

        let node_id = match self.kg_add_node(&label, KGNodeType::Memory, props, agent_id, tenant_id) {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!("failed to create KG node for memory {}: {}", entry_id, e);
                return;
            }
        };

        let existing_nodes = match self.kg_list_nodes(Some(KGNodeType::Memory), agent_id, tenant_id) {
            Ok(nodes) => nodes,
            Err(_) => return,
        };

        for existing in existing_nodes {
            if existing.id == node_id {
                continue;
            }
            let existing_tags: Vec<String> = existing.properties.get("tags")
                .and_then(|v: &serde_json::Value| v.as_array())
                .map(|arr| arr.iter().filter_map(|t| t.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let shared: Vec<_> = tags.iter().filter(|t| existing_tags.contains(t)).collect();
            if !shared.is_empty() {
                let weight = shared.len() as f32 / (tags.len().max(existing_tags.len())) as f32;
                if let Err(e) = self.kg_add_edge(&node_id, &existing.id, KGEdgeType::SimilarTo, Some(weight), agent_id, tenant_id) {
                    tracing::debug!("failed to create memory link edge: {}", e);
                }
            }
        }
    }
}
