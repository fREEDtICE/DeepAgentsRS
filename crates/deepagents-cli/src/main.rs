use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use deepagents::approval::{
    redact_command, ApprovalDecision, ApprovalRequest, DefaultApprovalPolicy, ExecutionMode,
};
use deepagents::audit::{AuditEvent, AuditSink};
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
        #[arg(long)]
        plugin: Vec<String>,
        #[arg(long = "skills-source")]
        skills_source: Vec<String>,
        #[arg(long, default_value_t = false)]
        skills_skip_invalid: bool,
        #[arg(long = "memory-source")]
        memory_source: Vec<String>,
        #[arg(long, default_value_t = false)]
        memory_allow_host_paths: bool,
        #[arg(long, default_value_t = 30000)]
        memory_max_injected_chars: usize,
        #[arg(long, default_value_t = false)]
        memory_disable: bool,
        #[arg(long, default_value_t = 8)]
        max_steps: usize,
        #[arg(long, default_value_t = 1000)]
        provider_timeout_ms: u64,
        #[arg(long, default_value = "off")]
        prompt_cache: String,
        #[arg(long, default_value_t = false)]
        prompt_cache_l2: bool,
        #[arg(long, default_value_t = 300000)]
        prompt_cache_ttl_ms: u64,
        #[arg(long, default_value_t = 1024)]
        prompt_cache_max_entries: usize,
        #[arg(long, default_value_t = false)]
        summarization_disable: bool,
        #[arg(long, default_value_t = 12000)]
        summarization_max_char_budget: usize,
        #[arg(long, default_value_t = 12)]
        summarization_max_turns_visible: usize,
        #[arg(long, default_value_t = 3)]
        summarization_min_recent_messages: usize,
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        summarization_redact_tool_args: bool,
        #[arg(long, default_value_t = 2000)]
        summarization_max_tool_arg_chars: usize,
        #[arg(long, default_value_t = 6)]
        summarization_truncate_keep_last: usize,
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
}

#[derive(Subcommand, Debug)]
enum SkillCmd {
    Init {
        dir: String,
    },
    Validate {
        #[arg(long = "source")]
        sources: Vec<String>,
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    List {
        #[arg(long = "source")]
        sources: Vec<String>,
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let root = args.root.clone();
    let mode = resolve_execution_mode(args.execution_mode.as_deref());
    let allow_list = resolve_allow_list(&args);

    let audit_path = resolve_audit_path(args.audit_json.as_deref());
    let audit_sink: Option<std::sync::Arc<dyn AuditSink>> = audit_path
        .as_deref()
        .map(|p| std::sync::Arc::new(JsonlFileAuditSink::new(p)) as std::sync::Arc<dyn AuditSink>);

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

    let backend = deepagents::create_local_sandbox_backend(root.clone(), backend_shell_allow)?;
    let agent = deepagents::create_deep_agent_with_backend(backend);

    match args.cmd {
        Cmd::Tool {
            name,
            input,
            pretty,
            state_file,
        } => {
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
            plugin,
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
            let provider_bundle = build_provider_bundle(
                &provider,
                mock_script,
                model,
                base_url,
                api_key,
                api_key_env,
            )?;
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

            let mut skills: Vec<std::sync::Arc<dyn deepagents::skills::SkillPlugin>> = Vec::new();
            for p in plugin {
                skills.push(std::sync::Arc::new(
                    deepagents::skills::declarative::DeclarativeSkillPlugin::load_from_file(&p)?,
                ));
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

            if !memory_disable {
                let sources = if memory_source.is_empty() {
                    vec![".deepagents/AGENTS.md".to_string(), "AGENTS.md".to_string()]
                } else {
                    memory_source
                };
                let options = deepagents::runtime::MemoryLoadOptions {
                    allow_host_paths: memory_allow_host_paths,
                    max_injected_chars: memory_max_injected_chars,
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

            if !summarization_disable {
                let options = deepagents::runtime::SummarizationOptions {
                    policy: deepagents::runtime::SummarizationPolicyKind::Budget,
                    max_char_budget: summarization_max_char_budget,
                    max_turns_visible: summarization_max_turns_visible,
                    min_recent_messages: summarization_min_recent_messages,
                    redact_tool_args: summarization_redact_tool_args,
                    max_tool_arg_chars: summarization_max_tool_arg_chars,
                    truncate_tool_args_keep_last: summarization_truncate_keep_last,
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

            let prompt_cache_enabled = match prompt_cache.as_str() {
                "off" => false,
                "memory" => true,
                other => return Err(anyhow!("unknown --prompt-cache: {other}")),
            };
            let prompt_cache_options = deepagents::runtime::PromptCacheOptions {
                enabled: prompt_cache_enabled,
                backend: deepagents::runtime::CacheBackend::Memory,
                enable_l2_response_cache: prompt_cache_l2,
                ttl_ms: prompt_cache_ttl_ms,
                max_entries: prompt_cache_max_entries,
                provider_id,
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
                skills,
                deepagents::runtime::ResumableRunnerOptions {
                    config: deepagents::runtime::RuntimeConfig {
                        max_steps,
                        provider_timeout_ms,
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
                    let Some(interrupt) = out.interrupts.get(0).cloned() else {
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
                                        eprintln!("invalid JSON: {}", e.to_string());
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
            SkillCmd::Init { dir } => {
                init_skill_template(&dir)?;
            }
            SkillCmd::Validate { sources, pretty } => {
                let loaded = load_skills_from_sources(&sources)?;
                let out = serde_json::to_value(&loaded)?;
                print_json_value(out, pretty)?;
            }
            SkillCmd::List { sources, pretty } => {
                let loaded = load_skills_from_sources(&sources)?;
                let out = serde_json::json!({
                    "skills": loaded.metadata,
                    "tools": loaded.tools,
                    "diagnostics": loaded.diagnostics
                });
                print_json_value(out, pretty)?;
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
                let store_path = resolve_memory_store_path(&root, store.as_deref());
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
                let store_path = resolve_memory_store_path(&root, store.as_deref());
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
                let store_path = resolve_memory_store_path(&root, store.as_deref());
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

fn resolve_memory_store_path(root: &str, store: Option<&str>) -> std::path::PathBuf {
    if let Some(s) = store {
        return std::path::PathBuf::from(s);
    }
    std::path::PathBuf::from(root)
        .join(".deepagents")
        .join("memory_store.json")
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
        return Err(anyhow!("--source is required"));
    }
    let options = deepagents::skills::loader::SkillsLoadOptions {
        skip_invalid_sources: false,
        strict: true,
    };
    deepagents::skills::loader::load_skills(sources, options)
}

fn print_json_value(value: serde_json::Value, pretty: bool) -> Result<()> {
    if pretty {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{}", serde_json::to_string(&value)?);
    }
    Ok(())
}

fn init_skill_template(dir: &str) -> Result<()> {
    let path = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&path)?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "sample-skill".to_string())
        .to_lowercase()
        .replace(' ', "-");
    let skill_md = format!(
        "---\nname: {}\ndescription: Describe what this skill does and when to use it.\n---\n\n# {}\n\n## When to Use\n- \n\n## Steps\n- \n",
        name, name
    );
    let tools_json = serde_json::json!({
        "tools": [{
            "name": name,
            "description": "Describe the tool behavior.",
            "input_schema": { "type": "object", "properties": { "file_path": { "type": "string" } }, "required": [] },
            "steps": [{ "tool_name": "read_file", "arguments": { "file_path": "README.md", "limit": 20 } }],
            "policy": { "allow_filesystem": true, "allow_execute": false, "allow_network": false }
        }]
    });
    std::fs::write(path.join("SKILL.md"), skill_md)?;
    std::fs::write(
        path.join("tools.json"),
        serde_json::to_vec_pretty(&tools_json)?,
    )?;
    Ok(())
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

fn resolve_execution_mode(flag: Option<&str>) -> ExecutionMode {
    let s = flag
        .map(|s| s.to_string())
        .or_else(|| std::env::var("DEEPAGENTS_EXECUTION_MODE").ok())
        .unwrap_or_else(|| "non-interactive".to_string());
    match s.as_str() {
        "interactive" => ExecutionMode::Interactive,
        "non-interactive" => ExecutionMode::NonInteractive,
        "non_interactive" => ExecutionMode::NonInteractive,
        _ => ExecutionMode::NonInteractive,
    }
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

fn resolve_audit_path(flag: Option<&str>) -> Option<String> {
    flag.map(|s| s.to_string())
        .or_else(|| std::env::var("DEEPAGENTS_AUDIT_JSON").ok())
}

fn resolve_allow_list(args: &Args) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let cli_has_any = !args.shell_allow.is_empty() || args.shell_allow_file.is_some();
    if cli_has_any {
        out.extend(args.shell_allow.iter().cloned());
        if let Some(p) = args.shell_allow_file.as_deref() {
            out.extend(read_allow_file(p).unwrap_or_default());
        }
        return normalize_allow_list(out);
    }

    if let Ok(v) = std::env::var("DEEPAGENTS_SHELL_ALLOW") {
        out.extend(
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
    }
    if let Ok(p) = std::env::var("DEEPAGENTS_SHELL_ALLOW_FILE") {
        out.extend(read_allow_file(&p).unwrap_or_default());
    }
    normalize_allow_list(out)
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
    mock_script: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    api_key_env: Option<String>,
) -> Result<deepagents::provider::ProviderInitBundle> {
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
            let model = model
                .ok_or_else(|| anyhow!("--model is required for --provider openai-compatible"))?;
            let mut config = deepagents::llm::OpenAiCompatibleConfig::new(model);
            if let Some(base_url) = base_url {
                config = config.with_base_url(base_url);
            }
            let api_key = match (api_key, api_key_env) {
                (Some(api_key), _) => Some(api_key),
                (None, Some(env_name)) => Some(
                    std::env::var(&env_name)
                        .map_err(|_| anyhow!("missing env var for --api-key-env: {env_name}"))?,
                ),
                (None, None) => std::env::var("OPENAI_API_KEY").ok(),
            };
            if let Some(api_key) = api_key {
                config = config.with_api_key(api_key);
            }
            Ok(deepagents::provider::build_provider_bundle(
                provider_id,
                deepagents::provider::ProviderInitSpec::OpenAiCompatible { config },
            ))
        }
        "openrouter" => {
            let model =
                model.ok_or_else(|| anyhow!("--model is required for --provider openrouter"))?;
            let mut config = deepagents::llm::OpenRouterConfig::new(model);
            if let Some(base_url) = base_url {
                config = config.with_base_url(base_url);
            }
            let api_key = match (api_key, api_key_env) {
                (Some(api_key), _) => Some(api_key),
                (None, Some(env_name)) => Some(
                    std::env::var(&env_name)
                        .map_err(|_| anyhow!("missing env var for --api-key-env: {env_name}"))?,
                ),
                (None, None) => std::env::var("OPENROUTER_API_KEY").ok(),
            };
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
