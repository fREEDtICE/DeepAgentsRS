use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use deepagents::approval::{
    redact_command, ApprovalDecision, ApprovalRequest, DefaultApprovalPolicy, ExecutionMode,
};
use deepagents::audit::{AuditEvent, AuditSink};
use deepagents::config::{
    ConfigKey, ConfigManager, ConfigOverrides, ConfigScope, ConfigValue, EffectiveConfig,
    PromptCacheBackendKind,
};
use deepagents::memory::MemoryStore;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "deepagents", version, about = "deepagents (Rust)")]
struct Args {
    #[arg(long, default_value = ".")]
    root: String,

    #[arg(long = "shell-allow")]
    shell_allow: Vec<String>,

    #[arg(long)]
    shell_allow_file: Option<String>,

    #[arg(long)]
    execution_mode: Option<String>,

    #[arg(long)]
    audit_json: Option<String>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
enum Cmd {
    Tool {
        name: String,
        #[arg(long)]
        input: String,
        #[arg(long, default_value_t = false)]
        pretty: bool,
        #[arg(long)]
        state_file: Option<String>,
    },
    Run {
        #[arg(long)]
        input: String,
        #[arg(long, default_value = "mock")]
        provider: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        api_key_env: Option<String>,
        #[arg(long)]
        tool_choice: Option<String>,
        #[arg(long)]
        structured_output_schema: Option<String>,
        #[arg(long)]
        structured_output_name: Option<String>,
        #[arg(long)]
        structured_output_description: Option<String>,
        #[arg(long)]
        thread_id: Option<String>,
        #[arg(long)]
        mock_script: Option<String>,
        /// Load skill packages from a source directory. Repeat to add multiple sources.
        #[arg(long = "skills-source")]
        skills_source: Vec<String>,
        /// Skip invalid skill sources or packages instead of failing fast.
        #[arg(long, default_value_t = false)]
        skills_skip_invalid: bool,
        #[arg(long = "memory-source")]
        memory_source: Vec<String>,
        #[arg(long, default_value_t = false)]
        memory_allow_host_paths: bool,
        #[arg(long)]
        memory_max_injected_chars: Option<usize>,
        #[arg(long, default_value_t = false)]
        memory_disable: bool,
        #[arg(long)]
        max_steps: Option<usize>,
        #[arg(long)]
        provider_timeout_ms: Option<u64>,
        #[arg(long)]
        prompt_cache: Option<String>,
        #[arg(long, default_value_t = false)]
        prompt_cache_l2: bool,
        #[arg(long)]
        prompt_cache_ttl_ms: Option<u64>,
        #[arg(long)]
        prompt_cache_max_entries: Option<usize>,
        #[arg(long, default_value_t = false)]
        summarization_disable: bool,
        #[arg(long)]
        summarization_max_char_budget: Option<usize>,
        #[arg(long)]
        summarization_max_turns_visible: Option<usize>,
        #[arg(long)]
        summarization_min_recent_messages: Option<usize>,
        #[arg(long, action = clap::ArgAction::Set)]
        summarization_redact_tool_args: Option<bool>,
        #[arg(long)]
        summarization_max_tool_arg_chars: Option<usize>,
        #[arg(long)]
        summarization_truncate_keep_last: Option<usize>,
        #[arg(long = "interrupt-on")]
        interrupt_on: Vec<String>,
        #[arg(long)]
        events_jsonl: Option<String>,
        #[arg(long, default_value_t = false)]
        stream_events: bool,
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    Skill {
        #[command(subcommand)]
        cmd: SkillCmd,
    },
    Memory {
        #[command(subcommand)]
        cmd: MemoryCmd,
    },
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
}

#[derive(Subcommand, Debug)]
enum SkillCmd {
    /// Create a new skill package scaffold in DIR.
    Init {
        /// Target directory for the new skill package.
        dir: String,
        /// Pretty-print the JSON result.
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    /// Validate skills from one or more source directories without starting a run.
    Validate {
        /// Load skills from a source directory. Repeat to validate multiple sources.
        #[arg(long = "source")]
        sources: Vec<String>,
        /// Pretty-print the JSON result.
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    /// List discovered skills, tools, and override diagnostics from one or more source directories.
    List {
        /// Load skills from a source directory. Repeat to list multiple sources.
        #[arg(long = "source")]
        sources: Vec<String>,
        /// Pretty-print the JSON result.
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
}

#[derive(Subcommand, Debug)]
enum MemoryCmd {
    Put {
        #[arg(long)]
        key: String,
        #[arg(long)]
        value: String,
        #[arg(long)]
        tag: Vec<String>,
        #[arg(long)]
        store: Option<String>,
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    Query {
        #[arg(long)]
        prefix: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        store: Option<String>,
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    Compact {
        #[arg(long)]
        store: Option<String>,
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
}

#[derive(Subcommand, Debug)]
enum ConfigCmd {
    List {
        #[arg(long)]
        scope: Option<String>,
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    Get {
        key: String,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    Set {
        key: String,
        value: String,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    Unset {
        key: String,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    Schema {
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    Doctor {
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let root = args.root.clone();
    let config_manager = ConfigManager::new(root.clone())?;
    let root_overrides = build_root_overrides(&args)?;

    match args.cmd {
        Cmd::Config { cmd } => {
            handle_config_command(&config_manager, cmd)?;
        }
        Cmd::Tool {
            name,
            input,
            pretty,
            state_file,
        } => {
            let effective = config_manager.resolve_effective(&root_overrides)?;
            let mode = effective.security.execution_mode;
            let allow_list = effective.security.shell_allow_list.clone();
            let audit_sink =
                build_audit_sink(&config_manager, effective.audit.jsonl_path.as_deref());
            let policy: std::sync::Arc<dyn deepagents::approval::ApprovalPolicy> =
                std::sync::Arc::new(DefaultApprovalPolicy::new(allow_list.clone()));
            let backend_shell_allow = match mode {
                ExecutionMode::NonInteractive => Some(allow_list.clone()),
                ExecutionMode::Interactive => {
                    if allow_list.is_empty() {
                        None
                    } else {
                        Some(allow_list.clone())
                    }
                }
            };
            let backend =
                deepagents::create_local_sandbox_backend(root.clone(), backend_shell_allow)?;
            let agent = deepagents::create_deep_agent_with_backend(backend);
            let json: serde_json::Value =
                serde_json::from_str(&input).map_err(|e| anyhow!("invalid --input json: {e}"))?;
            if let Some(state_file) = state_file {
                let mut state = load_state(&state_file).unwrap_or_default();
                let execute_cmd = if name == "execute" {
                    json.get("command")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                };
                if name == "execute" {
                    if let Some(cmd) = execute_cmd.as_deref() {
                        let req = ApprovalRequest {
                            command: cmd.to_string(),
                            root: root.clone(),
                            mode,
                        };
                        let decision = policy.decide(&req);
                        match decision {
                            ApprovalDecision::Allow { .. } => {}
                            ApprovalDecision::Deny { code, reason } => {
                                if let Some(sink) = &audit_sink {
                                    let _ = sink.record(AuditEvent {
                                        timestamp_ms: now_ms(),
                                        root: root.clone(),
                                        mode: mode_str(mode),
                                        command_redacted: redact_command(cmd),
                                        decision: "deny".to_string(),
                                        decision_code: code.clone(),
                                        decision_reason: reason.clone(),
                                        exit_code: None,
                                        truncated: None,
                                        duration_ms: None,
                                    });
                                }
                                let err = anyhow!("command_not_allowed: {}: {}", code, reason);
                                let _ = save_state(&state_file, &state);
                                let resp = serde_json::json!({
                                    "output": serde_json::Value::Null,
                                    "state": state,
                                    "delta": serde_json::Value::Null,
                                    "error": err.to_string()
                                });
                                if pretty {
                                    println!("{}", serde_json::to_string_pretty(&resp)?);
                                } else {
                                    println!("{}", serde_json::to_string(&resp)?);
                                }
                                return Err(err);
                            }
                            ApprovalDecision::RequireApproval { code, reason } => {
                                if let Some(sink) = &audit_sink {
                                    let _ = sink.record(AuditEvent {
                                        timestamp_ms: now_ms(),
                                        root: root.clone(),
                                        mode: mode_str(mode),
                                        command_redacted: redact_command(cmd),
                                        decision: "require_approval".to_string(),
                                        decision_code: code.clone(),
                                        decision_reason: reason.clone(),
                                        exit_code: None,
                                        truncated: None,
                                        duration_ms: None,
                                    });
                                }
                                let err = anyhow!("command_not_allowed: {}: {}", code, reason);
                                let _ = save_state(&state_file, &state);
                                let resp = serde_json::json!({
                                    "output": serde_json::Value::Null,
                                    "state": state,
                                    "delta": serde_json::Value::Null,
                                    "error": err.to_string()
                                });
                                if pretty {
                                    println!("{}", serde_json::to_string_pretty(&resp)?);
                                } else {
                                    println!("{}", serde_json::to_string(&resp)?);
                                }
                                return Err(err);
                            }
                        }
                    }
                }

                let started = std::time::Instant::now();
                let result = agent.call_tool_stateful(&name, json, &mut state).await;
                match result {
                    Ok((out, delta)) => {
                        save_state(&state_file, &state)?;
                        if name == "execute" {
                            if let (Some(sink), Some(cmd)) = (&audit_sink, execute_cmd.as_deref()) {
                                let _ = sink.record(AuditEvent {
                                    timestamp_ms: now_ms(),
                                    root: root.clone(),
                                    mode: mode_str(mode),
                                    command_redacted: redact_command(cmd),
                                    decision: "allow".to_string(),
                                    decision_code: "allow".to_string(),
                                    decision_reason: "allowed".to_string(),
                                    exit_code: out
                                        .output
                                        .get("exit_code")
                                        .and_then(|v| v.as_i64())
                                        .map(|v| v as i32),
                                    truncated: out
                                        .output
                                        .get("truncated")
                                        .and_then(|v| v.as_bool()),
                                    duration_ms: Some(started.elapsed().as_millis() as u64),
                                });
                            }
                        }
                        let resp = serde_json::json!({
                            "output": out.output,
                            "content_blocks": out.content_blocks,
                            "state": state,
                            "delta": delta,
                            "error": serde_json::Value::Null
                        });
                        if pretty {
                            println!("{}", serde_json::to_string_pretty(&resp)?);
                        } else {
                            println!("{}", serde_json::to_string(&resp)?);
                        }
                    }
                    Err(e) => {
                        let _ = save_state(&state_file, &state);
                        let resp = serde_json::json!({
                            "output": serde_json::Value::Null,
                            "state": state,
                            "delta": serde_json::Value::Null,
                            "error": e.to_string()
                        });
                        if pretty {
                            println!("{}", serde_json::to_string_pretty(&resp)?);
                        } else {
                            println!("{}", serde_json::to_string(&resp)?);
                        }
                        return Err(e);
                    }
                }
            } else {
                let execute_cmd = if name == "execute" {
                    json.get("command")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                };
                if name == "execute" {
                    if let Some(cmd) = execute_cmd.as_deref() {
                        let req = ApprovalRequest {
                            command: cmd.to_string(),
                            root: root.clone(),
                            mode,
                        };
                        let decision = policy.decide(&req);
                        match decision {
                            ApprovalDecision::Allow { .. } => {}
                            ApprovalDecision::Deny { code, reason } => {
                                if let Some(sink) = &audit_sink {
                                    let _ = sink.record(AuditEvent {
                                        timestamp_ms: now_ms(),
                                        root: root.clone(),
                                        mode: mode_str(mode),
                                        command_redacted: redact_command(cmd),
                                        decision: "deny".to_string(),
                                        decision_code: code.clone(),
                                        decision_reason: reason.clone(),
                                        exit_code: None,
                                        truncated: None,
                                        duration_ms: None,
                                    });
                                }
                                return Err(anyhow!("command_not_allowed: {}: {}", code, reason));
                            }
                            ApprovalDecision::RequireApproval { code, reason } => {
                                if let Some(sink) = &audit_sink {
                                    let _ = sink.record(AuditEvent {
                                        timestamp_ms: now_ms(),
                                        root: root.clone(),
                                        mode: mode_str(mode),
                                        command_redacted: redact_command(cmd),
                                        decision: "require_approval".to_string(),
                                        decision_code: code.clone(),
                                        decision_reason: reason.clone(),
                                        exit_code: None,
                                        truncated: None,
                                        duration_ms: None,
                                    });
                                }
                                return Err(anyhow!("command_not_allowed: {}: {}", code, reason));
                            }
                        }
                    }
                }
                let started = std::time::Instant::now();
                let out = agent.call_tool(&name, json).await?;
                if name == "execute" {
                    if let (Some(sink), Some(cmd)) = (&audit_sink, execute_cmd.as_deref()) {
                        let _ = sink.record(AuditEvent {
                            timestamp_ms: now_ms(),
                            root: root.clone(),
                            mode: mode_str(mode),
                            command_redacted: redact_command(cmd),
                            decision: "allow".to_string(),
                            decision_code: "allow".to_string(),
                            decision_reason: "allowed".to_string(),
                            exit_code: out
                                .get("exit_code")
                                .and_then(|v| v.as_i64())
                                .map(|v| v as i32),
                            truncated: out.get("truncated").and_then(|v| v.as_bool()),
                            duration_ms: Some(started.elapsed().as_millis() as u64),
                        });
                    }
                }
                if pretty {
                    println!("{}", serde_json::to_string_pretty(&out)?);
                } else {
                    println!("{}", serde_json::to_string(&out)?);
                }
            }
        }
        Cmd::Run {
            input,
            provider,
            model,
            base_url,
            api_key,
            api_key_env,
            tool_choice,
            structured_output_schema,
            structured_output_name,
            structured_output_description,
            thread_id,
            mock_script,
            skills_source,
            skills_skip_invalid,
            memory_source,
            memory_allow_host_paths,
            memory_max_injected_chars,
            memory_disable,
            max_steps,
            provider_timeout_ms,
            prompt_cache,
            prompt_cache_l2,
            prompt_cache_ttl_ms,
            prompt_cache_max_entries,
            summarization_disable,
            summarization_max_char_budget,
            summarization_max_turns_visible,
            summarization_min_recent_messages,
            summarization_redact_tool_args,
            summarization_max_tool_arg_chars,
            summarization_truncate_keep_last,
            interrupt_on,
            events_jsonl,
            stream_events,
            pretty,
        } => {
            let mut overrides = root_overrides.clone();
            overrides.extend(build_run_overrides(
                &provider,
                model.as_deref(),
                base_url.as_deref(),
                api_key_env.as_deref(),
                &memory_source,
                memory_allow_host_paths,
                memory_max_injected_chars,
                memory_disable,
                max_steps,
                provider_timeout_ms,
                prompt_cache.as_deref(),
                prompt_cache_l2,
                prompt_cache_ttl_ms,
                prompt_cache_max_entries,
                summarization_disable,
                summarization_max_char_budget,
                summarization_max_turns_visible,
                summarization_min_recent_messages,
                summarization_redact_tool_args,
                summarization_max_tool_arg_chars,
                summarization_truncate_keep_last,
            )?);
            let effective = config_manager.resolve_effective(&overrides)?;
            let mode = effective.security.execution_mode;
            let allow_list = effective.security.shell_allow_list.clone();
            let audit_sink =
                build_audit_sink(&config_manager, effective.audit.jsonl_path.as_deref());
            let policy: std::sync::Arc<dyn deepagents::approval::ApprovalPolicy> =
                std::sync::Arc::new(DefaultApprovalPolicy::new(allow_list.clone()));
            let backend_shell_allow = match mode {
                ExecutionMode::NonInteractive => Some(allow_list.clone()),
                ExecutionMode::Interactive => {
                    if allow_list.is_empty() {
                        None
                    } else {
                        Some(allow_list.clone())
                    }
                }
            };
            let backend =
                deepagents::create_local_sandbox_backend(root.clone(), backend_shell_allow)?;
            let agent = deepagents::create_deep_agent_with_backend(backend);
            let provider_bundle =
                build_provider_bundle(&provider, &effective, mock_script, api_key, api_key_env)?;
            let provider_id = provider.clone();
            let provider = provider_bundle.provider;
            let tool_choice = resolve_tool_choice(tool_choice.as_deref())?;
            let structured_output = resolve_structured_output(
                structured_output_schema.as_deref(),
                structured_output_name.as_deref(),
                structured_output_description.as_deref(),
            )?;
            ensure_provider_request_supported(
                &provider_bundle.diagnostics,
                &tool_choice,
                structured_output.as_ref(),
            )?;
            if stream_events {
                eprintln!("{}", serde_json::to_string(&provider_bundle.diagnostics)?);
            }

            let subagent_registry = deepagents::subagents::builtins::default_registry()?;
            let subagent_mw: std::sync::Arc<dyn deepagents::runtime::RuntimeMiddleware> =
                std::sync::Arc::new(deepagents::subagents::SubAgentMiddleware::new(
                    subagent_registry,
                ));
            let patch_mw: std::sync::Arc<dyn deepagents::runtime::RuntimeMiddleware> =
                std::sync::Arc::new(
                    deepagents::runtime::patch_tool_calls::PatchToolCallsMiddleware::new(),
                );
            let mut asm = deepagents::runtime::RuntimeMiddlewareAssembler::new();
            asm.push(
                deepagents::runtime::RuntimeMiddlewareSlot::TodoList,
                "todolist",
                std::sync::Arc::new(deepagents::runtime::TodoListMiddleware::new()),
            );

            if effective.memory.enabled {
                let sources = effective.memory.sources.clone();
                let options = deepagents::runtime::MemoryLoadOptions {
                    allow_host_paths: effective.memory.allow_host_paths,
                    max_injected_chars: effective.memory.max_injected_chars,
                    ..Default::default()
                };
                let memory_mw: std::sync::Arc<dyn deepagents::runtime::RuntimeMiddleware> =
                    std::sync::Arc::new(deepagents::runtime::MemoryMiddleware::new(
                        root.clone(),
                        sources,
                        options,
                    ));
                asm.push(
                    deepagents::runtime::RuntimeMiddlewareSlot::Memory,
                    "memory",
                    memory_mw,
                );
            }

            if !skills_source.is_empty() {
                let options = deepagents::skills::loader::SkillsLoadOptions {
                    skip_invalid_sources: skills_skip_invalid,
                    strict: true,
                };
                let skills_mw: std::sync::Arc<dyn deepagents::runtime::RuntimeMiddleware> =
                    std::sync::Arc::new(deepagents::runtime::SkillsMiddleware::new(
                        skills_source,
                        options,
                    ));
                asm.push(
                    deepagents::runtime::RuntimeMiddlewareSlot::Skills,
                    "skills",
                    skills_mw,
                );
            }

            asm.push(
                deepagents::runtime::RuntimeMiddlewareSlot::FilesystemRuntime,
                "filesystem_runtime",
                std::sync::Arc::new(deepagents::runtime::FilesystemRuntimeMiddleware::new(
                    deepagents::runtime::FilesystemRuntimeOptions::default(),
                )),
            );
            asm.push(
                deepagents::runtime::RuntimeMiddlewareSlot::Subagents,
                "subagents",
                subagent_mw,
            );

            if effective.runtime.summarization.enabled {
                let options = deepagents::runtime::SummarizationOptions {
                    policy: deepagents::runtime::SummarizationPolicyKind::Budget,
                    max_char_budget: effective.runtime.summarization.max_char_budget,
                    max_turns_visible: effective.runtime.summarization.max_turns_visible,
                    min_recent_messages: effective.runtime.summarization.min_recent_messages,
                    redact_tool_args: effective.runtime.summarization.redact_tool_args,
                    max_tool_arg_chars: effective.runtime.summarization.max_tool_arg_chars,
                    truncate_tool_args_keep_last: effective
                        .runtime
                        .summarization
                        .truncate_keep_last,
                    ..Default::default()
                };
                let summarization_mw: std::sync::Arc<dyn deepagents::runtime::RuntimeMiddleware> =
                    std::sync::Arc::new(deepagents::runtime::SummarizationMiddleware::new(
                        root.clone(),
                        options,
                    ));
                asm.push(
                    deepagents::runtime::RuntimeMiddlewareSlot::Summarization,
                    "summarization",
                    summarization_mw,
                );
            }

            let model_id = effective
                .provider(&provider_id)
                .and_then(|p| p.model.clone())
                .unwrap_or_default();
            let prompt_cache_options = deepagents::runtime::PromptCacheOptions {
                enabled: matches!(
                    effective.runtime.prompt_cache.backend,
                    PromptCacheBackendKind::Memory
                ),
                backend: deepagents::runtime::CacheBackend::Memory,
                native: match effective.runtime.prompt_cache.native {
                    deepagents::config::PromptCacheNativeMode::Auto => {
                        deepagents::runtime::PromptCacheNativeMode::Auto
                    }
                    deepagents::config::PromptCacheNativeMode::Off => {
                        deepagents::runtime::PromptCacheNativeMode::Off
                    }
                    deepagents::config::PromptCacheNativeMode::Required => {
                        deepagents::runtime::PromptCacheNativeMode::Required
                    }
                },
                layout: match effective.runtime.prompt_cache.layout {
                    deepagents::config::PromptCacheLayoutMode::Auto => {
                        deepagents::runtime::PromptCacheLayoutMode::Auto
                    }
                    deepagents::config::PromptCacheLayoutMode::SingleSystem => {
                        deepagents::runtime::PromptCacheLayoutMode::SingleSystem
                    }
                    deepagents::config::PromptCacheLayoutMode::PreservePrefixSegments => {
                        deepagents::runtime::PromptCacheLayoutMode::PreservePrefixSegments
                    }
                },
                enable_l2_response_cache: effective.runtime.prompt_cache.l2,
                ttl_ms: effective.runtime.prompt_cache.ttl_ms,
                max_entries: effective.runtime.prompt_cache.max_entries,
                provider_id,
                model_id,
                partition: root.clone(),
            };
            asm.push(
                deepagents::runtime::RuntimeMiddlewareSlot::PromptCaching,
                "prompt_caching",
                std::sync::Arc::new(deepagents::runtime::PromptCachingMiddleware::new(
                    prompt_cache_options,
                )),
            );
            asm.push(
                deepagents::runtime::RuntimeMiddlewareSlot::PatchToolCalls,
                "patch_tool_calls",
                patch_mw,
            );

            let runtime_middlewares = asm.build()?;

            let mut initial_state = deepagents::state::AgentState::default();
            if let Some(tid) = thread_id {
                initial_state
                    .extra
                    .insert("thread_id".to_string(), serde_json::Value::String(tid));
            }

            let mut interrupt_on_map = std::collections::BTreeMap::new();
            for t in interrupt_on {
                interrupt_on_map.insert(t, true);
            }
            if interrupt_on_map.is_empty() && matches!(mode, ExecutionMode::Interactive) {
                for k in ["write_file", "edit_file", "delete_file", "execute"] {
                    interrupt_on_map.insert(k.to_string(), true);
                }
            }

            let mut runner = deepagents::runtime::ResumableRunner::new(
                agent,
                provider,
                deepagents::runtime::ResumableRunnerOptions {
                    config: deepagents::runtime::RuntimeConfig {
                        max_steps: effective.runtime.max_steps,
                        provider_timeout_ms: effective.runtime.provider_timeout_ms,
                    },
                    approval: Some(policy),
                    audit: audit_sink,
                    root: root.clone(),
                    mode,
                    interrupt_on: interrupt_on_map,
                },
            )
            .with_runtime_middlewares(runtime_middlewares)
            .with_initial_state(initial_state)
            .with_tool_choice(tool_choice);
            if let Some(structured_output) = structured_output {
                runner = runner.with_structured_output(structured_output);
            }

            runner.push_user_input(input);
            let use_event_stream = events_jsonl.is_some() || stream_events;
            let mut event_sink = if use_event_stream {
                Some(CliRunEventSink::new(
                    events_jsonl.as_deref(),
                    stream_events,
                )?)
            } else {
                None
            };
            let mut out = if let Some(sink) = event_sink.as_mut() {
                runner.run_with_events(sink).await
            } else {
                runner.run().await
            };

            if matches!(mode, ExecutionMode::Interactive) {
                let mut stderr = std::io::stderr();
                loop {
                    if out.status != deepagents::runtime::RunStatus::Interrupted {
                        break;
                    }
                    let Some(interrupt) = out.interrupts.first().cloned() else {
                        break;
                    };
                    eprintln!(
                        "HITL interrupt: tool={} tool_call_id={}",
                        interrupt.tool_name, interrupt.tool_call_id
                    );
                    eprintln!(
                        "proposed_args={}",
                        serde_json::to_string_pretty(&interrupt.proposed_args)
                            .unwrap_or_else(|_| interrupt.proposed_args.to_string())
                    );
                    let decision = loop {
                        eprint!("decision [a=approve,r=reject,e=edit]> ");
                        let _ = std::io::Write::flush(&mut stderr);
                        let mut line = String::new();
                        std::io::stdin().read_line(&mut line)?;
                        match line.trim() {
                            "a" | "approve" => break deepagents::runtime::HitlDecision::Approve,
                            "r" | "reject" => {
                                break deepagents::runtime::HitlDecision::Reject { reason: None }
                            }
                            "e" | "edit" => {
                                eprint!("edit args JSON> ");
                                let _ = std::io::Write::flush(&mut stderr);
                                let mut args_line = String::new();
                                std::io::stdin().read_line(&mut args_line)?;
                                match serde_json::from_str::<serde_json::Value>(args_line.trim()) {
                                    Ok(v) => {
                                        break deepagents::runtime::HitlDecision::Edit { args: v }
                                    }
                                    Err(e) => {
                                        eprintln!("invalid JSON: {}", e);
                                        continue;
                                    }
                                }
                            }
                            other => {
                                eprintln!("unknown decision: {}", other);
                                continue;
                            }
                        }
                    };
                    out = if let Some(sink) = event_sink.as_mut() {
                        runner
                            .resume_with_events(&interrupt.interrupt_id, decision, sink)
                            .await
                    } else {
                        runner.resume(&interrupt.interrupt_id, decision).await
                    };
                }
            } else if out.status == deepagents::runtime::RunStatus::Interrupted {
                if pretty {
                    println!("{}", serde_json::to_string_pretty(&out)?);
                } else {
                    println!("{}", serde_json::to_string(&out)?);
                }
                std::process::exit(2);
            }

            let ok = out.error.is_none() && out.status == deepagents::runtime::RunStatus::Completed;
            if pretty {
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("{}", serde_json::to_string(&out)?);
            }
            if !ok {
                return Err(anyhow!("runtime_error"));
            }
        }
        Cmd::Skill { cmd } => match cmd {
            SkillCmd::Init { dir, pretty } => {
                handle_skill_command("init", pretty, init_skill_template(&dir))?;
            }
            SkillCmd::Validate { sources, pretty } => {
                handle_skill_command(
                    "validate",
                    pretty,
                    load_skills_from_sources(&sources)
                        .map(|loaded| build_skill_report("validate", loaded)),
                )?;
            }
            SkillCmd::List { sources, pretty } => {
                handle_skill_command(
                    "list",
                    pretty,
                    load_skills_from_sources(&sources)
                        .map(|loaded| build_skill_report("list", loaded)),
                )?;
            }
        },
        Cmd::Memory { cmd } => match cmd {
            MemoryCmd::Put {
                key,
                value,
                tag,
                store,
                pretty,
            } => {
                if looks_like_secret(&value) {
                    return Err(anyhow!("invalid_request: value looks like a secret"));
                }
                let effective = config_manager.resolve_effective(&root_overrides)?;
                let store_path =
                    resolve_memory_store_path(&config_manager, &effective, store.as_deref());
                let store = deepagents::memory::FileMemoryStore::new(store_path);
                store.load().await?;
                store
                    .put(deepagents::memory::MemoryEntry {
                        key,
                        value,
                        tags: tag,
                        created_at: String::new(),
                        updated_at: String::new(),
                        last_accessed_at: String::new(),
                        access_count: 0,
                    })
                    .await?;
                let report = store.evict_if_needed().await?;
                store.flush().await?;
                let _ = store.render_agents_md().await;
                let out = serde_json::json!({ "status": "ok", "eviction": report });
                print_json_value(out, pretty)?;
            }
            MemoryCmd::Query {
                prefix,
                tag,
                limit,
                store,
                pretty,
            } => {
                let effective = config_manager.resolve_effective(&root_overrides)?;
                let store_path =
                    resolve_memory_store_path(&config_manager, &effective, store.as_deref());
                let store = deepagents::memory::FileMemoryStore::new(store_path);
                store.load().await?;
                let entries = store
                    .query(deepagents::memory::MemoryQuery {
                        prefix,
                        tag,
                        limit: Some(limit),
                    })
                    .await?;
                let out = serde_json::json!({ "entries": entries });
                print_json_value(out, pretty)?;
            }
            MemoryCmd::Compact { store, pretty } => {
                let effective = config_manager.resolve_effective(&root_overrides)?;
                let store_path =
                    resolve_memory_store_path(&config_manager, &effective, store.as_deref());
                let store = deepagents::memory::FileMemoryStore::new(store_path);
                store.load().await?;
                let report = store.evict_if_needed().await?;
                store.flush().await?;
                let _ = store.render_agents_md().await;
                let out = serde_json::json!({ "status": "ok", "eviction": report });
                print_json_value(out, pretty)?;
            }
        },
    }
    Ok(())
}

fn handle_config_command(config_manager: &ConfigManager, cmd: ConfigCmd) -> Result<()> {
    match cmd {
        ConfigCmd::List { scope, pretty } => {
            let scope = parse_config_scope(scope.as_deref(), ConfigScope::Effective)?;
            let entries = config_manager.list(scope, &ConfigOverrides::new())?;
            let out = serde_json::json!({ "scope": scope, "entries": entries });
            print_json_value(out, pretty)?;
        }
        ConfigCmd::Get { key, scope, pretty } => {
            let scope = parse_config_scope(scope.as_deref(), ConfigScope::Effective)?;
            let key = ConfigKey::parse(key)?;
            let out =
                serde_json::to_value(config_manager.get(scope, &key, &ConfigOverrides::new())?)?;
            print_json_value(out, pretty)?;
        }
        ConfigCmd::Set {
            key,
            value,
            scope,
            pretty,
        } => {
            let scope = parse_config_scope(scope.as_deref(), ConfigScope::Workspace)?;
            let key = ConfigKey::parse(key)?;
            let value = config_manager.parse_cli_value(&key, &value)?;
            config_manager.set(scope, &key, value)?;
            let out = serde_json::json!({ "status": "ok", "scope": scope, "key": key });
            print_json_value(out, pretty)?;
        }
        ConfigCmd::Unset { key, scope, pretty } => {
            let scope = parse_config_scope(scope.as_deref(), ConfigScope::Workspace)?;
            let key = ConfigKey::parse(key)?;
            config_manager.unset(scope, &key)?;
            let out = serde_json::json!({ "status": "ok", "scope": scope, "key": key });
            print_json_value(out, pretty)?;
        }
        ConfigCmd::Schema { pretty } => {
            let out = serde_json::to_value(config_manager.schema())?;
            print_json_value(out, pretty)?;
        }
        ConfigCmd::Doctor { pretty } => {
            let out = serde_json::to_value(config_manager.doctor(&ConfigOverrides::new())?)?;
            print_json_value(out, pretty)?;
        }
    }
    Ok(())
}

fn parse_config_scope(flag: Option<&str>, default: ConfigScope) -> Result<ConfigScope> {
    match flag {
        Some(value) => Ok(ConfigScope::parse(value)?),
        None => Ok(default),
    }
}

fn resolve_memory_store_path(
    config_manager: &ConfigManager,
    effective: &EffectiveConfig,
    store: Option<&str>,
) -> std::path::PathBuf {
    if let Some(s) = store {
        return std::path::PathBuf::from(s);
    }
    config_manager.resolve_path(&effective.memory.store_path)
}

fn looks_like_secret(s: &str) -> bool {
    let v = s.to_lowercase();
    if v.contains("begin private key") {
        return true;
    }
    if v.contains("aws_secret_access_key") {
        return true;
    }
    if v.contains("api_key") && v.contains("=") {
        return true;
    }
    if s.trim_start().starts_with("sk-") {
        return true;
    }
    false
}

fn load_skills_from_sources(sources: &[String]) -> Result<deepagents::skills::LoadedSkills> {
    if sources.is_empty() {
        return Err(anyhow!("invalid_arguments: --source is required"));
    }
    let options = deepagents::skills::loader::SkillsLoadOptions {
        skip_invalid_sources: false,
        strict: true,
    };
    deepagents::skills::loader::load_skills(sources, options)
}

/// Prints a structured JSON result for a skill command and exits with a stable
/// non-zero code on failure.
fn handle_skill_command(
    command: &'static str,
    pretty: bool,
    result: Result<serde_json::Value>,
) -> Result<()> {
    match result {
        Ok(value) => {
            print_json_value(value, pretty)?;
            Ok(())
        }
        Err(error) => {
            print_json_value(skill_command_error_output(command, &error), pretty)?;
            std::process::exit(1);
        }
    }
}

/// Builds the machine-readable failure payload used by `skill` subcommands.
fn skill_command_error_output(command: &'static str, error: &anyhow::Error) -> serde_json::Value {
    serde_json::json!({
        "ok": false,
        "command": command,
        "error": {
            "code": classify_skill_command_error(error),
            "message": format_error_chain(error),
        }
    })
}

/// Maps skill command failures onto a stable coarse-grained error code set for
/// CI and black-box tests.
fn classify_skill_command_error(error: &anyhow::Error) -> &'static str {
    let message = error.to_string();
    if message.starts_with("invalid_source:") {
        return "invalid_source";
    }
    if message.starts_with("invalid_arguments:") {
        return "invalid_arguments";
    }
    if message.contains("tool_conflict_with_core:") {
        return "tool_conflict_with_core";
    }
    if message.contains("template_write_failed:") {
        return "template_write_failed";
    }
    "skill_validation_failed"
}

/// Builds the stable success payload shared by `skill validate` and `skill
/// list`.
fn build_skill_report(
    command: &'static str,
    loaded: deepagents::skills::LoadedSkills,
) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "command": command,
        "summary": {
            "sources": loaded.diagnostics.sources.len(),
            "skills": loaded.metadata.len(),
            "tools": loaded.tools.len(),
            "overrides": loaded.diagnostics.overrides.len(),
        },
        "skills": loaded.metadata,
        "tools": loaded.tools,
        "diagnostics": loaded.diagnostics,
    })
}

/// Flattens an anyhow chain into one readable message so file and field details
/// survive JSON serialization.
fn format_error_chain(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(|cause| cause.to_string())
        .collect::<Vec<_>>()
        .join(": ")
}

fn print_json_value(value: serde_json::Value, pretty: bool) -> Result<()> {
    if pretty {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{}", serde_json::to_string(&value)?);
    }
    Ok(())
}

/// Creates a skill package scaffold and validates it immediately so the
/// generated template stays release-ready.
fn init_skill_template(dir: &str) -> Result<serde_json::Value> {
    let path = std::path::PathBuf::from(dir);
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("template_write_failed: skill init requires a directory path"))?;
    ensure_valid_skill_init_name(name)?;
    if path.join("SKILL.md").exists() || path.join("tools.json").exists() {
        return Err(anyhow!(
            "template_write_failed: {} already contains SKILL.md or tools.json",
            path.display()
        ));
    }
    std::fs::create_dir_all(&path)?;
    let skill_md = format!(
        "---\nname: {name}\ndescription: Describe what this skill does and when to use it.\n---\n\n# {name}\n\n## When to Use\n- \n\n## Steps\n- \n",
    );
    let tools_json = serde_json::json!({
        "tools": [{
            "name": name,
            "description": "Describe the tool behavior.",
            "input_schema": {
                "type": "object",
                "properties": { "file_path": { "type": "string" } },
                "required": ["file_path"],
                "additionalProperties": false
            },
            "steps": [{ "tool_name": "read_file", "arguments": { "limit": 20 } }],
            "policy": { "allow_filesystem": true, "allow_execute": false, "allow_network": false }
        }]
    });
    let skill_md_path = path.join("SKILL.md");
    let tools_json_path = path.join("tools.json");
    std::fs::write(&skill_md_path, skill_md)?;
    std::fs::write(&tools_json_path, serde_json::to_vec_pretty(&tools_json)?)?;

    let source_root = path
        .parent()
        .map(skill_source_name)
        .unwrap_or_else(|| ".".to_string());
    let package = deepagents::skills::validator::load_skill_dir(
        &path,
        &source_root,
        deepagents::skills::validator::SkillValidationOptions::default(),
    )?;

    Ok(serde_json::json!({
        "ok": true,
        "command": "init",
        "skill": package.metadata,
        "tools": package.tools,
        "source_root": path.parent().map(|parent| parent.to_string_lossy().to_string()),
        "files": [
            skill_md_path.to_string_lossy().to_string(),
            tools_json_path.to_string_lossy().to_string(),
        ],
    }))
}

/// Enforces the package-skill naming contract on the target directory name
/// before files are written.
fn ensure_valid_skill_init_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || name.starts_with('-')
        || name.ends_with('-')
        || name.contains("--")
        || !name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(anyhow!(
            "template_write_failed: skill directory name must use lowercase ASCII letters, digits, and '-' and match the skill name contract"
        ));
    }
    Ok(())
}

/// Derives the human-readable source label used by the loader from a source
/// directory path.
fn skill_source_name(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|segment| segment.to_str())
        .map(|segment| segment.to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn load_state(path: &str) -> Option<deepagents::state::AgentState> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn save_state(path: &str, state: &deepagents::state::AgentState) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(state)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

fn build_root_overrides(args: &Args) -> Result<ConfigOverrides> {
    let mut overrides = ConfigOverrides::new();

    let execution_mode = args
        .execution_mode
        .clone()
        .or_else(|| std::env::var("DEEPAGENTS_EXECUTION_MODE").ok())
        .map(|value| match value.as_str() {
            "non-interactive" => "non_interactive".to_string(),
            other => other.to_string(),
        });
    if let Some(execution_mode) = execution_mode {
        insert_override(
            &mut overrides,
            "security.execution_mode",
            ConfigValue::String(execution_mode),
        )?;
    }

    if let Some(audit_json) = args
        .audit_json
        .clone()
        .or_else(|| std::env::var("DEEPAGENTS_AUDIT_JSON").ok())
    {
        insert_override(
            &mut overrides,
            "audit.jsonl_path",
            ConfigValue::String(audit_json),
        )?;
    }

    let mut allow_list: Vec<String> = Vec::new();
    let cli_has_any = !args.shell_allow.is_empty() || args.shell_allow_file.is_some();
    if cli_has_any {
        allow_list.extend(args.shell_allow.iter().cloned());
        if let Some(path) = args.shell_allow_file.as_deref() {
            allow_list.extend(read_allow_file(path)?);
        }
    } else {
        if let Ok(value) = std::env::var("DEEPAGENTS_SHELL_ALLOW") {
            allow_list.extend(
                value
                    .split(',')
                    .map(|item| item.trim().to_string())
                    .filter(|item| !item.is_empty()),
            );
        }
        if let Ok(path) = std::env::var("DEEPAGENTS_SHELL_ALLOW_FILE") {
            allow_list.extend(read_allow_file(&path)?);
        }
    }
    let allow_list = normalize_allow_list(allow_list);
    if cli_has_any || !allow_list.is_empty() {
        insert_override(
            &mut overrides,
            "security.shell_allow_list",
            ConfigValue::StringList(allow_list),
        )?;
    }

    Ok(overrides)
}

#[allow(clippy::too_many_arguments)]
fn build_run_overrides(
    provider: &str,
    model: Option<&str>,
    base_url: Option<&str>,
    api_key_env: Option<&str>,
    memory_source: &[String],
    memory_allow_host_paths: bool,
    memory_max_injected_chars: Option<usize>,
    memory_disable: bool,
    max_steps: Option<usize>,
    provider_timeout_ms: Option<u64>,
    prompt_cache: Option<&str>,
    prompt_cache_l2: bool,
    prompt_cache_ttl_ms: Option<u64>,
    prompt_cache_max_entries: Option<usize>,
    summarization_disable: bool,
    summarization_max_char_budget: Option<usize>,
    summarization_max_turns_visible: Option<usize>,
    summarization_min_recent_messages: Option<usize>,
    summarization_redact_tool_args: Option<bool>,
    summarization_max_tool_arg_chars: Option<usize>,
    summarization_truncate_keep_last: Option<usize>,
) -> Result<ConfigOverrides> {
    let mut overrides = ConfigOverrides::new();
    let provider_id = canonical_provider_id(provider);

    if provider_id != "mock" && provider_id != "mock2" {
        insert_override(
            &mut overrides,
            &format!("providers.{provider_id}.enabled"),
            ConfigValue::Boolean(true),
        )?;
    }
    if let Some(model) = model {
        insert_override(
            &mut overrides,
            &format!("providers.{provider_id}.model"),
            ConfigValue::String(model.to_string()),
        )?;
    }
    if let Some(base_url) = base_url {
        insert_override(
            &mut overrides,
            &format!("providers.{provider_id}.base_url"),
            ConfigValue::String(base_url.to_string()),
        )?;
    }
    if let Some(api_key_env) = api_key_env {
        insert_override(
            &mut overrides,
            &format!("providers.{provider_id}.api_key_env"),
            ConfigValue::String(api_key_env.to_string()),
        )?;
    }
    if !memory_source.is_empty() {
        insert_override(
            &mut overrides,
            "memory.file.sources",
            ConfigValue::StringList(memory_source.to_vec()),
        )?;
    }
    if memory_allow_host_paths {
        insert_override(
            &mut overrides,
            "memory.file.allow_host_paths",
            ConfigValue::Boolean(true),
        )?;
    }
    if let Some(max_chars) = memory_max_injected_chars {
        insert_override(
            &mut overrides,
            "memory.file.max_injected_chars",
            ConfigValue::Integer(max_chars as i64),
        )?;
    }
    if memory_disable {
        insert_override(
            &mut overrides,
            "memory.file.enabled",
            ConfigValue::Boolean(false),
        )?;
    }
    if let Some(max_steps) = max_steps {
        insert_override(
            &mut overrides,
            "runtime.max_steps",
            ConfigValue::Integer(max_steps as i64),
        )?;
    }
    if let Some(timeout_ms) = provider_timeout_ms {
        insert_override(
            &mut overrides,
            "runtime.provider_timeout_ms",
            ConfigValue::Integer(timeout_ms as i64),
        )?;
    }
    if let Some(prompt_cache) = prompt_cache {
        insert_override(
            &mut overrides,
            "runtime.prompt_cache.backend",
            ConfigValue::String(prompt_cache.to_string()),
        )?;
    }
    if prompt_cache_l2 {
        insert_override(
            &mut overrides,
            "runtime.prompt_cache.l2",
            ConfigValue::Boolean(true),
        )?;
    }
    if let Some(ttl_ms) = prompt_cache_ttl_ms {
        insert_override(
            &mut overrides,
            "runtime.prompt_cache.ttl_ms",
            ConfigValue::Integer(ttl_ms as i64),
        )?;
    }
    if let Some(max_entries) = prompt_cache_max_entries {
        insert_override(
            &mut overrides,
            "runtime.prompt_cache.max_entries",
            ConfigValue::Integer(max_entries as i64),
        )?;
    }
    if summarization_disable {
        insert_override(
            &mut overrides,
            "runtime.summarization.enabled",
            ConfigValue::Boolean(false),
        )?;
    }
    if let Some(value) = summarization_max_char_budget {
        insert_override(
            &mut overrides,
            "runtime.summarization.max_char_budget",
            ConfigValue::Integer(value as i64),
        )?;
    }
    if let Some(value) = summarization_max_turns_visible {
        insert_override(
            &mut overrides,
            "runtime.summarization.max_turns_visible",
            ConfigValue::Integer(value as i64),
        )?;
    }
    if let Some(value) = summarization_min_recent_messages {
        insert_override(
            &mut overrides,
            "runtime.summarization.min_recent_messages",
            ConfigValue::Integer(value as i64),
        )?;
    }
    if let Some(value) = summarization_redact_tool_args {
        insert_override(
            &mut overrides,
            "runtime.summarization.redact_tool_args",
            ConfigValue::Boolean(value),
        )?;
    }
    if let Some(value) = summarization_max_tool_arg_chars {
        insert_override(
            &mut overrides,
            "runtime.summarization.max_tool_arg_chars",
            ConfigValue::Integer(value as i64),
        )?;
    }
    if let Some(value) = summarization_truncate_keep_last {
        insert_override(
            &mut overrides,
            "runtime.summarization.truncate_keep_last",
            ConfigValue::Integer(value as i64),
        )?;
    }

    Ok(overrides)
}

fn insert_override(overrides: &mut ConfigOverrides, key: &str, value: ConfigValue) -> Result<()> {
    let key = ConfigKey::parse(key.to_string())?;
    overrides.set(key, value);
    Ok(())
}

fn resolve_tool_choice(flag: Option<&str>) -> Result<deepagents::llm::ToolChoice> {
    let Some(flag) = flag.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(deepagents::llm::ToolChoice::Auto);
    };

    match flag {
        "auto" => Ok(deepagents::llm::ToolChoice::Auto),
        "none" => Ok(deepagents::llm::ToolChoice::None),
        "required" => Ok(deepagents::llm::ToolChoice::Required),
        _ => {
            if let Some(name) = flag.strip_prefix("named:") {
                let name = name.trim();
                if name.is_empty() {
                    anyhow::bail!("invalid --tool-choice: named tool cannot be empty");
                }
                return Ok(deepagents::llm::ToolChoice::Named {
                    name: name.to_string(),
                });
            }
            anyhow::bail!("invalid --tool-choice: expected auto|none|required|named:<tool>")
        }
    }
}

fn resolve_structured_output(
    schema_flag: Option<&str>,
    name_flag: Option<&str>,
    description_flag: Option<&str>,
) -> Result<Option<deepagents::llm::StructuredOutputSpec>> {
    let Some(schema_flag) = schema_flag.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };

    let schema_source = if let Some(path) = schema_flag.strip_prefix('@') {
        std::fs::read_to_string(path)
            .with_context(|| format!("invalid --structured-output-schema @file: {path}"))?
    } else {
        schema_flag.to_string()
    };
    let schema = serde_json::from_str(&schema_source)
        .map_err(|e| anyhow!("invalid --structured-output-schema json: {e}"))?;
    let spec = deepagents::llm::StructuredOutputSpec {
        name: name_flag
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("structured_output")
            .to_string(),
        schema,
        description: description_flag
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        strict: true,
    };
    spec.validate()?;
    Ok(Some(spec))
}

fn ensure_provider_request_supported(
    diagnostics: &deepagents::provider::ProviderDiagnostics,
    tool_choice: &deepagents::llm::ToolChoice,
    structured_output: Option<&deepagents::llm::StructuredOutputSpec>,
) -> Result<()> {
    if structured_output.is_some() && !diagnostics.supports_structured_output() {
        anyhow::bail!(
            "provider_unsupported_structured_output: {}",
            diagnostics.provider_id
        );
    }

    if matches!(
        tool_choice,
        deepagents::llm::ToolChoice::Required | deepagents::llm::ToolChoice::Named { .. }
    ) && !diagnostics.supports_tool_choice()
    {
        anyhow::bail!(
            "provider_unsupported_tool_calling: {}",
            diagnostics.provider_id
        );
    }

    Ok(())
}

fn read_allow_file(path: &str) -> Result<Vec<String>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read allow list: {path}"))?;
    let mut out = Vec::new();
    for line in content.lines() {
        let s = line.trim();
        if s.is_empty() {
            continue;
        }
        if s.starts_with('#') {
            continue;
        }
        out.push(s.to_string());
    }
    Ok(out)
}

fn normalize_allow_list(items: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for s in items {
        let t = s.trim();
        if t.is_empty() {
            continue;
        }
        if seen.insert(t.to_string()) {
            out.push(t.to_string());
        }
    }
    out
}

fn mode_str(mode: ExecutionMode) -> String {
    match mode {
        ExecutionMode::NonInteractive => "non_interactive".to_string(),
        ExecutionMode::Interactive => "interactive".to_string(),
    }
}

fn now_ms() -> i64 {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    dur.as_millis() as i64
}

struct CliRunEventSink {
    file: Option<std::fs::File>,
    echo_stderr: bool,
}

fn build_provider_bundle(
    provider_id: &str,
    effective: &EffectiveConfig,
    mock_script: Option<String>,
    api_key: Option<String>,
    explicit_api_key_env: Option<String>,
) -> Result<deepagents::provider::ProviderInitBundle> {
    let provider_id = canonical_provider_id(provider_id);
    match provider_id {
        "mock" => {
            let path = mock_script
                .ok_or_else(|| anyhow!("--mock-script is required for --provider mock"))?;
            let script = deepagents::provider::mock::MockProvider::load_from_file(&path)?;
            Ok(deepagents::provider::build_provider_bundle(
                provider_id,
                deepagents::provider::ProviderInitSpec::Mock {
                    script,
                    omit_call_ids: false,
                },
            ))
        }
        "mock2" => {
            let path = mock_script
                .ok_or_else(|| anyhow!("--mock-script is required for --provider mock2"))?;
            let script = deepagents::provider::mock::MockProvider::load_from_file(&path)?;
            Ok(deepagents::provider::build_provider_bundle(
                provider_id,
                deepagents::provider::ProviderInitSpec::Mock {
                    script,
                    omit_call_ids: true,
                },
            ))
        }
        "openai-compatible" | "openai_compatible" => {
            let provider_cfg = effective
                .provider("openai-compatible")
                .ok_or_else(|| anyhow!("missing config for provider openai-compatible"))?;
            let model = provider_cfg
                .model
                .clone()
                .ok_or_else(|| anyhow!("--model is required for --provider openai-compatible"))?;
            let mut config = deepagents::llm::OpenAiCompatibleConfig::new(model);
            if let Some(base_url) = provider_cfg.base_url.clone() {
                config = config.with_base_url(base_url);
            }
            let api_key = resolve_provider_api_key(
                api_key,
                explicit_api_key_env.as_deref(),
                provider_cfg
                    .api_key_env
                    .as_ref()
                    .map(|value| value.as_str()),
            )?;
            if let Some(api_key) = api_key {
                config = config.with_api_key(api_key);
            }
            Ok(deepagents::provider::build_provider_bundle(
                provider_id,
                deepagents::provider::ProviderInitSpec::OpenAiCompatible { config },
            ))
        }
        "openrouter" => {
            let provider_cfg = effective
                .provider("openrouter")
                .ok_or_else(|| anyhow!("missing config for provider openrouter"))?;
            let model = provider_cfg
                .model
                .clone()
                .ok_or_else(|| anyhow!("--model is required for --provider openrouter"))?;
            let mut config = deepagents::llm::OpenRouterConfig::new(model);
            if let Some(base_url) = provider_cfg.base_url.clone() {
                config = config.with_base_url(base_url);
            }
            let api_key = resolve_provider_api_key(
                api_key,
                explicit_api_key_env.as_deref(),
                provider_cfg
                    .api_key_env
                    .as_ref()
                    .map(|value| value.as_str()),
            )?;
            if let Some(api_key) = api_key {
                config = config.with_api_key(api_key);
            }
            Ok(deepagents::provider::build_provider_bundle(
                provider_id,
                deepagents::provider::ProviderInitSpec::OpenRouter { config },
            ))
        }
        other => Err(anyhow!("unknown --provider: {other}")),
    }
}

fn build_audit_sink(
    config_manager: &ConfigManager,
    audit_path: Option<&str>,
) -> Option<std::sync::Arc<dyn AuditSink>> {
    audit_path.map(|path| {
        let path = config_manager.resolve_path(path);
        let path_string = path.to_string_lossy().into_owned();
        std::sync::Arc::new(JsonlFileAuditSink::new(&path_string)) as std::sync::Arc<dyn AuditSink>
    })
}

fn canonical_provider_id(provider_id: &str) -> &str {
    match provider_id {
        "openai_compatible" => "openai-compatible",
        other => other,
    }
}

fn resolve_provider_api_key(
    direct_api_key: Option<String>,
    explicit_env_var: Option<&str>,
    configured_env_var: Option<&str>,
) -> Result<Option<String>> {
    if direct_api_key.is_some() {
        return Ok(direct_api_key);
    }
    if let Some(env_var) = explicit_env_var {
        return match std::env::var(env_var) {
            Ok(value) => Ok(Some(value)),
            Err(_) => Err(anyhow!("missing env var for api key: {env_var}")),
        };
    }
    let Some(env_var) = configured_env_var else {
        return Ok(None);
    };
    Ok(std::env::var(env_var).ok())
}

impl CliRunEventSink {
    fn new(path: Option<&str>, echo_stderr: bool) -> Result<Self> {
        let file = match path {
            Some(path) => Some(std::fs::File::create(path)?),
            None => None,
        };
        Ok(Self { file, echo_stderr })
    }
}

#[async_trait::async_trait]
impl deepagents::runtime::RunEventSink for CliRunEventSink {
    async fn emit(&mut self, event: deepagents::runtime::RunEvent) -> anyhow::Result<()> {
        use std::io::Write;

        let line = serde_json::to_string(&event)?;
        if let Some(file) = &mut self.file {
            writeln!(file, "{line}")?;
        }
        if self.echo_stderr {
            eprintln!("{line}");
        }
        Ok(())
    }
}

#[derive(Clone)]
struct JsonlFileAuditSink {
    path: std::path::PathBuf,
}

impl JsonlFileAuditSink {
    fn new(path: &str) -> Self {
        Self {
            path: std::path::PathBuf::from(path),
        }
    }
}

impl AuditSink for JsonlFileAuditSink {
    fn record(&self, event: AuditEvent) -> anyhow::Result<()> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let line = serde_json::to_string(&event)?;
        writeln!(f, "{}", line)?;
        Ok(())
    }
}
