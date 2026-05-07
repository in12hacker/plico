//! 技能进化系统 —— 从历史中提取、验证、进化技能
//!
//! 支持三种技能类型：
//! - 知识型：因果规则、检查清单、经验模板
//! - 配置型：工具调用链、参数映射、条件分支（DSL）
//! - 代码型：WASM 模块

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::kernel::cognition::skill_registry::SkillRegistry;
use crate::kernel::cognition::skill_validator::SkillValidator;
use crate::kernel::cognition::experience_miner::ExperienceMiner;
use crate::kernel::cognition::skill_composer::SkillComposer;
use crate::kernel::cognition::wasm_runtime::WasmRuntime;
use crate::kernel::cognition::dsl_interpreter::DslInterpreter;

use super::{
    CognitiveError, CognitiveResult, CodeSkill, ConfigSkill, DslSkill, KnowledgeSkill,
    Skill, SkillCandidate, SkillExecutionResult, SkillType, SkillUsageStats,
};
use super::SessionCognitiveState;
use crate::fs::embedding::EmbeddingProvider;
use crate::util::cosine_similarity;

/// 技能推荐
#[derive(Debug, Clone)]
pub struct SkillRecommendation {
    pub id: String,
    pub name: String,
    pub description: String,
    pub skill_type: SkillType,
    pub confidence: f32,
    pub estimated_tokens_saved: usize,
}

/// 验证结果
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub passed: bool,
    pub test_pass_rate: f32,
    pub issues: Vec<String>,
}

/// 技能进化系统
pub struct SkillForge {
    experience_miner: Arc<ExperienceMiner>,
    skill_validator: Arc<SkillValidator>,
    skill_composer: Arc<SkillComposer>,
    skill_registry: Arc<RwLock<SkillRegistry>>,
    wasm_runtime: Option<Arc<WasmRuntime>>,
    dsl_interpreter: Option<Arc<DslInterpreter>>,
    embedding: Option<Arc<dyn EmbeddingProvider>>,
}

impl Default for SkillForge {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillForge {
    pub fn new() -> Self {
        Self {
            experience_miner: Arc::new(ExperienceMiner::new()),
            skill_validator: Arc::new(SkillValidator::new()),
            skill_composer: Arc::new(SkillComposer::new()),
            skill_registry: Arc::new(RwLock::new(SkillRegistry::new())),
            wasm_runtime: None,
            dsl_interpreter: None,
            embedding: None,
        }
    }

    pub fn with_wasm_runtime(mut self, runtime: Arc<WasmRuntime>) -> Self {
        self.wasm_runtime = Some(runtime);
        self
    }

    pub fn with_dsl_interpreter(mut self, interpreter: Arc<DslInterpreter>) -> Self {
        self.dsl_interpreter = Some(interpreter);
        self
    }

    pub fn with_trajectory_tracker(self, tracker: Arc<crate::kernel::cognition::TrajectoryTracker>) -> Self {
        Self {
            experience_miner: Arc::new(ExperienceMiner::new().with_tracker(tracker)),
            ..self
        }
    }

    pub fn with_embedding(mut self, embedding: Arc<dyn EmbeddingProvider>) -> Self {
        self.embedding = Some(embedding);
        self
    }

    /// 从单次操作中提取技能候选
    pub async fn extract_candidate(
        &self,
        agent_id: &str,
        operation: &str,
    ) -> CognitiveResult<Vec<SkillCandidate>> {
        self.experience_miner.extract(agent_id, operation).await
    }

    /// 从会话中提取技能
    pub async fn extract_from_session(
        &self,
        session_state: &SessionCognitiveState,
    ) -> CognitiveResult<Vec<SkillCandidate>> {
        self.experience_miner.extract_from_session(session_state).await
    }

    /// 验证技能候选
    pub async fn validate_skill(
        &self,
        _agent_id: &str,
        candidate: &SkillCandidate,
    ) -> CognitiveResult<ValidationResult> {
        self.skill_validator.validate(candidate).await
    }

    /// 注册通过验证的技能
    pub async fn register_skill(
        &self,
        agent_id: &str,
        skill: Skill,
    ) -> CognitiveResult<String> {
        let mut registry = self.skill_registry.write().await;
        let skill_id = registry.register(agent_id, skill).await?;
        Ok(skill_id)
    }

    /// 为给定意图推荐相关技能
    pub async fn recommend(
        &self,
        agent_id: &str,
        intent: &str,
    ) -> CognitiveResult<Vec<SkillRecommendation>> {
        let registry = self.skill_registry.read().await;
        let all_skills = registry.list_for_agent(agent_id).await;

        let mut recommendations = Vec::new();
        for (skill_id, skill, stats) in all_skills {
            let relevance = self.compute_intent_skill_relevance(intent, &skill).await?;
            if relevance > 0.5 {
                let (name, desc, skill_type) = match &skill {
                    Skill::Knowledge(k) => (k.name.clone(), k.description.clone(), SkillType::Knowledge),
                    Skill::Config(c) => (c.name.clone(), c.description.clone(), SkillType::Config),
                    Skill::Code(code) => (code.name.clone(), code.description.clone(), SkillType::Code),
                };

                recommendations.push(SkillRecommendation {
                    id: skill_id,
                    name,
                    description: desc,
                    skill_type,
                    confidence: relevance,
                    estimated_tokens_saved: (stats.avg_tokens_saved * stats.invocations as f32) as usize,
                });
            }
        }

        recommendations.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());
        recommendations.truncate(5);

        Ok(recommendations)
    }

    /// 执行技能
    pub async fn execute_skill(
        &self,
        _agent_id: &str,
        skill_id: &str,
        inputs: serde_json::Value,
    ) -> CognitiveResult<SkillExecutionResult> {
        let registry = self.skill_registry.read().await;
        let skill = registry.get(skill_id).await
            .ok_or_else(|| CognitiveError::SkillFailed(format!("Skill not found: {}", skill_id)))?;

        match skill {
            Skill::Knowledge(k) => self.execute_knowledge_skill(k, inputs).await,
            Skill::Config(c) => self.execute_config_skill(c, inputs).await,
            Skill::Code(code) => self.execute_code_skill(code, inputs).await,
        }
    }

    /// 组合多个技能
    pub async fn compose_skills(
        &self,
        skill_ids: &[String],
    ) -> CognitiveResult<Option<Skill>> {
        let registry = self.skill_registry.read().await;
        let mut skills = Vec::new();
        for id in skill_ids {
            if let Some(skill) = registry.get(id).await {
                skills.push(skill);
            }
        }
        self.skill_composer.compose(&skills)
    }

    /// 获取技能使用统计
    pub async fn get_skill_stats(&self, skill_id: &str) -> Option<SkillUsageStats> {
        let registry = self.skill_registry.read().await;
        registry.get_stats(skill_id).await
    }

    // --- Private execution helpers ---

    async fn execute_knowledge_skill(
        &self,
        skill: KnowledgeSkill,
        _inputs: serde_json::Value,
    ) -> CognitiveResult<SkillExecutionResult> {
        Ok(SkillExecutionResult::Knowledge {
            items: skill.knowledge,
        })
    }

    async fn execute_config_skill(
        &self,
        skill: ConfigSkill,
        inputs: serde_json::Value,
    ) -> CognitiveResult<SkillExecutionResult> {
        let interpreter = self.dsl_interpreter.as_ref()
            .ok_or(CognitiveError::DslExecutionFailed("DSL interpreter not available".to_string()))?;

        let dsl = DslSkill {
            version: "1.0".to_string(),
            name: skill.name,
            description: skill.description,
            inputs: Vec::new(), // TODO
            steps: skill.tool_chain.into_iter().map(|step| {
                super::dsl_interpreter::DslStep::ToolCall {
                    tool: step.tool_name,
                    params: step.parameters,
                    output_as: Some(step.output_as),
                }
            }).collect(),
            outputs: Vec::new(), // TODO
        };

        let outputs = interpreter.execute(&dsl, inputs)
            .map_err(|e| CognitiveError::DslExecutionFailed(e.to_string()))?;

        Ok(SkillExecutionResult::Config { outputs })
    }

    async fn execute_code_skill(
        &self,
        skill: CodeSkill,
        inputs: serde_json::Value,
    ) -> CognitiveResult<SkillExecutionResult> {
        let runtime = self.wasm_runtime.as_ref()
            .ok_or(CognitiveError::WasmRuntimeNotAvailable)?;

        let outputs = runtime.execute(&skill.wasm_bytes, inputs, &skill.resource_limits).await?;
        Ok(SkillExecutionResult::Code { outputs })
    }

    async fn compute_intent_skill_relevance(
        &self,
        intent: &str,
        skill: &Skill,
    ) -> CognitiveResult<f32> {
        // Get skill description text for comparison
        let skill_text = match skill {
            Skill::Knowledge(k) => {
                // Check trigger conditions first (exact pattern match)
                for trigger in &k.trigger_conditions {
                    if trigger.intent_pattern == "*" {
                        return Ok(trigger.min_confidence);
                    }
                    if intent.to_lowercase().contains(&trigger.intent_pattern.to_lowercase()) {
                        return Ok(trigger.min_confidence);
                    }
                }
                format!("{} {}", k.name, k.description)
            }
            Skill::Config(c) => format!("{} {}", c.name, c.description),
            Skill::Code(code) => format!("{} {}", code.name, code.description),
        };

        // Use embedding similarity if available
        if let Some(ref embedding) = self.embedding {
            let intent_emb = embedding.embed(intent)
                .map_err(|e| CognitiveError::EmbeddingFailed(e.to_string()))?;
            let skill_emb = embedding.embed(&skill_text)
                .map_err(|e| CognitiveError::EmbeddingFailed(e.to_string()))?;
            let sim = cosine_similarity(&intent_emb.embedding, &skill_emb.embedding);
            return Ok(sim.clamp(0.0, 1.0));
        }

        // Fallback: keyword overlap
        let intent_lower = intent.to_lowercase();
        let skill_lower = skill_text.to_lowercase();
        let intent_words: Vec<&str> = intent_lower.split_whitespace().collect();
        let skill_words: Vec<&str> = skill_lower.split_whitespace().collect();
        let overlap = intent_words.iter().filter(|w| skill_words.contains(w)).count();
        let total = intent_words.len().max(skill_words.len()).max(1);
        Ok((overlap as f32 / total as f32).clamp(0.0, 1.0))
    }
}
