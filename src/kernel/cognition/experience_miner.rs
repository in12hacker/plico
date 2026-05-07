//! 经验挖掘器 —— 从操作历史中提取技能模式

use std::sync::Arc;

use super::{
    CognitiveResult, KnowledgeItem, OperationRecord,
    SkillCandidate, SkillType,
    SessionCognitiveState, TrajectoryTracker, TrajectoryPoint,
};

/// 经验挖掘器
pub struct ExperienceMiner {
    /// 最小重复次数才形成技能
    min_repetitions: usize,
    /// 最低成功率
    min_success_rate: f32,
    /// 轨迹追踪器（用于获取操作历史）
    trajectory_tracker: Option<Arc<TrajectoryTracker>>,
}

impl std::fmt::Debug for ExperienceMiner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExperienceMiner")
            .field("min_repetitions", &self.min_repetitions)
            .field("min_success_rate", &self.min_success_rate)
            .field("has_tracker", &self.trajectory_tracker.is_some())
            .finish()
    }
}

impl Default for ExperienceMiner {
    fn default() -> Self {
        Self::new()
    }
}

impl ExperienceMiner {
    pub fn new() -> Self {
        Self {
            min_repetitions: 3,
            min_success_rate: 0.8,
            trajectory_tracker: None,
        }
    }

    pub fn with_tracker(mut self, tracker: Arc<TrajectoryTracker>) -> Self {
        self.trajectory_tracker = Some(tracker);
        self
    }

    /// 从单次操作中提取技能候选 — 查找该操作在历史中的重复模式
    pub async fn extract(
        &self,
        agent_id: &str,
        operation: &str,
    ) -> CognitiveResult<Vec<SkillCandidate>> {
        let tracker = match &self.trajectory_tracker {
            Some(t) => t,
            None => return Ok(Vec::new()),
        };

        // Get recent trajectory and convert to OperationRecords
        let trajectory = tracker.get_recent_trajectory(agent_id, 200).await;
        let records = Self::trajectory_to_records(&trajectory);

        // Find patterns involving the current operation
        let all_candidates = self.extract_patterns(&records).await?;
        let relevant: Vec<SkillCandidate> = all_candidates
            .into_iter()
            .filter(|c| c.source_operations.iter().any(|op| op == operation))
            .collect();

        Ok(relevant)
    }

    /// Convert TrajectoryPoints to OperationRecords for pattern extraction
    fn trajectory_to_records(trajectory: &[TrajectoryPoint]) -> Vec<OperationRecord> {
        trajectory
            .iter()
            .filter(|p| p.operation != "declare_intent") // Skip intent declarations
            .map(|p| OperationRecord {
                operation: p.operation.clone(),
                params: serde_json::Value::Null,
                success: p.success,
                duration_ms: 0,
                timestamp_ms: p.timestamp_ms,
            })
            .collect()
    }

    /// 从会话状态中提取技能候选
    pub async fn extract_from_session(
        &self,
        session_state: &SessionCognitiveState,
    ) -> CognitiveResult<Vec<SkillCandidate>> {
        let mut candidates = Vec::new();

        // 基于会话中的优化报告提取知识型技能
        if let Some(ref report) = session_state.last_optimization {
            let mut knowledge_items = Vec::new();
            let mut has_lessons = false;

            for opt in &report.optimizations {
                if let super::OptimizationAction::LessonInjected { lesson, source } = opt {
                    knowledge_items.push(KnowledgeItem::Lesson {
                        situation: source.clone(),
                        insight: lesson.clone(),
                    });
                    has_lessons = true;
                }
            }

            if has_lessons {
                candidates.push(SkillCandidate {
                    id: format!("candidate_lessons_{}", session_state.session_id),
                    name: "Session Learnings".to_string(),
                    description: "Lessons learned from this session".to_string(),
                    skill_type: SkillType::Knowledge,
                    source_operations: vec![session_state.session_id.clone()],
                    confidence: 0.7,
                });
            }
        }

        Ok(candidates)
    }

    /// 从操作记录序列中提取重复模式（滑动窗口）
    pub async fn extract_patterns(
        &self,
        operations: &[OperationRecord],
    ) -> CognitiveResult<Vec<SkillCandidate>> {
        if operations.len() < 2 {
            return Ok(Vec::new());
        }

        let mut candidates = Vec::new();

        // Extract operation name sequences with sliding windows of size 2-5
        for window_size in 2..=5.min(operations.len()) {
            let mut seq_counts: std::collections::HashMap<Vec<String>, (usize, usize)> = std::collections::HashMap::new();

            for window in operations.windows(window_size) {
                let ops: Vec<String> = window.iter().map(|o| o.operation.clone()).collect();
                let success_count = window.iter().filter(|o| o.success).count();
                let entry = seq_counts.entry(ops).or_insert((0, 0));
                entry.0 += 1;  // total occurrences
                entry.1 += success_count;  // success count
            }

            for (seq, &(count, successes)) in &seq_counts {
                let total_ops = count * window_size;
                let success_rate = successes as f32 / total_ops as f32;

                if count >= self.min_repetitions && success_rate >= self.min_success_rate {
                    candidates.push(SkillCandidate {
                        id: format!("pattern_{}", seq.join("_")),
                        name: format!("Repeated pattern: {}", seq.join(" → ")),
                        description: format!(
                            "Operation sequence '{}' repeated {} times with {:.0}% success rate",
                            seq.join(" → "), count, success_rate * 100.0
                        ),
                        skill_type: SkillType::Knowledge,
                        source_operations: seq.clone(),
                        confidence: success_rate * (count as f32 / (self.min_repetitions as f32 + 1.0)).min(1.0),
                    });
                }
            }
        }

        // Sort by confidence descending
        candidates.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
        Ok(candidates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_creates_with_defaults() {
        let miner = ExperienceMiner::new();
        assert_eq!(miner.min_repetitions, 3);
        assert!((miner.min_success_rate - 0.8).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_extract_returns_empty_vec() {
        let miner = ExperienceMiner::new();
        let result = miner.extract("agent1", "op1").await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_extract_from_session_returns_empty_for_empty_session() {
        let miner = ExperienceMiner::new();
        let session = SessionCognitiveState {
            agent_id: "test".to_string(),
            session_id: "s1".to_string(),
            context_quality_score: 0.0,
            context_utilization: 0.0,
            attention_focus: vec![],
            detected_patterns: vec![],
            last_optimization: None,
        };
        let result = miner.extract_from_session(&session).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_extract_patterns_returns_empty_for_empty_operations() {
        let miner = ExperienceMiner::new();
        let result = miner.extract_patterns(&[]).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_extract_with_tracker_finds_repeated_patterns() {
        let tracker = Arc::new(TrajectoryTracker::new());
        // Record a repeated pattern: search → read → create (3 times)
        for _ in 0..4 {
            tracker.record_operation("agent1", "search", true).await;
            tracker.record_operation("agent1", "read", true).await;
            tracker.record_operation("agent1", "create", true).await;
        }

        let miner = ExperienceMiner::new().with_tracker(tracker);
        let candidates = miner.extract("agent1", "search").await.unwrap();
        assert!(!candidates.is_empty());
        assert!(candidates.iter().any(|c| c.source_operations.contains(&"search".to_string())));
    }

    #[tokio::test]
    async fn test_extract_ignores_intent_declarations() {
        let tracker = Arc::new(TrajectoryTracker::new());
        tracker.record_intent("agent1", "some intent").await;
        tracker.record_operation("agent1", "op1", true).await;
        tracker.record_intent("agent1", "another intent").await;
        tracker.record_operation("agent1", "op1", true).await;

        let miner = ExperienceMiner::new().with_tracker(tracker);
        let candidates = miner.extract("agent1", "op1").await.unwrap();
        // Not enough repeated ops (only 2, min is 3), so should be empty
        assert!(candidates.is_empty());
    }
}
