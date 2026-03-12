//! 本地后端：把一个本机目录当作“沙箱根目录”，并在其内部提供文件系统与命令执行能力。
//!
//! 安全边界：
//! - 所有路径输入最终都会被解析到 `root` 下；越界访问会被拒绝（`PermissionDenied`）。
//! - 命令执行可选启用 allow list，并对危险 shell 语法做保守拦截。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use globset::{Glob, GlobSetBuilder};
use regex::Regex;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use walkdir::WalkDir;

use crate::backends::protocol::{
    Backend, BackendError, BackendErrorCode, FilesystemBackend, SandboxBackend,
};
use crate::types::{EditResult, ExecResult, FileInfo, GrepMatch, WriteResult};

/// 基于本机文件系统的沙箱实现。
///
/// - `root`：沙箱根目录（所有文件操作与命令执行的工作目录）。
/// - `shell_allow_list`：可选的命令白名单；启用后仅允许列出的“程序名”作为每段命令的起始 token。
#[derive(Debug, Clone)]
pub struct LocalSandbox {
    root: PathBuf,
    shell_allow_list: Option<Vec<String>>,
}

impl LocalSandbox {
    /// 创建一个以 `root` 为根的本地沙箱。
    ///
    /// 若 `root` 已存在则尽量 canonicalize，避免符号链接/相对路径带来的歧义。
    pub fn new(root: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let root = root.into();
        Ok(Self {
            root: canonicalize_if_possible(&root)?,
            shell_allow_list: None,
        })
    }

    /// 设置命令 allow list。
    ///
    /// - `None`：不做 allow list 校验（完全由调用方控制风险）。
    /// - `Some(vec)`：对 `execute` 进行白名单校验与危险模式拦截。
    pub fn with_shell_allow_list(mut self, allow_list: Option<Vec<String>>) -> Self {
        self.shell_allow_list = allow_list;
        self
    }

    /// 将外部输入路径解析为沙箱内的绝对路径。
    ///
    /// 处理策略（偏保守）：
    /// - 空输入直接拒绝。
    /// - 绝对路径：若已在 `root` 内直接接受；否则尝试通过 canonicalize “纠正”到 `root` 内，
    ///   再不行则按“虚拟挂载”逻辑把 `/foo/bar` 映射为 `root/foo/bar`。
    /// - 相对路径：直接拼到 `root` 下。
    /// - 最终统一 normalize 并检查必须仍在 `root` 内，作为越界访问的最后一道防线。
    fn resolve_path(&self, input: &str) -> Result<PathBuf, BackendError> {
        if input.trim().is_empty() {
            return Err(BackendError::new(
                BackendErrorCode::InvalidPath,
                "invalid_path: empty",
            ));
        }

        let p = Path::new(input);
        let joined = if p.is_absolute() {
            if p.starts_with(&self.root) {
                p.to_path_buf()
            } else {
                // 兼容“真实绝对路径”与“虚拟绝对路径”两类输入：
                // - 真实绝对路径：如果 canonicalize 后落在 root 内，允许访问。
                // - 虚拟绝对路径：将其当作 root 下的相对路径映射。
                let mut remapped: Option<PathBuf> = None;
                if p.exists() && p.is_dir() {
                    if let Ok(canon) = p.canonicalize() {
                        if canon.starts_with(&self.root) {
                            remapped = Some(canon);
                        }
                    }
                }
                if let Some(parent) = p.parent() {
                    if parent.exists() {
                        if let Ok(canon_parent) = parent.canonicalize() {
                            if canon_parent.starts_with(&self.root) {
                                if let Some(name) = p.file_name() {
                                    remapped = Some(canon_parent.join(name));
                                } else {
                                    remapped = Some(canon_parent);
                                }
                            }
                        }
                    }
                }
                remapped.unwrap_or_else(|| {
                    let virtual_rel = input.trim_start_matches('/');
                    self.root.join(virtual_rel)
                })
            }
        } else {
            self.root.join(p)
        };

        let resolved = normalize_path(&joined).map_err(|e| {
            BackendError::new(BackendErrorCode::InvalidPath, format!("invalid_path: {e}"))
        })?;
        // 最终越界校验：即使前面发生了各种拼接/重映射，也必须保证落在 root 内。
        if !resolved.starts_with(&self.root) {
            return Err(BackendError::new(
                BackendErrorCode::PermissionDenied,
                "permission_denied: outside root",
            ));
        }
        Ok(resolved)
    }

    /// 解析并校验目录路径：必须存在且为目录。
    fn resolve_dir(&self, input: &str) -> Result<PathBuf, BackendError> {
        let p = self.resolve_path(input)?;
        if !p.exists() {
            return Err(BackendError::new(
                BackendErrorCode::FileNotFound,
                "file_not_found",
            ));
        }
        if !p.is_dir() {
            return Err(BackendError::new(
                BackendErrorCode::InvalidPath,
                "invalid_path: not a directory",
            ));
        }
        Ok(p)
    }
}

#[async_trait]
impl Backend for LocalSandbox {}

#[async_trait]
impl FilesystemBackend for LocalSandbox {
    async fn ls_info(&self, path: &str) -> Result<Vec<FileInfo>, BackendError> {
        let dir = self.resolve_dir(path)?;
        let mut out = Vec::new();
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| {
                BackendError::with_source(BackendErrorCode::IoError, "io_error: read_dir failed", e)
            })?
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.path());

        for entry in entries {
            let p = entry.path();
            let meta = entry.metadata().map_err(|e| {
                BackendError::with_source(BackendErrorCode::IoError, "io_error: metadata failed", e)
            })?;
            let modified_at = meta.modified().ok().and_then(system_time_to_iso8601);
            out.push(FileInfo {
                path: p.to_string_lossy().to_string(),
                is_dir: Some(meta.is_dir()),
                size: Some(meta.len()),
                modified_at,
            });
        }
        Ok(out)
    }

    async fn create_dir_all(&self, dir_path: &str) -> Result<(), BackendError> {
        let p = self.resolve_path(dir_path)?;
        if p.exists() {
            if p.is_dir() {
                return Ok(());
            }
            return Err(BackendError::new(
                BackendErrorCode::InvalidPath,
                "already_exists_not_dir",
            ));
        }
        tokio::fs::create_dir_all(&p).await.map_err(|e| {
            BackendError::with_source(
                BackendErrorCode::IoError,
                "io_error: create_dir_all failed",
                e,
            )
        })?;
        Ok(())
    }

    async fn read(
        &self,
        file_path: &str,
        offset: usize,
        limit: usize,
    ) -> Result<String, BackendError> {
        let p = self.resolve_path(file_path)?;
        if !p.exists() {
            return Err(BackendError::new(
                BackendErrorCode::FileNotFound,
                "file_not_found",
            ));
        }
        if p.is_dir() {
            return Err(BackendError::new(
                BackendErrorCode::IsDirectory,
                "is_directory",
            ));
        }

        let content = tokio::fs::read_to_string(&p).await.map_err(|e| {
            BackendError::with_source(
                BackendErrorCode::IoError,
                "io_error: read_to_string failed",
                e,
            )
        })?;
        if content.is_empty() {
            return Ok("System reminder: File exists but has empty contents".to_string());
        }

        // 输出格式与本 IDE 的“cat -n”一致：`行号→内容`，方便上层直接展示。
        let lines: Vec<&str> = content.lines().collect();
        let start = offset.min(lines.len());
        let end = (start + limit).min(lines.len());
        let mut buf = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            let line_no = start + i + 1;
            buf.push_str(&format!("{line_no:>6}→{line}\n"));
        }
        Ok(buf)
    }

    async fn read_bytes(&self, file_path: &str, max_bytes: usize) -> Result<Vec<u8>, BackendError> {
        let p = self.resolve_path(file_path)?;
        if !p.exists() {
            return Err(BackendError::new(
                BackendErrorCode::FileNotFound,
                "file_not_found",
            ));
        }
        if p.is_dir() {
            return Err(BackendError::new(
                BackendErrorCode::IsDirectory,
                "is_directory",
            ));
        }
        if max_bytes == 0 {
            return Err(BackendError::new(BackendErrorCode::TooLarge, "too_large"));
        }
        let meta = tokio::fs::metadata(&p).await.map_err(|e| {
            BackendError::with_source(BackendErrorCode::IoError, "io_error: metadata failed", e)
        })?;
        let len = meta.len();
        if len > max_bytes as u64 {
            return Err(BackendError::new(BackendErrorCode::TooLarge, "too_large"));
        }
        let buf = tokio::fs::read(&p).await.map_err(|e| {
            BackendError::with_source(BackendErrorCode::IoError, "io_error: read failed", e)
        })?;
        Ok(buf)
    }

    async fn write_file(
        &self,
        file_path: &str,
        content: &str,
    ) -> Result<WriteResult, BackendError> {
        let p = self.resolve_path(file_path)?;
        if p.exists() {
            return Ok(WriteResult {
                error: Some("file_exists".to_string()),
                path: None,
            });
        }
        let parent = p.parent().ok_or_else(|| {
            BackendError::new(
                BackendErrorCode::InvalidPath,
                "invalid_path: missing parent",
            )
        })?;
        if !parent.exists() {
            return Ok(WriteResult {
                error: Some("parent_not_found".to_string()),
                path: None,
            });
        }
        tokio::fs::write(&p, content).await.map_err(|e| {
            BackendError::with_source(BackendErrorCode::IoError, "io_error: write failed", e)
        })?;
        Ok(WriteResult {
            error: None,
            path: Some(p.to_string_lossy().to_string()),
        })
    }

    async fn delete_file(
        &self,
        file_path: &str,
    ) -> Result<crate::types::DeleteResult, BackendError> {
        let p = self.resolve_path(file_path)?;
        if !p.exists() {
            return Ok(crate::types::DeleteResult {
                error: Some("file_not_found".to_string()),
                path: None,
            });
        }
        if p.is_dir() {
            return Ok(crate::types::DeleteResult {
                error: Some("is_directory".to_string()),
                path: None,
            });
        }
        tokio::fs::remove_file(&p).await.map_err(|e| {
            BackendError::with_source(BackendErrorCode::IoError, "io_error: remove_file failed", e)
        })?;
        Ok(crate::types::DeleteResult {
            error: None,
            path: Some(p.to_string_lossy().to_string()),
        })
    }

    async fn edit_file(
        &self,
        file_path: &str,
        old_string: &str,
        new_string: &str,
    ) -> Result<EditResult, BackendError> {
        let p = self.resolve_path(file_path)?;
        if !p.exists() {
            return Ok(EditResult {
                error: Some("file_not_found".to_string()),
                path: None,
                occurrences: None,
            });
        }
        if p.is_dir() {
            return Ok(EditResult {
                error: Some("is_directory".to_string()),
                path: None,
                occurrences: None,
            });
        }

        let content = tokio::fs::read_to_string(&p).await.map_err(|e| {
            BackendError::with_source(
                BackendErrorCode::IoError,
                "io_error: read_to_string failed",
                e,
            )
        })?;
        if !content.contains(old_string) {
            return Ok(EditResult {
                error: Some("no_match".to_string()),
                path: None,
                occurrences: None,
            });
        }
        let occurrences = content.matches(old_string).count() as u64;
        let new_content = content.replace(old_string, new_string);
        tokio::fs::write(&p, new_content).await.map_err(|e| {
            BackendError::with_source(BackendErrorCode::IoError, "io_error: write failed", e)
        })?;
        Ok(EditResult {
            error: None,
            path: Some(p.to_string_lossy().to_string()),
            occurrences: Some(occurrences),
        })
    }

    async fn glob(&self, pattern: &str) -> Result<Vec<String>, BackendError> {
        let pat = pattern.trim();
        if pat.is_empty() {
            return Err(BackendError::new(
                BackendErrorCode::InvalidInput,
                "invalid_glob: empty",
            ));
        }
        let mut builder = GlobSetBuilder::new();
        // 输入允许以 `/` 开头；对本地遍历时匹配 root 下的相对路径即可。
        let normalized = pat.strip_prefix('/').unwrap_or(pat);
        let glob = Glob::new(normalized).map_err(|e| {
            BackendError::with_source(
                BackendErrorCode::InvalidInput,
                "invalid_glob: parse failed",
                e,
            )
        })?;
        builder.add(glob);
        let set = builder.build().map_err(|e| {
            BackendError::with_source(
                BackendErrorCode::InvalidInput,
                "invalid_glob: build failed",
                e,
            )
        })?;

        let mut out = Vec::new();
        for entry in WalkDir::new(&self.root).into_iter().filter_map(|e| e.ok()) {
            let p = entry.path();
            if p.is_dir() {
                continue;
            }
            let rel = p.strip_prefix(&self.root).unwrap_or(p);
            if set.is_match(rel) {
                out.push(p.to_string_lossy().to_string());
            }
        }
        out.sort();
        Ok(out)
    }

    async fn grep(
        &self,
        pattern: &str,
        path: Option<&str>,
        glob: Option<&str>,
    ) -> Result<Vec<GrepMatch>, BackendError> {
        if pattern.is_empty() {
            return Err(BackendError::new(
                BackendErrorCode::InvalidInput,
                "invalid_pattern: empty",
            ));
        }
        let root = match path {
            Some(p) => self.resolve_dir(p)?,
            None => self.root.clone(),
        };

        let globset = if let Some(g) = glob {
            let mut builder = GlobSetBuilder::new();
            builder.add(Glob::new(g).map_err(|e| {
                BackendError::with_source(
                    BackendErrorCode::InvalidInput,
                    "invalid_glob: parse failed",
                    e,
                )
            })?);
            Some(builder.build().map_err(|e| {
                BackendError::with_source(
                    BackendErrorCode::InvalidInput,
                    "invalid_glob: build failed",
                    e,
                )
            })?)
        } else {
            None
        };

        let mut out = Vec::new();
        for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
            let p = entry.path();
            if p.is_dir() {
                continue;
            }
            if let Some(gs) = &globset {
                let rel = p.strip_prefix(&self.root).unwrap_or(p);
                if !gs.is_match(rel) {
                    continue;
                }
            }
            // grep 以“文本包含”作为最小实现：无法按 UTF-8 读入的文件直接跳过。
            let content = match tokio::fs::read_to_string(p).await {
                Ok(c) => c,
                Err(_) => continue,
            };
            for (idx, line) in content.lines().enumerate() {
                if line.contains(pattern) {
                    out.push(GrepMatch {
                        path: p.to_string_lossy().to_string(),
                        line: (idx + 1) as u64,
                        text: line.to_string(),
                    });
                    // 防止极端情况下返回过大结果，保证调用方内存可控。
                    if out.len() >= 10_000 {
                        return Ok(out);
                    }
                }
            }
        }
        Ok(out)
    }
}

#[async_trait]
impl SandboxBackend for LocalSandbox {
    async fn execute(
        &self,
        command: &str,
        timeout_secs: Option<u64>,
    ) -> Result<ExecResult, BackendError> {
        if let Some(allow) = &self.shell_allow_list {
            if !is_shell_command_allowed(command, allow) {
                return Err(BackendError::new(
                    BackendErrorCode::CommandNotAllowed,
                    "command_not_allowed",
                ));
            }
        }

        // 使用 `sh -lc` 是为了兼容常见 shell 语法与环境变量展开；
        // 若启用 allow list，会在此之前做保守校验，避免注入与危险语法。
        let mut cmd = Command::new("sh");
        cmd.arg("-lc").arg(command).current_dir(&self.root);

        let run = async {
            let output = cmd.output().await.map_err(|e| {
                BackendError::with_source(
                    BackendErrorCode::IoError,
                    "io_error: process spawn failed",
                    e,
                )
            })?;
            let mut combined = String::new();
            combined.push_str(&String::from_utf8_lossy(&output.stdout));
            combined.push_str(&String::from_utf8_lossy(&output.stderr));
            let mut truncated = None;
            if combined.len() > 200_000 {
                combined.truncate(200_000);
                truncated = Some(true);
            }
            Ok::<_, BackendError>(ExecResult {
                exit_code: output.status.code().unwrap_or(-1),
                output: combined,
                truncated,
            })
        };

        match timeout_secs {
            Some(secs) => timeout(Duration::from_secs(secs), run)
                .await
                .map_err(|_| BackendError::new(BackendErrorCode::Timeout, "timeout"))?,
            None => run.await,
        }
    }
}

fn canonicalize_if_possible(p: &Path) -> anyhow::Result<PathBuf> {
    if p.exists() {
        Ok(p.canonicalize()?)
    } else {
        Ok(p.to_path_buf())
    }
}

/// 规范化路径（去掉 `.` / `..`），并尽可能 canonicalize 现存路径片段。
///
/// 目的：
/// - 降低路径绕过（例如 `..`）风险。
/// - 在目标不存在时仍能返回一个“合理”的规范路径。
fn normalize_path(path: &Path) -> Result<PathBuf, String> {
    let components: Vec<_> = path.components().collect();
    let mut out = PathBuf::new();
    for c in components {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }

    if out.exists() {
        out.canonicalize().map_err(|e| e.to_string())
    } else {
        let mut ancestor = out.parent().ok_or_else(|| "invalid_path".to_string())?;
        while !ancestor.exists() {
            ancestor = match ancestor.parent() {
                Some(p) => p,
                None => return Ok(out),
            };
        }
        let rel = out
            .strip_prefix(ancestor)
            .map_err(|_| "invalid_path".to_string())?;
        let canon = ancestor.canonicalize().map_err(|e| e.to_string())?;
        Ok(canon.join(rel))
    }
}

fn system_time_to_iso8601(t: SystemTime) -> Option<String> {
    use std::time::UNIX_EPOCH;
    let dur = t.duration_since(UNIX_EPOCH).ok()?;
    let secs = dur.as_secs() as i64;
    let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)?;
    Some(dt.to_rfc3339())
}

/// 检测常见危险 shell 片段（保守策略，宁可误杀也不放行）。
fn contains_dangerous_patterns(command: &str) -> bool {
    const DANGEROUS_SUBSTRINGS: [&str; 15] = [
        "$(", "`", "$'", "\n", "\r", "\t", "<(", ">(", "<<<", "<<", ">>", ">", "<", "${", "\u{0}",
    ];
    if DANGEROUS_SUBSTRINGS.iter().any(|p| command.contains(p)) {
        return true;
    }
    let bare_var = Regex::new(r"\$[A-Za-z_]").expect("regex compile");
    if bare_var.is_match(command) {
        return true;
    }
    contains_standalone_ampersand(command)
}

/// 识别裸 `&`（后台执行），但允许 `&&`（逻辑与）。
fn contains_standalone_ampersand(command: &str) -> bool {
    let bytes = command.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] != b'&' {
            continue;
        }
        let prev_is_amp = i > 0 && bytes[i - 1] == b'&';
        let next_is_amp = i + 1 < bytes.len() && bytes[i + 1] == b'&';
        if !(prev_is_amp || next_is_amp) {
            return true;
        }
    }
    false
}

/// allow list 校验入口：把命令按 `;` / `|` / `&&` 分段，逐段校验首 token 是否在白名单内。
///
/// 注意：这里不做“完整 shell 解析”，而是用简单规则分割并搭配危险模式拦截。
fn is_shell_command_allowed(command: &str, allow_list: &[String]) -> bool {
    if allow_list.is_empty() {
        return false;
    }
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }
    if contains_dangerous_patterns(trimmed) {
        return false;
    }

    let allow_set: std::collections::HashSet<&str> =
        allow_list.iter().map(|s| s.as_str()).collect();
    let segments = split_shell_segments(trimmed);
    let mut found = false;
    for seg in segments {
        let seg = seg.trim();
        if seg.is_empty() {
            continue;
        }
        let tokens = shell_like_split(seg);
        if tokens.is_empty() {
            continue;
        }
        found = true;
        if !allow_set.contains(tokens[0].as_str()) {
            return false;
        }
    }
    found
}

/// 按常见分隔符把命令拆成多个“段”，用于逐段 allow list 校验。
fn split_shell_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut buf = String::new();
    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '&' && chars.peek() == Some(&'&') {
            chars.next();
            segments.push(buf.trim().to_string());
            buf.clear();
            continue;
        }
        if ch == '|' {
            if chars.peek() == Some(&'|') {
                chars.next();
                segments.push(buf.trim().to_string());
                buf.clear();
                continue;
            }
            segments.push(buf.trim().to_string());
            buf.clear();
            continue;
        }
        if ch == ';' {
            segments.push(buf.trim().to_string());
            buf.clear();
            continue;
        }
        buf.push(ch);
    }
    if !buf.trim().is_empty() {
        segments.push(buf.trim().to_string());
    }
    segments
}

/// 类 shell 的 token 分割：支持单/双引号包裹，但不处理转义与更复杂语法。
fn shell_like_split(segment: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut quote: Option<char> = None;
    for ch in segment.chars() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                } else {
                    buf.push(ch);
                }
            }
            None => {
                if ch == '"' || ch == '\'' {
                    quote = Some(ch);
                    continue;
                }
                if ch.is_whitespace() {
                    if !buf.is_empty() {
                        out.push(buf.clone());
                        buf.clear();
                    }
                    continue;
                }
                buf.push(ch);
            }
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}
