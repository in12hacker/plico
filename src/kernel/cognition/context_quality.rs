//! 上下文质量引擎 —— 解决上下文腐败问题
//!
//! 持续分析上下文的 token 构成，识别并处理：
//! - 重复信息
//! - 过时信息
//! - 低相关性信息
//! - 临时/噪声信息

use std::sync::Arc;

use crate::fs::embedding::EmbeddingProvider;
use crate::fs::graph::KnowledgeGraph;
use crate::fs::search::SemanticSearch;
use crate::memory::LayeredMemory;

use super::{CognitiveResult, TokenBreakdown};

/// 上下文质量分析结果
#[derive(Debug, Clone)]
pub struct ContextQuality {
    pub score: f32,
    pub token_count: usize,
    pub breakdown: TokenBreakdown,
    pub issues: Vec<ContextIssue>,
}

/// 上下文质量问题
#[derive(Debug, Clone)]
pub enum ContextIssue {
    HighRedundancy { redundant_ratio: f32 },
    HighTemporaryRatio { temp_ratio: f32 },
    ContainsStaleInfo { stale_cids: Vec<String> },
    AttentionScattered { topics: Vec<String> },
    FailureLogHeavy { failure_ratio: f32 },
}

/// 压缩后的上下文
#[derive(Debug, Clone)]
pub struct CompressedContext {
    pub retained_cids: Vec<String>,
    pub summary_cids: Vec<String>,
    pub token_count: usize,
    pub reason: String,
    pub removed: Vec<RemovalRecord>,
}

/// 移除记录
#[derive(Debug, Clone)]
pub struct RemovalRecord {
    pub cid: String,
    pub reason: RemovalReason,
    pub token_savings: usize,
}

/// 移除原因
#[derive(Debug, Clone)]
pub enum RemovalReason {
    DuplicateOf(String),
    SupersededBy(String),
    TemporaryExpired,
    LowRelevance { score: f32 },
    ConsolidatedInto(String),
}

/// 上下文质量引擎
#[allow(dead_code)]
pub struct ContextQualityEngine {
    embedding: Arc<dyn EmbeddingProvider>,
    search: Arc<dyn SemanticSearch>,
    kg: Option<Arc<dyn KnowledgeGraph>>,
    memory: Arc<LayeredMemory>,
}

impl ContextQualityEngine {
    pub fn new(
        embedding: Arc<dyn EmbeddingProvider>,
        search: Arc<dyn SemanticSearch>,
        memory: Arc<LayeredMemory>,
    ) -> Self {
        Self {
            embedding,
            search,
            kg: None,
            memory,
        }
    }

    pub fn with_kg(mut self, kg: Arc<dyn KnowledgeGraph>) -> Self {
        self.kg = Some(kg);
        self
    }

    /// 分析给定上下文的 token 构成和质量
    pub async fn analyze(&self, _agent_id: &str, context_cids: &[String]) -> CognitiveResult<ContextQuality> {
        if context_cids.is_empty() {
            return Ok(ContextQuality {
                score: 1.0,
                token_count: 0,
                breakdown: TokenBreakdown::default(),
                issues: Vec::new(),
            });
        }

        let mut breakdown = TokenBreakdown::default();
        let mut issues = Vec::new();

        // TODO: 实际分析每个CID的元数据来计算breakdown
        // 这里先用启发式方法估算
        let token_count = context_cids.len() * 500; // 粗略估算
        breakdown.core_knowledge = token_count / 3;
        breakdown.procedural_info = token_count / 4;
        breakdown.temporary_data = token_count / 6;
        breakdown.redundant_info = token_count / 8;
        breakdown.stale_info = token_count / 10;

        // 计算质量评分
        let total = token_count as f32;
        let noise_ratio = (breakdown.temporary_data + breakdown.redundant_info + breakdown.stale_info) as f32 / total;
        let score = (1.0 - noise_ratio).clamp(0.0, 1.0);

        if breakdown.redundant_info as f32 / total > 0.15 {
            issues.push(ContextIssue::HighRedundancy {
                redundant_ratio: breakdown.redundant_info as f32 / total,
            });
        }

        if breakdown.temporary_data as f32 / total > 0.2 {
            issues.push(ContextIssue::HighTemporaryRatio {
                temp_ratio: breakdown.temporary_data as f32 / total,
            });
        }

        Ok(ContextQuality {
            score,
            token_count,
            breakdown,
            issues,
        })
    }

    /// 压缩上下文：去重、去噪、提取精华
    pub async fn compress(&self, agent_id: &str, context_cids: &[String]) -> CognitiveResult<CompressedContext> {
        if context_cids.len() <= 3 {
            return Ok(CompressedContext {
                retained_cids: context_cids.to_vec(),
                summary_cids: Vec::new(),
                token_count: context_cids.len() * 500,
                reason: "Context too small to compress".to_string(),
                removed: Vec::new(),
            });
        }

        // 1. 分析质量
        let quality = self.analyze(agent_id, context_cids).await?;

        // 2. 识别可移除的信息
        let removable = self.identify_removable(agent_id, context_cids).await?;

        // 3. 确定保留的CID
        let removed_cids: Vec<String> = removable.iter().map(|r| r.cid.clone()).collect();
        let retained: Vec<String> = context_cids
            .iter()
            .filter(|cid| !removed_cids.contains(cid))
            .cloned()
            .collect();

        // 4. 生成摘要（如果移除过多）
        let summaries = if retained.len() < context_cids.len() / 2 {
            self.generate_summaries(agent_id, context_cids, &removed_cids).await?
        } else {
            Vec::new()
        };

        let token_savings: usize = removable.iter().map(|r| r.token_savings).sum();
        let new_token_count = quality.token_count.saturating_sub(token_savings);

        let reason = if !removable.is_empty() {
            format!(
                "Removed {} redundant/stale/temporary items, saved {} tokens",
                removable.len(),
                token_savings
            )
        } else {
            "No significant compression possible".to_string()
        };

        Ok(CompressedContext {
            retained_cids: retained,
            summary_cids: summaries,
            token_count: new_token_count,
            reason,
            removed: removable,
        })
    }

    /// 识别冗余信息（基于embedding相似度）
    async fn identify_removable(
        &self,
        _agent_id: &str,
        context_cids: &[String],
    ) -> CognitiveResult<Vec<RemovalRecord>> {
        let mut removable = Vec::new();

        // 策略1：基于embedding相似度识别重复
        let embeddings = self.get_embeddings(context_cids).await?;
        for (i, emb_i) in embeddings.iter().enumerate() {
            for (j, emb_j) in embeddings.iter().enumerate().skip(i + 1) {
                if emb_i.is_empty() || emb_j.is_empty() {
                    continue;
                }
                let similarity = cosine_similarity(emb_i, emb_j);
                if similarity > 0.95 {
                    removable.push(RemovalRecord {
                        cid: context_cids[j].clone(),
                        reason: RemovalReason::DuplicateOf(context_cids[i].clone()),
                        token_savings: 500, // TODO: actual token count
                    });
                }
            }
        }

        // 策略2：基于因果图谱识别过时信息
        if let Some(ref kg) = self.kg {
            for cid in context_cids {
                if let Some(superseder) = self.find_superseder(kg, cid).await? {
                    if !removable.iter().any(|r| r.cid == *cid) {
                        removable.push(RemovalRecord {
                            cid: cid.clone(),
                            reason: RemovalReason::SupersededBy(superseder),
                            token_savings: 500,
                        });
                    }
                }
            }
        }

        // 策略3：基于标签识别临时信息
        for cid in context_cids {
            if self.is_temporary(cid).await? && !removable.iter().any(|r| r.cid == *cid) {
                removable.push(RemovalRecord {
                    cid: cid.clone(),
                    reason: RemovalReason::TemporaryExpired,
                    token_savings: 300,
                });
            }
        }

        // 去重：同一个CID只移除一次
        let mut seen = std::collections::HashSet::new();
        removable.retain(|r| seen.insert(r.cid.clone()));

        Ok(removable)
    }

    /// 为被移除的信息生成摘要
    async fn generate_summaries(
        &self,
        _agent_id: &str,
        _context_cids: &[String],
        _removed_cids: &[String],
    ) -> CognitiveResult<Vec<String>> {
        // TODO: 实际生成摘要并存储到CAS，返回CID
        // 目前返回空，表示不生成摘要
        Ok(Vec::new())
    }

    /// 获取CID的embedding
    async fn get_embeddings(&self, cids: &[String]) -> CognitiveResult<Vec<Vec<f32>>> {
        let mut embeddings = Vec::with_capacity(cids.len());
        for cid in cids {
            // 尝试从记忆系统获取内容并embed
            let text = self.cid_to_text(cid).await.unwrap_or_default();
            if text.is_empty() {
                embeddings.push(Vec::new());
            } else {
                match self.embedding.embed(&text) {
                    Ok(result) => embeddings.push(result.embedding),
                    Err(_) => embeddings.push(Vec::new()),
                }
            }
        }
        Ok(embeddings)
    }

    /// 将CID转换为文本（用于embedding）
    async fn cid_to_text(&self, cid: &str) -> Option<String> {
        // TODO: 从CAS获取对象内容并转为文本
        // 简化实现：返回CID本身作为fallback
        Some(cid.to_string())
    }

    /// 在因果图谱中查找覆盖者
    async fn find_superseder(
        &self,
        _kg: &Arc<dyn KnowledgeGraph>,
        _cid: &str,
    ) -> CognitiveResult<Option<String>> {
        // TODO: 查询因果图谱，查找该CID是否被后续操作覆盖
        Ok(None)
    }

    /// 判断CID是否为临时信息
    async fn is_temporary(&self, _cid: &str) -> CognitiveResult<bool> {
        // TODO: 基于标签/元数据判断
        // 启发式：包含debug、temp、log等标签的视为临时
        Ok(false)
    }
}

/// 计算余弦相似度
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}
