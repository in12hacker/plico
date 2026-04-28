//! Compiled-in default prompt templates.
//!
//! Each prompt is registered once at kernel startup. The original prompt
//! functions still exist but delegate to the registry for backward compat.

use super::registry::{PromptRegistry, PromptTemplate};

pub fn register_defaults(reg: &mut PromptRegistry) {
    reg.register_default(
        PromptTemplate::new(
            "contradiction",
            "Two statements are contradictory if they assign DIFFERENT values to the SAME attribute \
             (e.g. different dates, versions, names, numbers, or choices for the same subject). \
             Even if phrased differently ('required' vs 'recommended'), conflicting specifics count.\n\n\
             Statement A: {{old_content}}\n\
             Statement B: {{new_content}}\n\n\
             Do these two statements contradict each other? Answer ONLY 'yes' or 'no'.\n\
             Answer:",
            &["old_content", "new_content"],
        ).with_max_tokens(8),
    );

    reg.register_default(
        PromptTemplate::new(
            "summarization",
            "Compress these memories into the SHORTEST possible summary (fewer words than the input). \
             Keep only key facts, decisions, and action items. Remove filler and redundancy. \
             Output ONLY the summary, nothing else.\n\n\
             Memories:\n{{entries_text}}\n\n\
             Summary:",
            &["entries_text"],
        ).with_max_tokens(512),
    );

    reg.register_default(
        PromptTemplate::new(
            "intent_classification",
            "Classify the following query into exactly ONE category. \
             Output ONLY the category name, nothing else.\n\n\
             Categories:\n\
             - factual: looking up a single known fact or number (\"What is X?\", \"How many Y per day?\", \"Who did Z?\")\n\
             - temporal: time-related queries (\"When did\", \"before\", \"after\", \"last week\")\n\
             - multi_hop: requires connecting multiple pieces of information (\"Why did X cause Y?\")\n\
             - preference: about preferences/opinions (\"What does user prefer?\", \"favorite\")\n\
             - aggregation: requires listing or summarizing MULTIPLE distinct items (\"List all X\", \"Summarize all Y\")\n\n\
             Query: {{query}}\n\n\
             Category:",
            &["query"],
        ).with_max_tokens(16),
    );

    reg.register_default(
        PromptTemplate::new(
            "foresight",
            "An AI agent has declared the following intent:\n\
             \"{{intent_description}}\"\n\n\
             Recent memory context:\n{{recent_memories}}\n\n\
             Predict what information the agent will most likely need next.\n\
             List up to 5 topics/keywords, one per line.",
            &["intent_description", "recent_memories"],
        ).with_max_tokens(256),
    );

    reg.register_default(
        PromptTemplate::new(
            "split",
            "This memory is used by multiple intent types:\n\
             Content: \"{{content}}\"\n\
             Intent hits:\n{{intent_hits}}\n\n\
             Split this into separate memories, one per intent type.\n\
             Output format: one line per split, format: TYPE|CONTENT\n\
             Types: episodic, semantic, procedural",
            &["content", "intent_hits"],
        ).with_max_tokens(512),
    );

    reg.register_default(
        PromptTemplate::new(
            "merge",
            "Merge these two similar memories into one concise memory:\n\
             Memory A: \"{{memory_a}}\"\n\
             Memory B: \"{{memory_b}}\"\n\n\
             Output ONLY the merged content, nothing else.",
            &["memory_a", "memory_b"],
        ).with_max_tokens(256),
    );

    reg.register_default(
        PromptTemplate::new(
            "update",
            "Two memories contain contradictory information:\n\
             Old: \"{{old_content}}\"\n\
             New: \"{{new_content}}\"\n\n\
             Fuse them into a single accurate memory. Output ONLY the fused content.",
            &["old_content", "new_content"],
        ).with_max_tokens(256),
    );

    reg.register_default(
        PromptTemplate::new(
            "rewrite",
            "Rewrite the following query to improve search retrieval. \
             Add synonyms, expand abbreviations, and add helpful context. \
             Output ONLY the rewritten query, nothing else.\n\n\
             Original query: {{query}}\n\n\
             Rewritten query:",
            &["query"],
        ).with_max_tokens(128),
    );

    reg.register_default(
        PromptTemplate::new(
            "causal_context",
            "Causal chain (oldest → newest):\n{{causal_chain}}\n\
             → Current [{{target_id}}]: {{target_content}}",
            &["causal_chain", "target_id", "target_content"],
        ),
    );

    reg.register_default(
        PromptTemplate::new(
            "cross_agent_distillation",
            "This is an agent's specific experience:\n\
             \"{{content}}\"\n\n\
             Generalize this into a universal skill or procedure that any agent could use.\n\
             Remove agent-specific details but preserve the actionable steps.\n\
             Output ONLY the generalized skill description, nothing else.",
            &["content"],
        ).with_max_tokens(256),
    );

    reg.register_default(
        PromptTemplate::new(
            "kg_extract",
            "Extract entities, relationships, and user preferences from the following text.\n\
             Output ONLY valid JSON with two arrays: \"triples\" and \"preferences\".\n\
             Format: {\"triples\": [{\"subject\":\"...\",\"predicate\":\"...\",\"object\":\"...\",\"type\":\"...\"}], \
             \"preferences\": [{\"category\":\"...\",\"preference\":\"...\",\"confidence\":0.8}]}\n\
             Valid triple types: causes, follows, mentions, part_of, related_to, has_participant, has_fact.\n\
             Preference categories: topic, style, tool, language, domain, format.\n\
             Rules:\n\
             - lowercase all values\n\
             - use concise predicates (1-3 words)\n\
             - replace pronouns with entity names\n\
             - extract temporal events with \"follows\" or \"causes\" relations\n\
             - extract preferences only when the user clearly states or implies a preference\n\n\
             Text: {{text}}",
            &["text"],
        ).with_max_tokens(1024),
    );

    reg.register_default(
        PromptTemplate::new(
            "summarizer_l0_system",
            "You are a precise text summarizer. Respond with only the summary — no preamble, no quotes, no explanation. Output 2-3 sentences maximum.",
            &[],
        ),
    );

    reg.register_default(
        PromptTemplate::new(
            "summarizer_l0_user",
            "Summarize this briefly:\n\n{{content}}",
            &["content"],
        ).with_max_tokens(128),
    );

    reg.register_default(
        PromptTemplate::new(
            "summarizer_l1_system",
            "You are a detailed text summarizer. Respond with only the summary — no preamble, no quotes, no explanation. Capture the key points in 1-2 paragraphs.",
            &[],
        ),
    );

    reg.register_default(
        PromptTemplate::new(
            "summarizer_l1_user",
            "Summarize this in detail:\n\n{{content}}",
            &["content"],
        ).with_max_tokens(512),
    );
}
