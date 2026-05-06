# Soul v3.0 架构设计 — 认知共生内核

> 基于 system-v3.md 的设计原则，本文档定义核心架构模块的接口和数据流。

---

## 目录

1. [架构总览](#1-架构总览)
2. [认知循环引擎 (CognitiveLoop)](#2-认知循环引擎-cognitiveloop)
3. [上下文质量引擎 (ContextQualityEngine)](#3-上下文质量引擎-contextqualityengine)
4. [意图语义网络 (IntentSemanticNetwork)](#4-意图语义网络-intentsemanticnetwork)
5. [技能进化系统 (SkillForge)](#5-技能进化系统-skillforge)
6. [WASM 技能运行时](#6-wasm-技能运行时)
7. [DSL 技能解释器](#7-dsl-技能解释器)
8. [数据流图](#8-数据流图)
9. [重构文件变更清单](#9-重构文件变更清单)

---

## 1. 架构总览

```
┌─────────────────────────────────────────────────────────────┐
│                     Application Layer                        │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────────┐   │
│  │  aicli  │  │plico-mcp│  │plico-sse│  │  Agent SDK  │   │
│  └────┬────┘  └────┬────┘  └────┬────┘  └──────┬──────┘   │
└───────┼────────────┼────────────┼──────────────┼──────────┘
        │            │            │              │
        └────────────┴────────────┴──────────────┘
                           │
                    ┌──────┴──────┐
                    │   API Layer  │  ← 协议适配，无状态
                    │  (semantic)  │
                    └──────┬──────┘
                           │
┌──────────────────────────┼──────────────────────────────────┐
│                          │                                   │
│  ┌───────────────────────┴───────────────────────┐          │
│  │            AIKernel (Orchestrator)             │          │
│  │                                                │          │
│  │  ┌─────────────────────────────────────────┐   │          │
│  │  │       CognitiveLoop Engine              │   │          │
│  │  │  ┌─────────────┐    ┌───────────────┐  │   │          │
│  │  │  │ContextQuality│    │IntentSemantic │  │   │          │
│  │  │  │   Engine     │    │   Network     │  │   │          │
│  │  │  └─────────────┘    └───────────────┘  │   │          │
│  │  │  ┌─────────────┐    ┌───────────────┐  │   │          │
│  │  │  │ SkillForge   │    │  WASM Runtime │  │   │          │
│  │  │  │ (Evolution)  │    │   + DSL Exec  │  │   │          │
│  │  │  └─────────────┘    └───────────────┘  │   │          │
│  │  └─────────────────────────────────────────┘   │          │
│  │                                                │          │
│  │  ┌─────────┐ ┌─────────┐ ┌─────────┐        │          │
│  │  │   CAS   │ │  Memory │ │ Semantic│        │          │
│  │  │ Storage │ │  Tiers  │ │   FS    │        │          │
│  │  └─────────┘ └─────────┘ └─────────┘        │          │
│  │                                                │          │
│  │  ┌─────────┐ ┌─────────┐ ┌─────────┐        │          │
│  │  │Scheduler│ │  Graph  │ │  Event  │        │          │
│  │  │         │ │   KG    │ │   Bus   │        │          │
│  │  └─────────┘ └─────────┘ └─────────┘        │          │
│  └───────────────────────────────────────────────┘          │
└───────────────────────────────────────────────────────────────┘
```

---

## 2. 认知循环引擎 (CognitiveLoop)

### 职责

CognitiveLoop 是 Soul v3.0 的核心创新。它不是被动等待 Agent 请求，而是**持续监控 Agent 的认知状态并主动优化**。

### 设计原则

1. **可观测**：所有优化行为记录到事件日志，Agent 可以查询"为什么给我看这个？"
2. **可覆盖**：Agent 可以通过 API 禁用特定优化策略
3. **可调试**：每次优化生成 `CognitiveOptimizationReport`

### 核心接口

```rust
//! src/kernel/cognition/cognitive_loop.rs

use std::sync::Arc;
use tokio::sync::RwLock;

/// CognitiveLoop 是 Plico v3.0 的核心认知引擎。
/// 它持续监控 Agent 的认知状态，主动优化认知环境。
pub struct CognitiveLoop {
    /// 上下文质量分析器
    context_analyzer: Arc<ContextQualityEngine>,
    
    /// 意图语义网络
    intent_network: Arc<IntentSemanticNetwork>,
    
    /// 技能进化系统
    skill_forge: Arc<SkillForge>,
    
    /// Agent 认知轨迹追踪
    trajectory_tracker: Arc<TrajectoryTracker>,
    
    /// 主动优化配置
    config: CognitiveConfig,
    
    /// 运行状态
    state: RwLock<CognitiveState>,
}

/// 主动优化配置
#[derive(Debug, Clone)]
pub struct CognitiveConfig {
    /// 上下文压缩阈值（默认 70%）
    pub context_compression_threshold: f32,
    
    /// 是否启用主动预加载
    pub proactive_prefetch_enabled: bool,
    
    /// 是否启用失败模式识别
    pub failure_pattern_detection_enabled: bool,
    
    /// 是否启用技能自动提取
    pub skill_extraction_enabled: bool,
    
    /// 优化报告详细程度
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

/// 认知状态
#[derive(Debug, Default)]
pub struct CognitiveState {
    /// 当前活跃的 Agent 会话
    pub active_sessions: HashMap<String, SessionCognitiveState>,
    
    /// 最近一次优化时间戳
    pub last_optimization_ms: u64,
    
    /// 累计优化统计
    pub stats: CognitiveStats,
}

/// 单个会话的认知状态
#[derive(Debug, Clone)]
pub struct SessionCognitiveState {
    pub agent_id: String,
    pub session_id: String,
    
    /// 当前上下文质量评分 (0.0 - 1.0)
    pub context_quality_score: f32,
    
    /// 当前上下文已用 token 比例
    pub context_utilization: f32,
    
    /// Agent 当前的注意力焦点（最近高频访问的CID/标签）
    pub attention_focus: Vec<String>,
    
    /// 已检测到的认知模式
    pub detected_patterns: Vec<CognitivePattern>,
    
    /// 最近一次优化的报告
    pub last_optimization: Option<CognitiveOptimizationReport>,
}

/// 认知模式
#[derive(Debug, Clone)]
pub enum CognitivePattern {
    /// 重复执行相同操作序列
    RepetitiveSequence { operations: Vec<String>, count: usize },
    
    /// 在相同问题上反复失败
    RepeatedFailure { problem_signature: String, failure_count: usize },
    
    /// 上下文快速膨胀（大量临时信息）
    ContextBloat { added_tokens: usize, time_window_ms: u64 },
    
    /// 注意力漂移（频繁切换主题）
    AttentionDrift { topics: Vec<String>, switch_count: usize },
}

/// 认知优化报告
#[derive(Debug, Clone)]
pub struct CognitiveOptimizationReport {
    pub timestamp_ms: u64,
    pub optimizations: Vec<OptimizationAction>,
    pub token_savings: usize,
    pub quality_delta: f32,  // 优化前后的质量评分变化
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

    /// Agent 声明意图时触发认知分析
    pub async fn on_intent_declared(
        &self,
        agent_id: &str,
        intent: &str,
        current_context: &[String],  // 当前上下文中的 CID 列表
    ) -> Result<CognitiveOptimizationReport, CognitiveError> {
        let mut report = CognitiveOptimizationReport {
            timestamp_ms: now_ms(),
            optimizations: Vec::new(),
            token_savings: 0,
            quality_delta: 0.0,
        };

        // 1. 分析当前上下文质量
        let quality = self.context_analyzer.analyze(agent_id, current_context).await?;
        
        // 2. 如果质量低于阈值，执行压缩
        if quality.score < 0.6 {
            let compressed = self.context_analyzer.compress(agent_id, current_context).await?;
            report.optimizations.push(OptimizationAction::ContextCompressed {
                original_tokens: quality.token_count,
                compressed_tokens: compressed.token_count,
                reason: compressed.reason,
            });
            report.token_savings += quality.token_count - compressed.token_count;
        }

        // 3. 基于意图语义网络预加载相关上下文
        if self.config.proactive_prefetch_enabled {
            let related = self.intent_network.find_related(agent_id, intent).await?;
            if !related.is_empty() {
                report.optimizations.push(OptimizationAction::ContextPrefetched {
                    cids: related.iter().map(|r| r.cid.clone()).collect(),
                    relevance_scores: related.iter().map(|r| r.score).collect(),
                });
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

        Ok(report)
    }

    /// Agent 完成操作后触发经验提取
    pub async fn on_operation_completed(
        &self,
        agent_id: &str,
        operation: &str,
        success: bool,
        context_before: &[String],
        context_after: &[String],
    ) -> Result<(), CognitiveError> {
        // 1. 追踪认知轨迹
        self.trajectory_tracker.record_operation(agent_id, operation, success).await;

        // 2. 如果成功，提取技能候选
        if success && self.config.skill_extraction_enabled {
            self.skill_forge.extract_candidate(agent_id, operation, context_before, context_after).await?;
        }

        // 3. 如果失败，记录失败模式
        if !success && self.config.failure_pattern_detection_enabled {
            self.record_failure_pattern(agent_id, operation, context_before).await?;
        }

        Ok(())
    }

    /// 定时任务：检查所有活跃会话的上下文质量
    pub async fn run_periodic_check(&self) -> Result<Vec<CognitiveOptimizationReport>, CognitiveError> {
        let mut reports = Vec::new();
        let state = self.state.read().await;
        
        for (agent_id, session_state) in &state.active_sessions {
            // 如果上下文利用率超过阈值，触发压缩
            if session_state.context_utilization > self.config.context_compression_threshold {
                // ... 触发压缩逻辑
            }
        }
        
        Ok(reports)
    }

    // ... 辅助方法
}
```

---

## 3. 上下文质量引擎 (ContextQualityEngine)

### 职责

解决"上下文腐败"问题。持续分析上下文的 token 构成，识别并处理：
- 重复信息
- 过时信息
- 低相关性信息
- 临时/噪声信息

### 核心接口

```rust
//! src/kernel/cognition/context_quality.rs

/// 上下文质量分析结果
#[derive(Debug, Clone)]
pub struct ContextQuality {
    /// 质量评分 (0.0 - 1.0)
    pub score: f32,
    
    /// 总 token 数
    pub token_count: usize,
    
    /// 各类信息的 token 分布
    pub token_breakdown: TokenBreakdown,
    
    /// 检测到的质量问题
    pub issues: Vec<ContextIssue>,
}

#[derive(Debug, Clone, Default)]
pub struct TokenBreakdown {
    pub core_knowledge: usize,      // 核心知识（高价值、持久）
    pub procedural_info: usize,     // 过程信息（操作步骤、中间结果）
    pub temporary_data: usize,      // 临时数据（调试输出、临时变量）
    pub redundant_info: usize,      // 冗余信息（重复、已确认的内容）
    pub stale_info: usize,          // 过时信息（已被后续操作覆盖）
}

#[derive(Debug, Clone)]
pub enum ContextIssue {
    /// 高冗余度
    HighRedundancy { redundant_ratio: f32 },
    
    /// 高临时数据比例
    HighTemporaryRatio { temp_ratio: f32 },
    
    /// 包含过时信息
    ContainsStaleInfo { stale_cids: Vec<String> },
    
    /// 注意力分散（上下文包含多个不相关主题）
    AttentionScattered { topics: Vec<String> },
    
    /// 失败日志占比过高
    FailureLogHeavy { failure_ratio: f32 },
}

/// 压缩后的上下文
#[derive(Debug, Clone)]
pub struct CompressedContext {
    /// 保留的 CID 列表（按相关性排序）
    pub retained_cids: Vec<String>,
    
    /// 新增的摘要 CID（压缩后的精华）
    pub summary_cids: Vec<String>,
    
    /// 压缩后的 token 数
    pub token_count: usize,
    
    /// 压缩原因说明
    pub reason: String,
    
    /// 被移除的信息说明
    pub removed: Vec<RemovalRecord>,
}

#[derive(Debug, Clone)]
pub struct RemovalRecord {
    pub cid: String,
    pub reason: RemovalReason,
    pub token_savings: usize,
}

#[derive(Debug, Clone)]
pub enum RemovalReason {
    DuplicateOf(String),           // 与某CID重复
    SupersededBy(String),          // 被某CID覆盖
    TemporaryExpired,              // 临时信息已过期
    LowRelevance { score: f32 },   // 相关性过低
    ConsolidatedInto(String),      // 已合并入某摘要
}

pub struct ContextQualityEngine {
    /// Embedding provider，用于计算语义相关性
    embedding: Arc<dyn EmbeddingProvider>,
    
    /// 语义搜索后端
    search: Arc<dyn SemanticSearch>,
    
    /// 知识图谱
    kg: Option<Arc<dyn KnowledgeGraph>>,
    
    /// 记忆系统
    memory: Arc<LayeredMemory>,
}

impl ContextQualityEngine {
    /// 分析给定上下文的 token 构成和质量
    pub async fn analyze(&self, agent_id: &str, context_cids: &[String]) -> Result<ContextQuality, CognitiveError> {
        // 1. 获取每个 CID 的 token 数和元数据
        // 2. 计算语义相关性矩阵
        // 3. 识别冗余、过时、临时信息
        // 4. 生成质量评分
        todo!("实现分析逻辑")
    }

    /// 压缩上下文：去重、去噪、提取精华
    pub async fn compress(&self, agent_id: &str, context_cids: &[String]) -> Result<CompressedContext, CognitiveError> {
        // 1. 分析上下文质量
        let quality = self.analyze(agent_id, context_cids).await?;
        
        // 2. 识别可移除的信息
        let to_remove = self.identify_removable(agent_id, context_cids, &quality).await?;
        
        // 3. 对同类信息生成摘要
        let summaries = self.generate_summaries(agent_id, context_cids, &to_remove).await?;
        
        // 4. 组装压缩后的上下文
        let compressed = self.assemble_compressed(context_cids, &to_remove, &summaries).await?;
        
        Ok(compressed)
    }

    /// 识别冗余信息
    async fn identify_removable(
        &self,
        agent_id: &str,
        context_cids: &[String],
        quality: &ContextQuality,
    ) -> Result<Vec<RemovalRecord>, CognitiveError> {
        let mut removable = Vec::new();
        
        // 策略1：识别重复信息（基于embedding相似度）
        let embeddings = self.get_embeddings(context_cids).await?;
        for (i, emb_i) in embeddings.iter().enumerate() {
            for (j, emb_j) in embeddings.iter().enumerate().skip(i + 1) {
                let similarity = cosine_similarity(emb_i, emb_j);
                if similarity > 0.95 {
                    // 高度相似，标记其中一个为冗余
                    removable.push(RemovalRecord {
                        cid: context_cids[j].clone(),
                        reason: RemovalReason::DuplicateOf(context_cids[i].clone()),
                        token_savings: self.get_token_count(&context_cids[j]).await?,
                    });
                }
            }
        }
        
        // 策略2：识别过时信息（基于因果图谱）
        if let Some(ref kg) = self.kg {
            for cid in context_cids {
                if let Some(superseder) = self.find_superseder(kg, cid).await? {
                    removable.push(RemovalRecord {
                        cid: cid.clone(),
                        reason: RemovalReason::SupersededBy(superseder),
                        token_savings: self.get_token_count(cid).await?,
                    });
                }
            }
        }
        
        // 策略3：识别临时信息（基于标签/类型）
        for cid in context_cids {
            if self.is_temporary(cid).await? {
                removable.push(RemovalRecord {
                    cid: cid.clone(),
                    reason: RemovalReason::TemporaryExpired,
                    token_savings: self.get_token_count(cid).await?,
                });
            }
        }
        
        Ok(removable)
    }

    /// 生成摘要（将同类信息压缩为精华）
    async fn generate_summaries(
        &self,
        agent_id: &str,
        context_cids: &[String],
        removed: &[RemovalRecord],
    ) -> Result<Vec<String>, CognitiveError> {
        // 1. 按主题聚类
        // 2. 对每个聚类生成L0摘要
        // 3. 如果聚类包含失败信息，额外生成"经验教训"摘要
        todo!("实现摘要生成逻辑")
    }
}
```

---

## 4. 意图语义网络 (IntentSemanticNetwork)

### 职责

维护意图之间的语义关系网络（因果、时序、层次、关联），用于：
- 预测 Agent 接下来可能需要的上下文
- 关联历史经验与当前任务
- 理解 Agent 的认知轨迹

### 核心接口

```rust
//! src/kernel/cognition/intent_network.rs

/// 语义关系类型
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SemanticRelation {
    /// 因果关系：A 导致 B
    Causes,
    
    /// 时序关系：A 在 B 之前发生
    Precedes,
    
    /// 层次关系：A 是 B 的组成部分
    PartOf,
    
    /// 关联关系：A 和 B 经常一起出现
    CoOccurs,
    
    /// 替代关系：A 和 B 是不同解决方案
    Alternative,
}

/// 语义节点
#[derive(Debug, Clone)]
pub struct SemanticNode {
    pub id: String,
    pub embedding: Vec<f32>,
    pub node_type: NodeType,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone)]
pub enum NodeType {
    Intent,      // 意图
    Task,        // 任务
    Skill,       // 技能
    Knowledge,   // 知识
    Experience,  // 经验
}

/// 语义边
#[derive(Debug, Clone)]
pub struct SemanticEdge {
    pub from: String,
    pub to: String,
    pub relation: SemanticRelation,
    pub strength: f32,  // 关系强度 (0.0 - 1.0)
    pub evidence_count: usize,  // 支持该关系的证据数量
}

/// 相关上下文推荐
#[derive(Debug, Clone)]
pub struct RelatedContext {
    pub cid: String,
    pub score: f32,
    pub relation_path: Vec<SemanticRelation>,  // 从当前意图到该CID的语义路径
    pub reason: String,
}

pub struct IntentSemanticNetwork {
    /// 节点存储
    nodes: RwLock<HashMap<String, SemanticNode>>,
    
    /// 边存储（按关系类型分索引）
    edges: RwLock<HashMap<SemanticRelation, Vec<SemanticEdge>>>,
    
    /// Embedding provider，用于语义匹配
    embedding: Arc<dyn EmbeddingProvider>,
    
    /// Agent 认知轨迹
    trajectories: RwLock<HashMap<String, Vec<TrajectoryPoint>>>,
}

#[derive(Debug, Clone)]
pub struct TrajectoryPoint {
    pub timestamp_ms: u64,
    pub intent: String,
    pub operation: String,
    pub success: bool,
    pub context_cids: Vec<String>,
}

impl IntentSemanticNetwork {
    /// 从 Agent 的操作历史中学习语义关系
    pub async fn learn_from_history(
        &self,
        agent_id: &str,
        trajectory: &[TrajectoryPoint],
    ) -> Result<LearningReport, CognitiveError> {
        // 1. 提取意图序列
        // 2. 基于时序建立 Precedes 关系
        // 3. 基于因果建立 Causes 关系（成功→成功的操作链）
        // 4. 基于共现建立 CoOccurs 关系
        // 5. 使用 embedding 相似度聚类相似意图
        todo!("实现学习逻辑")
    }

    /// 为给定意图查找相关上下文
    pub async fn find_related(
        &self,
        agent_id: &str,
        intent: &str,
    ) -> Result<Vec<RelatedContext>, CognitiveError> {
        // 1. 计算意图的 embedding
        let intent_embedding = self.embedding.embed(intent).await?;
        
        // 2. 在语义网络中查找语义相近的节点
        let similar_nodes = self.find_semantically_similar(&intent_embedding).await?;
        
        // 3. 基于关系网络扩散查找
        let related_by_relation = self.expand_by_relation(&similar_nodes).await?;
        
        // 4. 基于 Agent 的个人历史轨迹查找
        let related_by_history = self.find_from_trajectory(agent_id, intent).await?;
        
        // 5. 合并、去重、排序
        let merged = self.merge_results(similar_nodes, related_by_relation, related_by_history);
        
        Ok(merged)
    }

    /// 预测 Agent 接下来可能需要的上下文
    pub async fn predict_next_context(
        &self,
        agent_id: &str,
        current_intent: &str,
    ) -> Result<Vec<RelatedContext>, CognitiveError> {
        // 1. 查找 current_intent 的 Precedes 关系
        // 2. 基于历史轨迹统计转移概率
        // 3. 返回高概率的后续意图相关的上下文
        todo!("实现预测逻辑")
    }

    /// 关联历史经验与当前任务
    pub async fn associate_experience(
        &self,
        agent_id: &str,
        current_intent: &str,
    ) -> Result<Vec<ExperienceAssociation>, CognitiveError> {
        // 1. 查找语义相似的过往意图
        // 2. 对比当前上下文与历史上下文的差异
        // 3. 提取可复用的经验和需规避的失败
        todo!("实现关联逻辑")
    }
}

/// 学习报告
#[derive(Debug, Clone)]
pub struct LearningReport {
    pub new_nodes: usize,
    pub new_edges: usize,
    pub strengthened_edges: usize,
    pub discovered_patterns: Vec<String>,
}

/// 经验关联
#[derive(Debug, Clone)]
pub struct ExperienceAssociation {
    pub experience_intent: String,
    pub similarity_score: f32,
    pub reusable_knowledge: Vec<String>,
    pub failure_lessons: Vec<String>,
    pub suggested_skills: Vec<String>,
}
```

---

## 5. 技能进化系统 (SkillForge)

### 职责

从 Agent 的操作历史中提取成功模式，转化为可复用、可验证、可进化的技能。

### 技能类型

```rust
//! src/kernel/cognition/skill_forge.rs

/// 技能类型
#[derive(Debug, Clone)]
pub enum Skill {
    /// 知识型技能：因果规则、检查清单、经验模板
    Knowledge(KnowledgeSkill),
    
    /// 配置型技能：工具调用链、参数映射、条件分支
    Config(ConfigSkill),
    
    /// 代码型技能：WASM 模块
    Code(CodeSkill),
}

/// 知识型技能
#[derive(Debug, Clone)]
pub struct KnowledgeSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    
    /// 触发条件：什么情况下激活这个技能
    pub trigger_conditions: Vec<TriggerCondition>,
    
    /// 知识内容：规则、检查清单、经验教训
    pub knowledge: Vec<KnowledgeItem>,
    
    /// 来源：从哪些经验中提取的
    pub sources: Vec<ExperienceSource>,
    
    /// 验证状态
    pub validation: ValidationStatus,
    
    /// 使用统计
    pub usage_stats: SkillUsageStats,
}

#[derive(Debug, Clone)]
pub enum KnowledgeItem {
    Rule { condition: String, action: String },
    Checklist { items: Vec<String> },
    Lesson { situation: String, insight: String },
    Warning { pattern: String, consequence: String },
}

/// 配置型技能
#[derive(Debug, Clone)]
pub struct ConfigSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    
    /// 工具调用链
    pub tool_chain: Vec<ToolCallStep>,
    
    /// 参数映射规则
    pub parameter_mappings: Vec<ParameterMapping>,
    
    /// 条件分支
    pub conditional_branches: Vec<ConditionalBranch>,
}

#[derive(Debug, Clone)]
pub struct ToolCallStep {
    pub step_id: String,
    pub tool_name: String,
    pub parameters: serde_json::Value,
    pub output_as: String,  // 输出变量名
}

/// 代码型技能（WASM）
#[derive(Debug, Clone)]
pub struct CodeSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    
    /// WASM 模块字节码
    pub wasm_bytes: Vec<u8>,
    
    /// 输入/输出类型签名
    pub signature: FunctionSignature,
    
    /// 资源限制
    pub resource_limits: ResourceLimits,
}

#[derive(Debug, Clone)]
pub struct FunctionSignature {
    pub inputs: Vec<(String, WasmType)>,
    pub outputs: Vec<(String, WasmType)>,
}

#[derive(Debug, Clone)]
pub struct ResourceLimits {
    pub max_memory_mb: usize,
    pub max_execution_time_ms: u64,
    pub max_stack_size: usize,
}
```

### 核心接口

```rust
pub struct SkillForge {
    /// 经验挖掘器：从操作历史中提取模式
    experience_miner: Arc<ExperienceMiner>,
    
    /// 技能验证器：验证技能的有效性
    skill_validator: Arc<SkillValidator>,
    
    /// 技能组合器：组合多个技能
    skill_composer: Arc<SkillComposer>,
    
    /// 技能注册表：版本管理和检索
    skill_registry: Arc<SkillRegistry>,
    
    /// WASM 运行时
    wasm_runtime: Option<Arc<WasmRuntime>>,
}

impl SkillForge {
    /// 从单次任务执行中提取技能候选
    pub async fn extract_candidate(
        &self,
        agent_id: &str,
        task_description: &str,
        operations: &[OperationRecord],
        context_before: &[String],
        context_after: &[String],
    ) -> Result<Vec<SkillCandidate>, CognitiveError> {
        // 1. 识别操作序列中的模式
        // 2. 判断是否是可复用的模式（重复出现、成功率高）
        // 3. 生成技能候选（知识型/配置型/代码型）
        todo!("实现提取逻辑")
    }

    /// 验证技能候选的有效性
    pub async fn validate_skill(
        &self,
        agent_id: &str,
        candidate: &SkillCandidate,
    ) -> Result<ValidationResult, CognitiveError> {
        // 1. 在相似任务上回测
        // 2. 检查与现有技能的冲突/重复
        // 3. 评估 token 节省效果
        todo!("实现验证逻辑")
    }

    /// 注册通过验证的技能
    pub async fn register_skill(
        &self,
        agent_id: &str,
        skill: Skill,
    ) -> Result<String, CognitiveError> {
        // 1. 存入技能注册表
        // 2. 更新意图语义网络（技能作为节点）
        // 3. 生成技能元数据
        todo!("实现注册逻辑")
    }

    /// 为给定意图推荐相关技能
    pub async fn recommend(
        &self,
        agent_id: &str,
        intent: &str,
    ) -> Result<Vec<SkillRecommendation>, CognitiveError> {
        // 1. 基于意图语义匹配查找相关技能
        // 2. 基于 Agent 历史使用模式排序
        // 3. 返回推荐列表（含置信度）
        todo!("实现推荐逻辑")
    }

    /// 执行技能
    pub async fn execute_skill(
        &self,
        agent_id: &str,
        skill_id: &str,
        inputs: serde_json::Value,
    ) -> Result<SkillExecutionResult, CognitiveError> {
        let skill = self.skill_registry.get(skill_id).await?;
        match skill {
            Skill::Knowledge(k) => self.execute_knowledge_skill(k, inputs).await,
            Skill::Config(c) => self.execute_config_skill(c, inputs).await,
            Skill::Code(code) => self.execute_code_skill(code, inputs).await,
        }
    }

    async fn execute_knowledge_skill(
        &self,
        skill: KnowledgeSkill,
        inputs: serde_json::Value,
    ) -> Result<SkillExecutionResult, CognitiveError> {
        // 知识型技能：返回结构化的知识内容
        // 不执行外部操作，只提供信息
        Ok(SkillExecutionResult::Knowledge {
            items: skill.knowledge,
        })
    }

    async fn execute_config_skill(
        &self,
        skill: ConfigSkill,
        inputs: serde_json::Value,
    ) -> Result<SkillExecutionResult, CognitiveError> {
        // 配置型技能：按DSL解释执行工具调用链
        // 需要调用外部工具
        todo!("实现DSL执行逻辑")
    }

    async fn execute_code_skill(
        &self,
        skill: CodeSkill,
        inputs: serde_json::Value,
    ) -> Result<SkillExecutionResult, CognitiveError> {
        // 代码型技能：在WASM沙箱中执行
        let runtime = self.wasm_runtime.as_ref()
            .ok_or(CognitiveError::WasmRuntimeNotAvailable)?;
        runtime.execute(&skill.wasm_bytes, inputs, &skill.resource_limits).await
    }
}
```

---

## 6. WASM 技能运行时

```rust
//! src/kernel/cognition/wasm_runtime.rs

pub struct WasmRuntime {
    engine: wasmtime::Engine,
    module_cache: RwLock<HashMap<String, wasmtime::Module>>,
}

impl WasmRuntime {
    pub fn new() -> Result<Self, CognitiveError> {
        let mut config = wasmtime::Config::new();
        config.wasm_multi_memory(true);
        config.consume_fuel(true);  // 限制执行时间
        
        let engine = wasmtime::Engine::new(&config)
            .map_err(|e| CognitiveError::WasmInitFailed(e.to_string()))?;
        
        Ok(Self {
            engine,
            module_cache: RwLock::new(HashMap::new()),
        })
    }

    pub async fn execute(
        &self,
        wasm_bytes: &[u8],
        inputs: serde_json::Value,
        limits: &ResourceLimits,
    ) -> Result<SkillExecutionResult, CognitiveError> {
        // 1. 编译 WASM 模块（或从缓存获取）
        // 2. 创建受限的 Store（内存限制、燃料限制）
        // 3. 注入 host 函数（允许WASM调用Plico API）
        // 4. 执行，捕获输出
        // 5. 返回结构化结果
        todo!("实现WASM执行逻辑")
    }
}
```

---

## 7. DSL 技能解释器

```rust
//! src/kernel/cognition/dsl_interpreter.rs

/// DSL 技能定义（声明式）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DslSkill {
    pub version: String,
    pub name: String,
    pub description: String,
    
    /// 输入参数定义
    pub inputs: Vec<DslInput>,
    
    /// 步骤链
    pub steps: Vec<DslStep>,
    
    /// 输出定义
    pub outputs: Vec<DslOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DslStep {
    /// 调用工具
    ToolCall {
        tool: String,
        params: serde_json::Value,
        output_as: Option<String>,
    },
    
    /// 条件分支
    If {
        condition: DslCondition,
        then_steps: Vec<DslStep>,
        else_steps: Vec<DslStep>,
    },
    
    /// 循环
    ForEach {
        over: String,  // 变量名
        steps: Vec<DslStep>,
    },
    
    /// 并行执行
    Parallel {
        branches: Vec<Vec<DslStep>>,
    },
    
    /// 调用记忆
    Recall {
        query: String,
        filter: Option<serde_json::Value>,
        output_as: String,
    },
    
    /// 存储结果
    Store {
        key: String,
        value: serde_json::Value,
        tags: Vec<String>,
    },
}

pub struct DslInterpreter {
    tool_registry: Arc<ToolRegistry>,
    memory: Arc<LayeredMemory>,
}

impl DslInterpreter {
    pub async fn execute(&self, dsl: &DslSkill, inputs: serde_json::Value) -> Result<serde_json::Value, CognitiveError> {
        let mut context = ExecutionContext::new(inputs);
        
        for step in &dsl.steps {
            self.execute_step(step, &mut context).await?;
        }
        
        Ok(context.get_outputs(&dsl.outputs))
    }

    async fn execute_step(&self, step: &DslStep, context: &mut ExecutionContext) -> Result<(), CognitiveError> {
        match step {
            DslStep::ToolCall { tool, params, output_as } => {
                let resolved_params = context.resolve_params(params);
                let result = self.tool_registry.call(tool, resolved_params).await?;
                if let Some(name) = output_as {
                    context.set_variable(name, result);
                }
            }
            DslStep::If { condition, then_steps, else_steps } => {
                let cond_result = self.evaluate_condition(condition, context).await?;
                let steps = if cond_result { then_steps } else { else_steps };
                for step in steps {
                    self.execute_step(step, context).await?;
                }
            }
            // ... 其他步骤类型
        }
        Ok(())
    }
}
```

---

## 8. 数据流图

### 8.1 意图声明时的数据流

```
Agent ──declare_intent("修复auth模块bug")──→ API Layer
                                                    │
                                                    ↓
┌─────────────────────────────────────────────────────────────────┐
│                         AIKernel                                 │
│                                                                  │
│  ┌─────────────────┐    ┌─────────────────┐    ┌─────────────┐  │
│  │ CognitiveLoop   │───→│ ContextQuality  │───→│ 压缩上下文   │  │
│  │                 │    │   Engine        │    │ (去重/去噪) │  │
│  │                 │───→│ IntentSemantic  │───→│ 预加载相关   │  │
│  │                 │    │   Network       │    │ 上下文      │  │
│  │                 │───→│   SkillForge    │───→│ 推荐技能    │  │
│  └─────────────────┘    └─────────────────┘    └─────────────┘  │
│           │                                               │      │
│           ↓                                               ↓      │
│  ┌─────────────────────────────────────────────────────────────┐│
│  │              CognitiveOptimizationReport                     ││
│  │  - 压缩了多少token                                           ││
│  │  - 预加载了哪些CID                                           ││
│  │  - 推荐了哪些技能                                            ││
│  │  - 质量评分变化                                              ││
│  └─────────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────────┘
                                                    │
                                                    ↓
Agent ←──优化后的上下文 + 优化报告─── API Layer
```

### 8.2 操作完成后的数据流

```
Agent ──完成操作（成功/失败）──→ API Layer
                                        │
                                        ↓
┌─────────────────────────────────────────────────────────────┐
│                      AIKernel                                │
│                                                              │
│  ┌─────────────────┐    ┌─────────────────┐                │
│  │ CognitiveLoop   │───→│ TrajectoryTracker│               │
│  │                 │    │ （记录认知轨迹）  │               │
│  │                 │───→│   SkillForge     │               │
│  │                 │    │ （提取技能候选）  │               │
│  │                 │───→│ IntentSemantic   │               │
│  │                 │    │   Network         │               │
│  │                 │    │ （学习语义关系）  │               │
│  └─────────────────┘    └─────────────────┘                │
└─────────────────────────────────────────────────────────────┘
```

---

## 9. 重构文件变更清单

### 9.1 删除的文件（旧大脑模块）

```
src/kernel/ops/intent_decomposer.rs      → 删除（功能被 IntentSemanticNetwork 取代）
src/kernel/ops/temporal_projection.rs    → 删除（功能被 IntentSemanticNetwork + TrajectoryTracker 取代）
src/kernel/ops/goal_generator.rs         → 删除（功能被 SkillForge 取代）
src/kernel/ops/skill_discovery.rs        → 删除（功能被 SkillForge 取代）
src/kernel/ops/cross_domain_skill.rs     → 删除（功能被 SkillForge 的 SkillComposer 取代）
src/kernel/ops/self_healing.rs           → 删除（功能被 ContextQualityEngine + SkillForge 取代）
```

### 9.2 新增的文件（认知引擎）

```
src/kernel/cognition/mod.rs              → 认知模块入口
src/kernel/cognition/cognitive_loop.rs   → 认知循环引擎
src/kernel/cognition/context_quality.rs  → 上下文质量引擎
src/kernel/cognition/intent_network.rs   → 意图语义网络
src/kernel/cognition/skill_forge.rs      → 技能进化系统
src/kernel/cognition/experience_miner.rs → 经验挖掘器
src/kernel/cognition/skill_validator.rs  → 技能验证器
src/kernel/cognition/skill_composer.rs   → 技能组合器
src/kernel/cognition/skill_registry.rs   → 技能注册表
src/kernel/cognition/wasm_runtime.rs     → WASM技能运行时
src/kernel/cognition/dsl_interpreter.rs  → DSL技能解释器
src/kernel/cognition/trajectory_tracker.rs → 认知轨迹追踪
```

### 9.3 修改的文件

```
src/kernel/mod.rs                        → 集成 CognitiveLoop
src/kernel/ops/prefetch.rs               → 移除 Brain modules，接入 CognitiveLoop
src/kernel/ops/session.rs                → 移除 goal_generator 调用，接入 CognitiveLoop
src/kernel/ops/intent_executor.rs        → 移除 SkillDiscriminator/PlanAdaptor，接入 SkillForge
src/kernel/ops/dashboard.rs              → 移除 llm_available() 的 LLM 调用
src/api/semantic.rs                      → 新增 CognitiveOptimizationReport 相关类型
src/lib.rs                               → 新增 pub mod cognition
```

### 9.4 新增的灵魂文档

```
system-v3.md                             → 灵魂 v3.0（已创建）
docs/design/soul-v3-architecture.md      → 架构设计文档（本文档）
```

---

## 10. 关键设计决策记录

### ADR-1：为什么保留 Agent 的决策权？

即使 Plico 拥有认知增强能力，Agent 仍然保留最终决策权。这是因为：
1. **可解释性**：Agent 需要知道"为什么给我看这个"
2. **可覆盖**：Agent 可以禁用或覆盖 Plico 的优化建议
3. **安全性**：防止 Plico 的偏见或错误影响 Agent 的关键决策

### ADR-2：为什么使用 WASM 而不是直接执行代码？

WASM 提供：
1. **沙箱安全**：代码在受限环境中运行，无法直接访问系统资源
2. **可移植性**：任何语言编译为 WASM 后都可运行
3. **资源控制**：精确的内存和执行时间限制
4. **确定性**：相同的输入总是产生相同的输出

### ADR-3：为什么使用平衡策略（70%阈值）？

平衡策略的权衡：
- **保守策略**：Agent 请求时才优化 → 浪费 token，上下文可能已经腐败
- **积极策略**：持续优化 → 可能干扰 Agent 的工作流，引入意外变化
- **平衡策略**：在上下文达到70%时自动优化 → 既防止腐败，又保持 Agent 的控制感

---

*本文档与 system-v3.md 配套使用。实现细节在具体模块的 Rust 代码中定义。*
