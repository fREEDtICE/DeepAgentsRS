use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("backend operation failed: {0}")]
    Other(String),
}

#[async_trait]
pub trait Backend: Send + Sync {
    async fn healthcheck(&self) -> Result<(), BackendError> {
        Ok(())
    }
}

#[async_trait]
pub trait FilesystemBackend: Backend {
    async fn ls_info(&self, path: &str) -> Result<Vec<crate::types::FileInfo>, BackendError>;
    async fn create_dir_all(&self, _dir_path: &str) -> Result<(), BackendError> {
        Err(BackendError::Other("not_supported".to_string()))
    }
    async fn read(
        &self,
        file_path: &str,
        offset: usize,
        limit: usize,
    ) -> Result<String, BackendError>;
    async fn read_bytes(&self, file_path: &str, max_bytes: usize) -> Result<Vec<u8>, BackendError>;
    async fn write_file(
        &self,
        file_path: &str,
        content: &str,
    ) -> Result<crate::types::WriteResult, BackendError>;
    async fn delete_file(
        &self,
        _file_path: &str,
    ) -> Result<crate::types::DeleteResult, BackendError> {
        Ok(crate::types::DeleteResult {
            error: Some("not_supported".to_string()),
            path: None,
        })
    }
    async fn edit_file(
        &self,
        file_path: &str,
        old_string: &str,
        new_string: &str,
    ) -> Result<crate::types::EditResult, BackendError>;
    async fn glob(&self, pattern: &str) -> Result<Vec<String>, BackendError>;
    async fn grep(
        &self,
        pattern: &str,
        path: Option<&str>,
        glob: Option<&str>,
    ) -> Result<Vec<crate::types::GrepMatch>, BackendError>;
}

#[async_trait]
pub trait SandboxBackend: FilesystemBackend {
    async fn execute(
        &self,
        command: &str,
        timeout_secs: Option<u64>,
    ) -> Result<crate::types::ExecResult, BackendError>;
}
