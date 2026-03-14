use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::Result;

use crate::memory::{
    build_memory_pack, FileMemoryStore, MemoryActorInput, MemoryDiagnostics,
    MemoryIdentityResolver, MemoryPolicy, MemoryRetrievalDiagnostics, MemoryRuntimeMode,
    MemoryStore,
};
use crate::runtime::RuntimeMiddleware;
use crate::state::AgentState;
use crate::types::Message;

#[derive(Debug, Clone)]
pub struct MemoryLoadOptions {
    pub allow_host_paths: bool,
    pub max_injected_chars: usize,
    pub max_source_bytes: usize,
    pub strict: bool,
    pub runtime_mode: MemoryRuntimeMode,
    pub store_path: PathBuf,
    pub store_policy: MemoryPolicy,
    pub actor: MemoryActorInput,
}

impl Default for MemoryLoadOptions {
    fn default() -> Self {
        Self {
            allow_host_paths: false,
            max_injected_chars: 30_000,
            max_source_bytes: 10 * 1024 * 1024,
            strict: true,
            runtime_mode: MemoryRuntimeMode::Compatibility,
            store_path: PathBuf::from(".deepagents/memory_store.json"),
            store_policy: MemoryPolicy::default(),
            actor: MemoryActorInput::default(),
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
    pub compatibility: Option<MemoryDiagnostics>,
    pub retrieval: Option<MemoryRetrievalDiagnostics>,
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

        match self.options.runtime_mode {
            MemoryRuntimeMode::Compatibility => {
                let (contents, diagnostics) =
                    load_sources(&self.root, &self.sources, &self.options)?;
                let block = build_memory_block(&contents, &self.sources, &diagnostics);
                state.private.memory_loaded = true;
                state.private.memory_contents = Some(contents);
                state.extra.insert(
                    "memory_diagnostics".to_string(),
                    serde_json::to_value(&diagnostics)?,
                );
                *self.state.write().unwrap() = LoadedMemory {
                    compatibility: Some(diagnostics.clone()),
                    retrieval: None,
                };

                if !has_injection_marker(&messages) {
                    messages.insert(0, system_message(block));
                }
            }
            MemoryRuntimeMode::Scoped => {
                let (rendered, diagnostics) =
                    load_scoped_memory(&self.root, &self.options, &messages).await?;
                state.private.memory_loaded = true;
                state.private.memory_contents = Some(BTreeMap::from([(
                    "__memory_pack__".to_string(),
                    rendered.clone(),
                )]));
                state.extra.insert(
                    "memory_retrieval".to_string(),
                    serde_json::to_value(&diagnostics)?,
                );
                *self.state.write().unwrap() = LoadedMemory {
                    compatibility: None,
                    retrieval: Some(diagnostics.clone()),
                };

                if !has_injection_marker(&messages) {
                    messages.insert(0, system_message(rendered));
                }
            }
        }

        Ok(messages)
    }
}

fn system_message(content: String) -> Message {
    Message {
        role: "system".to_string(),
        content,
        content_blocks: None,
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        status: None,
    }
}

fn has_injection_marker(messages: &[Message]) -> bool {
    messages
        .iter()
        .filter(|message| message.role == "system")
        .any(|message| message.content.contains("DEEPAGENTS_MEMORY_INJECTED_V"))
}

async fn load_scoped_memory(
    root: &Path,
    options: &MemoryLoadOptions,
    messages: &[Message],
) -> Result<(String, MemoryRetrievalDiagnostics)> {
    let store_path = resolve_store_path(root, &options.store_path);
    let store = FileMemoryStore::new(store_path);
    store.load().await?;
    store.set_policy(options.store_policy.clone()).await?;

    let resolver = crate::memory::LocalIdentityResolver::new(root.to_string_lossy().to_string());
    let actor = resolver.resolve_actor(&options.actor);
    let (_, diagnostics, rendered) =
        build_memory_pack(&store, &actor, messages, options.max_injected_chars).await?;
    Ok((rendered, diagnostics))
}

fn resolve_store_path(root: &Path, store_path: &Path) -> PathBuf {
    if store_path.is_absolute() {
        store_path.to_path_buf()
    } else {
        root.join(store_path)
    }
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
    for source in sources {
        if let Some(content) = contents.get(source) {
            sections.push(format!("{source}\n{content}"));
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

    for source in sources {
        match load_one_source(root, source, options) {
            Ok(Some(content)) => {
                diagnostics.loaded_sources += 1;
                out.insert(source.clone(), content);
            }
            Ok(None) => {
                diagnostics.skipped_not_found += 1;
            }
            Err(error) => {
                if options.strict {
                    return Err(error);
                }
                diagnostics.errors.push(error.to_string());
            }
        }
    }

    let combined = sources
        .iter()
        .filter_map(|source| {
            out.get(source)
                .map(|content| format!("{source}\n{content}"))
        })
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
    let path = resolve_source_path(root, source, options)?;
    if path.file_name().and_then(|name| name.to_str()) != Some("AGENTS.md") {
        anyhow::bail!(
            "invalid_request: memory source must be AGENTS.md: {}",
            source
        );
    }
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(anyhow::anyhow!("memory_io_error: {}: {}", source, error)),
    };
    if meta.file_type().is_symlink() {
        anyhow::bail!("permission_denied: symlink not allowed: {}", source);
    }
    if meta.len() as usize > options.max_source_bytes {
        anyhow::bail!("memory_quota_exceeded: source too large: {}", source);
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(anyhow::anyhow!("memory_io_error: {}: {}", source, error)),
    };
    Ok(Some(content))
}

fn resolve_source_path(root: &Path, source: &str, options: &MemoryLoadOptions) -> Result<PathBuf> {
    let source_path = Path::new(source);
    let absolute = if source_path.is_absolute() {
        if !options.allow_host_paths {
            anyhow::bail!(
                "permission_denied: absolute host path not allowed: {}",
                source
            );
        }
        source_path.to_path_buf()
    } else {
        root.join(source_path)
    };

    if !source_path.is_absolute() {
        let relative = absolute
            .strip_prefix(root)
            .map_err(|_| anyhow::anyhow!("permission_denied: outside root: {}", source))?;
        let mut prefix = PathBuf::new();
        for component in relative.components() {
            if matches!(component, Component::ParentDir) {
                anyhow::bail!("permission_denied: outside root: {}", source);
            }
            prefix.push(component.as_os_str());
            let candidate = root.join(&prefix);
            if let Ok(meta) = std::fs::symlink_metadata(&candidate) {
                if meta.file_type().is_symlink() {
                    anyhow::bail!("permission_denied: symlink not allowed: {}", source);
                }
            }
        }
    }

    Ok(absolute)
}
