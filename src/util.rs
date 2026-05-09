//! Shared utility functions used across the codebase.

/// Current time in milliseconds since Unix epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Case-insensitive substring check without allocating a new String.
/// `needle_lower` must already be lowercase.
pub fn case_insensitive_contains(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    let needle_len = needle_lower.chars().count();
    let haystack_chars: Vec<char> = haystack.chars().collect();
    if haystack_chars.len() < needle_len {
        return false;
    }
    let needle_chars: Vec<char> = needle_lower.chars().collect();
    haystack_chars.windows(needle_len).any(|window| {
        window
            .iter()
            .zip(needle_chars.iter())
            .all(|(h, n)| h.to_lowercase().eq(std::iter::once(*n)))
    })
}

/// Compute cosine similarity between two vectors.
/// Returns 0.0 if vectors are empty or have different lengths.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
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

/// Truncate a string safely to at most `max_len` characters, ensuring no char boundary panics.
pub fn safe_truncate(s: &str, max_len: usize) -> &str {
    match s.char_indices().nth(max_len) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Slices a string safely around a given range, ensuring no char boundary panics.
pub fn safe_range(s: &str, start: usize, end: usize) -> &str {
    let start_idx = s.char_indices().map(|(i, _)| i).filter(|&i| i <= start).last().unwrap_or(0);
    let end_idx = s.char_indices().map(|(i, _)| i).filter(|&i| i >= end).next().unwrap_or(s.len());
    &s[start_idx..end_idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_now_ms_returns_reasonable_value() {
        let ms = now_ms();
        // Should be after 2020-01-01 (1577836800000 ms)
        assert!(ms > 1_577_836_800_000);
    }

    #[test]
    fn test_case_insensitive_contains_basic() {
        assert!(case_insensitive_contains("Hello World", "hello"));
        assert!(case_insensitive_contains("Hello World", "world"));
        assert!(!case_insensitive_contains("Hello World", "xyz"));
    }

    #[test]
    fn test_case_insensitive_contains_empty_needle() {
        assert!(case_insensitive_contains("anything", ""));
    }

    #[test]
    fn test_case_insensitive_contains_unicode() {
        assert!(case_insensitive_contains("Ünïcödé", "ünï"));
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let v = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < f32::EPSILON);
    }

    #[test]
    fn test_cosine_similarity_empty() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }
}
