//! Adaptive Context Budgeter — UCB1 multi-armed bandit for retrieval strategy selection.
//!
//! Each retrieval strategy (vector, BM25, KG, typed) is an "arm" in a bandit.
//! After each retrieval, if the returned results are actually used by the agent
//! (access_count increases), the strategy gets a positive reward.
//!
//! UCB1 score: avg_reward + C * sqrt(ln(total_pulls) / pulls(strategy))
//!
//! This dynamically adjusts RetrievalConfig weights: successful strategies gain
//! higher weight over time.

use std::collections::HashMap;

/// Identifies a retrieval strategy arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StrategyArm {
    Vector,
    Bm25,
    KnowledgeGraph,
    TypedRecall,
}

impl StrategyArm {
    pub fn all() -> &'static [StrategyArm] {
        &[
            StrategyArm::Vector,
            StrategyArm::Bm25,
            StrategyArm::KnowledgeGraph,
            StrategyArm::TypedRecall,
        ]
    }
}

/// Statistics for a single bandit arm.
#[derive(Debug, Clone)]
pub struct ArmStats {
    pub pulls: u64,
    pub total_reward: f64,
}

impl Default for ArmStats {
    fn default() -> Self {
        Self {
            pulls: 0,
            total_reward: 0.0,
        }
    }
}

impl ArmStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn avg_reward(&self) -> f64 {
        if self.pulls == 0 {
            0.0
        } else {
            self.total_reward / self.pulls as f64
        }
    }
}

/// UCB1 Multi-Armed Bandit for adaptive retrieval budget allocation.
#[derive(Debug, Clone)]
pub struct Ucb1Bandit {
    arms: HashMap<StrategyArm, ArmStats>,
    total_pulls: u64,
    exploration_constant: f64,
}

impl Ucb1Bandit {
    pub fn new(exploration_constant: f64) -> Self {
        let mut arms = HashMap::new();
        for &arm in StrategyArm::all() {
            arms.insert(arm, ArmStats::new());
        }
        Self {
            arms,
            total_pulls: 0,
            exploration_constant,
        }
    }

    /// Default bandit with C=1.41 (sqrt(2), standard UCB1).
    pub fn default_bandit() -> Self {
        Self::new(std::f64::consts::SQRT_2)
    }

    /// Record a pull with observed reward (0.0 to 1.0).
    pub fn record(&mut self, arm: StrategyArm, reward: f64) {
        let stats = self.arms.entry(arm).or_default();
        stats.pulls += 1;
        stats.total_reward += reward;
        self.total_pulls += 1;
    }

    /// Compute UCB1 score for an arm.
    pub fn ucb1_score(&self, arm: StrategyArm) -> f64 {
        let stats = match self.arms.get(&arm) {
            Some(s) => s,
            None => return f64::INFINITY,
        };

        if stats.pulls == 0 {
            return f64::INFINITY;
        }

        let exploitation = stats.avg_reward();
        let exploration = self.exploration_constant
            * ((self.total_pulls as f64).ln() / stats.pulls as f64).sqrt();

        exploitation + exploration
    }

    /// Select the arm with highest UCB1 score (exploration vs exploitation).
    pub fn select_arm(&self) -> StrategyArm {
        let mut best_arm = StrategyArm::Vector;
        let mut best_score = f64::NEG_INFINITY;

        for &arm in StrategyArm::all() {
            let score = self.ucb1_score(arm);
            if score > best_score {
                best_score = score;
                best_arm = arm;
            }
        }

        best_arm
    }

    /// Generate dynamic weights for all strategies based on UCB1 scores.
    ///
    /// Weights are normalized to sum to 1.0.
    pub fn strategy_weights(&self) -> HashMap<StrategyArm, f64> {
        let scores: Vec<(StrategyArm, f64)> = StrategyArm::all()
            .iter()
            .map(|&arm| {
                let score = self.ucb1_score(arm);
                let bounded = if score.is_infinite() { 10.0 } else { score.max(0.01) };
                (arm, bounded)
            })
            .collect();

        let total: f64 = scores.iter().map(|(_, s)| s).sum();
        if total == 0.0 {
            let uniform = 1.0 / StrategyArm::all().len() as f64;
            return StrategyArm::all().iter().map(|&a| (a, uniform)).collect();
        }

        scores.into_iter().map(|(arm, score)| (arm, score / total)).collect()
    }

    /// Get stats for a specific arm.
    pub fn arm_stats(&self, arm: StrategyArm) -> Option<&ArmStats> {
        self.arms.get(&arm)
    }

    /// Total pulls across all arms.
    pub fn total_pulls(&self) -> u64 {
        self.total_pulls
    }

    /// Convert weights to vector_weight and bm25_weight for RetrievalConfig.
    pub fn to_retrieval_weights(&self) -> (f32, f32) {
        let weights = self.strategy_weights();
        let vector_w = weights.get(&StrategyArm::Vector).copied().unwrap_or(0.25);
        let bm25_w = weights.get(&StrategyArm::Bm25).copied().unwrap_or(0.25);
        let total = vector_w + bm25_w;
        if total == 0.0 {
            return (0.5, 0.5);
        }
        (
            (vector_w / total) as f32,
            (bm25_w / total) as f32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cold_start_explores_all() {
        let bandit = Ucb1Bandit::default_bandit();
        let weights = bandit.strategy_weights();
        let values: Vec<f64> = weights.values().copied().collect();
        let first = values[0];
        for v in &values {
            assert!(
                (v - first).abs() < 0.01,
                "cold start should have uniform weights"
            );
        }
    }

    #[test]
    fn test_ucb1_prefers_unpulled_arms() {
        let mut bandit = Ucb1Bandit::default_bandit();
        bandit.record(StrategyArm::Vector, 0.5);
        bandit.record(StrategyArm::Vector, 0.5);

        let selected = bandit.select_arm();
        assert_ne!(
            selected,
            StrategyArm::Vector,
            "should explore unpulled arms first"
        );
    }

    #[test]
    fn test_reward_increases_weight() {
        let mut bandit = Ucb1Bandit::new(0.1);
        for _ in 0..20 {
            bandit.record(StrategyArm::Vector, 0.9);
            bandit.record(StrategyArm::Bm25, 0.1);
            bandit.record(StrategyArm::KnowledgeGraph, 0.3);
            bandit.record(StrategyArm::TypedRecall, 0.3);
        }

        let weights = bandit.strategy_weights();
        let vector_w = weights[&StrategyArm::Vector];
        let bm25_w = weights[&StrategyArm::Bm25];
        assert!(
            vector_w > bm25_w,
            "higher-reward strategy should get higher weight: vector={}, bm25={}",
            vector_w, bm25_w
        );
    }

    #[test]
    fn test_weights_sum_to_one() {
        let mut bandit = Ucb1Bandit::default_bandit();
        bandit.record(StrategyArm::Vector, 0.8);
        bandit.record(StrategyArm::Bm25, 0.2);

        let weights = bandit.strategy_weights();
        let sum: f64 = weights.values().sum();
        assert!(
            (sum - 1.0).abs() < 0.01,
            "weights should sum to 1.0, got {}",
            sum
        );
    }

    #[test]
    fn test_convergence_after_many_pulls() {
        let mut bandit = Ucb1Bandit::new(0.5);
        for _ in 0..100 {
            bandit.record(StrategyArm::Vector, 0.9);
            bandit.record(StrategyArm::Bm25, 0.1);
            bandit.record(StrategyArm::KnowledgeGraph, 0.5);
            bandit.record(StrategyArm::TypedRecall, 0.5);
        }

        let selected = bandit.select_arm();
        assert_eq!(
            selected,
            StrategyArm::Vector,
            "after 100 rounds, should exploit the best arm"
        );
    }

    #[test]
    fn test_to_retrieval_weights() {
        let mut bandit = Ucb1Bandit::new(0.1);
        for _ in 0..50 {
            bandit.record(StrategyArm::Vector, 0.8);
            bandit.record(StrategyArm::Bm25, 0.2);
            bandit.record(StrategyArm::KnowledgeGraph, 0.5);
            bandit.record(StrategyArm::TypedRecall, 0.5);
        }

        let (vector_w, bm25_w) = bandit.to_retrieval_weights();
        assert!(vector_w > bm25_w, "vector should have higher weight");
        assert!((vector_w + bm25_w - 1.0).abs() < 0.01, "should sum to 1.0");
    }

    #[test]
    fn test_exploration_bonus_decreases_with_more_pulls() {
        let mut bandit = Ucb1Bandit::default_bandit();
        for _ in 0..10 {
            bandit.record(StrategyArm::Vector, 0.5);
            bandit.record(StrategyArm::Bm25, 0.5);
        }
        let score_at_10 = bandit.ucb1_score(StrategyArm::Vector);

        for _ in 0..90 {
            bandit.record(StrategyArm::Vector, 0.5);
            bandit.record(StrategyArm::Bm25, 0.5);
        }
        let score_at_100 = bandit.ucb1_score(StrategyArm::Vector);

        assert!(
            score_at_10 > score_at_100,
            "exploration bonus should shrink with more data: {} vs {}",
            score_at_10, score_at_100
        );
    }
}
