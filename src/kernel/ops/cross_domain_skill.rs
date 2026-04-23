//! Cross-Domain Skill Composition — detects and composes skills that co-occur across domains.
//!
//! Implements the autonomous skill formation for Node 24:
//! - Track co-occurrence of operations across domains
//! - Identify cross-domain composition patterns with high success rates
//! - Generate composition candidates for skill fusion

use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Debug, Clone)]
pub struct SkillNode {
    pub operation: String,
    pub domain: String,
    pub count: usize,
}

#[derive(Debug, Clone)]
pub struct CoOccurrence {
    pub skill_a: String,
    pub skill_b: String,
    pub count: usize,
    pub success_count: usize,
    pub total_count: usize,
}

#[derive(Debug, Clone)]
pub struct SkillGraph {
    nodes: HashMap<String, SkillNode>,
    edges: HashMap<String, Vec<CoOccurrence>>,
}

impl SkillGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: HashMap::new(),
        }
    }

    pub fn add_node(&mut self, operation: &str, domain: &str) {
        let key = format!("{}:{}", operation, domain);
        let count = self.nodes.entry(key.clone()).or_insert_with(|| SkillNode {
            operation: operation.to_string(),
            domain: domain.to_string(),
            count: 0,
        }).count;
        self.nodes.get_mut(&key).unwrap().count = count + 1;
    }

    pub fn add_cooccurrence(&mut self, skill_a: &str, skill_b: &str, success: bool) {
        let edge_key = if skill_a <= skill_b {
            format!("{}:{}", skill_a, skill_b)
        } else {
            format!("{}:{}", skill_b, skill_a)
        };

        let cooccurrence = self.edges.entry(edge_key.clone()).or_insert_with(Vec::new);

        if let Some(existing) = cooccurrence.iter_mut().find(|c| c.skill_a == skill_a && c.skill_b == skill_b) {
            existing.total_count += 1;
            if success {
                existing.success_count += 1;
            }
            existing.count += 1;
        } else {
            cooccurrence.push(CoOccurrence {
                skill_a: skill_a.to_string(),
                skill_b: skill_b.to_string(),
                count: 1,
                success_count: if success { 1 } else { 0 },
                total_count: 1,
            });
        }
    }

    pub fn get_compositions(&self) -> Vec<CompositionCandidate> {
        let mut candidates = Vec::new();

        for edge in self.edges.values().flatten() {
            if edge.count >= 1 {
                let success_rate = edge.success_count as f32 / edge.total_count as f32;
                candidates.push(CompositionCandidate {
                    skills: vec![edge.skill_a.clone(), edge.skill_b.clone()],
                    domains: vec![],
                    cooccurrence_count: edge.count,
                    success_rate,
                });
            }
        }

        candidates
    }
}

impl Default for SkillGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct CompositionCandidate {
    pub skills: Vec<String>,
    pub domains: Vec<String>,
    pub cooccurrence_count: usize,
    pub success_rate: f32,
}

pub struct CrossDomainSkillComposer {
    graph: RwLock<SkillGraph>,
    min_cross_domain_count: usize,
}

impl CrossDomainSkillComposer {
    pub fn new(min_cross_domain_count: usize) -> Self {
        Self {
            graph: RwLock::new(SkillGraph::new()),
            min_cross_domain_count,
        }
    }

    pub fn record_sequence(&self, operations: &[String], domains: &[String], success: bool) {
        if operations.len() != domains.len() || operations.len() < 2 {
            return;
        }

        let mut graph = self.graph.write().unwrap();

        for i in 0..operations.len() {
            graph.add_node(&operations[i], &domains[i]);
        }

        for i in 0..operations.len() - 1 {
            let skill_a = format!("{}:{}", &operations[i], &domains[i]);
            let skill_b = format!("{}:{}", &operations[i + 1], &domains[i + 1]);
            graph.add_cooccurrence(&skill_a, &skill_b, success);
        }
    }

    pub fn get_composition_candidates(&self) -> Vec<CompositionCandidate> {
        let graph = self.graph.read().unwrap();

        graph
            .get_compositions()
            .into_iter()
            .filter(|c| {
                c.cooccurrence_count >= self.min_cross_domain_count && c.success_rate >= 0.6
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cross_domain_skill_records_sequence() {
        let composer = CrossDomainSkillComposer::new(2);

        composer.record_sequence(
            &["read".to_string(), "write".to_string()],
            &["storage".to_string(), "storage".to_string()],
            true,
        );
        composer.record_sequence(
            &["read".to_string(), "write".to_string()],
            &["storage".to_string(), "storage".to_string()],
            true,
        );

        let candidates = composer.get_composition_candidates();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].skills[0], "read:storage");
        assert_eq!(candidates[0].skills[1], "write:storage");
        assert_eq!(candidates[0].cooccurrence_count, 2);
        assert_eq!(candidates[0].success_rate, 1.0);
    }

    #[test]
    fn test_cross_domain_skill_detects_cross_domain_composition() {
        let composer = CrossDomainSkillComposer::new(2);

        composer.record_sequence(
            &["read".to_string(), "analyze".to_string()],
            &["storage".to_string(), "analysis".to_string()],
            true,
        );
        composer.record_sequence(
            &["read".to_string(), "analyze".to_string()],
            &["storage".to_string(), "analysis".to_string()],
            true,
        );
        composer.record_sequence(
            &["read".to_string(), "analyze".to_string()],
            &["storage".to_string(), "analysis".to_string()],
            false,
        );

        let candidates = composer.get_composition_candidates();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].cooccurrence_count, 3);
        assert!((candidates[0].success_rate - 0.6666666).abs() < 0.001);
    }

    #[test]
    fn test_cross_domain_skill_composition_candidates() {
        let composer = CrossDomainSkillComposer::new(2);

        composer.record_sequence(
            &["call".to_string(), "result".to_string()],
            &["tool".to_string(), "tool".to_string()],
            true,
        );
        composer.record_sequence(
            &["call".to_string(), "result".to_string()],
            &["tool".to_string(), "tool".to_string()],
            true,
        );

        let candidates = composer.get_composition_candidates();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].cooccurrence_count, 2);
        assert_eq!(candidates[0].success_rate, 1.0);
    }
}