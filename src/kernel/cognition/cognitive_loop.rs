//! 认知循环引擎 —— Plico v3.0 核心
//!
//! 持续监控 Agent 的认知状态，主动优化认知环境。
//! 所有优化行为可观测、可覆盖、可调试。

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;


use super::{
    CognitiveConfig, CognitivePattern, CognitiveResult,
    ContextQualityEngine, IntentSemanticNetwork, SkillForge,
    TrajectoryTracker, now_ms,
};

/// 认知循环引擎
pub struct CognitiveLoop {
    context_analyzer: Arc<ContextQualityEngine>,
    pub intent_network: Arc<IntentSemanticNetwork>,
    skill_forge: Arc<SkillForge>,
    pub trajectory_tracker: Arc<TrajectoryTracker>,
    config: CognitiveConfig,
    state: RwLock<CognitiveState>,
}

/// 认知状态
#[derive(Debug, Default)]
pub struct CognitiveState {
    pub active_sessions: HashMap<String, SessionCognitiveState>,
    pub last_optimization_ms: u64,
    pub stats: CognitiveStats,
}

/// 会话认知状态
#[derive(Debug, Clone)]
pub struct SessionCognitiveState {
    pub agent_id: String,
    pub session_id: String,
    pub context_quality_score: f32,
    pub context_utilization: f32,
    pub attention_focus: Vec<String>,
    pub detected_patterns: Vec<CognitivePattern>,
    pub last_optimization: Option<CognitiveOptimizationReport>,
}

/// 认知统计
#[derive(Debug, Clone, Default)]
pub struct CognitiveStats {
    pub total_optimizations: u64,
    pub total_token_savings: u64,
    pub total_skills_extracted: u64,
    pub total_lessons_injected: u64,
}

/// 认知优化报告
#[derive(Debug, Clone)]
pub struct CognitiveOptimizationReport {
    pub timestamp_ms: u64,
    pub agent_id: String,
    pub session_id: String,
    pub optimizations: Vec<OptimizationAction>,
    pub token_savings: usize,
    pub quality_delta: f32,
    pub context_before: ContextSnapshot,
    pub context_after: ContextSnapshot,
}

/// 上下文快照
#[derive(Debug, Clone, Default)]
pub struct ContextSnapshot {
    pub cid_count: usize,
    pub token_count: usize,
    pub quality_score: f32,
}

impl CognitiveLoop {
    pub fn new(
        context_analyzer: Arc<ContextQualityEngine>,
        intent_network: Arc<IntentSemanticNetwork>,
        skill_forge: Arc<SkillForge>,
    ) -> Self {
        Self {
            context_analyzer,
            intent_network,
            skill_forge,
            trajectory_tracker: Arc::new(TrajectoryTracker::new()),
            config: CognitiveConfig::default(),
            state: RwLock::new(CognitiveState::default()),
        }
    }

    pub fn with_config(mut self, config: CognitiveConfig) -> Self {
        self.config = config;
        self
    }

    /// Agent 声明意图时触发认知分析
    pub async fn on_intent_declared(
        &self,
        agent_id: &str,
        session_id: &str,
        intent: &str,
        current_context: &[String],
    ) -> CognitiveResult<CognitiveOptimizationReport> {
        let mut report = CognitiveOptimizationReport {
            timestamp_ms: now_ms(),
            agent_id: agent_id.to_string(),
            session_id: session_id.to_string(),
            optimizations: Vec::new(),
            token_savings: 0,
            quality_delta: 0.0,
            context_before: ContextSnapshot {
                cid_count: current_context.len(),
                token_count: 0, // TODO: compute
                quality_score: 0.0,
            },
            context_after: ContextSnapshot::default(),
        };

        // 1. 分析当前上下文质量
        let quality = self.context_analyzer.analyze(agent_id, current_context).await?;
        report.context_before.quality_score = quality.score;
        report.context_before.token_count = quality.token_count;

        let mut optimized_context = current_context.to_vec();

        // 2. 如果质量低于阈值或利用率过高，执行压缩
        let should_compress = quality.score < 0.6
            || (quality.token_count as f32 / 8192.0) > self.config.context_compression_threshold;

        if should_compress {
            let compressed = self.context_analyzer.compress(agent_id, current_context).await?;
            report.optimizations.push(OptimizationAction::ContextCompressed {
                original_tokens: quality.token_count,
                compressed_tokens: compressed.token_count,
                reason: compressed.reason,
            });
            report.token_savings += quality.token_count.saturating_sub(compressed.token_count);
            optimized_context = compressed.retained_cids;
            report.context_after.cid_count = optimized_context.len();
            report.context_after.token_count = compressed.token_count;
        }

        // 3. 基于意图语义网络预加载相关上下文
        if self.config.proactive_prefetch_enabled {
            let related = self.intent_network.find_related(agent_id, intent).await?;
            if !related.is_empty() {
                let cids: Vec<String> = related.iter().map(|r| r.cid.clone()).collect();
                let scores: Vec<f32> = related.iter().map(|r| r.score).collect();
                report.optimizations.push(OptimizationAction::ContextPrefetched {
                    cids: cids.clone(),
                    relevance_scores: scores,
                });
                // Merge prefetched CIDs without duplicates
                for cid in cids {
                    if !optimized_context.contains(&cid) {
                        optimized_context.push(cid);
                    }
                }
            }
        }

        // 4. 检测失败模式并注入经验教训
        if self.config.failure_pattern_detection_enabled {
            let lessons = self.detect_failure_lessons(agent_id, intent).await?;
            for lesson in lessons {
                report.optimizations.push(OptimizationAction::LessonInjected {
                    lesson: lesson.text,
                    source: lesson.source,
                });
            }
        }

        // 5. 推荐相关技能
        if self.config.skill_extraction_enabled {
            let skills = self.skill_forge.recommend(agent_id, intent).await?;
            for skill in skills {
                report.optimizations.push(OptimizationAction::SkillRecommended {
                    skill_id: skill.id,
                    skill_name: skill.name,
                    confidence: skill.confidence,
                });
            }
        }

        // 6. 更新认知轨迹
        self.trajectory_tracker.record_intent(agent_id, intent).await;

        // 7. 重新计算质量
        let after_quality = self.context_analyzer.analyze(agent_id, &optimized_context).await?;
        report.context_after.quality_score = after_quality.score;
        report.quality_delta = after_quality.score - quality.score;

        // 8. 更新会话状态
        self.update_session_state(agent_id, session_id, &report, &optimized_context).await;

        // 9. 更新全局统计
        {
            let mut state = self.state.write().await;
            state.stats.total_optimizations += 1;
            state.stats.total_token_savings += report.token_savings as u64;
            state.last_optimization_ms = now_ms();
        }

        Ok(report)
    }

    /// Agent 完成操作后触发经验提取
    pub async fn on_operation_completed(
        &self,
        agent_id: &str,
        operation: &str,
        success: bool,
        _context_before: &[String],
        _context_after: &[String],
    ) -> CognitiveResult<()> {
        // 1. 追踪认知轨迹
        self.trajectory_tracker.record_operation(agent_id, operation, success).await;

        // 2. 如果成功，提取技能候选
        if success && self.config.skill_extraction_enabled {
            // Async skill extraction happens in background
            let forge = Arc::clone(&self.skill_forge);
            let agent_id = agent_id.to_string();
            let operation = operation.to_string();
            tokio::spawn(async move {
                // TODO: extract skill candidate from recent trajectory
                let _ = forge.extract_candidate(&agent_id, &operation).await;
            });
        }

        // 3. 如果失败，记录失败模式
        if !success && self.config.failure_pattern_detection_enabled {
            self.trajectory_tracker.record_failure(agent_id, operation).await;
        }

        Ok(())
    }

    /// 定时任务：检查所有活跃会话的上下文质量
    pub async fn run_periodic_check(&self) -> CognitiveResult<Vec<CognitiveOptimizationReport>> {
        let reports = Vec::new();
        let sessions_to_check = {
            let state = self.state.read().await;
            state.active_sessions.keys().cloned().collect::<Vec<_>>()
        };

        for key in sessions_to_check {
            let session = {
                let state = self.state.read().await;
                state.active_sessions.get(&key).cloned()
            };

            if let Some(session_state) = session {
                if session_state.context_utilization > self.config.context_compression_threshold {
                    // Trigger compression for this session
                    // TODO: get actual context CIDs and compress
                }
            }
        }

        Ok(reports)
    }

    /// 注册新会话
    pub async fn register_session(&self, agent_id: &str, session_id: &str) {
        let mut state = self.state.write().await;
        let key = format!("{}:{}", agent_id, session_id);
        state.active_sessions.insert(key, SessionCognitiveState {
            agent_id: agent_id.to_string(),
            session_id: session_id.to_string(),
            context_quality_score: 1.0,
            context_utilization: 0.0,
            attention_focus: Vec::new(),
            detected_patterns: Vec::new(),
            last_optimization: None,
        });
    }

    /// 结束会话
    pub async fn end_session(&self, agent_id: &str, session_id: &str) {
        let key = format!("{}:{}", agent_id, session_id);
        let mut state = self.state.write().await;
        if let Some(session_state) = state.active_sessions.remove(&key) {
            // Extract final skills from session trajectory
            if self.config.skill_extraction_enabled {
                let forge = Arc::clone(&self.skill_forge);
                tokio::spawn(async move {
                    let _ = forge.extract_from_session(&session_state).await;
                });
            }
        }
    }

    /// 获取优化统计
    pub async fn stats(&self) -> CognitiveStats {
        let state = self.state.read().await;
        state.stats.clone()
    }

    /// 更新会话上下文利用率
    pub async fn update_context_utilization(
        &self,
        agent_id: &str,
        session_id: &str,
        utilization: f32,
    ) {
        let key = format!("{}:{}", agent_id, session_id);
        let mut state = self.state.write().await;
        if let Some(session) = state.active_sessions.get_mut(&key) {
            session.context_utilization = utilization;
        }
    }

    // --- Private helpers ---

    async fn detect_failure_lessons(
        &self,
        agent_id: &str,
        intent: &str,
    ) -> CognitiveResult<Vec<FailureLesson>> {
        let failures = self.trajectory_tracker.get_recent_failures(agent_id, 10).await;
        let mut lessons = Vec::new();

        for failure in failures {
            // Check if this failure is semantically related to current intent
            let related = self.intent_network.is_semantically_related(&failure.intent, intent).await?;
            if related {
                lessons.push(FailureLesson {
                    text: format!("Previous attempt on '{}' failed: {}", failure.intent, failure.operation),
                    source: failure.session_id,
                });
            }
        }

        Ok(lessons)
    }

    async fn update_session_state(
        &self,
        agent_id: &str,
        session_id: &str,
        report: &CognitiveOptimizationReport,
        context: &[String],
    ) {
        let key = format!("{}:{}", agent_id, session_id);
        let mut state = self.state.write().await;
        if let Some(session) = state.active_sessions.get_mut(&key) {
            session.last_optimization = Some(report.clone());
            session.context_quality_score = report.context_after.quality_score;
            // Update attention focus based on context
            session.attention_focus = context.iter().take(5).cloned().collect();
        }
    }
}

/// 优化动作
#[derive(Debug, Clone)]
pub enum OptimizationAction {
    /// 压缩冗余上下文
    ContextCompressed { original_tokens: usize, compressed_tokens: usize, reason: String },
    /// 预加载相关上下文
    ContextPrefetched { cids: Vec<String>, relevance_scores: Vec<f32> },
    /// 注入经验教训
    LessonInjected { lesson: String, source: String },
    /// 标记过时信息
    StaleInfoMarked { cids: Vec<String>, reason: String },
    /// 推荐技能
    SkillRecommended { skill_id: String, skill_name: String, confidence: f32 },
}

/// 失败经验教训
#[derive(Debug, Clone)]
pub struct FailureLesson {
    pub text: String,
    pub source: String,
}
