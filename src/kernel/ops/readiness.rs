//! Side-effect-free runtime readiness for public transport health checks.

use serde::{Deserialize, Serialize};

use crate::kernel::AIKernel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProbeState {
    Verified,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderReadiness {
    pub configured_backend: String,
    pub active_provider: String,
    pub probe_state: ProviderProbeState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerReadiness {
    pub projection_ready: bool,
    pub cognitive_present: bool,
    pub cognitive_progress: Option<super::cognitive_pipeline::CognitivePipelineSnapshot>,
}

#[derive(Clone)]
pub struct RuntimeReadiness {
    pub ready: bool,
    pub canonical_initialized: bool,
    pub canonical_memory_ledger_present: bool,
    pub workers: WorkerReadiness,
    pub embedding: ProviderReadiness,
    pub(crate) projection: super::projection_runtime::ProjectionRuntimeReadinessSnapshot,
}

impl AIKernel {
    /// Return bounded runtime state without probing providers or mutating storage.
    pub fn runtime_readiness(&self) -> RuntimeReadiness {
        // AIKernel construction succeeds only after CAS initialization succeeds.
        let canonical_initialized = true;
        let canonical_memory_ledger_present = true;
        let projection = self.projection.readiness_snapshot();
        let cognitive_progress = self
            .cognitive_pipeline
            .read()
            .ok()
            .and_then(|pipeline| pipeline.as_ref().map(|handle| handle.snapshot()));
        let workers = WorkerReadiness {
            projection_ready: projection.worker_ready,
            cognitive_present: cognitive_progress.is_some(),
            cognitive_progress,
        };
        let embedding = ProviderReadiness {
            configured_backend: self.config.inference.embedding_backend.clone(),
            active_provider: projection.identity.clone().unwrap_or_else(|_| "unavailable".into()),
            probe_state: if projection.identity.is_ok() {
                ProviderProbeState::Verified
            } else {
                ProviderProbeState::Unavailable
            },
        };

        let readiness = RuntimeReadiness {
            ready: canonical_initialized && canonical_memory_ledger_present && !projection.shutting_down,
            canonical_initialized,
            canonical_memory_ledger_present,
            workers,
            embedding,
            projection,
        };
        tracing::debug!(
            target: "plico::readiness",
            ready = readiness.ready,
            canonical_initialized = readiness.canonical_initialized,
            canonical_memory_ledger_present = readiness.canonical_memory_ledger_present,
            projection_worker_ready = readiness.workers.projection_ready,
            cognitive_worker_present = readiness.workers.cognitive_present,
            cognitive_accepted = readiness.workers.cognitive_progress.map(|progress| progress.accepted),
            cognitive_completed = readiness.workers.cognitive_progress.map(|progress| progress.completed),
            cognitive_in_flight = readiness.workers.cognitive_progress.map(|progress| progress.in_flight),
            configured_embedding_backend = %readiness.embedding.configured_backend,
            active_embedding_provider = %readiness.embedding.active_provider,
            embedding_probe_state = match readiness.embedding.probe_state {
                ProviderProbeState::Verified => "verified",
                ProviderProbeState::Unavailable => "unavailable",
            },
            "Runtime readiness inspected"
        );
        readiness
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use crate::fs::{EmbedError, EmbedResult, EmbeddingBuilderIdentity, EmbeddingIdentityError, EmbeddingProvider};
    use crate::llm::{ChatMessage, ChatOptions, LlmError, LlmProvider};

    use super::*;

    struct CountingProvider {
        calls: Arc<AtomicUsize>,
    }

    struct CountingLlm {
        calls: Arc<AtomicUsize>,
    }

    impl LlmProvider for CountingLlm {
        fn chat(&self, _messages: &[ChatMessage], _options: &ChatOptions) -> Result<(String, u32, u32), LlmError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok((String::new(), 0, 0))
        }

        fn model_name(&self) -> &str {
            "counting-llm-test"
        }
    }

    impl EmbeddingProvider for CountingProvider {
        fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(EmbedResult::new(vec![1.0, 0.0], 1))
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(texts.iter().map(|_| EmbedResult::new(vec![1.0, 0.0], 1)).collect())
        }

        fn dimension(&self) -> usize {
            2
        }

        fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
            Ok(EmbeddingBuilderIdentity::test_deterministic(
                "counting-test",
                2,
                "counting-test-v1",
            ))
        }

        fn model_name(&self) -> String {
            "counting-test".into()
        }
    }

    fn file_snapshot(root: &Path) -> Vec<PathBuf> {
        fn visit(root: &Path, current: &Path, paths: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(current) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if let Ok(relative) = path.strip_prefix(root) {
                    paths.push(relative.to_path_buf());
                }
                if path.is_dir() {
                    visit(root, &path, paths);
                }
            }
        }

        let mut paths = Vec::new();
        visit(root, root, &mut paths);
        paths.sort();
        paths
    }

    #[test]
    fn readiness_does_not_probe_provider_or_write_canonical_storage() {
        let dir = tempfile::tempdir().unwrap();
        let embedding_calls = Arc::new(AtomicUsize::new(0));
        let llm_calls = Arc::new(AtomicUsize::new(0));
        let kernel = AIKernel::with_providers(
            dir.path().to_path_buf(),
            Arc::new(CountingProvider {
                calls: Arc::clone(&embedding_calls),
            }),
            Arc::new(CountingLlm {
                calls: Arc::clone(&llm_calls),
            }),
        )
        .unwrap();
        let cids_before = kernel.cas.list_cids().unwrap();
        let files_before = file_snapshot(dir.path());

        let readiness = kernel.runtime_readiness();

        assert!(readiness.canonical_initialized);
        assert!(readiness.canonical_memory_ledger_present);
        assert!(!readiness.workers.projection_ready);
        assert_eq!(readiness.workers.cognitive_progress, None);
        assert_eq!(readiness.embedding.probe_state, ProviderProbeState::Verified);
        assert_eq!(readiness.embedding.active_provider, "counting-test");
        assert_eq!(embedding_calls.load(Ordering::Relaxed), 0);
        assert_eq!(llm_calls.load(Ordering::Relaxed), 0);
        assert_eq!(kernel.cas.list_cids().unwrap(), cids_before);
        assert_eq!(file_snapshot(dir.path()), files_before);
    }

    #[tokio::test]
    async fn readiness_reports_worker_presence_without_provider_probe() {
        let dir = tempfile::tempdir().unwrap();
        let embedding_calls = Arc::new(AtomicUsize::new(0));
        let llm_calls = Arc::new(AtomicUsize::new(0));
        let kernel = AIKernel::with_providers(
            dir.path().join("fresh-vault"),
            Arc::new(CountingProvider {
                calls: Arc::clone(&embedding_calls),
            }),
            Arc::new(CountingLlm {
                calls: Arc::clone(&llm_calls),
            }),
        )
        .unwrap();

        kernel.start_workers();
        let readiness = kernel.runtime_readiness();

        assert!(readiness.workers.projection_ready);
        assert!(readiness.workers.cognitive_present);
        assert_eq!(
            readiness.workers.cognitive_progress,
            Some(crate::kernel::ops::cognitive_pipeline::CognitivePipelineSnapshot::default())
        );
        assert!(readiness.ready);
        assert_eq!(embedding_calls.load(Ordering::Relaxed), 0);
        assert_eq!(llm_calls.load(Ordering::Relaxed), 0);
    }
}
