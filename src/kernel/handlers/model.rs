//! Model hot-swap handlers.

use crate::api::semantic::{ApiRequest, ApiResponse};

impl super::super::AIKernel {
    pub(crate) fn handle_model(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::SwitchEmbeddingModel { model_type, model_id, python_path } => {
                match self.switch_embedding_model(&model_type, &model_id, python_path.as_deref()) {
                    Ok(resp) => {
                        let mut r = ApiResponse::ok();
                        r.model_switch = Some(resp);
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::SwitchLlmModel { backend, model, url } => {
                match self.switch_llm_model(&backend, &model, url.as_deref()) {
                    Ok(resp) => {
                        let mut r = ApiResponse::ok();
                        r.model_switch = Some(resp);
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::CheckModelHealth { model_type } => {
                let health = self.check_model_health(&model_type);
                let mut r = ApiResponse::ok();
                r.model_health = Some(health);
                r
            }
            _ => unreachable!("non-model request routed to handle_model"),
        }
    }
}
