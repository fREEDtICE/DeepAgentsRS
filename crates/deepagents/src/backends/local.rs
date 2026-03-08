use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use globset::{Glob, GlobSetBuilder};
use regex::Regex;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use walkdir::WalkDir;

use crate::backends::protocol::{Backend, BackendError, FilesystemBackend, SandboxBackend};
use crate::types::{EditResult, ExecResult, FileInfo, GrepMatch, WriteResult};

#[derive(Debug, Clone)]
pub struct LocalSandbox {
    root: PathBuf,
    shell_allow_list: Option<Vec<String>>,
}

impl LocalSandbox {
    pub fn new(root: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let root = root.into();
        Ok(Self {
            root: canonicalize_if_possible(&root)?,
            shell_allow_list: None,
        })
    }

    pub fn with_shell_allow_list(mut self, allow_list: Option<Vec<String>>) -> Self {
        self.shell_allow_list = allow_list;
        self
    }

    fn resolve_path(&self, input: &str) -> Result<PathBuf, BackendError> {
        if input.trim().is_empty() {
            return Err(BackendError::Other("invalid_path: empty".to_string()));
        }

        let p = Path::new(input);
        let joined = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.root.join(p)
        };

        let resolved = normalize_path(&joined).map_err(|e| BackendError::Other(e))?;
        if !resolved.starts_with(&self.root) {
            return Err(BackendError::Other("permission_denied: outside root".to_string()));
        }
        Ok(resolved)
    }

    fn resolve_dir(&self, input: &str) -> Result<PathBuf, BackendError> {
        let p = self.resolve_path(input)?;
        if !p.exists() {
            return Err(BackendError::Other("file_not_found".to_string()));
        }
        if !p.is_dir() {
            return Err(BackendError::Other("invalid_path: not a directory".to_string()));
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
            .map_err(|e| BackendError::Other(e.to_string()))?
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.path());

        for entry in entries {
            let p = entry.path();
            let meta = entry
                .metadata()
                .map_err(|e| BackendError::Other(e.to_string()))?;
            let modified_at = meta
                .modified()
                .ok()
                .and_then(system_time_to_iso8601);
            out.push(FileInfo {
                path: p.to_string_lossy().to_string(),
                is_dir: Some(meta.is_dir()),
                size: Some(meta.len()),
                modified_at,
            });
        }
        Ok(out)
    }

    async fn read(&self, file_path: &str, offset: usize, limit: usize) -> Result<String, BackendError> {
        let p = self.resolve_path(file_path)?;
        if !p.exists() {
            return Err(BackendError::Other("file_not_found".to_string()));
        }
        if p.is_dir() {
            return Err(BackendError::Other("is_directory".to_string()));
        }

        let content = tokio::fs::read_to_string(&p)
            .await
            .map_err(|e| BackendError::Other(e.to_string()))?;
        if content.is_empty() {
            return Ok("System reminder: File exists but has empty contents".to_string());
        }

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

    async fn write_file(&self, file_path: &str, content: &str) -> Result<WriteResult, BackendError> {
        let p = self.resolve_path(file_path)?;
        if p.exists() {
            return Ok(WriteResult {
                error: Some("file_exists".to_string()),
                path: None,
            });
        }
        let parent = p
            .parent()
            .ok_or_else(|| BackendError::Other("invalid_path: missing parent".to_string()))?;
        if !parent.exists() {
            return Ok(WriteResult {
                error: Some("parent_not_found".to_string()),
                path: None,
            });
        }
        tokio::fs::write(&p, content)
            .await
            .map_err(|e| BackendError::Other(e.to_string()))?;
        Ok(WriteResult {
            error: None,
            path: Some(p.to_string_lossy().to_string()),
        })
    }

    async fn delete_file(&self, file_path: &str) -> Result<crate::types::DeleteResult, BackendError> {
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
        tokio::fs::remove_file(&p)
            .await
            .map_err(|e| BackendError::Other(e.to_string()))?;
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

        let content = tokio::fs::read_to_string(&p)
            .await
            .map_err(|e| BackendError::Other(e.to_string()))?;
        if !content.contains(old_string) {
            return Ok(EditResult {
                error: Some("no_match".to_string()),
                path: None,
                occurrences: None,
            });
        }
        let occurrences = content.matches(old_string).count() as u64;
        let new_content = content.replace(old_string, new_string);
        tokio::fs::write(&p, new_content)
            .await
            .map_err(|e| BackendError::Other(e.to_string()))?;
        Ok(EditResult {
            error: None,
            path: Some(p.to_string_lossy().to_string()),
            occurrences: Some(occurrences),
        })
    }

    async fn glob(&self, pattern: &str) -> Result<Vec<String>, BackendError> {
        let pat = pattern.trim();
        if pat.is_empty() {
            return Err(BackendError::Other("invalid_glob: empty".to_string()));
        }
        let mut builder = GlobSetBuilder::new();
        let normalized = pat.strip_prefix('/').unwrap_or(pat);
        let glob = Glob::new(normalized).map_err(|e| BackendError::Other(e.to_string()))?;
        builder.add(glob);
        let set = builder.build().map_err(|e| BackendError::Other(e.to_string()))?;

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
            return Err(BackendError::Other("invalid_pattern: empty".to_string()));
        }
        let root = match path {
            Some(p) => self.resolve_dir(p)?,
            None => self.root.clone(),
        };

        let globset = if let Some(g) = glob {
            let mut builder = GlobSetBuilder::new();
            builder.add(Glob::new(g).map_err(|e| BackendError::Other(e.to_string()))?);
            Some(builder.build().map_err(|e| BackendError::Other(e.to_string()))?)
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
    async fn execute(&self, command: &str, timeout_secs: Option<u64>) -> Result<ExecResult, BackendError> {
        if let Some(allow) = &self.shell_allow_list {
            if !is_shell_command_allowed(command, allow) {
                return Err(BackendError::Other("command_not_allowed".to_string()));
            }
        }

        let mut cmd = Command::new("sh");
        cmd.arg("-lc").arg(command).current_dir(&self.root);

        let run = async {
            let output = cmd.output().await.map_err(|e| BackendError::Other(e.to_string()))?;
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
                .map_err(|_| BackendError::Other("timeout".to_string()))?,
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

fn contains_dangerous_patterns(command: &str) -> bool {
    const DANGEROUS_SUBSTRINGS: [&str; 15] = [
        "$(",
        "`",
        "$'",
        "\n",
        "\r",
        "\t",
        "<(",
        ">(",
        "<<<",
        "<<",
        ">>",
        ">",
        "<",
        "${",
        "\u{0}",
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

    let allow_set: std::collections::HashSet<&str> = allow_list.iter().map(|s| s.as_str()).collect();
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

fn split_shell_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut buf = String::new();
    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '&' {
            if chars.peek() == Some(&'&') {
                chars.next();
                segments.push(buf.trim().to_string());
                buf.clear();
                continue;
            }
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

fn shell_like_split(segment: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut chars = segment.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(ch) = chars.next() {
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
