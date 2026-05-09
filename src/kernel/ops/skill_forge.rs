//! Intelligent Skill Forge — pattern-driven skill evolution (v41).
//!
//! Replaces mechanical hit-counters with semantic density detection. 
//! Fulfills Soul 3.0 Axiom 9: "Better with Use" through skill auto-generation.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use serde::{Deserialize, Serialize};
use crate::util::cosine_similarity;
use crate::kernel::event_bus::KernelEvent;

/// Threshold for semantic density. If average similarity within a cluster 
/// exceeds this, it's considered a "stable pattern" (muscle memory).
const DENSITY_THRESHOLD: f32 = 0.92;
/// Minimum number of samples to trigger density check.
const MIN_SAMPLES: usize = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCandidate {
    pub name: String,
    pub tool_name: String,
    pub pattern_summary: String,
    pub confidence: f32,
    pub suggested_params: serde_json::Value,
}

/// Tracks the semantic clusters of tool calls to identify candidates for solidification.
pub struct IntelligentSkillForge {
    /// agent_id -> tool_name -> [vector_samples]
    trace_memory: RwLock<HashMap<String, HashMap<String, Vec<Vec<f32>>>>>,
}

impl IntelligentSkillForge {
    pub fn new() -> Self {
        Self {
            trace_memory: RwLock::new(HashMap::new()),
        }
    }

    /// Records a tool call and checks if it forms a tight semantic cluster.
    pub fn record_and_evaluate(
        &self,
        agent_id: &str,
        tool_name: &str,
        params: &serde_json::Value,
        embedding: Vec<f32>,
    ) -> Option<SkillCandidate> {
        let mut all_traces = self.trace_memory.write().unwrap();
        let agent_traces = all_traces.entry(agent_id.to_string()).or_default();
        let tool_samples = agent_traces.entry(tool_name.to_string()).or_default();

        tool_samples.push(embedding.clone());

        if tool_samples.len() < MIN_SAMPLES {
            return None;
        }

        // Calculate Semantic Density (average pairwise similarity)
        let mut total_sim = 0.0;
        let mut count = 0;
        let recent_samples = if tool_samples.len() > 10 { &tool_samples[tool_samples.len()-10..] } else { &tool_samples[..] };
        
        for (i, a) in recent_samples.iter().enumerate() {
            for b in recent_samples.iter().skip(i + 1) {
                total_sim += cosine_similarity(a, b);
                count += 1;
            }
        }

        let density = if count > 0 { total_sim / count as f32 } else { 0.0 };

        if density > DENSITY_THRESHOLD {
            tracing::info!(agent_id, tool_name, density, "Tight semantic cluster detected - suggesting skill solidification");
            
            // Clear samples once suggested to avoid spamming
            tool_samples.clear();

            Some(SkillCandidate {
                name: format!("AutoSkill-{}-{}", tool_name, &uuid::Uuid::new_v4().to_string()[..4]),
                tool_name: tool_name.to_string(),
                pattern_summary: format!("Repeated high-fidelity pattern detected (density={:.2})", density),
                confidence: density,
                suggested_params: params.clone(), // In a real brain, we'd find the centroid/template
            })
        } else {
            None
        }
    }
}
