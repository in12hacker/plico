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

        // 2. 检查与现有技能的冲突（TODO）

        // 3. 回测验证（如果有足够历史数据）
        let test_pass_rate = if candidate.source_operations.len() >= self.backtest_samples {
            // TODO: 实际回测
            0.85
        } else {
            0.7 // 样本不足时降低预期
        };

        let passed = issues.is_empty() && test_pass_rate > 0.6;

        Ok(ValidationResult {
            passed,
            test_pass_rate,
            issues,
        })
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
        // backtest_samples = 5: >=5 ops => test_pass_rate 0.85, <5 ops => 0.7
        let mut candidate = valid_candidate();
        candidate.source_operations = vec!["o1", "o2", "o3", "o4", "o5"]
            .into_iter()
            .map(String::from)
            .collect();
        let result = validator.validate(&candidate).await.unwrap();
        assert!(result.passed);
        assert_eq!(result.test_pass_rate, 0.85);
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
        // Fewer than backtest_samples (5) yields test_pass_rate=0.7, still > 0.6
        assert!(result.passed);
        assert_eq!(result.test_pass_rate, 0.7);
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
}
