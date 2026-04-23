use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Debug, Clone)]
pub struct GoalTemplate {
    pub trigger_keywords: Vec<String>,
    pub action_sequence: Vec<String>,
    pub success_count: usize,
    pub total_count: usize,
}

impl GoalTemplate {
    pub fn success_rate(&self) -> f32 {
        if self.total_count == 0 {
            return 0.0;
        }
        self.success_count as f32 / self.total_count as f32
    }
}

#[derive(Debug, Clone)]
pub struct SelfGoal {
    pub goal_text: String,
    pub confidence: f32,
    pub template_id: usize,
}

pub struct GoalGenerator {
    templates: RwLock<HashMap<String, Vec<GoalTemplate>>>,
}

impl GoalGenerator {
    pub fn new() -> Self {
        Self {
            templates: RwLock::new(HashMap::new()),
        }
    }

    pub fn record_goal(&self, agent_id: &str, keywords: &[String], actions: &[String], success: bool) {
        let mut templates = self.templates.write().unwrap();
        let agent_templates = templates.entry(agent_id.to_string()).or_insert_with(Vec::new);

        if let Some(existing) = agent_templates.iter_mut().find(|t| t.trigger_keywords == keywords && t.action_sequence == actions) {
            existing.total_count += 1;
            if success {
                existing.success_count += 1;
            }
        } else {
            agent_templates.push(GoalTemplate {
                trigger_keywords: keywords.to_vec(),
                action_sequence: actions.to_vec(),
                success_count: if success { 1 } else { 0 },
                total_count: 1,
            });
        }
    }

    pub fn generate_goals(&self, agent_id: &str, context: &str) -> Vec<SelfGoal> {
        let templates = self.templates.read().unwrap();
        let agent_templates = templates.get(agent_id);

        let Some(agent_templates) = agent_templates else {
            return Vec::new();
        };

        let context_words: Vec<&str> = context.split_whitespace().collect();
        let mut goals = Vec::new();

        for (idx, template) in agent_templates.iter().enumerate() {
            let matches = template.trigger_keywords.iter().any(|kw| {
                context_words.iter().any(|w| w.to_lowercase().contains(&kw.to_lowercase()))
            });

            if matches {
                goals.push(SelfGoal {
                    goal_text: template.action_sequence.join(" -> "),
                    confidence: template.success_rate(),
                    template_id: idx,
                });
            }
        }

        goals
    }
}

impl Default for GoalGenerator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_goal_generator_records_template() {
        let generator = GoalGenerator::new();

        generator.record_goal(
            "agent1",
            &["analyze".to_string(), "report".to_string()],
            &["collect".to_string(), "summarize".to_string()],
            true,
        );

        let goals = generator.generate_goals("agent1", "I need to analyze data and generate a report");
        assert_eq!(goals.len(), 1);
        assert_eq!(goals[0].confidence, 1.0);
    }

    #[test]
    fn test_goal_generator_matches_context() {
        let generator = GoalGenerator::new();

        generator.record_goal(
            "agent1",
            &["backup".to_string(), "verify".to_string()],
            &["copy".to_string(), "check".to_string()],
            true,
        );
        generator.record_goal(
            "agent1",
            &["backup".to_string(), "verify".to_string()],
            &["copy".to_string(), "check".to_string()],
            false,
        );

        let goals = generator.generate_goals("agent1", "please backup the files and verify integrity");
        assert_eq!(goals.len(), 1);
        assert!((goals[0].confidence - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_goal_generator_no_match_returns_empty() {
        let generator = GoalGenerator::new();

        generator.record_goal(
            "agent1",
            &["backup".to_string(), "verify".to_string()],
            &["copy".to_string(), "check".to_string()],
            true,
        );

        let goals = generator.generate_goals("agent1", "just reading some documents");
        assert_eq!(goals.len(), 0);
    }
}