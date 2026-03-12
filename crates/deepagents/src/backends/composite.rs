//! 组合后端：把多个沙箱后端按“路径前缀”拼成一个统一后端视图。
//!
//! 典型用途：把不同根目录（或不同实现）的后端“挂载”到不同路径前缀下，
//! 对上层暴露一个统一的 `FilesystemBackend` / `SandboxBackend`。

use std::sync::Arc;

use async_trait::async_trait;

use crate::backends::protocol::{Backend, BackendError, FilesystemBackend, SandboxBackend};
use crate::types::{EditResult, ExecResult, FileInfo, GrepMatch, WriteResult};

/// 按路径前缀路由的组合后端。
///
/// - `default`：未命中任何路由时使用的后端。
/// - `routes`：前缀到后端的映射；匹配时采用“最长前缀优先”，以支持嵌套挂载。
#[derive(Clone)]
pub struct CompositeBackend {
    default: Arc<dyn SandboxBackend>,
    routes: Vec<Route>,
}

#[derive(Clone)]
struct Route {
    prefix: String,
    backend: Arc<dyn SandboxBackend>,
}

impl CompositeBackend {
    /// 创建组合后端，并设置默认后端。
    pub fn new(default: Arc<dyn SandboxBackend>) -> Self {
        Self {
            default,
            routes: Vec::new(),
        }
    }

    /// 添加一条路由：把 `prefix` 下的路径交给指定后端处理。
    ///
    /// 约定：
    /// - `prefix` 会被规范化为以 `/` 开头、以 `/` 结尾，避免边界匹配歧义。
    pub fn with_route(
        mut self,
        prefix: impl Into<String>,
        backend: Arc<dyn SandboxBackend>,
    ) -> Self {
        let mut p = prefix.into();
        if !p.starts_with('/') {
            p = format!("/{p}");
        }
        if !p.ends_with('/') {
            p.push('/');
        }
        self.routes.push(Route { prefix: p, backend });
        self
    }

    fn resolve_backend_and_path<'a>(
        &'a self,
        path: &'a str,
    ) -> (&'a Arc<dyn SandboxBackend>, String) {
        let p = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };

        // 在所有匹配的路由里选“最长前缀”，以支持更精细的挂载覆盖更粗的挂载。
        let mut best: Option<(&Arc<dyn SandboxBackend>, &str)> = None;
        for r in &self.routes {
            if matches_prefix(&p, &r.prefix) {
                match best {
                    None => best = Some((&r.backend, r.prefix.as_str())),
                    Some((_b, prev_prefix)) => {
                        if r.prefix.len() > prev_prefix.len() {
                            best = Some((&r.backend, r.prefix.as_str()));
                        }
                    }
                }
            }
        }

        let Some((backend, prefix)) = best else {
            return (&self.default, p);
        };

        let stripped = strip_prefix_compat(&p, prefix);
        (backend, stripped)
    }

    fn resolve_backend_and_pattern<'a>(
        &'a self,
        pattern: &'a str,
    ) -> (&'a Arc<dyn SandboxBackend>, String) {
        // 对 glob/grep 这类模式字符串：只有以 `/` 开头时才参与路由，
        // 否则视为后端内部相对模式，由默认后端处理。
        if !pattern.starts_with('/') {
            return (&self.default, pattern.to_string());
        }
        self.resolve_backend_and_path(pattern)
    }
}

/// 判断 `path` 是否命中 `prefix`。
///
/// 兼容两种场景：
/// - `prefix` 既可表示目录（以 `/` 结尾），也可表示精确路径（去掉末尾 `/`）。
fn matches_prefix(path: &str, prefix: &str) -> bool {
    if path == prefix.trim_end_matches('/') {
        return true;
    }
    path.starts_with(prefix)
}

/// 把全局路径去掉挂载前缀，返回“后端视角”的路径。
///
/// 例如：
/// - path: `/mnt/a.txt`, prefix: `/mnt/` => `/a.txt`
/// - path: `/mnt`, prefix: `/mnt/` => `/`
fn strip_prefix_compat(path: &str, prefix: &str) -> String {
    let pfx = prefix.trim_end_matches('/');
    if path == pfx || path == prefix {
        return "/".to_string();
    }
    let rest = path.strip_prefix(prefix).unwrap_or("");
    if rest.is_empty() {
        "/".to_string()
    } else {
        format!("/{rest}")
    }
}

#[async_trait]
impl Backend for CompositeBackend {
    async fn healthcheck(&self) -> Result<(), BackendError> {
        self.default.healthcheck().await?;
        for r in &self.routes {
            r.backend.healthcheck().await?;
        }
        Ok(())
    }
}

#[async_trait]
impl FilesystemBackend for CompositeBackend {
    async fn ls_info(&self, path: &str) -> Result<Vec<FileInfo>, BackendError> {
        let (b, p) = self.resolve_backend_and_path(path);
        b.ls_info(&p).await
    }

    async fn create_dir_all(&self, dir_path: &str) -> Result<(), BackendError> {
        let (b, p) = self.resolve_backend_and_path(dir_path);
        b.create_dir_all(&p).await
    }

    async fn read(
        &self,
        file_path: &str,
        offset: usize,
        limit: usize,
    ) -> Result<String, BackendError> {
        let (b, p) = self.resolve_backend_and_path(file_path);
        b.read(&p, offset, limit).await
    }

    async fn read_bytes(&self, file_path: &str, max_bytes: usize) -> Result<Vec<u8>, BackendError> {
        let (b, p) = self.resolve_backend_and_path(file_path);
        b.read_bytes(&p, max_bytes).await
    }

    async fn write_file(
        &self,
        file_path: &str,
        content: &str,
    ) -> Result<WriteResult, BackendError> {
        let (b, p) = self.resolve_backend_and_path(file_path);
        b.write_file(&p, content).await
    }

    async fn delete_file(
        &self,
        file_path: &str,
    ) -> Result<crate::types::DeleteResult, BackendError> {
        let (b, p) = self.resolve_backend_and_path(file_path);
        b.delete_file(&p).await
    }

    async fn edit_file(
        &self,
        file_path: &str,
        old_string: &str,
        new_string: &str,
    ) -> Result<EditResult, BackendError> {
        let (b, p) = self.resolve_backend_and_path(file_path);
        b.edit_file(&p, old_string, new_string).await
    }

    async fn glob(&self, pattern: &str) -> Result<Vec<String>, BackendError> {
        let (b, p) = self.resolve_backend_and_pattern(pattern);
        b.glob(&p).await
    }

    async fn grep(
        &self,
        pattern: &str,
        path: Option<&str>,
        glob: Option<&str>,
    ) -> Result<Vec<GrepMatch>, BackendError> {
        let Some(path) = path else {
            return self.default.grep(pattern, None, glob).await;
        };
        let (b, p) = self.resolve_backend_and_path(path);
        b.grep(pattern, Some(&p), glob).await
    }
}

#[async_trait]
impl SandboxBackend for CompositeBackend {
    async fn execute(
        &self,
        command: &str,
        timeout_secs: Option<u64>,
    ) -> Result<ExecResult, BackendError> {
        // 组合后端目前不对“命令执行”做路由：统一交给默认后端执行，
        // 以避免跨后端的工作目录/环境差异带来的语义不一致。
        self.default.execute(command, timeout_secs).await
    }
}
