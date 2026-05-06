//! 经验挖掘器 —— 从操作历史中提取技能模式

use super::{
    CognitiveResult, KnowledgeItem, OperationRecord,
    SkillCandidate, SkillType,
    SessionCognitiveState,
};

/// 经验挖掘器
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct ExperienceMiner {
    /// 最小重复次数才形成技能
    min_repetitions: usize,
    /// 最低成功率
    min_success_rate: f32,
}

impl ExperienceMiner {
    pub fn new() -> Self {
        Self {
            min_repetitions: 3,
            min_success_rate: 0.8,
        }
    }

    /// 从单次操作中提取技能候选
    pub async fn extract(
        &self,
        _agent_id: &str,
        _operation: &str,
    ) -> CognitiveResult<Vec<SkillCandidate>> {
        // TODO: 实际实现
        Ok(Vec::new())
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

    /// 从操作记录序列中提取重复模式
    pub async fn extract_patterns(
        &self,
        _operations: &[OperationRecord],
    ) -> CognitiveResult<Vec<SkillCandidate>> {
        // TODO: 实现序列模式挖掘（如 PrefixSpan 或简单滑动窗口）
        Ok(Vec::new())
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
}
