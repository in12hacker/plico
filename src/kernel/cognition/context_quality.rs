//! 上下文质量引擎 —— 解决上下文腐败问题
//!
//! 持续分析上下文的 token 构成，识别并处理：
//! - 重复信息
//! - 过时信息
//! - 低相关性信息
//! - 临时/噪声信息

use crate::util::cosine_similarity;
use std::sync::Arc;

use crate::cas::CASStorage;
use crate::fs::embedding::EmbeddingProvider;
use crate::fs::graph::{KnowledgeGraph, KGEdgeType};
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
    cas: Arc<CASStorage>,
}

const TEMPORARY_TAGS: &[&str] = &["temp", "debug", "scratch", "log", "stderr", "stdout", "tmp", "ephemeral"];

impl ContextQualityEngine {
    pub fn new(
        embedding: Arc<dyn EmbeddingProvider>,
        search: Arc<dyn SemanticSearch>,
        memory: Arc<LayeredMemory>,
        cas: Arc<CASStorage>,
    ) -> Self {
        Self {
            embedding,
            search,
            kg: None,
            memory,
            cas,
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
        let mut total_tokens = 0usize;

        // Classify each CID by tags and compute real token counts
        for cid in context_cids {
            let tokens = self.cid_token_count(cid);
            total_tokens += tokens;

            let tags = self.cid_tags(cid);
            let lower_tags: Vec<String> = tags.iter().map(|t| t.to_lowercase()).collect();
            let is_temp = TEMPORARY_TAGS.iter().any(|t| lower_tags.iter().any(|lt| lt.contains(t)));

            if is_temp {
                breakdown.temporary_data += tokens;
            } else if lower_tags.iter().any(|t| t.starts_with("skill:") || t.contains("procedural")) {
                breakdown.procedural_info += tokens;
            } else {
                breakdown.core_knowledge += tokens;
            }
        }

        // Check for redundancy via embedding similarity
        let embeddings = self.get_embeddings(context_cids).await?;
        let mut redundant_tokens = 0usize;
        let mut seen_redundant = std::collections::HashSet::new();
        for (i, emb_i) in embeddings.iter().enumerate() {
            if emb_i.is_empty() { continue; }
            for (j, emb_j) in embeddings.iter().enumerate().skip(i + 1) {
                if emb_j.is_empty() { continue; }
                if cosine_similarity(emb_i, emb_j) > 0.95 && !seen_redundant.contains(&j) {
                    redundant_tokens += self.cid_token_count(&context_cids[j]);
                    seen_redundant.insert(j);
                }
            }
        }
        breakdown.redundant_info = redundant_tokens;

        // Check for stale info via KG Supersedes edges
        let mut stale_tokens = 0usize;
        let mut stale_cids = Vec::new();
        if let Some(ref kg) = self.kg {
            for cid in context_cids {
                if let Ok(Some(_)) = self.find_superseder(kg, cid).await {
                    stale_tokens += self.cid_token_count(cid);
                    stale_cids.push(cid.clone());
                }
            }
        }
        breakdown.stale_info = stale_tokens;

        // Calculate quality score
        let total = total_tokens as f32;
        let score = if total > 0.0 {
            let noise_ratio = (breakdown.temporary_data + breakdown.redundant_info + breakdown.stale_info) as f32 / total;
            (1.0 - noise_ratio).clamp(0.0, 1.0)
        } else {
            1.0
        };

        if total > 0.0 && breakdown.redundant_info as f32 / total > 0.15 {
            issues.push(ContextIssue::HighRedundancy {
                redundant_ratio: breakdown.redundant_info as f32 / total,
            });
        }

        if total > 0.0 && breakdown.temporary_data as f32 / total > 0.2 {
            issues.push(ContextIssue::HighTemporaryRatio {
                temp_ratio: breakdown.temporary_data as f32 / total,
            });
        }

        if !stale_cids.is_empty() {
            issues.push(ContextIssue::ContainsStaleInfo { stale_cids });
        }

        Ok(ContextQuality {
            score,
            token_count: total_tokens,
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
                        token_savings: self.cid_token_count(&context_cids[j]),
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
                            token_savings: self.cid_token_count(cid),
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
                    token_savings: self.cid_token_count(cid),
                });
            }
        }

        // 去重：同一个CID只移除一次
        let mut seen = std::collections::HashSet::new();
        removable.retain(|r| seen.insert(r.cid.clone()));

        Ok(removable)
    }

    /// 为被移除的信息生成 L0 摘要（浓缩版，存储到 CAS）
    async fn generate_summaries(
        &self,
        agent_id: &str,
        _context_cids: &[String],
        removed_cids: &[String],
    ) -> CognitiveResult<Vec<String>> {
        if removed_cids.is_empty() {
            return Ok(Vec::new());
        }

        // Collect text from removed CIDs
        let mut parts = Vec::new();
        for cid in removed_cids {
            if let Some(text) = self.cid_to_text(cid).await {
                if !text.is_empty() {
                    // Truncate each entry to ~200 chars for L0 summary
                    let truncated = if text.len() > 200 {
                        format!("{}…", &text[..200])
                    } else {
                        text
                    };
                    parts.push(truncated);
                }
            }
        }

        if parts.is_empty() {
            return Ok(Vec::new());
        }

        // Concatenate into a summary document (cap at ~2000 chars total)
        let mut summary = String::new();
        for (i, part) in parts.iter().enumerate() {
            if summary.len() > 2000 { break; }
            if i > 0 { summary.push_str("\n---\n"); }
            summary.push_str(part);
        }

        // Store summary as CAS object
        use crate::cas::AIObject;
        let meta = crate::cas::AIObjectMeta::text(["summary", "l0", "auto-generated"])
            .with_agent(agent_id);
        let obj = AIObject::new(summary.into_bytes(), meta);
        let cid = obj.cid.clone();
        match self.cas.put(&obj) {
            Ok(_) => Ok(vec![cid]),
            Err(e) => {
                tracing::warn!("Failed to store summary: {}", e);
                Ok(Vec::new())
            }
        }
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
        let obj = self.cas.get(cid).ok()?;
        String::from_utf8(obj.data).ok()
    }

    /// 获取CID的实际token数量（按字符数估算，~4 chars/token）
    fn cid_token_count(&self, cid: &str) -> usize {
        self.cas.get_raw(cid)
            .ok()
            .map(|obj| (obj.data.len() / 4).max(1))
            .unwrap_or(0)
    }

    /// 获取CID的元数据标签
    fn cid_tags(&self, cid: &str) -> Vec<String> {
        self.cas.get_raw(cid)
            .ok()
            .map(|obj| obj.meta.tags)
            .unwrap_or_default()
    }

    /// 在因果图谱中查找覆盖者（Supersedes边）
    async fn find_superseder(
        &self,
        kg: &Arc<dyn KnowledgeGraph>,
        cid: &str,
    ) -> CognitiveResult<Option<String>> {
        // Supersedes边方向: new_cid --Supersedes--> old_cid
        // 所以查old_cid的incoming Supersedes边可以找到new_cid
        let neighbors = kg.get_neighbors(cid, Some(KGEdgeType::Supersedes), 1)
            .map_err(|e| super::CognitiveError::AnalysisFailed(e.to_string()))?;
        // 返回最近的superseder
        Ok(neighbors.first().map(|(node, _)| node.id.clone()))
    }

    /// 判断CID是否为临时信息（基于标签）
    async fn is_temporary(&self, cid: &str) -> CognitiveResult<bool> {
        let tags = self.cid_tags(cid);
        Ok(tags.iter().any(|tag| {
            let lower = tag.to_lowercase();
            TEMPORARY_TAGS.iter().any(|t| lower.contains(t))
        }))
    }
}
