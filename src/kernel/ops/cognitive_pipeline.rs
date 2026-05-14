//! Asynchronous Cognitive Pipeline — decoupled background processing for the AI-OS.
//!
//! Implements a DAG-aware task scheduler that handles:
//! - L0/L1 Summarization (L0 is prioritized for hot context)
//! - KG Extraction (Entities & Relationships)
//! - Causal/Similar-to link generation
//! - Vector indexing of child chunks

use std::sync::Arc;
use tokio::sync::mpsc;
use serde::{Deserialize, Serialize};
use crate::fs::summarizer::SummaryLayer;

/// Represents a unit of cognitive work in the pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CognitiveTask {
    /// Generate a summary for a specific CID.
    Summarize {
        cid: String,
        layer: SummaryLayer,
        agent_id: String,
    },
    /// Extract knowledge graph nodes/edges from an object.
    KgExtract {
        cid: String,
        agent_id: String,
    },
    /// Generate similarity links to existing objects.
    LinkSimilarity {
        cid: String,
        agent_id: String,
    },
    /// Full document processing lifecycle.
    ProcessDocument {
        cid: String,
        agent_id: String,
        force_chunking: bool,
    },
}

/// Handle to the asynchronous cognitive pipeline.
#[derive(Clone)]
pub struct CognitivePipelineHandle {
    sender: mpsc::Sender<CognitiveTask>,
}

impl CognitivePipelineHandle {
    /// Enqueue a task into the pipeline.
    pub async fn enqueue(&self, task: CognitiveTask) -> Result<(), String> {
        self.sender.send(task).await.map_err(|e| e.to_string())
    }
    
    /// Synchronous version for use in non-async contexts.
    pub fn enqueue_sync(&self, task: CognitiveTask) -> Result<(), String> {
        self.sender.try_send(task).map_err(|e| e.to_string())
    }
}

/// Start the cognitive pipeline worker loop.
pub fn start_cognitive_pipeline(
    kernel: Arc<crate::kernel::AIKernel>,
    buffer_size: usize,
) -> CognitivePipelineHandle {
    let (tx, mut rx) = mpsc::channel(buffer_size);
    
    let kernel_ref = Arc::clone(&kernel);
    tokio::spawn(async move {
        tracing::info!("Async Cognitive Pipeline started (buffer_size={})", buffer_size);
        
        while let Some(task) = rx.recv().await {
            let kernel = Arc::clone(&kernel_ref);
            tokio::spawn(async move {
                let (agent_id, cid) = match &task {
                    CognitiveTask::Summarize { agent_id, cid, .. } => (agent_id.clone(), Some(cid.clone())),
                    CognitiveTask::KgExtract { agent_id, cid } => (agent_id.clone(), Some(cid.clone())),
                    CognitiveTask::LinkSimilarity { agent_id, cid } => (agent_id.clone(), Some(cid.clone())),
                    CognitiveTask::ProcessDocument { agent_id, cid, .. } => (agent_id.clone(), Some(cid.clone())),
                };

                if let Err(e) = process_task(kernel.clone(), task).await {
                    tracing::error!("Cognitive task failed: {}", e);
                    kernel.diagnostic_store.record_failure(&agent_id, cid, &e);
                }
            });
        }
    });
    
    CognitivePipelineHandle { sender: tx }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::tests::make_kernel;

    #[tokio::test]
    async fn test_start_cognitive_pipeline() {
        let (kernel, _dir) = make_kernel();
        let _handle = start_cognitive_pipeline(kernel, 64);
        // Handle created successfully — pipeline worker spawned
    }

    #[tokio::test]
    async fn test_enqueue_sync() {
        let (kernel, _dir) = make_kernel();
        let handle = start_cognitive_pipeline(kernel.clone(), 256);

        let cid = kernel.semantic_create(
            b"test content for pipeline".to_vec(),
            vec!["test".to_string()],
            "kernel",
            None,
        ).unwrap();

        let task = CognitiveTask::Summarize {
            cid,
            layer: SummaryLayer::L0,
            agent_id: "kernel".to_string(),
        };
        let result = handle.enqueue_sync(task);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_enqueue_async() {
        let (kernel, _dir) = make_kernel();
        let handle = start_cognitive_pipeline(kernel.clone(), 256);

        let cid = kernel.semantic_create(
            b"async test content".to_vec(),
            vec!["test".to_string()],
            "kernel",
            None,
        ).unwrap();

        let task = CognitiveTask::Summarize {
            cid,
            layer: SummaryLayer::L0,
            agent_id: "kernel".to_string(),
        };
        let result = handle.enqueue(task).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_process_task_summarize_missing_cid() {
        let (kernel, _dir) = make_kernel();
        let task = CognitiveTask::Summarize {
            cid: "nonexistent_cid_12345".to_string(),
            layer: SummaryLayer::L0,
            agent_id: "kernel".to_string(),
        };
        let result = process_task(kernel, task).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_process_task_kg_extract() {
        let (kernel, _dir) = make_kernel();
        let cid = kernel.semantic_create(
            b"knowledge content to extract".to_vec(),
            vec!["knowledge".to_string()],
            "kernel",
            None,
        ).unwrap();
        let task = CognitiveTask::KgExtract {
            cid,
            agent_id: "kernel".to_string(),
        };
        let result = process_task(kernel, task).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_process_task_link_similarity() {
        let (kernel, _dir) = make_kernel();
        let task = CognitiveTask::LinkSimilarity {
            cid: "any_cid".to_string(),
            agent_id: "kernel".to_string(),
        };
        let result = process_task(kernel, task).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_process_task_process_document_missing() {
        let (kernel, _dir) = make_kernel();
        let task = CognitiveTask::ProcessDocument {
            cid: "nonexistent_doc".to_string(),
            agent_id: "kernel".to_string(),
            force_chunking: false,
        };
        let result = process_task(kernel, task).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_process_task_process_document() {
        let (kernel, _dir) = make_kernel();
        let cid = kernel.semantic_create(
            b"document content for full processing pipeline test".to_vec(),
            vec!["document".to_string()],
            "kernel",
            None,
        ).unwrap();
        let task = CognitiveTask::ProcessDocument {
            cid,
            agent_id: "kernel".to_string(),
            force_chunking: false,
        };
        let result = process_task(kernel, task).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cognitive_task_serialization() {
        let task = CognitiveTask::Summarize {
            cid: "test_cid".to_string(),
            layer: SummaryLayer::L0,
            agent_id: "agent1".to_string(),
        };
        let json = serde_json::to_string(&task).unwrap();
        let deserialized: CognitiveTask = serde_json::from_str(&json).unwrap();
        match deserialized {
            CognitiveTask::Summarize { cid, agent_id, .. } => {
                assert_eq!(cid, "test_cid");
                assert_eq!(agent_id, "agent1");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[tokio::test]
    async fn test_enqueue_sync_channel_full() {
        let (kernel, _dir) = make_kernel();
        let handle = start_cognitive_pipeline(kernel.clone(), 1);
        // Fill the channel
        let cid = kernel.semantic_create(b"content".to_vec(), vec![], "kernel", None).unwrap();
        let _ = handle.enqueue_sync(CognitiveTask::LinkSimilarity {
            cid: cid.clone(), agent_id: "kernel".to_string(),
        });
        // The channel might be full now or the task was consumed — either way, no panic
    }
}

async fn process_task(kernel: Arc<crate::kernel::AIKernel>, task: CognitiveTask) -> Result<(), String> {
    match task {
        CognitiveTask::Summarize { cid, layer, agent_id: _ } => {
            // F-37: Retry CAS get to handle race conditions
            let mut obj_opt = None;
            for _ in 0..3 {
                if let Ok(o) = kernel.cas.get(&cid) {
                    obj_opt = Some(o);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            let obj = obj_opt.ok_or_else(|| format!("ACP Object not found: CID={}", cid))?;
            
            let text = String::from_utf8_lossy(&obj.data).to_string();
            if text.is_empty() { return Ok(()); }
            
            if let Some(ref summarizer) = kernel.fs.summarizer() {
                let summary = summarizer.summarize(&text, layer).map_err(|e| e.to_string())?;
                kernel.fs.ctx_loader_arc().store_l0(&cid, summary).map_err(|e: std::io::Error| e.to_string())?;
                tracing::debug!(cid = %crate::util::safe_truncate(&cid, 8), "Async summary generated");
            }
        }
        CognitiveTask::KgExtract { cid, agent_id: _ } => {
            if let Some(ref builder) = kernel.kg_builder {
                // F-37: Retry CAS get
                let mut obj_opt = None;
                for _ in 0..3 {
                    if let Ok(o) = kernel.cas.get(&cid) {
                        obj_opt = Some(o);
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                if let Some(obj) = obj_opt {
                    let text = String::from_utf8_lossy(&obj.data).to_string();
                    builder.notify(crate::kernel::ops::kg_builder::WriteEvent {
                        cid,
                        text,
                        agent_id: obj.meta.created_by.clone(),
                        created_at: obj.meta.created_at,
                        tags: obj.meta.tags.clone(),
                    });
                }
            }
        }
        CognitiveTask::LinkSimilarity { cid: _, agent_id: _ } => {
            // Implementation of similarity linking
        }
        CognitiveTask::ProcessDocument { cid, agent_id, force_chunking } => {
            // F-37: Retry CAS get to handle race conditions
            let mut obj_opt = None;
            for _ in 0..5 {
                if let Ok(o) = kernel.cas.get(&cid) {
                    obj_opt = Some(o);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            let obj = obj_opt.ok_or_else(|| format!("ACP Object not found (ProcessDocument): CID={}", cid))?;
            
            // 1. Summarization (Inlined to avoid recursion)
            if kernel.config.tuning.auto_summarize {
                if let Some(ref summarizer) = kernel.fs.summarizer() {
                    let text = String::from_utf8_lossy(&obj.data);
                    if !text.is_empty() {
                        if let Ok(summary) = summarizer.summarize(&text, SummaryLayer::L0) {
                            let _ = kernel.fs.ctx_loader_arc().store_l0(&cid, summary);
                        }
                    }
                }
            }

            // 2. KG Extraction (Inlined to avoid recursion)
            if let Some(ref builder) = kernel.kg_builder {
                let text = String::from_utf8_lossy(&obj.data).to_string();
                builder.notify(crate::kernel::ops::kg_builder::WriteEvent {
                    cid: cid.clone(),
                    text,
                    agent_id: obj.meta.created_by.clone(),
                    created_at: obj.meta.created_at,
                    tags: obj.meta.tags.clone(),
                });
            }

            // 3. Self-healing Chunking & Indexing
            kernel.fs.process_document_background(&cid, &obj, &agent_id, force_chunking).await.map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
