//! Skill Discovery — detects repeated execution sequences to form skill candidates.
//!
//! Implements the autonomous skill formation for Node 23:
//! - Track operation sequences per agent
//! - Identify repeated patterns with high success rates
//! - Generate named skill candidates for review

use std::collections::HashMap;
use std::sync::RwLock;

/// Operation sequence with execution statistics.
#[derive(Debug, Clone)]
pub struct OpSequence {
    pub operations: Vec<String>,
    pub count: usize,
    pub success_rate: f32,
    pub avg_duration_ms: u64,
    pub last_seen_ms: u64,
}

/// Skill candidate extracted from repeated sequences.
#[derive(Debug, Clone)]
pub struct SkillCandidate {
    pub operations: Vec<String>,
    pub count: usize,
    pub success_rate: f32,
    pub recommended_name: String,
}

/// Tracks operation sequences per agent to discover skill candidates.
pub struct SkillDiscriminator {
    min_sequence_count: usize,
    sequences: RwLock<HashMap<String, Vec<OpSequence>>>,
}

impl SkillDiscriminator {
    /// Create a new SkillDiscriminator.
    pub fn new(min_sequence_count: usize) -> Self {
        Self {
            min_sequence_count,
            sequences: RwLock::new(HashMap::new()),
        }
    }

    /// Record an execution sequence for an agent.
    ///
    /// If the same operation pattern exists, increments count and updates stats.
    /// Otherwise adds a new OpSequence.
    pub fn record_sequence(
        &self,
        agent_id: &str,
        operations: Vec<String>,
        success: bool,
        duration_ms: u64,
    ) {
        let mut sequences = self.sequences.write().unwrap();
        let agent_sequences = sequences.entry(agent_id.to_string()).or_insert_with(Vec::new);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        if let Some(seq) = agent_sequences.iter_mut().find(|s| s.operations == operations) {
            seq.count += 1;
            let total_successes = (seq.success_rate * (seq.count - 1) as f32).round() as usize;
            let new_successes = if success { 1 } else { 0 };
            seq.success_rate = (total_successes + new_successes) as f32 / seq.count as f32;
            let total_duration = seq.avg_duration_ms * (seq.count - 1) as u64;
            seq.avg_duration_ms = (total_duration + duration_ms) / seq.count as u64;
            seq.last_seen_ms = now;
        } else {
            agent_sequences.push(OpSequence {
                operations,
                count: 1,
                success_rate: if success { 1.0 } else { 0.0 },
                avg_duration_ms: duration_ms,
                last_seen_ms: now,
            });
        }
    }

    /// Get skill candidates for an agent.
    ///
    /// Returns sequences where count >= min_sequence_count AND success_rate >= 0.8.
    /// Generates recommended names like "skill_read_call_create_v3".
    pub fn get_skill_candidates(&self, agent_id: &str) -> Vec<SkillCandidate> {
        let sequences = self.sequences.read().unwrap();
        let agent_sequences = match sequences.get(agent_id) {
            Some(seqs) => seqs,
            None => return Vec::new(),
        };

        agent_sequences
            .iter()
            .filter(|s| s.count >= self.min_sequence_count && s.success_rate >= 0.8)
            .map(|s| {
                let op_names: String = s.operations.iter().take(4).map(|s| s.as_str()).collect::<Vec<_>>().join("_");
                let recommended_name = format!("skill_{}_{}", op_names, s.count);
                SkillCandidate {
                    operations: s.operations.clone(),
                    count: s.count,
                    success_rate: s.success_rate,
                    recommended_name,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_discovery_records_sequence() {
        let disc = SkillDiscriminator::new(2);

        disc.record_sequence("agent1", vec!["read".to_string(), "write".to_string()], true, 100);
        disc.record_sequence("agent1", vec!["read".to_string(), "write".to_string()], true, 150);

        let candidates = disc.get_skill_candidates("agent1");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].count, 2);
        assert_eq!(candidates[0].success_rate, 1.0);
        assert_eq!(candidates[0].operations, vec!["read".to_string(), "write".to_string()]);
    }

    #[test]
    fn test_skill_discovery_detects_repeated_sequence() {
        let disc = SkillDiscriminator::new(2);

        disc.record_sequence("agent1", vec!["read".to_string()], false, 50);
        disc.record_sequence("agent1", vec!["read".to_string()], false, 60);
        disc.record_sequence("agent1", vec!["read".to_string()], false, 70);

        let candidates = disc.get_skill_candidates("agent1");
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_skill_discovery_skill_candidate_generation() {
        let disc = SkillDiscriminator::new(2);

        for _ in 0..5 {
            disc.record_sequence(
                "agent1",
                vec!["read".to_string(), "call".to_string(), "create".to_string()],
                true,
                200,
            );
        }

        let candidates = disc.get_skill_candidates("agent1");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].count, 5);
        assert_eq!(candidates[0].success_rate, 1.0);
        assert!(candidates[0].recommended_name.contains("skill_read_call"));
    }
}
