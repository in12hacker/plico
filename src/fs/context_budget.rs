//! Context Budget Engine — adaptive multi-object context assembly.
//!
//! Given a token budget and a ranked list of CIDs (from search, memory, or KG),
//! assemble the optimal mix of L0/L1/L2 layers. Like virtual memory paging for
//! AI: the kernel decides which "pages" to load based on relevance and budget.
//!
//! Algorithm: greedy by relevance score. Most relevant CIDs get the highest
//! layer (L2) that fits the remaining budget, then progressively downgrade
//! to L1/L0 as budget shrinks.

use super::context_loader::{ContextLayer, ContextLoader, LoadedContext};

/// A candidate CID with its relevance score (0.0–1.0).
#[derive(Debug, Clone)]
pub struct ContextCandidate {
    pub cid: String,
    pub relevance: f32,
}

/// Result of budget allocation: which layer was assigned to each CID.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BudgetAllocation {
    pub items: Vec<LoadedContext>,
    pub total_tokens: usize,
    pub budget: usize,
    pub candidates_considered: usize,
    pub candidates_included: usize,
}

/// Assemble context within a token budget from ranked candidates.
///
/// Candidates should be pre-sorted by relevance (highest first).
/// The engine greedily assigns the highest layer that fits the remaining budget.
pub fn assemble(
    loader: &ContextLoader,
    candidates: &[ContextCandidate],
    budget_tokens: usize,
) -> BudgetAllocation {
    let mut items = Vec::new();
    let mut remaining = budget_tokens;
    let candidates_considered = candidates.len();

    for candidate in candidates {
        if remaining == 0 {
            break;
        }

        let layer = pick_layer(loader, &candidate.cid, remaining);
        let layer = match layer {
            Some(l) => l,
            None => continue,
        };

        match loader.load(&candidate.cid, layer) {
            Ok(loaded) => {
                if loaded.tokens_estimate <= remaining {
                    remaining = remaining.saturating_sub(loaded.tokens_estimate);
                    items.push(loaded);
                } else if layer != ContextLayer::L0 {
                    if let Ok(l0) = loader.load(&candidate.cid, ContextLayer::L0) {
                        if l0.tokens_estimate <= remaining {
                            remaining = remaining.saturating_sub(l0.tokens_estimate);
                            items.push(l0);
                        }
                    }
                }
            }
            Err(_) => continue,
        }
    }

    let total_tokens = items.iter().map(|i| i.tokens_estimate).sum();
    let candidates_included = items.len();

    BudgetAllocation {
        items,
        total_tokens,
        budget: budget_tokens,
        candidates_considered,
        candidates_included,
    }
}

/// Pick the highest layer that fits the remaining budget.
fn pick_layer(
    loader: &ContextLoader,
    cid: &str,
    remaining_tokens: usize,
) -> Option<ContextLayer> {
    if remaining_tokens >= ContextLayer::L2.tokens_approx() {
        return Some(ContextLayer::L2);
    }

    if let Ok(l2) = loader.load(cid, ContextLayer::L2) {
        if l2.tokens_estimate <= remaining_tokens {
            return Some(ContextLayer::L2);
        }
    }

    if remaining_tokens >= ContextLayer::L1.tokens_approx() {
        return Some(ContextLayer::L1);
    }

    if let Ok(l1) = loader.load(cid, ContextLayer::L1) {
        if l1.tokens_estimate <= remaining_tokens {
            return Some(ContextLayer::L1);
        }
    }

    if let Ok(l0) = loader.load(cid, ContextLayer::L0) {
        if l0.tokens_estimate <= remaining_tokens {
            return Some(ContextLayer::L0);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::cas::CASStorage;

    fn make_loader() -> (ContextLoader, tempfile::TempDir, Arc<CASStorage>) {
        let dir = tempfile::tempdir().unwrap();
        let cas = Arc::new(CASStorage::new(dir.path().join("cas")).unwrap());
        let loader = ContextLoader::new(
            dir.path().join("context"),
            None,
            Arc::clone(&cas),
        ).unwrap();
        (loader, dir, cas)
    }

    fn store_object(cas: &CASStorage, content: &str) -> String {
        use crate::cas::object::{AIObject, AIObjectMeta, ContentType};
        let meta = AIObjectMeta {
            content_type: ContentType::Text,
            tags: vec![],
            created_by: "test".to_string(),
            created_at: 0,
            intent: None,
            tenant_id: "default".to_string(),
        };
        let obj = AIObject::new(content.as_bytes().to_vec(), meta);
        cas.put(&obj).unwrap()
    }

    #[test]
    fn test_assemble_single_item_fits_budget() {
        let (loader, _dir, cas) = make_loader();
        let cid = store_object(&cas, "Hello world, this is a test document.");
        loader.store_l0(&cid, "test doc summary".to_string()).unwrap();

        let candidates = vec![ContextCandidate { cid: cid.clone(), relevance: 1.0 }];
        let result = assemble(&loader, &candidates, 10000);

        assert_eq!(result.candidates_included, 1);
        assert_eq!(result.items[0].layer, ContextLayer::L2);
        assert!(result.total_tokens <= result.budget);
    }

    #[test]
    fn test_assemble_budget_forces_downgrade() {
        let (loader, _dir, cas) = make_loader();
        let content = "word ".repeat(3000);
        let cid = store_object(&cas, &content);
        loader.store_l0(&cid, "short summary".to_string()).unwrap();

        let candidates = vec![ContextCandidate { cid: cid.clone(), relevance: 1.0 }];
        // Budget too small for L2 (3000 words ≈ 2250 tokens) but big enough for L0
        let result = assemble(&loader, &candidates, 50);

        assert_eq!(result.candidates_included, 1);
        assert_eq!(result.items[0].layer, ContextLayer::L0);
    }

    #[test]
    fn test_assemble_multiple_candidates_greedy() {
        let (loader, _dir, cas) = make_loader();

        let cid1 = store_object(&cas, "First document about Rust programming.");
        loader.store_l0(&cid1, "Rust programming".to_string()).unwrap();

        let cid2 = store_object(&cas, "Second document about Python scripting.");
        loader.store_l0(&cid2, "Python scripting".to_string()).unwrap();

        let cid3 = store_object(&cas, "Third document about Go concurrency.");
        loader.store_l0(&cid3, "Go concurrency".to_string()).unwrap();

        let candidates = vec![
            ContextCandidate { cid: cid1.clone(), relevance: 0.9 },
            ContextCandidate { cid: cid2.clone(), relevance: 0.7 },
            ContextCandidate { cid: cid3.clone(), relevance: 0.5 },
        ];

        let result = assemble(&loader, &candidates, 10000);
        assert_eq!(result.candidates_included, 3);
        assert!(result.total_tokens <= result.budget);
    }

    #[test]
    fn test_assemble_zero_budget_returns_empty() {
        let (loader, _dir, cas) = make_loader();
        let cid = store_object(&cas, "content");
        let candidates = vec![ContextCandidate { cid, relevance: 1.0 }];

        let result = assemble(&loader, &candidates, 0);
        assert_eq!(result.candidates_included, 0);
        assert_eq!(result.total_tokens, 0);
    }

    #[test]
    fn test_assemble_empty_candidates() {
        let (loader, _dir, _cas) = make_loader();
        let result = assemble(&loader, &[], 10000);
        assert_eq!(result.candidates_included, 0);
        assert_eq!(result.candidates_considered, 0);
    }

    #[test]
    fn test_assemble_respects_budget_across_items() {
        let (loader, _dir, cas) = make_loader();

        // Create 5 objects, each ~100 tokens (75 words ≈ 56 tokens via 3/4 ratio)
        let mut candidates = Vec::new();
        for i in 0..5 {
            let content = format!("Document {} content. ", i).repeat(20);
            let cid = store_object(&cas, &content);
            loader.store_l0(&cid, format!("Doc {} summary", i)).unwrap();
            candidates.push(ContextCandidate { cid, relevance: 1.0 - i as f32 * 0.1 });
        }

        // Budget for ~3 L2 items
        let result = assemble(&loader, &candidates, 200);
        assert!(result.total_tokens <= 200);
        assert!(result.candidates_included <= 5);
    }

    #[test]
    fn test_budget_allocation_serializable() {
        let allocation = BudgetAllocation {
            items: vec![],
            total_tokens: 0,
            budget: 1000,
            candidates_considered: 5,
            candidates_included: 0,
        };
        let json = serde_json::to_string(&allocation).unwrap();
        let _: BudgetAllocation = serde_json::from_str(&json).unwrap();
    }
}
