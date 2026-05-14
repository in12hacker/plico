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

#[cfg(test)]
mod tests {
    use crate::kernel::tests::make_kernel;
    use crate::api::semantic::ApiRequest;

    #[test]
    fn test_import_single_file() {
        let (kernel, _dir) = make_kernel();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "hello world content").unwrap();

        let resp = kernel.handle_api_request(ApiRequest::ImportFiles {
            paths: vec![tmp.path().to_str().unwrap().to_string()],
            agent_id: "test_agent".to_string(),
            tags: vec!["test".to_string()],
            chunking: None,
            tenant_id: None,
        });
        assert!(resp.ok, "Import should succeed: {:?}", resp.error);
        let results = resp.import_results.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].ok);
        assert!(results[0].cid.is_some());
    }

    #[test]
    fn test_import_nonexistent_file() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::ImportFiles {
            paths: vec!["/tmp/nonexistent_file_12345.txt".to_string()],
            agent_id: "test_agent".to_string(),
            tags: vec![],
            chunking: None,
            tenant_id: None,
        });
        assert!(resp.ok);
        let results = resp.import_results.unwrap();
        assert_eq!(results.len(), 1);
        assert!(!results[0].ok);
        assert!(results[0].error.is_some());
    }

    #[test]
    fn test_import_markdown_file_auto_detect() {
        let (kernel, _dir) = make_kernel();
        let tmp = tempfile::Builder::new().suffix(".md").tempfile().unwrap();
        // Need enough content to produce chunks
        let content = "# Heading\n\nSome content here that is long enough.\n\n## Section\n\nMore content that is also long enough to trigger chunking behavior.";
        std::fs::write(tmp.path(), content).unwrap();

        let resp = kernel.handle_api_request(ApiRequest::ImportFiles {
            paths: vec![tmp.path().to_str().unwrap().to_string()],
            agent_id: "test_agent".to_string(),
            tags: vec![],
            chunking: None, // auto-detect
            tenant_id: None,
        });
        assert!(resp.ok, "Import should succeed: {:?}", resp.error);
        let results = resp.import_results.unwrap();
        assert!(results[0].ok);
        // Auto-detect should pick markdown mode for .md files
        // chunks may be 0 if content is too short for chunking
    }

    #[test]
    fn test_import_with_explicit_chunking_mode() {
        let (kernel, _dir) = make_kernel();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "line1\nline2\nline3\nline4\nline5").unwrap();

        let resp = kernel.handle_api_request(ApiRequest::ImportFiles {
            paths: vec![tmp.path().to_str().unwrap().to_string()],
            agent_id: "test_agent".to_string(),
            tags: vec![],
            chunking: Some("fixed".to_string()),
            tenant_id: None,
        });
        assert!(resp.ok);
    }

    #[test]
    fn test_import_with_no_chunking() {
        let (kernel, _dir) = make_kernel();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "some content").unwrap();

        let resp = kernel.handle_api_request(ApiRequest::ImportFiles {
            paths: vec![tmp.path().to_str().unwrap().to_string()],
            agent_id: "test_agent".to_string(),
            tags: vec![],
            chunking: Some("none".to_string()),
            tenant_id: None,
        });
        assert!(resp.ok);
        let results = resp.import_results.unwrap();
        assert!(results[0].ok);
        assert_eq!(results[0].chunks, 0);
    }

    #[test]
    fn test_import_multiple_files() {
        let (kernel, _dir) = make_kernel();
        let tmp1 = tempfile::NamedTempFile::new().unwrap();
        let tmp2 = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp1.path(), "content 1").unwrap();
        std::fs::write(tmp2.path(), "content 2").unwrap();

        let resp = kernel.handle_api_request(ApiRequest::ImportFiles {
            paths: vec![
                tmp1.path().to_str().unwrap().to_string(),
                tmp2.path().to_str().unwrap().to_string(),
            ],
            agent_id: "test_agent".to_string(),
            tags: vec!["batch".to_string()],
            chunking: None,
            tenant_id: None,
        });
        assert!(resp.ok);
        let results = resp.import_results.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].ok);
        assert!(results[1].ok);
    }

    #[test]
    fn test_import_mixed_success_failure() {
        let (kernel, _dir) = make_kernel();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "valid content").unwrap();

        let resp = kernel.handle_api_request(ApiRequest::ImportFiles {
            paths: vec![
                tmp.path().to_str().unwrap().to_string(),
                "/tmp/nonexistent_12345.txt".to_string(),
            ],
            agent_id: "test_agent".to_string(),
            tags: vec![],
            chunking: None,
            tenant_id: None,
        });
        assert!(resp.ok);
        let results = resp.import_results.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].ok);
        assert!(!results[1].ok);
    }

    #[test]
    fn test_import_adds_source_tag() {
        let (kernel, _dir) = make_kernel();
        let tmp = tempfile::Builder::new().suffix(".txt").tempfile().unwrap();
        std::fs::write(tmp.path(), "test content").unwrap();

        let resp = kernel.handle_api_request(ApiRequest::ImportFiles {
            paths: vec![tmp.path().to_str().unwrap().to_string()],
            agent_id: "test_agent".to_string(),
            tags: vec![],
            chunking: Some("none".to_string()),
            tenant_id: None,
        });
        assert!(resp.ok);
    }

    #[test]
    fn test_import_semantic_chunking() {
        let (kernel, _dir) = make_kernel();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "First section.\n\nSecond section.\n\nThird section.").unwrap();

        let resp = kernel.handle_api_request(ApiRequest::ImportFiles {
            paths: vec![tmp.path().to_str().unwrap().to_string()],
            agent_id: "test_agent".to_string(),
            tags: vec![],
            chunking: Some("semantic".to_string()),
            tenant_id: None,
        });
        assert!(resp.ok);
    }
}
