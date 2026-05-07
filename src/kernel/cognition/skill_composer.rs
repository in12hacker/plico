//! 技能组合器 —— 组合多个技能形成新技能

use super::{
    CognitiveResult, Skill, KnowledgeSkill,
    ValidationStatus, SkillUsageStats,
};

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

        // Merge all Knowledge skills
        let mut all_knowledge = Vec::new();
        let mut all_triggers = Vec::new();
        let mut all_sources = Vec::new();
        let mut names = Vec::new();

        for skill in skills {
            match skill {
                Skill::Knowledge(k) => {
                    names.push(k.name.clone());
                    all_knowledge.extend(k.knowledge.clone());
                    all_triggers.extend(k.trigger_conditions.clone());
                    all_sources.extend(k.sources.clone());
                }
                // Skip non-knowledge skills for now
                _ => {}
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::KnowledgeItem;

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
            make_skill("a", vec![KnowledgeItem::Rule {
                condition: "c1".to_string(),
                action: "a1".to_string(),
            }]),
            make_skill("b", vec![
                KnowledgeItem::Rule {
                    condition: "c2".to_string(),
                    action: "a2".to_string(),
                },
                KnowledgeItem::Lesson {
                    situation: "s".to_string(),
                    insight: "i".to_string(),
                },
            ]),
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
}
