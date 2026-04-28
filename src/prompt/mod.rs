//! Prompt Registry — versioned, overridable prompt templates.
//!
//! All LLM prompts in Plico flow through this registry. Each prompt is a named
//! template with typed placeholder variables. Defaults are compiled-in (zero-config),
//! but can be overridden at global or per-agent scope at runtime.
//!
//! Resolution order: agent override > global override > compiled default.

mod registry;
mod defaults;

pub use registry::{PromptRegistry, PromptTemplate, RenderError};
pub use defaults::register_defaults;
