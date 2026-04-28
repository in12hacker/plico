//! ONNX Runtime embedding backend.
//!
//! Requires `EMBEDDING_BACKEND=ort` and `PLICO_MODEL_DIR` to point to a directory
//! containing `model.onnx` and `tokenizer.json` (e.g. from sentence-transformers/all-MiniLM-L6-v2).
//!
//! Feature-gated behind `ort-backend` to avoid pulling in ONNX Runtime on all builds.

#[cfg(feature = "ort-backend")]
mod inner {
    use super::super::types::{EmbedError, EmbeddingProvider, EmbedResult};
    use ndarray::Array2;
    use std::path::Path;
    use std::sync::Mutex;
    use ort::session::Session;
    use ort::value::Tensor;

    /// ONNX Runtime embedding backend using a local model.
    ///
    /// Model must be a sentence-transformers compatible ONNX model (e.g. all-MiniLM-L6-v2).
    /// Expected files in `model_dir`:
    /// - `model.onnx` — the ONNX model
    /// - `tokenizer.json` — the HuggingFace tokenizer config
    pub struct OrtEmbeddingBackend {
        session: Mutex<Session>,
        tokenizer: tokenizers::Tokenizer,
        dimension: usize,
        /// Max sequence length the model accepts (e.g. 256 or 384).
        max_length: usize,
    }

    impl OrtEmbeddingBackend {
        /// Create a new ORT embedding backend from a model directory.
        ///
        /// Returns `Err(EmbedError::ModelNotFound)` if `model.onnx` or `tokenizer.json`
        /// is not present in the directory.
        pub fn new(model_dir: &Path) -> Result<Self, EmbedError> {
            let model_path = model_dir.join("model.onnx");
            let tokenizer_path = model_dir.join("tokenizer.json");

            if !model_path.exists() {
                return Err(EmbedError::ModelNotFound(format!(
                    "model.onnx not found in {}",
                    model_dir.display()
                )));
            }
            if !tokenizer_path.exists() {
                return Err(EmbedError::ModelNotFound(format!(
                    "tokenizer.json not found in {}",
                    model_dir.display()
                )));
            }

            // Load tokenizer
            let tokenizer: tokenizers::Tokenizer = tokenizers::Tokenizer::from_file(tokenizer_path)
                .map_err(|e| EmbedError::Runtime(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("failed to load tokenizer.json: {e}")
                )))?;

            let session = Session::builder()
                .map_err(|e| EmbedError::Onnx(format!("failed to build session: {e}")))?
                .commit_from_file(&model_path)
                .map_err(|e| EmbedError::Onnx(format!("failed to load model.onnx: {e}")))?;

            // all-MiniLM-L6-v2 outputs 384-dim embeddings
            let dimension = 384;
            let max_length = 256; // safe default for all-MiniLM-L6-v2

            tracing::info!(
                "OrtEmbeddingBackend loaded: all-MiniLM-L6-v2 ({}d, max_len={})",
                dimension, max_length
            );

            Ok(Self {
                session: Mutex::new(session),
                tokenizer,
                dimension,
                max_length,
            })
        }

        /// Encode a single text into token IDs.
        fn encode(&self, text: &str) -> Result<(Vec<i64>, Vec<i64>), EmbedError> {
            let encoding = self.tokenizer.encode(text, true)
                .map_err(|e| EmbedError::Onnx(format!("tokenization failed: {e}")))?;

            let ids: Vec<i64> = encoding.get_ids().iter().map(|&x| x as i64).collect();
            let mask: Vec<i64> = encoding.get_attention_mask().iter().map(|&x| x as i64).collect();

            // Truncate if necessary
            let ids = if ids.len() > self.max_length {
                ids[..self.max_length].to_vec()
            } else {
                ids
            };
            let mask = if mask.len() > self.max_length {
                mask[..self.max_length].to_vec()
            } else {
                mask
            };

            Ok((ids, mask))
        }

        /// L2-normalize a vector in-place.
        fn normalize(v: &mut [f32]) {
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for x in v.iter_mut() {
                    *x /= norm;
                }
            }
        }

        /// Mean-pool the last hidden state over non-padding tokens (2D version).
        fn pool_2d(&self, hidden: &Array2<f32>, mask: &[i64], seq_len: usize) -> Vec<f32> {
            // hidden shape: (batch=1, seq_len * hidden=384) row-major
            let hidden_slice = hidden.as_slice().unwrap();
            let mut sum = vec![0.0f32; self.dimension];
            let mut count = 0usize;

            for i in 0..seq_len.min(mask.len()) {
                if mask[i] == 1 {
                    count += 1;
                    let base = i * self.dimension;
                    for d in 0..self.dimension {
                        sum[d] += hidden_slice[base + d];
                    }
                }
            }

            if count > 0 {
                for d in 0..self.dimension {
                    sum[d] /= count as f32;
                }
            }

            Self::normalize(&mut sum);
            sum
        }

        fn embed_single(&self, text: &str) -> Result<EmbedResult, EmbedError> {
            let (input_ids, attention_mask) = self.encode(text)?;
            let batch_size = 1;
            let seq_len = input_ids.len();

            // Create input tensors using ort's Tensor::from_array
            let input_ids_tensor: Tensor<i64> = Tensor::from_array(
                ([batch_size, seq_len], input_ids.into_boxed_slice())
            ).map_err(|e| EmbedError::Onnx(format!("failed to create input_ids tensor: {e}")))?;

            let attention_tensor: Tensor<i64> = Tensor::from_array(
                ([batch_size, seq_len], attention_mask.clone().into_boxed_slice())
            ).map_err(|e| EmbedError::Onnx(format!("failed to create attention_mask tensor: {e}")))?;

            // all-MiniLM-L6-v2 does NOT use token_type_ids in the ONNX export,
            // but we pass zeros to be safe
            let token_type_tensor: Tensor<i64> = Tensor::from_array(
                ([batch_size, seq_len], vec![0i64; seq_len].into_boxed_slice())
            ).map_err(|e| EmbedError::Onnx(format!("failed to create token_type_ids tensor: {e}")))?;

            // Run inference
            let flat: Vec<f32> = {
                let mut session = self.session.lock().unwrap();
                let outputs = session
                    .run(ort::inputs! {
                        "input_ids" => input_ids_tensor,
                        "attention_mask" => attention_tensor,
                        "token_type_ids" => token_type_tensor,
                    })
                    .map_err(|e| EmbedError::Onnx(format!("inference failed: {e}")))?;

                // Output is "last_hidden_state" — shape (batch, seq_len, hidden=384)
                let last_hidden = outputs.get("last_hidden_state")
                    .ok_or_else(|| EmbedError::Onnx(
                        "model output missing 'last_hidden_state'".to_string()
                    ))?;

                last_hidden
                    .try_extract_array()
                    .map_err(|e| EmbedError::Onnx(format!("failed to extract hidden state: {e}")))?
                    .iter()
                    .copied()
                    .collect::<Vec<f32>>()
            };

            // Reshape to (batch=1, seq_len*hidden) and view as 2D
            let hidden_2d: Array2<f32> = Array2::from_shape_vec((1, flat.len()), flat)
                .map_err(|e| EmbedError::Onnx(format!("bad hidden shape: {e}")))?;

            let embedding = self.pool_2d(&hidden_2d, &attention_mask, seq_len);
            let input_tokens = input_ids.len() as u32;
            Ok(EmbedResult::new(embedding, input_tokens))
        }
    }

    impl EmbeddingProvider for OrtEmbeddingBackend {
        fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
            self.embed_single(text)
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
            texts.iter().map(|t| self.embed_single(t)).collect()
        }

        fn dimension(&self) -> usize {
            self.dimension
        }

        fn model_name(&self) -> &str {
            "all-MiniLM-L6-v2"
        }
    }
}

// Always re-export (the inner impl is feature-gated)
#[cfg(not(feature = "ort-backend"))]
mod inner {
    use super::super::types::{EmbedError, EmbeddingProvider, EmbedResult};
    use std::path::Path;

    /// Stub for when `ort-backend` feature is not enabled.
    /// Disabled via feature gate so no ort code is compiled.
    pub struct OrtEmbeddingBackend;

    impl OrtEmbeddingBackend {
        pub fn new(_model_dir: &Path) -> Result<Self, EmbedError> {
            Err(EmbedError::ModelNotFound(
                "ort-backend feature not enabled".to_string(),
            ))
        }
    }

    impl EmbeddingProvider for OrtEmbeddingBackend {
        fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
            unreachable!("ort-backend feature not enabled")
        }
        fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
            unreachable!("ort-backend feature not enabled")
        }
        fn dimension(&self) -> usize {
            384
        }
        fn model_name(&self) -> &str {
            "all-MiniLM-L6-v2"
        }
    }
}

pub use inner::OrtEmbeddingBackend;