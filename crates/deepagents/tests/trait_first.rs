use std::sync::Arc;

use async_trait::async_trait;

use deepagents::backends::protocol::{
    BackendError, BackendErrorCode, FilesystemBackend, SandboxBackend,
};
use deepagents::types::{EditResult, ExecResult, FileInfo, GrepMatch, WriteResult};

#[derive(Clone)]
struct MockBackend;

#[async_trait]
impl deepagents::backends::protocol::Backend for MockBackend {}

#[async_trait]
impl FilesystemBackend for MockBackend {
    async fn ls_info(&self, path: &str) -> Result<Vec<FileInfo>, BackendError> {
        Ok(vec![FileInfo {
            path: format!("{path}/a.txt"),
            is_dir: Some(false),
            size: Some(3),
            modified_at: None,
        }])
    }

    async fn read(
        &self,
        _file_path: &str,
        _offset: usize,
        _limit: usize,
    ) -> Result<String, BackendError> {
        Ok("     1→hello\n".to_string())
    }

    async fn read_bytes(
        &self,
        _file_path: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, BackendError> {
        if max_bytes == 0 {
            return Err(BackendError::new(BackendErrorCode::TooLarge, "too_large"));
        }
        Ok(vec![0u8; 1])
    }

    async fn write_file(
        &self,
        file_path: &str,
        _content: &str,
    ) -> Result<WriteResult, BackendError> {
        Ok(WriteResult {
            error: None,
            path: Some(file_path.to_string()),
        })
    }

    async fn edit_file(
        &self,
        file_path: &str,
        _old_string: &str,
        _new_string: &str,
    ) -> Result<EditResult, BackendError> {
        Ok(EditResult {
            error: None,
            path: Some(file_path.to_string()),
            occurrences: Some(1),
        })
    }

    async fn glob(&self, _pattern: &str) -> Result<Vec<String>, BackendError> {
        Ok(vec!["/root/a.txt".to_string()])
    }

    async fn grep(
        &self,
        _pattern: &str,
        _path: Option<&str>,
        _glob: Option<&str>,
    ) -> Result<Vec<GrepMatch>, BackendError> {
        Ok(vec![GrepMatch {
            path: "/root/a.txt".to_string(),
            line: 1,
            text: "needle".to_string(),
        }])
    }
}

#[async_trait]
impl SandboxBackend for MockBackend {
    async fn execute(
        &self,
        _command: &str,
        _timeout_secs: Option<u64>,
    ) -> Result<ExecResult, BackendError> {
        Ok(ExecResult {
            exit_code: 0,
            output: "ok".to_string(),
            truncated: None,
        })
    }
}

#[tokio::test]
async fn agent_can_use_third_party_backend_via_traits() {
    let backend: Arc<dyn SandboxBackend> = Arc::new(MockBackend);
    let agent = deepagents::create_deep_agent_with_backend(backend);

    let out = agent
        .call_tool(
            "ls",
            serde_json::json!({
              "path": "/root"
            }),
        )
        .await
        .unwrap();

    let arr = out.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert!(arr[0]
        .get("path")
        .unwrap()
        .as_str()
        .unwrap()
        .ends_with("/a.txt"));
}
