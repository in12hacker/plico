//! Intent resolution operations.

use crate::intent::ResolvedIntent;

impl crate::kernel::AIKernel {
    /// Resolve natural language text into structured API requests.
    pub fn intent_resolve(&self, text: &str, agent_id: &str) -> Vec<ResolvedIntent> {
        match self.intent_router.resolve(text, agent_id) {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!("Intent resolution failed: {}", e);
                vec![]
            }
        }
    }
}
