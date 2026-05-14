//! 技能验证器 —— 验证技能候选的有效性

use super::{CognitiveResult, Skill, SkillCandidate, ValidationResult};

/// 技能验证器
#[derive(Debug, Default)]
pub struct SkillValidator {
    /// 回测样本数
    backtest_samples: usize,
}

impl SkillValidator {
    pub fn new() -> Self {
        Self {
            backtest_samples: 5,
        }
    }

    /// 验证技能候选
    pub async fn validate(&self, candidate: &SkillCandidate) -> CognitiveResult<ValidationResult> {
        let mut issues = Vec::new();

        // 1. 检查基本属性
        if candidate.name.is_empty() {
            issues.push("Skill name is empty".to_string());
        }
        if candidate.description.is_empty() {
            issues.push("Skill description is empty".to_string());
        }
        if candidate.confidence < 0.5 {
            issues.push(format!("Confidence too low: {:.2}", candidate.confidence));
        }

        // 2. 回测验证：基于 confidence 和源操作数量计算
        let test_pass_rate = self.backtest(candidate);

        let passed = issues.is_empty() && test_pass_rate > 0.6;

        Ok(ValidationResult {
            passed,
            test_pass_rate,
            issues,
        })
    }

    /// 验证技能候选并检测与已有技能的冲突
    pub async fn validate_with_conflict_check(
        &self,
        candidate: &SkillCandidate,
        existing_skills: &[&Skill],
    ) -> CognitiveResult<ValidationResult> {
        let mut result = self.validate(candidate).await?;

        // 检查与已有技能的冲突
        for skill in existing_skills {
            if let Some(conflict) = self.detect_conflict(candidate, skill) {
                result.issues.push(conflict);
            }
        }

        result.passed = result.issues.is_empty() && result.test_pass_rate > 0.6;
        Ok(result)
    }

    /// 检测候选与单个已有技能的冲突
    fn detect_conflict(&self, candidate: &SkillCandidate, existing: &Skill) -> Option<String> {
        let (existing_name, existing_desc) = match existing {
            Skill::Knowledge(k) => (&k.name, &k.description),
            Skill::Config(c) => (&c.name, &c.description),
            Skill::Code(code) => (&code.name, &code.description),
        };

        // 名称完全匹配
        if candidate.name == *existing_name {
            return Some(format!(
                "Name conflict with existing skill '{}': identical name",
                existing_name
            ));
        }

        // 描述高度重叠（简单的词级交集 > 70%）
        let cand_words: std::collections::HashSet<&str> = candidate.description.split_whitespace().collect();
        let exist_words: std::collections::HashSet<&str> = existing_desc.split_whitespace().collect();
        if !cand_words.is_empty() && !exist_words.is_empty() {
            let intersection = cand_words.intersection(&exist_words).count();
            let min_size = cand_words.len().min(exist_words.len());
            let overlap = intersection as f32 / min_size as f32;
            if overlap > 0.7 {
                return Some(format!(
                    "Description overlap with skill '{}' ({:.0}% similarity)",
                    existing_name, overlap * 100.0
                ));
            }
        }

        None
    }

    /// 基于 confidence 和源操作数量计算回测通过率
    fn backtest(&self, candidate: &SkillCandidate) -> f32 {
        if candidate.source_operations.is_empty() {
            return 0.5; // 无源操作，降低信心
        }

        let op_count = candidate.source_operations.len();

        if op_count >= self.backtest_samples {
            // 足够样本：confidence 加权，考虑操作数量
            let volume_factor = (op_count as f32 / (self.backtest_samples as f32 * 2.0)).min(1.0);
            candidate.confidence * 0.7 + volume_factor * 0.3
        } else {
            // 样本不足：仍基于 confidence，轻度惩罚
            let penalty = 0.1 * (1.0 - op_count as f32 / self.backtest_samples as f32);
            (candidate.confidence - penalty).max(0.5)
        }
    }

    /// 验证已注册的技能
    pub async fn validate_skill(&self, skill: &Skill) -> CognitiveResult<ValidationResult> {
        match skill {
            Skill::Knowledge(k) => self.validate_knowledge_skill(k).await,
            Skill::Config(c) => self.validate_config_skill(c).await,
            Skill::Code(code) => self.validate_code_skill(code).await,
        }
    }

    async fn validate_knowledge_skill(
        &self,
        skill: &super::KnowledgeSkill,
    ) -> CognitiveResult<ValidationResult> {
        let mut issues = Vec::new();

        if skill.knowledge.is_empty() {
            issues.push("Knowledge skill has no knowledge items".to_string());
        }

        for (i, item) in skill.knowledge.iter().enumerate() {
            match item {
                super::KnowledgeItem::Rule { condition, action } => {
                    if condition.is_empty() {
                        issues.push(format!("Rule {} has empty condition", i));
                    }
                    if action.is_empty() {
                        issues.push(format!("Rule {} has empty action", i));
                    }
                }
                super::KnowledgeItem::Checklist { items } => {
                    if items.is_empty() {
                        issues.push(format!("Checklist {} is empty", i));
                    }
                }
                super::KnowledgeItem::Lesson { situation, insight } => {
                    if situation.is_empty() {
                        issues.push(format!("Lesson {} has empty situation", i));
                    }
                    if insight.is_empty() {
                        issues.push(format!("Lesson {} has empty insight", i));
                    }
                }
                super::KnowledgeItem::Warning { pattern, consequence } => {
                    if pattern.is_empty() {
                        issues.push(format!("Warning {} has empty pattern", i));
                    }
                    if consequence.is_empty() {
                        issues.push(format!("Warning {} has empty consequence", i));
                    }
                }
            }
        }

        Ok(ValidationResult {
            passed: issues.is_empty(),
            test_pass_rate: if issues.is_empty() { 0.95 } else { 0.5 },
            issues,
        })
    }

    async fn validate_config_skill(
        &self,
        skill: &super::ConfigSkill,
    ) -> CognitiveResult<ValidationResult> {
        let mut issues = Vec::new();

        if skill.tool_chain.is_empty() {
            issues.push("Config skill has empty tool chain".to_string());
        }

        for (i, step) in skill.tool_chain.iter().enumerate() {
            if step.tool_name.is_empty() {
                issues.push(format!("Step {} has empty tool name", i));
            }
            if step.step_id.is_empty() {
                issues.push(format!("Step {} has empty step ID", i));
            }
        }

        Ok(ValidationResult {
            passed: issues.is_empty(),
            test_pass_rate: if issues.is_empty() { 0.9 } else { 0.5 },
            issues,
        })
    }

    async fn validate_code_skill(
        &self,
        skill: &super::CodeSkill,
    ) -> CognitiveResult<ValidationResult> {
        let mut issues = Vec::new();

        if skill.wasm_bytes.is_empty() {
            issues.push("Code skill has empty WASM bytes".to_string());
        }

        if skill.signature.inputs.is_empty() && skill.signature.outputs.is_empty() {
            issues.push("Code skill has empty signature".to_string());
        }

        // TODO: 尝试编译/验证WASM模块

        Ok(ValidationResult {
            passed: issues.is_empty(),
            test_pass_rate: if issues.is_empty() { 0.85 } else { 0.5 },
            issues,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::cognition::{
        KnowledgeItem, KnowledgeSkill, Skill, SkillCandidate, SkillType, SkillUsageStats,
        ValidationStatus,
    };

    fn valid_candidate() -> SkillCandidate {
        SkillCandidate {
            id: "c1".to_string(),
            name: "Test".to_string(),
            description: "A test skill".to_string(),
            skill_type: SkillType::Knowledge,
            source_operations: vec![
                "op1".to_string(),
                "op2".to_string(),
                "op3".to_string(),
            ],
            confidence: 0.8,
        }
    }

    #[tokio::test]
    async fn new_creates_with_default_backtest_samples() {
        let validator = SkillValidator::new();
        // backtest_samples = 5: >=5 ops => confidence-weighted rate
        let mut candidate = valid_candidate();
        candidate.source_operations = vec!["o1", "o2", "o3", "o4", "o5"]
            .into_iter()
            .map(String::from)
            .collect();
        let result = validator.validate(&candidate).await.unwrap();
        assert!(result.passed);
        assert!(result.test_pass_rate > 0.6, "expected > 0.6, got {}", result.test_pass_rate);
    }

    #[tokio::test]
    async fn validate_passes_for_high_confidence_candidate() {
        let validator = SkillValidator::new();
        let candidate = valid_candidate();
        let result = validator.validate(&candidate).await.unwrap();
        assert!(result.passed);
        assert!(result.issues.is_empty());
    }

    #[tokio::test]
    async fn validate_rejects_empty_name() {
        let validator = SkillValidator::new();
        let mut candidate = valid_candidate();
        candidate.name = "".to_string();
        let result = validator.validate(&candidate).await.unwrap();
        assert!(!result.passed);
        assert!(result.issues.iter().any(|i| i.contains("name is empty")));
    }

    #[tokio::test]
    async fn validate_rejects_empty_description() {
        let validator = SkillValidator::new();
        let mut candidate = valid_candidate();
        candidate.description = "".to_string();
        let result = validator.validate(&candidate).await.unwrap();
        assert!(!result.passed);
        assert!(result.issues.iter().any(|i| i.contains("description is empty")));
    }

    #[tokio::test]
    async fn validate_rejects_low_confidence() {
        let validator = SkillValidator::new();
        let mut candidate = valid_candidate();
        candidate.confidence = 0.2;
        let result = validator.validate(&candidate).await.unwrap();
        assert!(!result.passed);
        assert!(result.issues.iter().any(|i| i.contains("Confidence too low")));
    }

    #[tokio::test]
    async fn validate_passes_with_insufficient_operations() {
        let validator = SkillValidator::new();
        let mut candidate = valid_candidate();
        candidate.source_operations = vec!["op1".to_string(), "op2".to_string()];
        let result = validator.validate(&candidate).await.unwrap();
        // Fewer than backtest_samples (5) => confidence-based with mild penalty
        assert!(result.passed);
        assert!(result.test_pass_rate > 0.6, "expected > 0.6, got {}", result.test_pass_rate);
    }

    #[tokio::test]
    async fn validate_skill_dispatches_knowledge_skill() {
        let validator = SkillValidator::new();
        let skill = Skill::Knowledge(KnowledgeSkill {
            id: "test".to_string(),
            name: "Test Skill".to_string(),
            description: "desc".to_string(),
            trigger_conditions: vec![],
            knowledge: vec![KnowledgeItem::Rule {
                condition: "c".to_string(),
                action: "a".to_string(),
            }],
            sources: vec![],
            validation: ValidationStatus::Pending,
            usage_stats: SkillUsageStats::default(),
        });
        let result = validator.validate_skill(&skill).await.unwrap();
        assert!(result.passed);
        assert!(result.issues.is_empty());
    }

    #[tokio::test]
    async fn conflict_detection_finds_name_conflict() {
        let validator = SkillValidator::new();
        let candidate = SkillCandidate {
            id: "c1".into(),
            name: "My Skill".into(),
            description: "something new".into(),
            skill_type: SkillType::Knowledge,
            source_operations: vec!["op1".into(), "op2".into()],
            confidence: 0.8,
        };
        let existing = Skill::Knowledge(KnowledgeSkill {
            id: "e1".into(),
            name: "My Skill".into(),
            description: "something different".into(),
            trigger_conditions: vec![],
            knowledge: vec![],
            sources: vec![],
            validation: ValidationStatus::Pending,
            usage_stats: SkillUsageStats::default(),
        });

        let result = validator.validate_with_conflict_check(&candidate, &[&existing]).await.unwrap();
        assert!(!result.passed);
        assert!(result.issues.iter().any(|i| i.contains("Name conflict")));
    }

    #[tokio::test]
    async fn conflict_detection_finds_description_overlap() {
        let validator = SkillValidator::new();
        let candidate = SkillCandidate {
            id: "c1".into(),
            name: "New Skill".into(),
            description: "helps with search and file management tasks".into(),
            skill_type: SkillType::Knowledge,
            source_operations: vec!["op1".into()],
            confidence: 0.8,
        };
        let existing = Skill::Knowledge(KnowledgeSkill {
            id: "e1".into(),
            name: "Old Skill".into(),
            description: "helps with search and file management operations".into(),
            trigger_conditions: vec![],
            knowledge: vec![],
            sources: vec![],
            validation: ValidationStatus::Pending,
            usage_stats: SkillUsageStats::default(),
        });

        let result = validator.validate_with_conflict_check(&candidate, &[&existing]).await.unwrap();
        assert!(result.issues.iter().any(|i| i.contains("Description overlap")));
    }

    #[tokio::test]
    async fn conflict_detection_no_conflict_for_different_skills() {
        let validator = SkillValidator::new();
        let candidate = SkillCandidate {
            id: "c1".into(),
            name: "Search Helper".into(),
            description: "assists with search queries".into(),
            skill_type: SkillType::Knowledge,
            source_operations: vec!["op1".into(), "op2".into()],
            confidence: 0.8,
        };
        let existing = Skill::Knowledge(KnowledgeSkill {
            id: "e1".into(),
            name: "Code Reviewer".into(),
            description: "reviews code quality".into(),
            trigger_conditions: vec![],
            knowledge: vec![],
            sources: vec![],
            validation: ValidationStatus::Pending,
            usage_stats: SkillUsageStats::default(),
        });

        let result = validator.validate_with_conflict_check(&candidate, &[&existing]).await.unwrap();
        assert!(result.passed);
        assert!(!result.issues.iter().any(|i| i.contains("conflict") || i.contains("overlap")));
    }

    #[tokio::test]
    async fn backtest_higher_rate_with_more_operations() {
        let validator = SkillValidator::new();
        let few_ops = SkillCandidate {
            id: "c1".into(), name: "A".into(), description: "B".into(),
            skill_type: SkillType::Knowledge,
            source_operations: vec!["op1".into()],
            confidence: 0.8,
        };
        let many_ops = SkillCandidate {
            id: "c2".into(), name: "A".into(), description: "B".into(),
            skill_type: SkillType::Knowledge,
            source_operations: vec!["op1".into(), "op2".into(), "op3".into(), "op4".into(), "op5".into(), "op6".into()],
            confidence: 0.8,
        };
        let r1 = validator.validate(&few_ops).await.unwrap();
        let r2 = validator.validate(&many_ops).await.unwrap();
        assert!(r2.test_pass_rate > r1.test_pass_rate,
            "more ops should yield higher pass rate: {} vs {}", r2.test_pass_rate, r1.test_pass_rate);
    }
}
