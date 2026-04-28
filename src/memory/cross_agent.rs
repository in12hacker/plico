//! Cross-Agent Knowledge Distiller — automatically distill procedural memories
//! from one agent into shared skills available to all agents.
//!
//! When Agent A solves a problem and stores a Procedural memory, the OS detects
//! whether this experience is relevant to other agents' work patterns (via tag
//! overlap). If so, it produces a Shared-scope distilled version.
//!
//! - **No LLM**: Direct copy with Shared scope, tags union
//! - **With LLM**: Generalize agent-specific experience into universal skill description

use crate::memory::layered::{MemoryEntry, MemoryContent, MemoryType, MemoryTier, MemoryScope};
use uuid::Uuid;

/// Minimum tag overlap ratio to consider a procedural memory relevant to another agent.
const TAG_RELEVANCE_THRESHOLD: f64 = 0.3;

/// Result of a distillation attempt.
#[derive(Debug, Clone)]
pub struct DistillResult {
    pub source_id: String,
    pub source_agent: String,
    pub target_agents: Vec<String>,
    pub distilled_entry: MemoryEntry,
}

/// Check if a procedural memory from one agent is relevant to another agent's work.
///
/// Relevance is measured by tag overlap: if the fraction of shared tags exceeds
/// the threshold, the memory is considered relevant.
pub fn is_relevant_to_agent(
    procedural_entry: &MemoryEntry,
    other_agent_tags: &[String],
) -> bool {
    if procedural_entry.tags.is_empty() || other_agent_tags.is_empty() {
        return false;
    }
    let overlap = procedural_entry
        .tags
        .iter()
        .filter(|t| other_agent_tags.contains(t))
        .count();
    let ratio = overlap as f64 / procedural_entry.tags.len().min(other_agent_tags.len()) as f64;
    ratio >= TAG_RELEVANCE_THRESHOLD
}

/// Collect the unique tags from all memories of a given agent.
pub fn collect_agent_tags(entries: &[MemoryEntry], agent_id: &str) -> Vec<String> {
    let mut tags: Vec<String> = entries
        .iter()
        .filter(|e| e.agent_id == agent_id)
        .flat_map(|e| e.tags.clone())
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

/// Rule-based distillation: copy the procedural entry as Shared scope
/// with a union of all relevant agents' tags.
pub fn distill_to_shared(
    entry: &MemoryEntry,
    relevant_agent_tags: &[Vec<String>],
) -> MemoryEntry {
    let mut shared = entry.clone();
    shared.id = Uuid::new_v4().to_string();
    shared.scope = MemoryScope::Shared;
    shared.causal_parent = Some(entry.id.clone());

    for agent_tags in relevant_agent_tags {
        for tag in agent_tags {
            if !shared.tags.contains(tag) {
                shared.tags.push(tag.clone());
            }
        }
    }

    shared
}

/// LLM-enhanced distillation: generalize agent-specific experience into
/// a universal skill description.
pub fn distill_with_llm(
    entry: &MemoryEntry,
    relevant_agent_tags: &[Vec<String>],
    llm_fn: impl Fn(&str) -> Option<String>,
) -> MemoryEntry {
    let prompt = distillation_prompt(entry);
    let mut shared = distill_to_shared(entry, relevant_agent_tags);

    if let Some(response) = llm_fn(&prompt) {
        let text = response.trim().to_string();
        if !text.is_empty() {
            shared.content = MemoryContent::Text(text);
        }
    }

    shared
}

/// Build the LLM prompt for generalizing a procedural memory.
pub fn distillation_prompt(entry: &MemoryEntry) -> String {
    format!(
        "This is an agent's specific experience:\n\
         \"{}\"\n\n\
         Generalize this into a universal skill or procedure that any agent could use.\n\
         Remove agent-specific details but preserve the actionable steps.\n\
         Output ONLY the generalized skill description, nothing else.",
        entry.content.display()
    )
}

/// Run cross-agent distillation on a newly stored procedural memory.
///
/// Checks all other agents' tag profiles. If relevance exceeds the threshold
/// for any other agent, produces a Shared-scope distilled entry.
///
/// Returns `Some(DistillResult)` if distillation was triggered, `None` otherwise.
pub fn try_distill_for_sharing(
    new_procedural: &MemoryEntry,
    all_entries: &[MemoryEntry],
    llm_fn: impl Fn(&str) -> Option<String>,
) -> Option<DistillResult> {
    if new_procedural.memory_type != MemoryType::Procedural
        && new_procedural.tier != MemoryTier::Procedural
    {
        return None;
    }

    if new_procedural.scope == MemoryScope::Shared {
        return None;
    }

    let other_agents: Vec<String> = {
        let mut agents: Vec<String> = all_entries
            .iter()
            .map(|e| e.agent_id.clone())
            .filter(|a| a != &new_procedural.agent_id)
            .collect();
        agents.sort();
        agents.dedup();
        agents
    };

    if other_agents.is_empty() {
        return None;
    }

    let mut relevant_agents = Vec::new();
    let mut relevant_tags = Vec::new();

    for agent in &other_agents {
        let agent_tags = collect_agent_tags(all_entries, agent);
        if is_relevant_to_agent(new_procedural, &agent_tags) {
            relevant_agents.push(agent.clone());
            relevant_tags.push(agent_tags);
        }
    }

    if relevant_agents.is_empty() {
        return None;
    }

    let distilled = distill_with_llm(new_procedural, &relevant_tags, llm_fn);

    Some(DistillResult {
        source_id: new_procedural.id.clone(),
        source_agent: new_procedural.agent_id.clone(),
        target_agents: relevant_agents,
        distilled_entry: distilled,
    })
}

/// Privacy guard: ensure distilled entries never leak private content.
/// Returns true if the entry is safe to share.
pub fn is_safe_to_share(entry: &MemoryEntry) -> bool {
    let content = entry.content.display();
    let sensitive_patterns = ["password", "secret", "api_key", "token", "credential", "private_key"];
    !sensitive_patterns
        .iter()
        .any(|p| content.to_lowercase().contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_procedural(id: &str, agent: &str, content: &str, tags: Vec<String>) -> MemoryEntry {
        let mut e = MemoryEntry::ephemeral(agent, content);
        e.id = id.to_string();
        e.tier = MemoryTier::Procedural;
        e.memory_type = MemoryType::Procedural;
        e.tags = tags;
        e
    }

    fn make_entry(id: &str, agent: &str, content: &str, tags: Vec<String>) -> MemoryEntry {
        let mut e = MemoryEntry::ephemeral(agent, content);
        e.id = id.to_string();
        e.tags = tags;
        e
    }

    #[test]
    fn test_relevant_by_tag_overlap() {
        let proc = make_procedural(
            "p1", "agent-a", "deploy to staging",
            vec!["deploy".into(), "staging".into(), "ci".into()],
        );
        let other_tags = vec!["deploy".into(), "production".into()];
        assert!(is_relevant_to_agent(&proc, &other_tags));
    }

    #[test]
    fn test_not_relevant_no_overlap() {
        let proc = make_procedural(
            "p1", "agent-a", "data analysis",
            vec!["analytics".into(), "python".into()],
        );
        let other_tags = vec!["deploy".into(), "staging".into()];
        assert!(!is_relevant_to_agent(&proc, &other_tags));
    }

    #[test]
    fn test_not_relevant_empty_tags() {
        let proc = make_procedural("p1", "agent-a", "something", vec![]);
        let other_tags = vec!["deploy".into()];
        assert!(!is_relevant_to_agent(&proc, &other_tags));
    }

    #[test]
    fn test_distill_to_shared_changes_scope() {
        let proc = make_procedural(
            "p1", "agent-a", "deploy using git push",
            vec!["deploy".into()],
        );
        let distilled = distill_to_shared(&proc, &[vec!["ci".into()]]);
        assert_eq!(distilled.scope, MemoryScope::Shared);
        assert_ne!(distilled.id, proc.id);
        assert_eq!(distilled.causal_parent.as_deref(), Some("p1"));
        assert!(distilled.tags.contains(&"ci".to_string()));
    }

    #[test]
    fn test_distill_with_llm_generalizes() {
        let proc = make_procedural(
            "p1", "agent-a", "I deployed by running git push origin main",
            vec!["deploy".into()],
        );
        let distilled = distill_with_llm(&proc, &[], |_| {
            Some("Deploy by pushing to the main branch of the remote repository".to_string())
        });
        assert!(distilled.content.display().contains("Deploy by pushing"));
    }

    #[test]
    fn test_privacy_guard_blocks_sensitive() {
        let mut entry = make_procedural("p1", "agent-a", "use api_key=ABC123", vec![]);
        assert!(!is_safe_to_share(&entry));

        entry.content = MemoryContent::Text("deploy to staging".into());
        assert!(is_safe_to_share(&entry));
    }

    #[test]
    fn test_already_shared_not_distilled_again() {
        let mut proc = make_procedural(
            "p1", "agent-a", "shared skill",
            vec!["deploy".into()],
        );
        proc.scope = MemoryScope::Shared;

        let other = make_entry("e1", "agent-b", "other note", vec!["deploy".into()]);
        let result = try_distill_for_sharing(&proc, &[other], |_| None);
        assert!(result.is_none(), "already shared should not be distilled again");
    }

    #[test]
    fn test_full_distillation_pipeline() {
        let proc = make_procedural(
            "p1", "agent-a", "run tests with cargo test",
            vec!["testing".into(), "rust".into()],
        );
        let other = make_entry(
            "e1", "agent-b", "I need help with rust testing",
            vec!["testing".into(), "help".into()],
        );

        let result = try_distill_for_sharing(&proc, &[proc.clone(), other], |_| None);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.source_agent, "agent-a");
        assert!(r.target_agents.contains(&"agent-b".to_string()));
        assert_eq!(r.distilled_entry.scope, MemoryScope::Shared);
    }

    #[test]
    fn test_no_distillation_when_no_other_agents() {
        let proc = make_procedural(
            "p1", "agent-a", "run tests",
            vec!["testing".into()],
        );
        let result = try_distill_for_sharing(&proc, &[proc.clone()], |_| None);
        assert!(result.is_none());
    }
}
