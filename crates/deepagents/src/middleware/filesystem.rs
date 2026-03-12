use std::sync::Arc;

use async_trait::async_trait;

use crate::backends::SandboxBackend;
use crate::middleware::protocol::MiddlewareContext;
use crate::middleware::ToolExecutionMiddleware;
use crate::state::{
    DefaultFilesystemReducer, FileDelta, FileRecord, FilesystemDelta, StateReducer,
};

#[derive(Clone)]
/// 文件系统中间件：把“工具对文件的影响”归约进 [`AgentState`](crate::state::AgentState)。
///
/// 目前关注的工具：
/// - `write_file` / `edit_file`：写入或编辑文件后，重新读取文件内容并写入状态
/// - `delete_file`：删除文件后，把对应路径标记为删除
///
/// 归约方式由 `reducer` 决定（默认是 [`DefaultFilesystemReducer`]），用于把增量合并进
/// agent 的 [`FilesystemState`](crate::state::FilesystemState)。
pub struct FilesystemMiddleware {
    reducer: Arc<dyn StateReducer<crate::state::FilesystemState, FilesystemDelta>>,
    max_lines: usize,
}

impl FilesystemMiddleware {
    /// 创建一个带默认 reducer 的中间件，并限制最多读取 `2000` 行文件内容。
    pub fn new() -> Self {
        Self {
            reducer: Arc::new(DefaultFilesystemReducer),
            max_lines: 2000,
        }
    }

    /// 设置中间件同步文件内容时的最大行数。
    ///
    /// 该值会被钳制到至少 `1` 行，避免出现“永远不读取任何内容”的边界情况。
    pub fn with_max_lines(mut self, max_lines: usize) -> Self {
        self.max_lines = max_lines.max(1);
        self
    }

    /// 替换文件系统状态的归约器。
    ///
    /// 典型用途是：测试中注入一个可观测/可控的 reducer，或为不同的合并策略提供实现。
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
impl ToolExecutionMiddleware for FilesystemMiddleware {
    async fn after_tool(&self, ctx: &mut MiddlewareContext<'_>) -> anyhow::Result<()> {
        // 工具本身执行失败时，不尝试推断文件系统副作用，避免写入错误状态。
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
                // tool output 里若带有 error 字段且非 null，代表工具层失败（即使 transport 层成功）。
                let ok = output.get("error").map(|e| e.is_null()).unwrap_or(true);
                if !ok {
                    return Ok(());
                }
                // 优先从 output 取 path（工具可能返回最终写入路径），否则回退到 input 的 file_path。
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

                // 重新读取文件内容，保证状态与实际文件一致（而不是只相信工具返回的片段）。
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
                // 删除文件时同样做工具层 ok 判定，避免把失败的删除误认为成功。
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

        // 只有在确实存在文件变更时才归约，避免频繁触碰状态造成不必要的噪声。
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
    // backend.read 返回的是“带行号前缀”的文本（形如 ` 12→内容`），这里把前缀剥离成纯内容数组。
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
