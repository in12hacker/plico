//! Hierarchical Semantic Chunking — splits documents into child chunks for retrieval
//! and keeps parent chunks for context expansion.
//!
//! Strategy:
//! - Detect sentence boundaries, then group consecutive sentences whose embedding
//!   similarity exceeds a threshold into a single child chunk (~256 tokens).
//! - Each child chunk carries a `parent_cid` tag linking back to the full document.
//! - At search time, child chunks are retrieved; then the parent document provides
//!   surrounding context to the LLM.
//!
//! Controlled by `PLICO_CHUNKING` env var: `semantic` | `fixed` | `none` (default).

use crate::fs::embedding::{EmbeddingProvider, EmbedError};

/// A produced chunk with its text and byte offset within the parent document.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub text: String,
    pub start_byte: usize,
    pub end_byte: usize,
}

/// Chunking mode, read from `PLICO_CHUNKING` env var.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkingMode {
    None,
    Fixed,
    Semantic,
}

impl ChunkingMode {
    pub fn from_env() -> Self {
        match std::env::var("PLICO_CHUNKING").as_deref() {
            Ok("semantic") => Self::Semantic,
            Ok("fixed") => Self::Fixed,
            _ => Self::None,
        }
    }
}

/// Target child chunk size in characters (approximate; actual split is sentence-aligned).
const TARGET_CHUNK_CHARS: usize = 800;
/// Minimum chunk size — don't create tiny fragments.
const MIN_CHUNK_CHARS: usize = 100;
/// Cosine similarity threshold: if adjacent sentence embeddings drop below this,
/// a semantic boundary is detected.
const SEMANTIC_BOUNDARY_THRESHOLD: f32 = 0.5;

/// Split text into sentence-aligned chunks.
///
/// In `Semantic` mode, uses embedding similarity to detect topic shifts.
/// In `Fixed` mode, splits purely by character count at sentence boundaries.
pub fn chunk_document(
    text: &str,
    mode: ChunkingMode,
    embedding: Option<&dyn EmbeddingProvider>,
) -> Vec<Chunk> {
    if mode == ChunkingMode::None || text.len() < MIN_CHUNK_CHARS * 2 {
        return vec![];
    }

    let sentences = split_sentences(text);
    if sentences.len() <= 1 {
        return vec![];
    }

    match mode {
        ChunkingMode::Semantic if embedding.is_some() => {
            semantic_chunk(text, &sentences, embedding.unwrap())
        }
        _ => fixed_chunk(text, &sentences),
    }
}

/// Split text into sentences using simple heuristics.
fn split_sentences(text: &str) -> Vec<(usize, usize)> {
    let mut result = Vec::new();
    let mut start = 0;

    for (i, c) in text.char_indices() {
        let byte_after = i + c.len_utf8();
        if (c == '.' || c == '!' || c == '?' || c == '\n')
            && byte_after < text.len()
        {
            let next = text[byte_after..].chars().next();
            if c == '\n' || next.map_or(true, |nc| nc.is_whitespace() || nc.is_uppercase()) {
                let end = byte_after;
                let trimmed = text[start..end].trim();
                if !trimmed.is_empty() {
                    result.push((start, end));
                }
                start = end;
            }
        }
    }

    if start < text.len() {
        let trimmed = text[start..].trim();
        if !trimmed.is_empty() {
            result.push((start, text.len()));
        }
    }

    result
}

/// Fixed-size chunking at sentence boundaries.
fn fixed_chunk(text: &str, sentences: &[(usize, usize)]) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut group_start = sentences[0].0;
    let mut current_len = 0;

    for &(s_start, s_end) in sentences {
        let s_len = s_end - s_start;
        if current_len > 0 && current_len + s_len > TARGET_CHUNK_CHARS {
            let chunk_text = text[group_start..s_start].trim().to_string();
            if chunk_text.len() >= MIN_CHUNK_CHARS {
                chunks.push(Chunk {
                    text: chunk_text,
                    start_byte: group_start,
                    end_byte: s_start,
                });
            }
            group_start = s_start;
            current_len = 0;
        }
        current_len += s_len;
    }

    let last_end = sentences.last().map(|s| s.1).unwrap_or(text.len());
    let chunk_text = text[group_start..last_end].trim().to_string();
    if chunk_text.len() >= MIN_CHUNK_CHARS {
        chunks.push(Chunk {
            text: chunk_text,
            start_byte: group_start,
            end_byte: last_end,
        });
    }

    chunks
}

/// Semantic chunking: detect topic boundaries using embedding similarity.
fn semantic_chunk(
    text: &str,
    sentences: &[(usize, usize)],
    embedding: &dyn EmbeddingProvider,
) -> Vec<Chunk> {
    let sentence_texts: Vec<&str> = sentences.iter().map(|&(s, e)| &text[s..e]).collect();

    let embeddings: Vec<Vec<f32>> = match embed_sentences(embedding, &sentence_texts) {
        Ok(embs) => embs,
        Err(_) => return fixed_chunk(text, sentences),
    };

    let mut boundaries = vec![false; sentences.len()];
    for i in 1..embeddings.len() {
        let sim = cosine_similarity(&embeddings[i - 1], &embeddings[i]);
        if sim < SEMANTIC_BOUNDARY_THRESHOLD {
            boundaries[i] = true;
        }
    }

    let mut chunks = Vec::new();
    let mut group_start = sentences[0].0;
    let mut current_len = 0;

    for (idx, &(s_start, s_end)) in sentences.iter().enumerate() {
        let s_len = s_end - s_start;
        let is_boundary = boundaries[idx] || (current_len + s_len > TARGET_CHUNK_CHARS * 2);

        if idx > 0 && is_boundary && current_len >= MIN_CHUNK_CHARS {
            let chunk_text = text[group_start..s_start].trim().to_string();
            if chunk_text.len() >= MIN_CHUNK_CHARS {
                chunks.push(Chunk {
                    text: chunk_text,
                    start_byte: group_start,
                    end_byte: s_start,
                });
            }
            group_start = s_start;
            current_len = 0;
        }
        current_len += s_len;
    }

    let last_end = sentences.last().map(|s| s.1).unwrap_or(text.len());
    let chunk_text = text[group_start..last_end].trim().to_string();
    if chunk_text.len() >= MIN_CHUNK_CHARS {
        chunks.push(Chunk {
            text: chunk_text,
            start_byte: group_start,
            end_byte: last_end,
        });
    }

    chunks
}

fn embed_sentences(
    embedding: &dyn EmbeddingProvider,
    sentences: &[&str],
) -> Result<Vec<Vec<f32>>, EmbedError> {
    let batch_size = 32;
    let mut all_embeddings = Vec::with_capacity(sentences.len());

    for batch in sentences.chunks(batch_size) {
        let results = embedding.embed_batch(batch)?;
        for r in results {
            all_embeddings.push(r.embedding);
        }
    }

    Ok(all_embeddings)
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a < 1e-10 || norm_b < 1e-10 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunking_mode_from_env() {
        std::env::remove_var("PLICO_CHUNKING");
        assert_eq!(ChunkingMode::from_env(), ChunkingMode::None);
    }

    #[test]
    fn test_split_sentences_basic() {
        let text = "Hello world. This is a test. And another sentence!";
        let sents = split_sentences(text);
        assert!(sents.len() >= 2, "should split into multiple sentences: {:?}", sents);
    }

    #[test]
    fn test_split_sentences_newlines() {
        let text = "First line.\nSecond line.\nThird line.";
        let sents = split_sentences(text);
        assert!(sents.len() >= 2, "newlines should split: {:?}", sents);
    }

    #[test]
    fn test_fixed_chunk_small_text() {
        let text = "Short text.";
        let chunks = chunk_document(text, ChunkingMode::Fixed, None);
        assert!(chunks.is_empty(), "short text should not be chunked");
    }

    #[test]
    fn test_fixed_chunk_large_text() {
        let long_text = (0..50)
            .map(|i| format!("This is sentence number {}. ", i))
            .collect::<String>();
        let chunks = chunk_document(&long_text, ChunkingMode::Fixed, None);
        assert!(chunks.len() >= 2, "long text should produce multiple chunks: {}", chunks.len());
        for chunk in &chunks {
            assert!(chunk.text.len() >= MIN_CHUNK_CHARS, "chunk too small: {}", chunk.text.len());
        }
    }

    #[test]
    fn test_none_mode_produces_no_chunks() {
        let text = "A. B. C. D. E. F. G. H. I. J. K. L. M. N. O. P. Q. R. S. T.".repeat(20);
        let chunks = chunk_document(&text, ChunkingMode::None, None);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);

        let c = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &c).abs() < 1e-6);
    }

    #[test]
    fn test_chunk_preserves_content() {
        let long_text = (0..50)
            .map(|i| format!("Sentence {} about topic alpha. ", i))
            .collect::<String>();
        let chunks = chunk_document(&long_text, ChunkingMode::Fixed, None);
        let reconstructed: String = chunks.iter().map(|c| c.text.clone()).collect::<Vec<_>>().join(" ");
        for i in 0..50 {
            assert!(
                reconstructed.contains(&format!("Sentence {}", i)) || long_text[..MIN_CHUNK_CHARS].contains(&format!("Sentence {}", i)),
                "chunk should preserve sentence {}", i,
            );
        }
    }
}
