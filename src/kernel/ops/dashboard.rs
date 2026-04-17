//! Dashboard and metrics operations.

use crate::api::semantic::DashboardStatus;

impl crate::kernel::AIKernel {
    /// Build runtime kernel metrics from live system state.
    pub fn dashboard_status(&self) -> DashboardStatus {
        let kg_node_count = self.knowledge_graph.as_ref()
            .map(|kg| kg.node_count().unwrap_or(0))
            .unwrap_or(0);
        let kg_edge_count = self.knowledge_graph.as_ref()
            .map(|kg| kg.edge_count().unwrap_or(0))
            .unwrap_or(0);

        let cas_object_count = self.cas.list_cids()
            .map(|c| c.len())
            .unwrap_or(0);

        DashboardStatus {
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            cas_object_count,
            agent_count: self.scheduler.list_agents().len(),
            tag_count: self.fs.list_tags().len(),
            kg_node_count,
            kg_edge_count,
        }
    }
}
