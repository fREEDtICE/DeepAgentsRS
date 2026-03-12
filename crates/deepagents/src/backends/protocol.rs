use async_trait::async_trait;
use std::fmt::{Display, Formatter};
use thiserror::Error;

/// 后端统一错误类型。
///
/// 设计要点：
/// - `code` 用于机器可读的错误分类（便于上层映射到提示/重试策略）。
/// - `message` 用于人类可读的简短说明（尽量稳定）。
/// - `source` 保留底层错误（用于调试/链路追踪），但不强制暴露给最终用户。
#[derive(Debug, Error)]
#[error("{code}: {message}")]
pub struct BackendError {
    pub code: BackendErrorCode,
    pub message: String,
    #[source]
    pub source: Option<anyhow::Error>,
}

/// 错误分类码（稳定的枚举集合）。
///
/// 该枚举用于跨不同后端实现对齐语义：同一种失败应尽量映射到同一个 `BackendErrorCode`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendErrorCode {
    NotSupported,
    InvalidInput,
    InvalidPath,
    PermissionDenied,
    FileNotFound,
    IsDirectory,
    TooLarge,
    NoMatch,
    Timeout,
    CommandNotAllowed,
    IoError,
    Unknown,
}

impl BackendErrorCode {
    /// 返回稳定的机器可读字符串，用于日志/协议传输/前端展示 key。
    pub fn as_str(&self) -> &'static str {
        match self {
            BackendErrorCode::NotSupported => "not_supported",
            BackendErrorCode::InvalidInput => "invalid_input",
            BackendErrorCode::InvalidPath => "invalid_path",
            BackendErrorCode::PermissionDenied => "permission_denied",
            BackendErrorCode::FileNotFound => "file_not_found",
            BackendErrorCode::IsDirectory => "is_directory",
            BackendErrorCode::TooLarge => "too_large",
            BackendErrorCode::NoMatch => "no_match",
            BackendErrorCode::Timeout => "timeout",
            BackendErrorCode::CommandNotAllowed => "command_not_allowed",
            BackendErrorCode::IoError => "io_error",
            BackendErrorCode::Unknown => "unknown",
        }
    }
}

impl Display for BackendErrorCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl BackendError {
    /// 创建一个不包含底层 `source` 的错误。
    pub fn new(code: BackendErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source: None,
        }
    }

    /// 创建一个包含底层 `source` 的错误，便于保留上下文信息。
    pub fn with_source(
        code: BackendErrorCode,
        message: impl Into<String>,
        source: impl Into<anyhow::Error>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            source: Some(source.into()),
        }
    }

    pub fn code_str(&self) -> &'static str {
        self.code.as_str()
    }
}

/// 后端最小能力集合（可选健康检查）。
///
/// 该 trait 主要用于统一约束：后端需要是 `Send + Sync`，并允许上层做探活/自检。
#[async_trait]
pub trait Backend: Send + Sync {
    async fn healthcheck(&self) -> Result<(), BackendError> {
        Ok(())
    }
}

/// 文件系统能力：读写/遍历/查找等操作。
///
/// 说明：
/// - 部分方法提供默认实现并返回 `NotSupported`，便于后端按需实现。
/// - 所有路径参数均使用字符串，是为了兼容跨进程/跨语言协议输入。
#[async_trait]
pub trait FilesystemBackend: Backend {
    async fn ls_info(&self, path: &str) -> Result<Vec<crate::types::FileInfo>, BackendError>;
    async fn create_dir_all(&self, _dir_path: &str) -> Result<(), BackendError> {
        Err(BackendError::new(
            BackendErrorCode::NotSupported,
            "not_supported",
        ))
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

/// 沙箱能力：在受控环境内执行命令。
///
/// 该 trait 继承 `FilesystemBackend`，因为命令执行通常与工作目录/文件交互绑定。
#[async_trait]
pub trait SandboxBackend: FilesystemBackend {
    async fn execute(
        &self,
        command: &str,
        timeout_secs: Option<u64>,
    ) -> Result<crate::types::ExecResult, BackendError>;
}
