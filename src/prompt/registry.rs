//! Core PromptRegistry: stores templates, resolves overrides, renders prompts.

use std::collections::HashMap;
use std::sync::RwLock;

/// A versioned prompt template with named placeholders.
#[derive(Debug, Clone)]
pub struct PromptTemplate {
    pub name: String,
    pub version: u32,
    pub template: String,
    pub variables: Vec<String>,
    pub max_tokens_hint: Option<usize>,
}

impl PromptTemplate {
    pub fn new(name: &str, template: &str, variables: &[&str]) -> Self {
        Self {
            name: name.to_string(),
            version: 1,
            template: template.to_string(),
            variables: variables.iter().map(|s| s.to_string()).collect(),
            max_tokens_hint: None,
        }
    }

    pub fn with_version(mut self, version: u32) -> Self {
        self.version = version;
        self
    }

    pub fn with_max_tokens(mut self, hint: usize) -> Self {
        self.max_tokens_hint = Some(hint);
        self
    }
}

/// Error returned when rendering fails.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("prompt '{0}' not found in registry")]
    NotFound(String),
    #[error("missing variable '{var}' for prompt '{prompt}'")]
    MissingVariable { prompt: String, var: String },
}

/// Override key: (prompt_name, optional agent_id).
type OverrideKey = (String, Option<String>);

/// Thread-safe prompt registry with compiled defaults + runtime overrides.
pub struct PromptRegistry {
    defaults: HashMap<String, PromptTemplate>,
    overrides: RwLock<HashMap<OverrideKey, PromptTemplate>>,
}

impl Default for PromptRegistry {
    fn default() -> Self {
        Self {
            defaults: HashMap::new(),
            overrides: RwLock::new(HashMap::new()),
        }
    }
}

impl PromptRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a compiled-in default template.
    pub fn register_default(&mut self, template: PromptTemplate) {
        self.defaults.insert(template.name.clone(), template);
    }

    /// Set a runtime override (global if agent_id is None, per-agent otherwise).
    pub fn set_override(&self, name: &str, template: PromptTemplate, agent_id: Option<&str>) {
        let key = (name.to_string(), agent_id.map(|s| s.to_string()));
        self.overrides.write().unwrap().insert(key, template);
    }

    /// Remove a runtime override.
    pub fn remove_override(&self, name: &str, agent_id: Option<&str>) {
        let key = (name.to_string(), agent_id.map(|s| s.to_string()));
        self.overrides.write().unwrap().remove(&key);
    }

    /// Resolve the effective template: agent override > global override > default.
    pub fn resolve(&self, name: &str, agent_id: Option<&str>) -> Option<PromptTemplate> {
        let overrides = self.overrides.read().unwrap();

        if let Some(aid) = agent_id {
            let agent_key = (name.to_string(), Some(aid.to_string()));
            if let Some(t) = overrides.get(&agent_key) {
                return Some(t.clone());
            }
        }

        let global_key = (name.to_string(), None);
        if let Some(t) = overrides.get(&global_key) {
            return Some(t.clone());
        }

        self.defaults.get(name).cloned()
    }

    /// Render a prompt by resolving the template and substituting variables.
    pub fn render(
        &self,
        name: &str,
        vars: &HashMap<&str, String>,
        agent_id: Option<&str>,
    ) -> Result<String, RenderError> {
        let template = self.resolve(name, agent_id)
            .ok_or_else(|| RenderError::NotFound(name.to_string()))?;

        let mut result = template.template.clone();
        for var_name in &template.variables {
            let placeholder = format!("{{{{{}}}}}", var_name);
            if result.contains(&placeholder) {
                let value = vars.get(var_name.as_str())
                    .ok_or_else(|| RenderError::MissingVariable {
                        prompt: name.to_string(),
                        var: var_name.clone(),
                    })?;
                result = result.replace(&placeholder, value);
            }
        }
        Ok(result)
    }

    /// List all registered prompt names (defaults + overrides).
    pub fn list_prompts(&self) -> Vec<String> {
        let mut names: Vec<String> = self.defaults.keys().cloned().collect();
        names.sort();
        names
    }

    /// Get info about a prompt (for API introspection).
    pub fn get_info(&self, name: &str, agent_id: Option<&str>) -> Option<PromptInfo> {
        let template = self.resolve(name, agent_id)?;
        let is_override = {
            let overrides = self.overrides.read().unwrap();
            if let Some(aid) = agent_id {
                overrides.contains_key(&(name.to_string(), Some(aid.to_string())))
            } else {
                overrides.contains_key(&(name.to_string(), None))
            }
        };
        Some(PromptInfo {
            name: template.name,
            version: template.version,
            variables: template.variables,
            max_tokens_hint: template.max_tokens_hint,
            is_override,
        })
    }
}

/// Serializable info about a prompt template.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PromptInfo {
    pub name: String,
    pub version: u32,
    pub variables: Vec<String>,
    pub max_tokens_hint: Option<usize>,
    pub is_override: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::register_defaults;

    #[test]
    fn test_register_and_render() {
        let mut reg = PromptRegistry::new();
        reg.register_default(PromptTemplate::new(
            "test",
            "Hello {{name}}, you are {{role}}.",
            &["name", "role"],
        ));
        let mut vars = HashMap::new();
        vars.insert("name", "Alice".to_string());
        vars.insert("role", "engineer".to_string());
        let result = reg.render("test", &vars, None).unwrap();
        assert_eq!(result, "Hello Alice, you are engineer.");
    }

    #[test]
    fn test_override_priority() {
        let mut reg = PromptRegistry::new();
        reg.register_default(PromptTemplate::new("greet", "Default {{name}}", &["name"]));
        reg.set_override("greet", PromptTemplate::new("greet", "Global {{name}}", &["name"]), None);
        reg.set_override("greet", PromptTemplate::new("greet", "Agent {{name}}", &["name"]), Some("agent-1"));

        let mut vars = HashMap::new();
        vars.insert("name", "X".to_string());

        let default_render = reg.render("greet", &vars, Some("agent-2")).unwrap();
        assert_eq!(default_render, "Global X");

        let agent_render = reg.render("greet", &vars, Some("agent-1")).unwrap();
        assert_eq!(agent_render, "Agent X");
    }

    #[test]
    fn test_missing_prompt() {
        let reg = PromptRegistry::new();
        let vars = HashMap::new();
        assert!(matches!(reg.render("nonexistent", &vars, None), Err(RenderError::NotFound(_))));
    }

    #[test]
    fn test_defaults_register() {
        let mut reg = PromptRegistry::new();
        register_defaults(&mut reg);
        let prompts = reg.list_prompts();
        assert!(prompts.contains(&"contradiction".to_string()));
        assert!(prompts.contains(&"summarization".to_string()));
        assert!(prompts.contains(&"intent_classification".to_string()));
    }
}
