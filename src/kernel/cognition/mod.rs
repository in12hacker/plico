//! Plico Soul v3.0 — 认知共生内核 (Cognitive Symbiotic Kernel)
//!
//! 本模块实现 Plico 的认知增强层，包含：
//! - CognitiveLoop: 认知循环引擎，持续监控并主动优化Agent认知环境
//! - ContextQualityEngine: 上下文质量引擎，解决上下文腐败问题
//! - IntentSemanticNetwork: 意图语义网络，基于embedding的因果/时序/关联关系
//! - SkillForge: 技能进化系统，从历史中提取、验证、进化技能
//! - WASM Runtime + DSL Interpreter: 混合技能执行环境
//!
//! 设计原则：
//! - Plico 优化 Agent 的输入质量，Agent 决定输出内容
//! - 所有主动优化行为对 Agent 可观测、可覆盖、可调试

pub mod cognitive_loop;
pub mod context_quality;
pub mod dsl_interpreter;
pub mod experience_miner;
pub mod intent_network;
pub mod skill_composer;
pub mod skill_forge;
pub mod skill_registry;
pub mod skill_validator;
pub mod trajectory_tracker;
pub mod wasm_runtime;

// Re-export core types for inter-module use
pub use cognitive_loop::{CognitiveLoop, CognitiveOptimizationReport, OptimizationAction, CognitiveStats, CognitiveState, SessionCognitiveState};
pub use context_quality::{ContextQualityEngine, ContextQuality, CompressedContext, RemovalRecord, RemovalReason, ContextIssue};
pub use intent_network::{IntentSemanticNetwork, SemanticRelation, SemanticNode, SemanticEdge, RelatedContext, LearningReport, ExperienceAssociation};
pub use skill_forge::{SkillForge, SkillRecommendation, ValidationResult};
pub use trajectory_tracker::{TrajectoryTracker, FailureRecord, FailureStats};
pub use experience_miner::ExperienceMiner;
pub use skill_validator::SkillValidator;
pub use skill_composer::SkillComposer;
pub use skill_registry::SkillRegistry;
pub use wasm_runtime::WasmRuntime;
pub use dsl_interpreter::{DslInterpreter, DslSkill, DslStep};

use thiserror::Error;

/// 认知模块统一错误类型
#[derive(Error, Debug, Clone)]
pub enum CognitiveError {
    #[error("Context analysis failed: {0}")]
    AnalysisFailed(String),

    #[error("Compression failed: {0}")]
    CompressionFailed(String),

    #[error("Semantic network operation failed: {0}")]
    NetworkFailed(String),

    #[error("Skill operation failed: {0}")]
    SkillFailed(String),

    #[error("WASM runtime not available")]
    WasmRuntimeNotAvailable,

    #[error("WASM init failed: {0}")]
    WasmInitFailed(String),

    #[error("WASM execution failed: {0}")]
    WasmExecutionFailed(String),

    #[error("DSL execution failed: {0}")]
    DslExecutionFailed(String),

    #[error("Invalid skill type: {0}")]
    InvalidSkillType(String),

    #[error("Embedding failed: {0}")]
    EmbeddingFailed(String),

    #[error("IO error: {0}")]
    Io(String),
}

/// 认知模块统一Result类型
pub type CognitiveResult<T> = Result<T, CognitiveError>;

/// 时间戳辅助函数
pub(crate) use crate::util::now_ms;

/// Token分解统计
#[derive(Debug, Clone, Default)]
pub struct TokenBreakdown {
    pub core_knowledge: usize,
    pub procedural_info: usize,
    pub temporary_data: usize,
    pub redundant_info: usize,
    pub stale_info: usize,
}

/// 操作记录
#[derive(Debug, Clone)]
pub struct OperationRecord {
    pub operation: String,
    pub params: serde_json::Value,
    pub success: bool,
    pub duration_ms: u64,
    pub timestamp_ms: u64,
}

/// 技能候选
#[derive(Debug, Clone)]
pub struct SkillCandidate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub skill_type: SkillType,
    pub source_operations: Vec<String>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillType {
    Knowledge,
    Config,
    Code,
}

/// 技能使用统计
#[derive(Debug, Clone, Default)]
pub struct SkillUsageStats {
    pub invocations: u64,
    pub successes: u64,
    pub avg_tokens_saved: f32,
    pub last_used_ms: u64,
}

/// 验证状态
#[derive(Debug, Clone)]
pub enum ValidationStatus {
    Pending,
    Validated { validated_at_ms: u64, test_pass_rate: f32 },
    Rejected { reason: String },
    Deprecated { replaced_by: Option<String> },
}

/// 技能执行结果
#[derive(Debug, Clone)]
pub enum SkillExecutionResult {
    Knowledge { items: Vec<KnowledgeItem> },
    Config { outputs: serde_json::Value },
    Code { outputs: serde_json::Value },
}

/// 知识项
#[derive(Debug, Clone)]
pub enum KnowledgeItem {
    Rule { condition: String, action: String },
    Checklist { items: Vec<String> },
    Lesson { situation: String, insight: String },
    Warning { pattern: String, consequence: String },
}

/// 经验来源
#[derive(Debug, Clone)]
pub struct ExperienceSource {
    pub session_id: String,
    pub operation: String,
    pub timestamp_ms: u64,
    pub success: bool,
}

/// 触发条件
#[derive(Debug, Clone)]
pub struct TriggerCondition {
    pub intent_pattern: String,
    pub min_confidence: f32,
    pub required_context_tags: Vec<String>,
}

/// 报告详细程度
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportVerbosity {
    None,
    Summary,
    Detailed,
    Debug,
}

/// 认知模式
#[derive(Debug, Clone)]
pub enum CognitivePattern {
    RepetitiveSequence { operations: Vec<String>, count: usize },
    RepeatedFailure { problem_signature: String, failure_count: usize },
    ContextBloat { added_tokens: usize, time_window_ms: u64 },
    AttentionDrift { topics: Vec<String>, switch_count: usize },
}

/// 轨迹点
#[derive(Debug, Clone)]
pub struct TrajectoryPoint {
    pub timestamp_ms: u64,
    pub intent: String,
    pub operation: String,
    pub success: bool,
    pub context_cids: Vec<String>,
}

/// WASM类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmType {
    I32,
    I64,
    F32,
    F64,
    String,
    Bytes,
}

/// 函数签名
#[derive(Debug, Clone)]
pub struct FunctionSignature {
    pub inputs: Vec<(String, WasmType)>,
    pub outputs: Vec<(String, WasmType)>,
}

/// 资源限制
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    pub max_memory_mb: usize,
    pub max_execution_time_ms: u64,
    pub max_stack_size: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_memory_mb: 64,
            max_execution_time_ms: 5000,
            max_stack_size: 1024 * 1024,
        }
    }
}

// ── 技能类型定义 ──

/// 技能枚举
#[derive(Debug, Clone)]
pub enum Skill {
    Knowledge(KnowledgeSkill),
    Config(ConfigSkill),
    Code(CodeSkill),
}

/// 知识型技能
#[derive(Debug, Clone)]
pub struct KnowledgeSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub trigger_conditions: Vec<TriggerCondition>,
    pub knowledge: Vec<KnowledgeItem>,
    pub sources: Vec<ExperienceSource>,
    pub validation: ValidationStatus,
    pub usage_stats: SkillUsageStats,
}

/// 配置型技能
#[derive(Debug, Clone)]
pub struct ConfigSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tool_chain: Vec<ToolCallStep>,
    pub parameter_mappings: Vec<ParameterMapping>,
    pub conditional_branches: Vec<ConditionalBranch>,
}

#[derive(Debug, Clone)]
pub struct ToolCallStep {
    pub step_id: String,
    pub tool_name: String,
    pub parameters: serde_json::Value,
    pub output_as: String,
}

#[derive(Debug, Clone)]
pub struct ParameterMapping {
    pub from: String,
    pub to: String,
    pub transform: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConditionalBranch {
    pub condition: String,
    pub true_steps: Vec<ToolCallStep>,
    pub false_steps: Vec<ToolCallStep>,
}

/// 代码型技能（WASM）
#[derive(Debug, Clone)]
pub struct CodeSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub wasm_bytes: Vec<u8>,
    pub signature: FunctionSignature,
    pub resource_limits: ResourceLimits,
}

/// 认知优化配置
#[derive(Debug, Clone)]
pub struct CognitiveConfig {
    pub context_compression_threshold: f32,
    pub proactive_prefetch_enabled: bool,
    pub failure_pattern_detection_enabled: bool,
    pub skill_extraction_enabled: bool,
    pub report_verbosity: ReportVerbosity,
}

impl Default for CognitiveConfig {
    fn default() -> Self {
        Self {
            context_compression_threshold: 0.7,
            proactive_prefetch_enabled: true,
            failure_pattern_detection_enabled: true,
            skill_extraction_enabled: true,
            report_verbosity: ReportVerbosity::Summary,
        }
    }
}

// OptimizationAction 在 cognitive_loop.rs 中定义
