use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::Result;

use crate::memory::protocol::MemoryDiagnostics;
use crate::runtime::RuntimeMiddleware;
use crate::state::AgentState;
use crate::types::Message;

#[derive(Debug, Clone)]
pub struct MemoryLoadOptions {
    pub allow_host_paths: bool,
    pub max_injected_chars: usize,
    pub max_source_bytes: usize,
    pub strict: bool,
}

impl Default for MemoryLoadOptions {
    fn default() -> Self {
        Self {
            allow_host_paths: false,
            max_injected_chars: 30_000,
            max_source_bytes: 10 * 1024 * 1024,
            strict: true,
        }
    }
}

pub struct MemoryMiddleware {
    root: PathBuf,
    sources: Vec<String>,
    options: MemoryLoadOptions,
    state: Arc<RwLock<LoadedMemory>>,
}

#[derive(Debug, Clone, Default)]
pub struct LoadedMemory {
    pub diagnostics: MemoryDiagnostics,
}

impl MemoryMiddleware {
    pub fn new(root: impl Into<PathBuf>, sources: Vec<String>, options: MemoryLoadOptions) -> Self {
        Self {
            root: root.into(),
            sources,
            options,
            state: Arc::new(RwLock::new(LoadedMemory::default())),
        }
    }

    pub async fn loaded(&self) -> LoadedMemory {
        self.state.read().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl RuntimeMiddleware for MemoryMiddleware {
    async fn before_run(
        &self,
        mut messages: Vec<Message>,
        state: &mut AgentState,
    ) -> Result<Vec<Message>> {
        if state.private.memory_loaded || state.private.memory_contents.is_some() {
            return Ok(messages);
        }

        let (contents, diagnostics) = load_sources(&self.root, &self.sources, &self.options)?;
        state.private.memory_loaded = true;
        state.private.memory_contents = Some(contents.clone());
        state.extra.insert(
            "memory_diagnostics".to_string(),
            serde_json::to_value(&diagnostics)?,
        );
        *self.state.write().unwrap() = LoadedMemory {
            diagnostics: diagnostics.clone(),
        };

        if !has_injection_marker(&messages) {
            let block = build_memory_block(&contents, &self.sources, &diagnostics);
            messages.insert(
                0,
                Message {
                    role: "system".to_string(),
                    content: block,
                    content_blocks: None,
                    reasoning_content: None,
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    status: None,
                },
            );
        }

        Ok(messages)
    }
}

fn has_injection_marker(messages: &[Message]) -> bool {
    messages
        .iter()
        .filter(|m| m.role == "system")
        .any(|m| m.content.contains("DEEPAGENTS_MEMORY_INJECTED_V1"))
}

fn build_memory_block(
    contents: &BTreeMap<String, String>,
    sources: &[String],
    diagnostics: &MemoryDiagnostics,
) -> String {
    if let Some(c) = contents.get("__combined__") {
        let mut buf = String::new();
        buf.push_str("DEEPAGENTS_MEMORY_INJECTED_V1\n");
        buf.push_str("<agent_memory>\n");
        buf.push_str(c);
        buf.push_str("\n</agent_memory>\n\n");
        buf.push_str("<memory_guidelines>\n");
        buf.push_str("The above <agent_memory> was loaded from files in your filesystem.\n");
        buf.push_str(
            "Never store API keys, access tokens, passwords, or any other credentials in memory.\n",
        );
        buf.push_str("If the user asks you to remember something persistent (preferences, role, workflows), update memory immediately using file tools.\n");
        buf.push_str("If the information is transient or sensitive, do not write it to memory.\n");
        buf.push_str("</memory_guidelines>\n\n");
        buf.push_str("<memory_diagnostics>\n");
        buf.push_str(&format!(
            "loaded_sources={}; skipped_not_found={}; truncated={}; injected_chars={}; combined_chars={}\n",
            diagnostics.loaded_sources, diagnostics.skipped_not_found, diagnostics.truncated, diagnostics.injected_chars, diagnostics.combined_chars
        ));
        buf.push_str("</memory_diagnostics>\n");
        return buf;
    }

    let mut sections: Vec<String> = Vec::new();
    for s in sources {
        if let Some(c) = contents.get(s) {
            sections.push(format!("{s}\n{c}"));
        }
    }
    let body = if sections.is_empty() {
        "(No memory loaded)".to_string()
    } else {
        sections.join("\n\n")
    };

    let mut buf = String::new();
    buf.push_str("DEEPAGENTS_MEMORY_INJECTED_V1\n");
    buf.push_str("<agent_memory>\n");
    buf.push_str(&body);
    buf.push_str("\n</agent_memory>\n\n");
    buf.push_str("<memory_guidelines>\n");
    buf.push_str("The above <agent_memory> was loaded from files in your filesystem.\n");
    buf.push_str(
        "Never store API keys, access tokens, passwords, or any other credentials in memory.\n",
    );
    buf.push_str("If the user asks you to remember something persistent (preferences, role, workflows), update memory immediately using file tools.\n");
    buf.push_str("If the information is transient or sensitive, do not write it to memory.\n");
    buf.push_str("</memory_guidelines>\n\n");
    buf.push_str("<memory_diagnostics>\n");
    buf.push_str(&format!(
        "loaded_sources={}; skipped_not_found={}; truncated={}; injected_chars={}; combined_chars={}\n",
        diagnostics.loaded_sources, diagnostics.skipped_not_found, diagnostics.truncated, diagnostics.injected_chars, diagnostics.combined_chars
    ));
    buf.push_str("</memory_diagnostics>\n");
    buf
}

fn load_sources(
    root: &Path,
    sources: &[String],
    options: &MemoryLoadOptions,
) -> Result<(BTreeMap<String, String>, MemoryDiagnostics)> {
    let mut diagnostics = MemoryDiagnostics::default();
    let mut out: BTreeMap<String, String> = BTreeMap::new();

    for s in sources {
        match load_one_source(root, s, options) {
            Ok(Some(content)) => {
                diagnostics.loaded_sources += 1;
                out.insert(s.clone(), content);
            }
            Ok(None) => {
                diagnostics.skipped_not_found += 1;
            }
            Err(e) => {
                if options.strict {
                    return Err(e);
                }
                diagnostics.errors.push(e.to_string());
            }
        }
    }

    let combined = sources
        .iter()
        .filter_map(|s| out.get(s).map(|c| format!("{s}\n{c}")))
        .collect::<Vec<_>>()
        .join("\n\n");
    diagnostics.combined_chars = combined.chars().count();

    if diagnostics.combined_chars > options.max_injected_chars {
        diagnostics.truncated = true;
        let truncated = truncate_chars(&combined, options.max_injected_chars);
        diagnostics.injected_chars = truncated.chars().count();
        out.clear();
        out.insert("__combined__".to_string(), truncated);
    } else {
        diagnostics.injected_chars = diagnostics.combined_chars;
    }

    Ok((out, diagnostics))
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let head = max_chars / 2;
    let tail = max_chars - head;
    let head_str: String = s.chars().take(head).collect();
    let tail_str: String = s
        .chars()
        .rev()
        .take(tail)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head_str}\n\n...(memory truncated)...\n\n{tail_str}")
}

fn load_one_source(
    root: &Path,
    source: &str,
    options: &MemoryLoadOptions,
) -> Result<Option<String>> {
    let p = resolve_source_path(root, source, options)?;
    if p.file_name().and_then(|n| n.to_str()) != Some("AGENTS.md") {
        anyhow::bail!(
            "invalid_request: memory source must be AGENTS.md: {}",
            source
        );
    }
    let meta = match std::fs::symlink_metadata(&p) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow::anyhow!("memory_io_error: {}: {}", source, e)),
    };
    if meta.file_type().is_symlink() {
        anyhow::bail!("permission_denied: symlink not allowed: {}", source);
    }
    if meta.len() as usize > options.max_source_bytes {
        anyhow::bail!("memory_quota_exceeded: source too large: {}", source);
    }
    let content = match std::fs::read_to_string(&p) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow::anyhow!("memory_io_error: {}: {}", source, e)),
    };
    Ok(Some(content))
}

fn resolve_source_path(root: &Path, source: &str, options: &MemoryLoadOptions) -> Result<PathBuf> {
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let root_raw = root.to_path_buf();

    if source.starts_with("~/") || source.starts_with("~\\") {
        if !options.allow_host_paths {
            anyhow::bail!("permission_denied: host paths disabled: {}", source);
        }
        let home =
            std::env::var("HOME").map_err(|_| anyhow::anyhow!("memory_io_error: HOME not set"))?;
        let rest = &source[2..];
        return Ok(PathBuf::from(home).join(rest));
    }

    let p = PathBuf::from(source);
    if p.is_absolute() {
        if options.allow_host_paths {
            return Ok(p);
        }
        let p_canon = p.canonicalize().unwrap_or_else(|_| p.clone());
        if p_canon.starts_with(&root_canon)
            || p.starts_with(&root_raw)
            || p.starts_with(&root_canon)
        {
            return Ok(p_canon);
        }
        anyhow::bail!("permission_denied: outside root: {}", source);
    }

    Ok(root_canon.join(p))
}
