//! Compiled-in default prompt templates.
//!
//! Each prompt is registered once at kernel startup and accessed via the registry.

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
             - temporal: queries ABOUT TIME or DATES (\"When did X happen?\", \"How many days between X and Y?\", \"Which event happened first?\")\n\
             - multi_hop: requires connecting multiple pieces of information from different sources (\"Why did X cause Y?\", \"What is the relationship between X and Y?\")\n\
             - preference: asking for recommendations, suggestions, opinions, or personal preferences (\"Can you recommend X?\", \"Suggest some Y\", \"What does user prefer?\", \"favorite\", \"What should I Z?\")\n\
             - aggregation: requires counting or listing MULTIPLE distinct items across many entries (\"List all X\", \"How many total Y?\", \"Give me an overview of all Z\")\n\n\
             IMPORTANT: \"Can you recommend/suggest X?\" is PREFERENCE (asking for personalized advice), \
             NOT aggregation (counting items). \"How many X in total?\" is AGGREGATION.\n\n\
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

    // ── Intent-specific answer generation prompts ──

    reg.register_default(
        PromptTemplate::new(
            "answer_factual",
            "You are answering a factual question using conversation memories.\n\n\
             Retrieved memories:\n{{context}}\n\n\
             Question: {{question}}\n\n\
             Instructions:\n\
             - Find the specific fact, name, number, location, or detail asked about\n\
             - Answer with the exact information from the memories\n\
             - If multiple memories mention different facts, pick the most relevant one\n\
             - Be concise but complete — include all relevant details\n\
             - If the information is not in the memories, say \"I don't know\"",
            &["context", "question"],
        ).with_max_tokens(200),
    );

    reg.register_default(
        PromptTemplate::new(
            "answer_temporal",
            "You are answering a time-related question using conversation memories.\n\n\
             Retrieved memories:\n{{context}}\n\n\
             Question: {{question}}\n\n\
             Instructions:\n\
             - Look for DATES, TIME PERIODS, and SEQUENCE information in the memories\n\
             - For \"how many days/weeks/months between X and Y\": find both dates and calculate\n\
             - For \"which happened first\": compare the dates of both events\n\
             - For \"when did X happen\": find the specific date associated with event X\n\
             - Pay attention to date formats like [YYYY-MM-DD] at the start of each memory\n\
             - If the memories don't contain enough date information, say \"I don't know\"",
            &["context", "question"],
        ).with_max_tokens(200),
    );

    reg.register_default(
        PromptTemplate::new(
            "answer_preference",
            "You are answering a question about user preferences or recommendations using conversation memories.\n\n\
             Retrieved memories:\n{{context}}\n\n\
             Question: {{question}}\n\n\
             Instructions:\n\
             - Look for PATTERNS in what the user mentioned enjoying, preferring, or recommending\n\
             - Infer preferences from conversation context (e.g., if they mention buying from a store often, that's a preference)\n\
             - For \"recommend/suggest\" questions: base recommendations on the user's known interests and past mentions\n\
             - Consider both explicit statements (\"I like X\") and implicit signals (frequently mentioned topics)\n\
             - Provide specific, personalized suggestions based on the memories\n\
             - If the memories don't contain enough preference information, say \"I don't know\"",
            &["context", "question"],
        ).with_max_tokens(200),
    );

    reg.register_default(
        PromptTemplate::new(
            "answer_multi_hop",
            "You are answering a question that requires connecting multiple pieces of information from different conversation memories.\n\n\
             Retrieved memories:\n{{context}}\n\n\
             Question: {{question}}\n\n\
             Instructions:\n\
             - This question requires REASONING across multiple memories\n\
             - Identify which memories contain relevant pieces of the answer\n\
             - Connect the information: memory A may describe an event, memory B may explain its cause\n\
             - For \"why\" questions: find the cause-effect chain across memories\n\
             - For \"relationship\" questions: identify how entities are connected across different conversations\n\
             - State your reasoning briefly, then give the answer\n\
             - If the memories don't contain enough connecting information, say \"I don't know\"",
            &["context", "question"],
        ).with_max_tokens(200),
    );

    reg.register_default(
        PromptTemplate::new(
            "answer_aggregation",
            "You are answering a question that requires counting or listing multiple items from conversation memories.\n\n\
             Retrieved memories:\n{{context}}\n\n\
             Question: {{question}}\n\n\
             Instructions:\n\
             - Scan ALL memories for relevant items, events, or data points\n\
             - For \"how many\" questions: count EACH DISTINCT item mentioned across all memories\n\
             - For \"list all\" questions: enumerate every relevant item found\n\
             - Be EXHAUSTIVE — don't miss items mentioned in later memories\n\
             - If memories mention the same item multiple times, count it only once\n\
             - State the count/list clearly, then briefly cite which memories support it\n\
             - If the memories don't contain enough information, say \"I don't know\"",
            &["context", "question"],
        ).with_max_tokens(200),
    );

    reg.register_default(
        PromptTemplate::new(
            "query_decomposition",
            "The following question requires information from multiple conversation sessions. \
             Break it into 2-3 simpler sub-questions that can each be answered from a single session or context.\n\n\
             Original question: {{question}}\n\n\
             Output one sub-question per line, nothing else. Each sub-question should be self-contained \
             and searchable independently.",
            &["question"],
        ).with_max_tokens(200),
    );

}
