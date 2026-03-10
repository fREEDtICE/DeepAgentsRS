use std::sync::Arc;

use async_trait::async_trait;

use crate::backends::SandboxBackend;
use crate::middleware::protocol::MiddlewareContext;
use crate::middleware::Middleware;
use crate::state::{
    DefaultFilesystemReducer, FileDelta, FileRecord, FilesystemDelta, StateReducer,
};

#[derive(Clone)]
pub struct FilesystemMiddleware {
    reducer: Arc<dyn StateReducer<crate::state::FilesystemState, FilesystemDelta>>,
    max_lines: usize,
}

impl FilesystemMiddleware {
    pub fn new() -> Self {
        Self {
            reducer: Arc::new(DefaultFilesystemReducer),
            max_lines: 2000,
        }
    }

    pub fn with_max_lines(mut self, max_lines: usize) -> Self {
        self.max_lines = max_lines.max(1);
        self
    }

    pub fn with_reducer(
        mut self,
        reducer: Arc<dyn StateReducer<crate::state::FilesystemState, FilesystemDelta>>,
    ) -> Self {
        self.reducer = reducer;
        self
    }
}

impl Default for FilesystemMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Middleware for FilesystemMiddleware {
    async fn after_tool(&self, ctx: &mut MiddlewareContext<'_>) -> anyhow::Result<()> {
        if ctx.tool.error.is_some() {
            return Ok(());
        }
        let Some(output) = &ctx.tool.output else {
            return Ok(());
        };

        let tool_name = ctx.tool.tool_name.as_str();
        let mut delta = FilesystemDelta::default();

        match tool_name {
            "write_file" | "edit_file" => {
                let ok = output.get("error").map(|e| e.is_null()).unwrap_or(true);
                if !ok {
                    return Ok(());
                }
                let path = output
                    .get("path")
                    .and_then(|p| p.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| {
                        ctx.tool
                            .input
                            .get("file_path")
                            .and_then(|p| p.as_str())
                            .map(|s| s.to_string())
                    });
                let Some(path) = path else {
                    return Ok(());
                };

                let (content, truncated) =
                    read_file_lines(ctx.backend, &path, self.max_lines).await?;
                let now = now_iso8601();
                let record = FileRecord {
                    content,
                    created_at: None,
                    modified_at: Some(now),
                    deleted: false,
                    truncated,
                };
                delta.files.insert(
                    path,
                    FileDelta {
                        upsert: Some(record),
                        delete: false,
                    },
                );
            }
            "delete_file" => {
                let ok = output.get("error").map(|e| e.is_null()).unwrap_or(true);
                if !ok {
                    return Ok(());
                }
                let path = output
                    .get("path")
                    .and_then(|p| p.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| {
                        ctx.tool
                            .input
                            .get("file_path")
                            .and_then(|p| p.as_str())
                            .map(|s| s.to_string())
                    });
                let Some(path) = path else {
                    return Ok(());
                };
                let now = now_iso8601();
                delta.files.insert(
                    path,
                    FileDelta {
                        upsert: Some(FileRecord {
                            content: Vec::new(),
                            created_at: None,
                            modified_at: Some(now),
                            deleted: true,
                            truncated: false,
                        }),
                        delete: true,
                    },
                );
            }
            _ => {}
        }

        if !delta.files.is_empty() {
            self.reducer
                .reduce(&mut ctx.state.filesystem, delta.clone());
            ctx.filesystem_delta = Some(delta);
        }
        Ok(())
    }
}

async fn read_file_lines(
    backend: &dyn SandboxBackend,
    file_path: &str,
    max_lines: usize,
) -> Result<(Vec<String>, bool), anyhow::Error> {
    let raw = backend.read(file_path, 0, max_lines + 1).await?;
    let mut lines = Vec::new();
    for line in raw.lines() {
        let Some((_, content)) = line.split_once('→') else {
            continue;
        };
        lines.push(content.to_string());
    }
    let truncated = lines.len() > max_lines;
    if truncated {
        lines.truncate(max_lines);
    }
    Ok((lines, truncated))
}

fn now_iso8601() -> String {
    chrono::Utc::now().to_rfc3339()
}
