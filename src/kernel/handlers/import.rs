//! File import handler — reads local files and stores them in CAS with chunking.

use crate::api::semantic::{ApiRequest, ApiResponse, ImportFileResult};
use crate::fs::chunking::ChunkingMode;

impl crate::kernel::AIKernel {
    pub(crate) fn handle_import(&self, req: ApiRequest) -> ApiResponse {
        let ApiRequest::ImportFiles { paths, agent_id, tags, chunking, tenant_id } = req else {
            return ApiResponse::error("expected ImportFiles request");
        };

        self.ensure_agent_registered(&agent_id);

        let mode = match chunking.as_deref() {
            Some("markdown") | Some("md") => ChunkingMode::Markdown,
            Some("semantic") => ChunkingMode::Semantic,
            Some("fixed") => ChunkingMode::Fixed,
            Some("none") => ChunkingMode::None,
            _ => ChunkingMode::Markdown, // auto-detect defaults to markdown for .md files
        };

        let mut results = Vec::with_capacity(paths.len());

        for path_str in &paths {
            let path = std::path::Path::new(path_str);

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(e) => {
                    results.push(ImportFileResult {
                        path: path_str.clone(),
                        cid: None,
                        chunks: 0,
                        ok: false,
                        error: Some(format!("read error: {e}")),
                    });
                    continue;
                }
            };

            let file_mode = if chunking.is_none() {
                match path.extension().and_then(|e| e.to_str()) {
                    Some("md") | Some("markdown") => ChunkingMode::Markdown,
                    _ => mode,
                }
            } else {
                mode
            };

            let filename = path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");

            let mut file_tags = tags.clone();
            file_tags.push(format!("source:{}", filename));
            if file_tags.iter().all(|t| !t.starts_with("type:")) {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("text");
                file_tags.push(format!("type:{ext}"));
            }

            let create_resp = self.handle_api_request(ApiRequest::Create {
                api_version: None,
                content: content.clone(),
                content_encoding: Default::default(),
                tags: file_tags,
                agent_id: agent_id.clone(),
                tenant_id: tenant_id.clone(),
                agent_token: None,
                intent: None,
            });

            let cid = create_resp.cid.clone();

            let chunk_count = if file_mode != ChunkingMode::None {
                let chunks = crate::fs::chunking::chunk_document(&content, file_mode, None);
                for (ci, chunk) in chunks.iter().enumerate() {
                    let chunk_tags = vec![
                        format!("parent_cid:{}", cid.as_deref().unwrap_or("unknown")),
                        format!("chunk_idx:{ci}"),
                        "is_chunk:true".to_string(),
                        format!("source:{filename}"),
                    ];
                    let _ = self.handle_api_request(ApiRequest::Create {
                        api_version: None,
                        content: chunk.text.clone(),
                        content_encoding: Default::default(),
                        tags: chunk_tags,
                        agent_id: agent_id.clone(),
                        tenant_id: tenant_id.clone(),
                        agent_token: None,
                        intent: None,
                    });
                }
                chunks.len()
            } else {
                0
            };

            results.push(ImportFileResult {
                path: path_str.clone(),
                cid,
                chunks: chunk_count,
                ok: create_resp.ok,
                error: create_resp.error,
            });
        }

        let total = results.len();
        let ok_count = results.iter().filter(|r| r.ok).count();
        let mut resp = ApiResponse::ok();
        resp.message = Some(format!("Imported {ok_count}/{total} files"));
        resp.import_results = Some(results);
        resp
    }
}
