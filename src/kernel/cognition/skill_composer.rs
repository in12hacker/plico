//! 技能组合器 —— 组合多个技能形成新技能

use super::{
    CognitiveResult, ConfigSkill, KnowledgeItem, KnowledgeSkill, Skill, SkillUsageStats, ToolCallStep, ValidationStatus,
};

fn knowledge_item_to_json(item: &KnowledgeItem) -> serde_json::Value {
    match item {
        KnowledgeItem::Rule { condition, action } => serde_json::json!({
            "type": "rule", "condition": condition, "action": action
        }),
        KnowledgeItem::Checklist { items } => serde_json::json!({
            "type": "checklist", "items": items
        }),
        KnowledgeItem::Lesson { situation, insight } => serde_json::json!({
            "type": "lesson", "situation": situation, "insight": insight
        }),
        KnowledgeItem::Warning { pattern, consequence } => serde_json::json!({
            "type": "warning", "pattern": pattern, "consequence": consequence
        }),
    }
}

/// 技能组合器
#[derive(Debug, Default)]
pub struct SkillComposer;

impl SkillComposer {
    pub fn new() -> Self {
        Self
    }

    /// 组合多个技能为一个融合技能
    pub fn compose(&self, skills: &[Skill]) -> CognitiveResult<Option<Skill>> {
        if skills.len() < 2 {
            return Ok(None);
        }

        let has_knowledge = skills.iter().any(|s| matches!(s, Skill::Knowledge(_)));
        let has_config = skills.iter().any(|s| matches!(s, Skill::Config(_)));

        if has_config {
            // Config or mixed composition
            self.compose_with_config(skills, has_knowledge)
        } else if has_knowledge {
            // Pure Knowledge composition
            self.compose_knowledge(skills)
        } else {
            Ok(None)
        }
    }

    fn compose_knowledge(&self, skills: &[Skill]) -> CognitiveResult<Option<Skill>> {
        let mut all_knowledge = Vec::new();
        let mut all_triggers = Vec::new();
        let mut all_sources = Vec::new();
        let mut names = Vec::new();

        for skill in skills {
            if let Skill::Knowledge(k) = skill {
                names.push(k.name.clone());
                all_knowledge.extend(k.knowledge.clone());
                all_triggers.extend(k.trigger_conditions.clone());
                all_sources.extend(k.sources.clone());
            }
        }

        if all_knowledge.is_empty() {
            return Ok(None);
        }

        Ok(Some(Skill::Knowledge(KnowledgeSkill {
            id: format!("composed_{}", names.join("_")),
            name: format!("Composed: {}", names.join(" + ")),
            description: format!("Auto-composed skill merging {} skills", names.len()),
            trigger_conditions: all_triggers,
            knowledge: all_knowledge,
            sources: all_sources,
            validation: ValidationStatus::Pending,
            usage_stats: SkillUsageStats::default(),
        })))
    }

    fn compose_with_config(&self, skills: &[Skill], has_knowledge: bool) -> CognitiveResult<Option<Skill>> {
        let mut all_steps = Vec::new();
        let mut all_mappings = Vec::new();
        let mut names = Vec::new();

        // If mixed type: inject Knowledge items as DSL Store steps
        if has_knowledge {
            for skill in skills {
                if let Skill::Knowledge(k) = skill {
                    for (i, item) in k.knowledge.iter().enumerate() {
                        let key = format!("knowledge_{}_{}", k.id, i);
                        let value = knowledge_item_to_json(item);
                        all_steps.push(ToolCallStep {
                            step_id: format!("store_{}", key),
                            tool_name: "context.store".to_string(),
                            parameters: serde_json::json!({"key": key, "value": value}),
                            output_as: key,
                        });
                    }
                    names.push(k.name.clone());
                }
            }
        }

        // Merge Config skills: sequential tool_chains, merged parameter_mappings
        for skill in skills {
            if let Skill::Config(c) = skill {
                names.push(c.name.clone());
                all_steps.extend(c.tool_chain.clone());
                all_mappings.extend(c.parameter_mappings.clone());
            }
        }

        if all_steps.is_empty() {
            return Ok(None);
        }

        Ok(Some(Skill::Config(ConfigSkill {
            id: format!("composed_{}", names.join("_")),
            name: format!("Composed: {}", names.join(" + ")),
            description: format!("Auto-composed skill merging {} skills", names.len()),
            tool_chain: all_steps,
            parameter_mappings: all_mappings,
            conditional_branches: vec![],
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::super::KnowledgeItem;
    use super::*;

    fn make_skill(name: &str, knowledge: Vec<KnowledgeItem>) -> Skill {
        Skill::Knowledge(KnowledgeSkill {
            id: format!("id_{}", name),
            name: name.to_string(),
            description: format!("desc {}", name),
            trigger_conditions: vec![],
            knowledge,
            sources: vec![],
            validation: ValidationStatus::Pending,
            usage_stats: SkillUsageStats::default(),
        })
    }

    #[test]
    fn test_new_creates_composer() {
        let composer = SkillComposer::new();
        let _ = composer;
    }

    #[test]
    fn test_compose_returns_none_for_less_than_two_skills() {
        let composer = SkillComposer::new();
        let skills = vec![make_skill("a", vec![])];
        let result = composer.compose(&skills).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_compose_merges_knowledge_items() {
        let composer = SkillComposer::new();
        let skills = vec![
            make_skill(
                "a",
                vec![KnowledgeItem::Rule {
                    condition: "c1".to_string(),
                    action: "a1".to_string(),
                }],
            ),
            make_skill(
                "b",
                vec![
                    KnowledgeItem::Rule {
                        condition: "c2".to_string(),
                        action: "a2".to_string(),
                    },
                    KnowledgeItem::Lesson {
                        situation: "s".to_string(),
                        insight: "i".to_string(),
                    },
                ],
            ),
        ];
        let result = composer.compose(&skills).unwrap();
        assert!(result.is_some());
        match result.unwrap() {
            Skill::Knowledge(k) => {
                assert_eq!(k.knowledge.len(), 3);
                assert!(k.name.contains("Composed"));
            }
            _ => panic!("Expected Knowledge skill"),
        }
    }

    fn make_config_skill(name: &str, steps: Vec<ToolCallStep>) -> Skill {
        Skill::Config(ConfigSkill {
            id: format!("id_{}", name),
            name: name.to_string(),
            description: format!("desc {}", name),
            tool_chain: steps,
            parameter_mappings: vec![],
            conditional_branches: vec![],
        })
    }

    fn make_step(tool: &str, output: &str) -> ToolCallStep {
        ToolCallStep {
            step_id: format!("step_{}", tool),
            tool_name: tool.to_string(),
            parameters: serde_json::json!({}),
            output_as: output.to_string(),
        }
    }

    #[test]
    fn test_compose_config_skills_merges_tool_chains() {
        let composer = SkillComposer::new();
        let skills = vec![
            make_config_skill("c1", vec![make_step("search", "r1")]),
            make_config_skill("c2", vec![make_step("create", "r2")]),
        ];
        let result = composer.compose(&skills).unwrap();
        assert!(result.is_some());
        match result.unwrap() {
            Skill::Config(c) => {
                assert_eq!(c.tool_chain.len(), 2);
                assert_eq!(c.tool_chain[0].tool_name, "search");
                assert_eq!(c.tool_chain[1].tool_name, "create");
                assert!(c.name.contains("Composed"));
            }
            _ => panic!("Expected Config skill"),
        }
    }

    #[test]
    fn test_compose_mixed_knowledge_and_config() {
        let composer = SkillComposer::new();
        let skills = vec![
            make_skill(
                "k1",
                vec![KnowledgeItem::Rule {
                    condition: "cond".to_string(),
                    action: "act".to_string(),
                }],
            ),
            make_config_skill("c1", vec![make_step("search", "r1")]),
        ];
        let result = composer.compose(&skills).unwrap();
        assert!(result.is_some());
        match result.unwrap() {
            Skill::Config(c) => {
                // Should have 1 knowledge Store step + 1 config step
                assert_eq!(c.tool_chain.len(), 2);
                assert_eq!(c.tool_chain[0].tool_name, "context.store");
                assert_eq!(c.tool_chain[1].tool_name, "search");
            }
            _ => panic!("Expected Config skill for mixed composition"),
        }
    }
}
