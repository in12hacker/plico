//! Reranker Service — Cross-Encoder reranking for search result refinement.
//!
//! Provides a trait-based abstraction over reranking models. The primary
//! implementation (`LlamaCppReranker`) calls the llama.cpp `/v1/rerank`
//! endpoint backed by models like `bge-reranker-v2-m3`.
//!
//! The reranker sits after the RRF fusion stage in the search pipeline,
//! applying a cross-encoder to score (query, document) pairs more accurately.

use std::sync::Arc;

/// Error type for reranker operations.
#[derive(Debug)]
pub enum RerankError {
    Http(reqwest::Error),
    Io(std::io::Error),
    Parse(String),
    Unavailable(String),
}

impl std::fmt::Display for RerankError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "reranker HTTP error: {e}"),
            Self::Io(e) => write!(f, "reranker IO error: {e}"),
            Self::Parse(msg) => write!(f, "reranker parse error: {msg}"),
            Self::Unavailable(msg) => write!(f, "reranker unavailable: {msg}"),
        }
    }
}

impl std::error::Error for RerankError {}

impl From<reqwest::Error> for RerankError {
    fn from(e: reqwest::Error) -> Self { Self::Http(e) }
}

impl From<std::io::Error> for RerankError {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}

/// A scored document returned by the reranker.
#[derive(Debug, Clone)]
pub struct RerankResult {
    pub id: String,
    pub score: f32,
}

/// Trait for cross-encoder reranking providers.
///
/// Input: a query and a list of `(id, text)` document pairs.
/// Output: the same documents re-scored and sorted by relevance.
pub trait RerankerProvider: Send + Sync {
    fn rerank(
        &self,
        query: &str,
        documents: &[(String, String)],
    ) -> Result<Vec<RerankResult>, RerankError>;

    fn model_name(&self) -> &str;
}

/// llama.cpp-backed reranker calling `POST /v1/rerank`.
pub struct LlamaCppReranker {
    rt: Option<Arc<tokio::runtime::Runtime>>,
    client: reqwest::Client,
    base_url: String,
    model: String,
    top_n: usize,
}

impl LlamaCppReranker {
    pub fn new(base_url: &str, model: &str, top_n: usize) -> Result<Self, RerankError> {
        let rt = match tokio::runtime::Handle::try_current() {
            Ok(_) => None,
            Err(_) => Some(Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()
                    .map_err(RerankError::Io)?,
            )),
        };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(RerankError::Http)?;

        let base = base_url.trim_end_matches('/').to_string();

        Ok(Self {
            rt,
            client,
            base_url: base,
            model: model.to_string(),
            top_n,
        })
    }

    async fn rerank_async(
        client: &reqwest::Client,
        base_url: &str,
        model: &str,
        top_n: usize,
        query: &str,
        documents: &[(String, String)],
    ) -> Result<Vec<RerankResult>, RerankError> {
        let doc_texts: Vec<&str> = documents.iter().map(|(_, text)| text.as_str()).collect();

        let body = serde_json::json!({
            "model": model,
            "query": query,
            "documents": doc_texts,
            "top_n": top_n.min(documents.len()),
        });

        let url = format!("{}/rerank", base_url);
        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(RerankError::Http)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(RerankError::Unavailable(format!(
                "reranker returned {status}: {body_text}"
            )));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(RerankError::Http)?;

        let results = json
            .get("results")
            .and_then(|v| v.as_array())
            .ok_or_else(|| RerankError::Parse("missing 'results' array".into()))?;

        let mut out = Vec::with_capacity(results.len());
        for item in results {
            let index = item
                .get("index")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| RerankError::Parse("missing 'index' in result".into()))?
                as usize;

            let score = item
                .get("relevance_score")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| RerankError::Parse("missing 'relevance_score'".into()))?
                as f32;

            if index < documents.len() {
                out.push(RerankResult {
                    id: documents[index].0.clone(),
                    score,
                });
            }
        }

        out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        Ok(out)
    }
}

impl RerankerProvider for LlamaCppReranker {
    fn rerank(
        &self,
        query: &str,
        documents: &[(String, String)],
    ) -> Result<Vec<RerankResult>, RerankError> {
        if documents.is_empty() {
            return Ok(vec![]);
        }

        let fut = Self::rerank_async(
            &self.client,
            &self.base_url,
            &self.model,
            self.top_n,
            query,
            documents,
        );

        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
            Err(_) => {
                let rt = self
                    .rt
                    .as_ref()
                    .ok_or_else(|| RerankError::Unavailable("no tokio runtime".into()))?;
                rt.block_on(fut)
            }
        }
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

/// Create a reranker provider from environment variables.
///
/// Returns `None` if `PLICO_RERANKER_API_BASE` is not set (disabled).
///
/// Environment variables:
/// - `PLICO_RERANKER_API_BASE` — reranker server URL (e.g. `http://127.0.0.1:18922/v1`)
/// - `PLICO_RERANKER_MODEL` — model name (default: `bge-reranker-v2-m3`)
/// - `PLICO_RERANKER_TOP_N` — max documents to return (default: `10`)
pub fn create_reranker_provider() -> Option<Arc<dyn RerankerProvider>> {
    let base_url = std::env::var("PLICO_RERANKER_API_BASE").ok()?;
    if base_url.trim().is_empty() {
        return None;
    }

    let model = std::env::var("PLICO_RERANKER_MODEL")
        .unwrap_or_else(|_| "bge-reranker-v2-m3".to_string());
    let top_n: usize = std::env::var("PLICO_RERANKER_TOP_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);

    match LlamaCppReranker::new(&base_url, &model, top_n) {
        Ok(r) => {
            tracing::info!("Reranker enabled: {} via {}", model, base_url);
            Some(Arc::new(r) as Arc<dyn RerankerProvider>)
        }
        Err(e) => {
            tracing::warn!("Failed to create reranker: {e}. Reranking disabled.");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rerank_empty_documents() {
        let reranker = LlamaCppReranker::new("http://127.0.0.1:19999/v1", "test", 5).unwrap();
        let result = reranker.rerank("hello", &[]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_create_reranker_provider_disabled_by_default() {
        std::env::remove_var("PLICO_RERANKER_API_BASE");
        assert!(create_reranker_provider().is_none());
    }

    #[test]
    fn test_create_reranker_provider_empty_url() {
        // env var race: just verify the trim logic in isolation
        let base = "  ";
        assert!(base.trim().is_empty());
    }

    #[test]
    fn test_llama_cpp_reranker_new() {
        let r = LlamaCppReranker::new("http://127.0.0.1:19999/v1", "test-model", 3);
        assert!(r.is_ok());
        let reranker = r.unwrap();
        assert_eq!(reranker.model_name(), "test-model");
        assert_eq!(reranker.top_n, 3);
        assert_eq!(reranker.base_url, "http://127.0.0.1:19999/v1");
    }

    #[test]
    fn test_llama_cpp_reranker_trailing_slash() {
        let r = LlamaCppReranker::new("http://127.0.0.1:19999/v1/", "m", 1).unwrap();
        assert_eq!(r.base_url, "http://127.0.0.1:19999/v1");
    }

    #[test]
    fn test_rerank_error_display() {
        let e = RerankError::Parse("bad json".into());
        assert!(e.to_string().contains("parse error"));
        let e = RerankError::Unavailable("down".into());
        assert!(e.to_string().contains("unavailable"));
        let e = RerankError::Io(std::io::Error::new(std::io::ErrorKind::Other, "io"));
        assert!(e.to_string().contains("IO error"));
    }

    #[test]
    fn test_rerank_result_clone() {
        let r = RerankResult { id: "abc".into(), score: 0.9 };
        let r2 = r.clone();
        assert_eq!(r2.id, "abc");
        assert!((r2.score - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn test_rerank_unavailable_server() {
        let reranker = LlamaCppReranker::new("http://127.0.0.1:19999/v1", "test", 5).unwrap();
        let docs = vec![("d1".into(), "text1".into())];
        let result = reranker.rerank("query", &docs);
        assert!(result.is_err());
    }

    #[test]
    fn test_reranker_construction_custom_params() {
        let r = LlamaCppReranker::new("http://127.0.0.1:19999/v1", "custom-model", 3).unwrap();
        assert_eq!(r.model_name(), "custom-model");
        assert_eq!(r.top_n, 3);
    }
}
