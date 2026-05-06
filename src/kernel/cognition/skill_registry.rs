//! 技能注册表 —— 技能的版本管理和检索

use std::collections::HashMap;

use super::{CognitiveResult, Skill, SkillUsageStats, ValidationStatus};

/// 技能注册表
#[derive(Debug, Default)]
pub struct SkillRegistry {
    /// Agent ID -> (Skill ID -> Skill Record)
    agent_skills: HashMap<String, HashMap<String, SkillRecord>>,
}

#[derive(Debug, Clone)]
pub struct SkillRecord {
    pub skill: Skill,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub version: u32,
    pub stats: SkillUsageStats,
    pub validation: ValidationStatus,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            agent_skills: HashMap::new(),
        }
    }

    pub async fn register(&mut self, agent_id: &str, skill: Skill) -> CognitiveResult<String> {
        let skill_id = format!("skill_{}_{}", agent_id, uuid::Uuid::new_v4());
        let record = SkillRecord {
            skill,
            created_at_ms: super::now_ms(),
            updated_at_ms: super::now_ms(),
            version: 1,
            stats: SkillUsageStats::default(),
            validation: ValidationStatus::Pending,
        };

        self.agent_skills
            .entry(agent_id.to_string())
            .or_default()
            .insert(skill_id.clone(), record);

        Ok(skill_id)
    }

    pub async fn get(&self, skill_id: &str) -> Option<Skill> {
        for agent_skills in self.agent_skills.values() {
            if let Some(record) = agent_skills.get(skill_id) {
                return Some(record.skill.clone());
            }
        }
        None
    }

    pub async fn get_record(&self, skill_id: &str) -> Option<SkillRecord> {
        for agent_skills in self.agent_skills.values() {
            if let Some(record) = agent_skills.get(skill_id) {
                return Some(record.clone());
            }
        }
        None
    }

    pub async fn update(&mut self, skill_id: &str, skill: Skill) -> CognitiveResult<()> {
        for agent_skills in self.agent_skills.values_mut() {
            if let Some(record) = agent_skills.get_mut(skill_id) {
                record.skill = skill;
                record.updated_at_ms = super::now_ms();
                record.version += 1;
                return Ok(());
            }
        }
        Err(super::CognitiveError::SkillFailed(format!("Skill not found: {}", skill_id)))
    }

    pub async fn increment_usage(&mut self, skill_id: &str, tokens_saved: f32, success: bool) {
        for agent_skills in self.agent_skills.values_mut() {
            if let Some(record) = agent_skills.get_mut(skill_id) {
                record.stats.invocations += 1;
                if success {
                    record.stats.successes += 1;
                }
                // Rolling average of tokens saved
                let n = record.stats.invocations as f32;
                record.stats.avg_tokens_saved =
                    (record.stats.avg_tokens_saved * (n - 1.0) + tokens_saved) / n;
                record.stats.last_used_ms = super::now_ms();
            }
        }
    }

    pub async fn list_for_agent(&self, agent_id: &str) -> Vec<(String, Skill, SkillUsageStats)> {
        self.agent_skills
            .get(agent_id)
            .map(|skills| {
                skills.iter()
                    .map(|(id, record)| (id.clone(), record.skill.clone(), record.stats.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub async fn get_stats(&self, skill_id: &str) -> Option<SkillUsageStats> {
        self.get_record(skill_id).await.map(|r| r.stats)
    }

    pub async fn remove(&mut self, skill_id: &str) -> bool {
        for agent_skills in self.agent_skills.values_mut() {
            if agent_skills.remove(skill_id).is_some() {
                return true;
            }
        }
        false
    }

    pub async fn count_for_agent(&self, agent_id: &str) -> usize {
        self.agent_skills
            .get(agent_id)
            .map(|s| s.len())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::cognition::{KnowledgeItem, KnowledgeSkill, Skill, SkillUsageStats, ValidationStatus};

    fn test_skill() -> Skill {
        Skill::Knowledge(KnowledgeSkill {
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
        })
    }

    #[tokio::test]
    async fn new_creates_empty_registry() {
        let registry = SkillRegistry::new();
        assert_eq!(registry.count_for_agent("agent1").await, 0);
    }

    #[tokio::test]
    async fn register_generates_id_and_stores_skill() {
        let mut registry = SkillRegistry::new();
        let skill = test_skill();
        let id = registry.register("agent1", skill).await.unwrap();
        assert!(id.starts_with("skill_agent1_"));
        assert_eq!(registry.count_for_agent("agent1").await, 1);
    }

    #[tokio::test]
    async fn get_returns_correct_skill() {
        let mut registry = SkillRegistry::new();
        let skill = test_skill();
        let id = registry.register("agent1", skill.clone()).await.unwrap();
        let retrieved = registry.get(&id).await.unwrap();
        match retrieved {
            Skill::Knowledge(k) => {
                assert_eq!(k.name, "Test Skill");
                assert_eq!(k.description, "desc");
            }
            _ => panic!("Expected Knowledge skill"),
        }
    }

    #[tokio::test]
    async fn get_returns_none_for_unknown_id() {
        let registry = SkillRegistry::new();
        assert!(registry.get("unknown").await.is_none());
    }

    #[tokio::test]
    async fn update_updates_skill_and_bumps_version() {
        let mut registry = SkillRegistry::new();
        let skill = test_skill();
        let id = registry.register("agent1", skill).await.unwrap();

        let updated_skill = Skill::Knowledge(KnowledgeSkill {
            id: "test".to_string(),
            name: "Updated Skill".to_string(),
            description: "updated desc".to_string(),
            trigger_conditions: vec![],
            knowledge: vec![],
            sources: vec![],
            validation: ValidationStatus::Pending,
            usage_stats: SkillUsageStats::default(),
        });

        registry.update(&id, updated_skill).await.unwrap();
        let record = registry.get_record(&id).await.unwrap();
        assert_eq!(record.version, 2);
        match &record.skill {
            Skill::Knowledge(k) => assert_eq!(k.name, "Updated Skill"),
            _ => panic!("Expected Knowledge skill"),
        }
    }

    #[tokio::test]
    async fn increment_usage_updates_stats() {
        let mut registry = SkillRegistry::new();
        let skill = test_skill();
        let id = registry.register("agent1", skill).await.unwrap();

        registry.increment_usage(&id, 10.0, true).await;
        let stats = registry.get_stats(&id).await.unwrap();
        assert_eq!(stats.invocations, 1);
        assert_eq!(stats.successes, 1);
        assert_eq!(stats.avg_tokens_saved, 10.0);
        assert!(stats.last_used_ms > 0);

        registry.increment_usage(&id, 20.0, false).await;
        let stats = registry.get_stats(&id).await.unwrap();
        assert_eq!(stats.invocations, 2);
        assert_eq!(stats.successes, 1);
        assert_eq!(stats.avg_tokens_saved, 15.0);
    }

    #[tokio::test]
    async fn list_for_agent_filters_by_agent_id() {
        let mut registry = SkillRegistry::new();
        let skill = test_skill();
        registry.register("agent1", skill.clone()).await.unwrap();
        registry.register("agent2", skill).await.unwrap();

        let list1 = registry.list_for_agent("agent1").await;
        let list2 = registry.list_for_agent("agent2").await;
        assert_eq!(list1.len(), 1);
        assert_eq!(list2.len(), 1);
        assert!(registry.list_for_agent("agent3").await.is_empty());
    }

    #[tokio::test]
    async fn count_for_agent_returns_correct_count() {
        let mut registry = SkillRegistry::new();
        let skill = test_skill();
        assert_eq!(registry.count_for_agent("agent1").await, 0);
        registry.register("agent1", skill.clone()).await.unwrap();
        assert_eq!(registry.count_for_agent("agent1").await, 1);
        registry.register("agent1", skill).await.unwrap();
        assert_eq!(registry.count_for_agent("agent1").await, 2);
    }

    #[tokio::test]
    async fn remove_deletes_skill() {
        let mut registry = SkillRegistry::new();
        let skill = test_skill();
        let id = registry.register("agent1", skill).await.unwrap();
        assert!(registry.get(&id).await.is_some());

        let removed = registry.remove(&id).await;
        assert!(removed);
        assert!(registry.get(&id).await.is_none());
        assert_eq!(registry.count_for_agent("agent1").await, 0);
    }

    #[tokio::test]
    async fn get_stats_returns_correct_stats() {
        let mut registry = SkillRegistry::new();
        let skill = test_skill();
        let id = registry.register("agent1", skill).await.unwrap();

        registry.increment_usage(&id, 5.0, true).await;
        let stats = registry.get_stats(&id).await.unwrap();
        assert_eq!(stats.invocations, 1);
        assert_eq!(stats.successes, 1);
    }
}
