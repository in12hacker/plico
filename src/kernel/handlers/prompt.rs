//! Prompt Registry handlers (v31).

use crate::api::semantic::{ApiRequest, ApiResponse};
use crate::prompt::PromptTemplate;

impl super::super::AIKernel {
    pub(crate) fn handle_prompt(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::ListPrompts => {
                let names = self.prompt_registry.list_prompts();
                let mut r = ApiResponse::ok();
                r.data = Some(serde_json::to_string(&names).unwrap_or_default());
                r
            }

            ApiRequest::GetPromptInfo { name, agent_id } => {
                match self.prompt_registry.get_info(&name, agent_id.as_deref()) {
                    Some(info) => {
                        let mut r = ApiResponse::ok();
                        r.data = Some(serde_json::to_string(&info).unwrap_or_default());
                        r
                    }
                    None => ApiResponse::error(format!("prompt '{}' not found", name)),
                }
            }

            ApiRequest::SetPromptOverride { name, template, variables, agent_id } => {
                let var_refs: Vec<&str> = variables.iter().map(|s| s.as_str()).collect();
                let tpl = PromptTemplate::new(&name, &template, &var_refs);
                self.prompt_registry.set_override(&name, tpl, agent_id.as_deref());
                ApiResponse::ok()
            }

            ApiRequest::RemovePromptOverride { name, agent_id } => {
                self.prompt_registry.remove_override(&name, agent_id.as_deref());
                ApiResponse::ok()
            }

            _ => ApiResponse::error("unexpected request in handle_prompt"),
        }
    }
}
