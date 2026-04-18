//! System status operations — runtime kernel metrics.

use crate::api::semantic::SystemStatus;

impl crate::kernel::AIKernel {
    /// Build runtime kernel metrics from live system state.
    pub fn system_status(&self) -> SystemStatus {
        let kg_node_count = self.knowledge_graph.as_ref()
            .map(|kg| kg.node_count().unwrap_or(0))
            .unwrap_or(0);
        let kg_edge_count = self.knowledge_graph.as_ref()
            .map(|kg| kg.edge_count().unwrap_or(0))
            .unwrap_or(0);

        let cas_object_count = self.cas.list_cids()
            .map(|c| c.len())
            .unwrap_or(0);

        SystemStatus {
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            cas_object_count,
            agent_count: self.scheduler.list_agents().len(),
            tag_count: self.fs.list_tags().len(),
            kg_node_count,
            kg_edge_count,
        }
    }
}
