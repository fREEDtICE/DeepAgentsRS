use std::sync::Arc;

use async_trait::async_trait;

use crate::backends::protocol::{Backend, BackendError, FilesystemBackend, SandboxBackend};
use crate::types::{EditResult, ExecResult, FileInfo, GrepMatch, WriteResult};

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
    pub fn new(default: Arc<dyn SandboxBackend>) -> Self {
        Self {
            default,
            routes: Vec::new(),
        }
    }

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
        if !pattern.starts_with('/') {
            return (&self.default, pattern.to_string());
        }
        self.resolve_backend_and_path(pattern)
    }
}

fn matches_prefix(path: &str, prefix: &str) -> bool {
    if path == prefix.trim_end_matches('/') {
        return true;
    }
    path.starts_with(prefix)
}

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
        self.default.execute(command, timeout_secs).await
    }
}
