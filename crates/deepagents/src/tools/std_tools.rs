use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;

use crate::backends::SandboxBackend;
use crate::tools::protocol::{Tool, ToolResult};
use crate::types::ContentBlock;

pub fn default_tools(backend: Arc<dyn SandboxBackend>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(LsTool {
            backend: backend.clone(),
        }),
        Arc::new(ReadFileTool {
            backend: backend.clone(),
        }),
        Arc::new(WriteFileTool {
            backend: backend.clone(),
        }),
        Arc::new(EditFileTool {
            backend: backend.clone(),
        }),
        Arc::new(DeleteFileTool {
            backend: backend.clone(),
        }),
        Arc::new(GlobTool {
            backend: backend.clone(),
        }),
        Arc::new(GrepTool {
            backend: backend.clone(),
        }),
        Arc::new(ExecuteTool { backend }),
    ]
}

struct LsTool {
    backend: Arc<dyn SandboxBackend>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LsInput {
    path: String,
}

#[async_trait]
impl Tool for LsTool {
    fn name(&self) -> &'static str {
        "ls"
    }

    fn description(&self) -> &'static str {
        "Lists files and directories in a given path."
    }

    async fn call(&self, input: serde_json::Value) -> anyhow::Result<ToolResult> {
        let input: LsInput = serde_json::from_value(input)?;
        let infos = self.backend.ls_info(&input.path).await?;
        Ok(ToolResult {
            output: serde_json::to_value(infos)?,
            content_blocks: None,
        })
    }
}

struct ReadFileTool {
    backend: Arc<dyn SandboxBackend>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadFileInput {
    file_path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ReadFileOutput {
    Text {
        content: String,
        truncated: bool,
        next_offset: Option<usize>,
    },
    Image {
        file_path: String,
        mime_type: String,
        size_bytes: u64,
        content: String,
    },
}

const READ_FILE_DEFAULT_LIMIT: usize = 100;
const READ_FILE_DEFAULT_MAX_BYTES: usize = 4_000_000;

fn image_mime_for_path(path: &str) -> Option<&'static str> {
    let ext = Path::new(path)
        .extension()
        .and_then(|v| v.to_str())
        .map(|s| s.to_ascii_lowercase())?;
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Reads a file from the local filesystem and returns cat -n formatted output."
    }

    async fn call(&self, input: serde_json::Value) -> anyhow::Result<ToolResult> {
        let input: ReadFileInput = serde_json::from_value(input)?;
        let mode = input
            .mode
            .as_deref()
            .unwrap_or("auto")
            .trim()
            .to_ascii_lowercase();
        let image_mime = image_mime_for_path(&input.file_path);
        let use_image = match mode.as_str() {
            "auto" => image_mime.is_some(),
            "text" => false,
            "image" => true,
            _ => {
                return Err(anyhow::anyhow!("invalid_request: unknown mode"));
            }
        };

        if use_image {
            let mime = image_mime
                .ok_or_else(|| anyhow::anyhow!("invalid_request: unsupported_image_type"))?;
            let max_bytes = input.max_bytes.unwrap_or(READ_FILE_DEFAULT_MAX_BYTES);
            if max_bytes == 0 {
                return Err(anyhow::anyhow!("invalid_request: max_bytes must be > 0"));
            }
            let bytes = self.backend.read_bytes(&input.file_path, max_bytes).await?;
            let size_bytes = bytes.len() as u64;
            let base64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            let output = ReadFileOutput::Image {
                file_path: input.file_path,
                mime_type: mime.to_string(),
                size_bytes,
                content: "(image returned as content block)".to_string(),
            };
            return Ok(ToolResult {
                output: serde_json::to_value(output)?,
                content_blocks: Some(vec![ContentBlock::image_base64(mime, base64)]),
            });
        }

        let offset = input.offset.unwrap_or(0);
        let limit = input.limit.unwrap_or(READ_FILE_DEFAULT_LIMIT).max(1);
        let out = self
            .backend
            .read(&input.file_path, offset, limit + 1)
            .await?;
        let mut lines: Vec<&str> = out.lines().collect();
        let truncated = lines.len() > limit;
        if truncated {
            lines.truncate(limit);
        }
        let mut content = lines.join("\n");
        if !content.is_empty() {
            content.push('\n');
        }
        let next_offset = truncated.then_some(offset + limit);
        Ok(ToolResult {
            output: serde_json::to_value(ReadFileOutput::Text {
                content,
                truncated,
                next_offset,
            })?,
            content_blocks: None,
        })
    }
}

struct WriteFileTool {
    backend: Arc<dyn SandboxBackend>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteFileInput {
    file_path: String,
    content: String,
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Writes a new file to the filesystem."
    }

    async fn call(&self, input: serde_json::Value) -> anyhow::Result<ToolResult> {
        let input: WriteFileInput = serde_json::from_value(input)?;
        let res = self
            .backend
            .write_file(&input.file_path, &input.content)
            .await?;
        Ok(ToolResult {
            output: serde_json::to_value(res)?,
            content_blocks: None,
        })
    }
}

struct DeleteFileTool {
    backend: Arc<dyn SandboxBackend>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteFileInput {
    file_path: String,
}

#[async_trait]
impl Tool for DeleteFileTool {
    fn name(&self) -> &'static str {
        "delete_file"
    }

    fn description(&self) -> &'static str {
        "Deletes a file from the filesystem."
    }

    async fn call(&self, input: serde_json::Value) -> anyhow::Result<ToolResult> {
        let input: DeleteFileInput = serde_json::from_value(input)?;
        let res = self.backend.delete_file(&input.file_path).await?;
        Ok(ToolResult {
            output: serde_json::to_value(res)?,
            content_blocks: None,
        })
    }
}

struct EditFileTool {
    backend: Arc<dyn SandboxBackend>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditFileInput {
    file_path: String,
    old_string: String,
    new_string: String,
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "Performs exact string replacements in files."
    }

    async fn call(&self, input: serde_json::Value) -> anyhow::Result<ToolResult> {
        let input: EditFileInput = serde_json::from_value(input)?;
        let res = self
            .backend
            .edit_file(&input.file_path, &input.old_string, &input.new_string)
            .await?;
        Ok(ToolResult {
            output: serde_json::to_value(res)?,
            content_blocks: None,
        })
    }
}

struct GlobTool {
    backend: Arc<dyn SandboxBackend>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GlobInput {
    pattern: String,
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "glob"
    }

    fn description(&self) -> &'static str {
        "Find files matching a glob pattern."
    }

    async fn call(&self, input: serde_json::Value) -> anyhow::Result<ToolResult> {
        let input: GlobInput = serde_json::from_value(input)?;
        let res = self.backend.glob(&input.pattern).await?;
        Ok(ToolResult {
            output: serde_json::to_value(res)?,
            content_blocks: None,
        })
    }
}

struct GrepTool {
    backend: Arc<dyn SandboxBackend>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GrepOutputMode {
    FilesWithMatches,
    Content,
    Count,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrepInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    output_mode: Option<GrepOutputMode>,
    #[serde(default)]
    head_limit: Option<usize>,
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn description(&self) -> &'static str {
        "Search for a literal text pattern across files."
    }

    async fn call(&self, input: serde_json::Value) -> anyhow::Result<ToolResult> {
        let input: GrepInput = serde_json::from_value(input)?;
        let matches = self
            .backend
            .grep(&input.pattern, input.path.as_deref(), input.glob.as_deref())
            .await?;

        let head = input.head_limit.unwrap_or(100);
        let mode = input
            .output_mode
            .unwrap_or(GrepOutputMode::FilesWithMatches);
        let output = match mode {
            GrepOutputMode::FilesWithMatches => {
                let mut files = BTreeSet::new();
                for m in matches.iter() {
                    files.insert(m.path.clone());
                    if files.len() >= head {
                        break;
                    }
                }
                serde_json::to_value(files.into_iter().collect::<Vec<_>>())?
            }
            GrepOutputMode::Count => {
                let mut counts: BTreeMap<String, u64> = BTreeMap::new();
                for m in matches {
                    *counts.entry(m.path).or_default() += 1;
                }
                let mut entries: Vec<_> = counts.into_iter().collect();
                entries.truncate(head);
                serde_json::to_value(entries)?
            }
            GrepOutputMode::Content => {
                let mut out = Vec::new();
                for m in matches.into_iter().take(head) {
                    out.push(m);
                }
                serde_json::to_value(out)?
            }
        };

        Ok(ToolResult {
            output,
            content_blocks: None,
        })
    }
}

struct ExecuteTool {
    backend: Arc<dyn SandboxBackend>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecuteInput {
    command: String,
    #[serde(default)]
    timeout: Option<u64>,
}

#[async_trait]
impl Tool for ExecuteTool {
    fn name(&self) -> &'static str {
        "execute"
    }

    fn description(&self) -> &'static str {
        "Executes a shell command in an isolated sandbox environment."
    }

    async fn call(&self, input: serde_json::Value) -> anyhow::Result<ToolResult> {
        let input: ExecuteInput = serde_json::from_value(input)?;
        let res = self.backend.execute(&input.command, input.timeout).await?;
        Ok(ToolResult {
            output: serde_json::to_value(res)?,
            content_blocks: None,
        })
    }
}
