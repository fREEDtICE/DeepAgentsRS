use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use deepagents::approval::{
    redact_command, ApprovalDecision, ApprovalRequest, DefaultApprovalPolicy, ExecutionMode,
};
use deepagents::audit::{AuditEvent, AuditSink};
use deepagents::config::{
    ConfigDocument, ConfigKey, ConfigManager, ConfigOverrides, ConfigScope, ConfigValue,
    EffectiveConfig, PromptCacheBackendKind,
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
        state_file: Option<String>,
        #[arg(long)]
        mock_script: Option<String>,
        /// Load skill packages from a source directory. Repeat to add multiple sources.
        #[arg(long = "skills-source")]
        skills_source: Vec<String>,
        /// Use a file-backed skill registry directory.
        #[arg(long = "skill-registry")]
        skill_registry: Option<String>,
        /// Explicitly pin one or more skills by `name` or `name@version`.
        #[arg(long = "skill")]
        skill: Vec<String>,
        /// Explicitly disable one or more skills by `name` or `name@version`.
        #[arg(long = "disable-skill")]
        disable_skill: Vec<String>,
        /// Skill selection mode: `auto`, `manual`, or `off`.
        #[arg(long = "skill-select")]
        skill_select: Option<String>,
        /// Maximum number of active skills exposed to the provider.
        #[arg(long = "skill-max-active")]
        skill_max_active: Option<usize>,
        /// Emit skill selection events to stderr alongside the normal run result.
        #[arg(long, default_value_t = false)]
        explain_skills: bool,
        /// Recompute the sticky thread snapshot instead of reusing it.
        #[arg(long, default_value_t = false)]
        refresh_skill_snapshot: bool,
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
        #[arg(long)]
        audit_json: Option<String>,
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
    /// Install source packages into the local file-backed skill registry.
    Install {
        /// Load skills from a source directory. Repeat to install multiple sources.
        #[arg(long = "source")]
        sources: Vec<String>,
        /// Override the registry path. Defaults to `<root>/.deepagents/skills`.
        #[arg(long = "registry")]
        registry: Option<String>,
        /// Pretty-print the JSON result.
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    /// Show registry entries, lifecycle state, and governance diagnostics.
    Status {
        /// Override the registry path. Defaults to `<root>/.deepagents/skills`.
        #[arg(long = "registry")]
        registry: Option<String>,
        /// Pretty-print the JSON result.
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    /// Show all installed versions for a skill name.
    Versions {
        /// Skill name to inspect.
        name: String,
        /// Override the registry path. Defaults to `<root>/.deepagents/skills`.
        #[arg(long = "registry")]
        registry: Option<String>,
        /// Pretty-print the JSON result.
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    /// Enable one or more installed skill versions.
    Enable {
        /// `name` or `name@version`.
        identity: String,
        /// Override the registry path. Defaults to `<root>/.deepagents/skills`.
        #[arg(long = "registry")]
        registry: Option<String>,
        /// Pretty-print the JSON result.
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    /// Disable one or more installed skill versions.
    Disable {
        /// `name` or `name@version`.
        identity: String,
        /// Override the registry path. Defaults to `<root>/.deepagents/skills`.
        #[arg(long = "registry")]
        registry: Option<String>,
        /// Pretty-print the JSON result.
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    /// Quarantine an installed skill version.
    Quarantine {
        /// `name@version`.
        identity: String,
        /// Override the registry path. Defaults to `<root>/.deepagents/skills`.
        #[arg(long = "registry")]
        registry: Option<String>,
        /// Optional quarantine reason recorded in the registry.
        #[arg(long)]
        reason: Option<String>,
        /// Pretty-print the JSON result.
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    /// Remove one installed skill version from the registry.
    Remove {
        /// `name@version`.
        identity: String,
        /// Override the registry path. Defaults to `<root>/.deepagents/skills`.
        #[arg(long = "registry")]
        registry: Option<String>,
        /// Pretty-print the JSON result.
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    /// Resolve the effective skill snapshot for one input without running the model.
    Resolve {
        /// Input text used for deterministic selection.
        #[arg(long)]
        input: String,
        /// Override the registry path. Defaults to `<root>/.deepagents/skills`.
        #[arg(long = "registry")]
        registry: Option<String>,
        /// Load source overlays for this resolution only.
        #[arg(long = "source")]
        sources: Vec<String>,
        /// Explicitly pin one or more skills by `name` or `name@version`.
        #[arg(long = "skill")]
        skill: Vec<String>,
        /// Explicitly disable one or more skills by `name` or `name@version`.
        #[arg(long = "disable-skill")]
        disable_skill: Vec<String>,
        /// Skill selection mode: `auto`, `manual`, or `off`.
        #[arg(long = "skill-select")]
        skill_select: Option<String>,
        /// Maximum number of active skills exposed to the provider.
        #[arg(long = "skill-max-active")]
        skill_max_active: Option<usize>,
        /// Recompute the sticky snapshot instead of reusing prior state.
        #[arg(long, default_value_t = false)]
        refresh_skill_snapshot: bool,
        /// Pretty-print the JSON result.
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    /// Show the persisted skill audit record for one thread.
    Audit {
        /// Thread ID to inspect.
        #[arg(long = "thread-id")]
        thread_id: String,
        /// Root directory containing `.deepagents/skills/audit`.
        #[arg(long)]
        root: Option<String>,
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
        title: Option<String>,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long = "scope-id")]
        scope_id: Option<String>,
        #[arg(long = "type")]
        memory_type: Option<String>,
        #[arg(long, num_args = 0..=1, default_missing_value = "true")]
        pinned: Option<bool>,
        #[arg(long)]
        tag: Vec<String>,
        #[arg(long = "actor-user-id")]
        actor_user_id: Option<String>,
        #[arg(long = "actor-thread-id")]
        actor_thread_id: Option<String>,
        #[arg(long = "actor-workspace-id")]
        actor_workspace_id: Option<String>,
        #[arg(long = "actor-channel-account-id")]
        actor_channel_account_id: Option<String>,
        #[arg(long)]
        store: Option<String>,
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    Remember {
        #[arg(long)]
        key: String,
        #[arg(long)]
        value: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long = "scope-id")]
        scope_id: Option<String>,
        #[arg(long = "type")]
        memory_type: Option<String>,
        #[arg(long)]
        tag: Vec<String>,
        #[arg(long = "actor-user-id")]
        actor_user_id: Option<String>,
        #[arg(long = "actor-thread-id")]
        actor_thread_id: Option<String>,
        #[arg(long = "actor-workspace-id")]
        actor_workspace_id: Option<String>,
        #[arg(long = "actor-channel-account-id")]
        actor_channel_account_id: Option<String>,
        #[arg(long)]
        store: Option<String>,
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    Get {
        #[arg(long)]
        key: String,
        #[arg(long = "scope-id")]
        scope_id: Option<String>,
        #[arg(long = "actor-user-id")]
        actor_user_id: Option<String>,
        #[arg(long = "actor-thread-id")]
        actor_thread_id: Option<String>,
        #[arg(long = "actor-workspace-id")]
        actor_workspace_id: Option<String>,
        #[arg(long = "actor-channel-account-id")]
        actor_channel_account_id: Option<String>,
        #[arg(long)]
        store: Option<String>,
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    Edit {
        #[arg(long)]
        key: String,
        #[arg(long)]
        value: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long = "scope-id")]
        scope_id: Option<String>,
        #[arg(long = "type")]
        memory_type: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        confidence: Option<i64>,
        #[arg(long, allow_hyphen_values = true)]
        salience: Option<i64>,
        #[arg(long, default_value_t = false)]
        clear_tags: bool,
        #[arg(long)]
        tag: Vec<String>,
        #[arg(long = "actor-user-id")]
        actor_user_id: Option<String>,
        #[arg(long = "actor-thread-id")]
        actor_thread_id: Option<String>,
        #[arg(long = "actor-workspace-id")]
        actor_workspace_id: Option<String>,
        #[arg(long = "actor-channel-account-id")]
        actor_channel_account_id: Option<String>,
        #[arg(long)]
        store: Option<String>,
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    Pin {
        #[arg(long)]
        key: String,
        #[arg(long = "scope-id")]
        scope_id: Option<String>,
        #[arg(long = "actor-user-id")]
        actor_user_id: Option<String>,
        #[arg(long = "actor-thread-id")]
        actor_thread_id: Option<String>,
        #[arg(long = "actor-workspace-id")]
        actor_workspace_id: Option<String>,
        #[arg(long = "actor-channel-account-id")]
        actor_channel_account_id: Option<String>,
        #[arg(long)]
        store: Option<String>,
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    Unpin {
        #[arg(long)]
        key: String,
        #[arg(long = "scope-id")]
        scope_id: Option<String>,
        #[arg(long = "actor-user-id")]
        actor_user_id: Option<String>,
        #[arg(long = "actor-thread-id")]
        actor_thread_id: Option<String>,
        #[arg(long = "actor-workspace-id")]
        actor_workspace_id: Option<String>,
        #[arg(long = "actor-channel-account-id")]
        actor_channel_account_id: Option<String>,
        #[arg(long)]
        store: Option<String>,
        #[arg(long, default_value_t = false)]
        pretty: bool,
    },
    Delete {
        #[arg(long)]
        key: String,
        #[arg(long = "scope-id")]
        scope_id: Option<String>,
        #[arg(long = "actor-user-id")]
        actor_user_id: Option<String>,
        #[arg(long = "actor-thread-id")]
        actor_thread_id: Option<String>,
        #[arg(long = "actor-workspace-id")]
        actor_workspace_id: Option<String>,
        #[arg(long = "actor-channel-account-id")]
        actor_channel_account_id: Option<String>,
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
        #[arg(long)]
        scope: Option<String>,
        #[arg(long = "scope-id")]
        scope_id: Option<String>,
        #[arg(long = "type")]
        memory_type: Option<String>,
        #[arg(long, num_args = 0..=1, default_missing_value = "true")]
        pinned: Option<bool>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = false)]
        include_inactive: bool,
        #[arg(long = "actor-user-id")]
        actor_user_id: Option<String>,
        #[arg(long = "actor-thread-id")]
        actor_thread_id: Option<String>,
        #[arg(long = "actor-workspace-id")]
        actor_workspace_id: Option<String>,
        #[arg(long = "actor-channel-account-id")]
        actor_channel_account_id: Option<String>,
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
            state_file,
            mock_script,
            skills_source,
            skill_registry,
            skill,
            disable_skill,
            skill_select,
            skill_max_active,
            explain_skills,
            refresh_skill_snapshot,
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
            audit_json,
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
            if let Some(audit_json) = audit_json.as_deref() {
                insert_override(
                    &mut overrides,
                    "audit.jsonl_path",
                    ConfigValue::String(audit_json.to_string()),
                )?;
            }
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

            let skill_selection_mode = parse_skill_selection_mode(skill_select.as_deref())?;
            let skill_registry_dir = resolve_run_skill_registry_dir(
                &root,
                skill_registry.as_deref(),
                !skill.is_empty() || !disable_skill.is_empty(),
            );
            let enable_skill_runtime = !skills_source.is_empty()
                || skill_registry_dir.is_some()
                || !skill.is_empty()
                || !disable_skill.is_empty()
                || refresh_skill_snapshot
                || skill_max_active.is_some()
                || skill_selection_mode != deepagents::skills::selection::SkillSelectionMode::Off;
            if enable_skill_runtime {
                let options = deepagents::skills::loader::SkillsLoadOptions {
                    skip_invalid_sources: skills_skip_invalid,
                    strict: true,
                    allow_versionless_compat: true,
                };
                let mut middleware =
                    deepagents::runtime::SkillsMiddleware::new(skills_source, options)
                        .with_explicit_skills(skill)
                        .with_disabled_skills(disable_skill)
                        .with_selection_mode(skill_selection_mode)
                        .with_refresh_snapshot(refresh_skill_snapshot);
                if let Some(skill_registry_dir) = skill_registry_dir {
                    middleware = middleware.with_registry_dir(skill_registry_dir);
                }
                if let Some(skill_max_active) = skill_max_active {
                    middleware = middleware.with_max_active(skill_max_active);
                }
                let skills_mw: std::sync::Arc<dyn deepagents::runtime::RuntimeMiddleware> =
                    std::sync::Arc::new(middleware);
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

            let explicit_thread_id = thread_id.clone();
            let mut initial_state = state_file
                .as_deref()
                .and_then(load_state)
                .unwrap_or_default();
            let thread_id = ensure_cli_thread_id(&mut initial_state, explicit_thread_id.as_deref());

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
            let use_event_stream = events_jsonl.is_some() || stream_events || explain_skills;
            let mut event_sink = if use_event_stream {
                Some(CliRunEventSink::new(
                    events_jsonl.as_deref(),
                    stream_events || explain_skills,
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
                persist_run_state_and_skill_audit(
                    state_file.as_deref(),
                    &root,
                    &thread_id,
                    &out.state,
                    out.trace.as_ref(),
                    out.status,
                    out.error.as_ref(),
                )?;
                append_run_audit_record(
                    effective.audit.jsonl_path.as_deref(),
                    &root,
                    &thread_id,
                    out.status,
                    out.error.as_ref(),
                )?;
                strip_transient_thread_id_for_output(
                    &mut out,
                    explicit_thread_id.as_deref(),
                    state_file.as_deref(),
                );
                if pretty {
                    println!("{}", serde_json::to_string_pretty(&out)?);
                } else {
                    println!("{}", serde_json::to_string(&out)?);
                }
                std::process::exit(2);
            }

            let ok = out.error.is_none() && out.status == deepagents::runtime::RunStatus::Completed;
            persist_run_state_and_skill_audit(
                state_file.as_deref(),
                &root,
                &thread_id,
                &out.state,
                out.trace.as_ref(),
                out.status,
                out.error.as_ref(),
            )?;
            append_run_audit_record(
                effective.audit.jsonl_path.as_deref(),
                &root,
                &thread_id,
                out.status,
                out.error.as_ref(),
            )?;
            strip_transient_thread_id_for_output(
                &mut out,
                explicit_thread_id.as_deref(),
                state_file.as_deref(),
            );
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
            SkillCmd::Install {
                sources,
                registry,
                pretty,
            } => {
                handle_skill_command(
                    "install",
                    pretty,
                    skill_install_command(&root, registry.as_deref(), &sources),
                )?;
            }
            SkillCmd::Status { registry, pretty } => {
                handle_skill_command(
                    "status",
                    pretty,
                    skill_status_command(&root, registry.as_deref()),
                )?;
            }
            SkillCmd::Versions {
                name,
                registry,
                pretty,
            } => {
                handle_skill_command(
                    "versions",
                    pretty,
                    skill_versions_command(&root, registry.as_deref(), &name),
                )?;
            }
            SkillCmd::Enable {
                identity,
                registry,
                pretty,
            } => {
                handle_skill_command(
                    "enable",
                    pretty,
                    skill_lifecycle_command(
                        &root,
                        registry.as_deref(),
                        "enable",
                        &identity,
                        deepagents::skills::SkillLifecycleState::Enabled,
                        None,
                    ),
                )?;
            }
            SkillCmd::Disable {
                identity,
                registry,
                pretty,
            } => {
                handle_skill_command(
                    "disable",
                    pretty,
                    skill_lifecycle_command(
                        &root,
                        registry.as_deref(),
                        "disable",
                        &identity,
                        deepagents::skills::SkillLifecycleState::Disabled,
                        Some("disabled_by_cli".to_string()),
                    ),
                )?;
            }
            SkillCmd::Quarantine {
                identity,
                registry,
                reason,
                pretty,
            } => {
                handle_skill_command(
                    "quarantine",
                    pretty,
                    skill_quarantine_command(&root, registry.as_deref(), &identity, reason),
                )?;
            }
            SkillCmd::Remove {
                identity,
                registry,
                pretty,
            } => {
                handle_skill_command(
                    "remove",
                    pretty,
                    skill_remove_command(&root, registry.as_deref(), &identity),
                )?;
            }
            SkillCmd::Resolve {
                input,
                registry,
                sources,
                skill,
                disable_skill,
                skill_select,
                skill_max_active,
                refresh_skill_snapshot,
                pretty,
            } => {
                handle_skill_command(
                    "resolve",
                    pretty,
                    skill_resolve_command(
                        &root,
                        registry.as_deref(),
                        &input,
                        &sources,
                        &skill,
                        &disable_skill,
                        skill_select.as_deref(),
                        skill_max_active,
                        refresh_skill_snapshot,
                    ),
                )?;
            }
            SkillCmd::Audit {
                thread_id,
                root: audit_root,
                pretty,
            } => {
                let audit_root = audit_root.unwrap_or(root.clone());
                handle_skill_command(
                    "audit",
                    pretty,
                    skill_audit_command(&audit_root, &thread_id),
                )?;
            }
        },
        Cmd::Memory { cmd } => match cmd {
            MemoryCmd::Put {
                key,
                value,
                title,
                scope,
                scope_id,
                memory_type,
                pinned,
                tag,
                actor_user_id,
                actor_thread_id,
                actor_workspace_id,
                actor_channel_account_id,
                store,
                pretty,
            } => {
                if looks_like_secret(&value) {
                    return Err(anyhow!("invalid_request: value looks like a secret"));
                }
                let actor = MemoryActorContext::new(
                    actor_user_id,
                    actor_thread_id,
                    actor_workspace_id,
                    actor_channel_account_id,
                );
                let store = open_cli_memory_store(&config_manager, &root_overrides, store.as_deref()).await?;
                let existing = store.get(&key).await?;
                let entry = apply_memory_mutation(
                    existing,
                    key.clone(),
                    MemoryCommandDefaults {
                        scope: deepagents::memory::MemoryScope::User,
                        memory_type: deepagents::memory::MemoryType::Semantic,
                        pinned: false,
                    },
                    &actor,
                    MemoryMutation {
                        value: Some(value),
                        title,
                        scope: scope
                            .as_deref()
                            .map(|value| parse_memory_scope(Some(value), deepagents::memory::MemoryScope::User))
                            .transpose()?,
                        scope_id,
                        memory_type: memory_type
                            .as_deref()
                            .map(|value| parse_memory_type(Some(value), deepagents::memory::MemoryType::Semantic))
                            .transpose()?,
                        pinned,
                        tags: Some(tag),
                        ..Default::default()
                    },
                )?;
                store.put(entry.clone()).await?;
                let report = store.evict_if_needed().await?;
                store.flush().await?;
                let _ = store.render_agents_md().await;
                let mut out = memory_entry_json(&entry);
                if let Some(object) = out.as_object_mut() {
                    object.insert("success".to_string(), serde_json::Value::Bool(true));
                    object.insert("eviction".to_string(), serde_json::to_value(report)?);
                }
                print_json_value(out, pretty)?;
            }
            MemoryCmd::Remember {
                key,
                value,
                title,
                scope,
                scope_id,
                memory_type,
                tag,
                actor_user_id,
                actor_thread_id,
                actor_workspace_id,
                actor_channel_account_id,
                store,
                pretty,
            } => {
                if looks_like_secret(&value) {
                    return Err(anyhow!("invalid_request: value looks like a secret"));
                }
                let actor = MemoryActorContext::new(
                    actor_user_id,
                    actor_thread_id,
                    actor_workspace_id,
                    actor_channel_account_id,
                );
                let store = open_cli_memory_store(&config_manager, &root_overrides, store.as_deref()).await?;
                let existing = store.get(&key).await?;
                let entry = apply_memory_mutation(
                    existing,
                    key.clone(),
                    MemoryCommandDefaults {
                        scope: deepagents::memory::MemoryScope::User,
                        memory_type: deepagents::memory::MemoryType::Procedural,
                        pinned: true,
                    },
                    &actor,
                    MemoryMutation {
                        value: Some(value),
                        title,
                        scope: scope
                            .as_deref()
                            .map(|value| parse_memory_scope(Some(value), deepagents::memory::MemoryScope::User))
                            .transpose()?,
                        scope_id,
                        memory_type: memory_type
                            .as_deref()
                            .map(|value| parse_memory_type(Some(value), deepagents::memory::MemoryType::Procedural))
                            .transpose()?,
                        pinned: Some(true),
                        tags: Some(tag),
                        ..Default::default()
                    },
                )?;
                store.put(entry.clone()).await?;
                let report = store.evict_if_needed().await?;
                store.flush().await?;
                let _ = store.render_agents_md().await;
                let mut out = memory_entry_json(&entry);
                if let Some(object) = out.as_object_mut() {
                    object.insert("success".to_string(), serde_json::Value::Bool(true));
                    object.insert("eviction".to_string(), serde_json::to_value(report)?);
                }
                print_json_value(out, pretty)?;
            }
            MemoryCmd::Get {
                key,
                scope_id,
                actor_user_id,
                actor_thread_id,
                actor_workspace_id,
                actor_channel_account_id,
                store,
                pretty,
            } => {
                let actor = MemoryActorContext::new(
                    actor_user_id,
                    actor_thread_id,
                    actor_workspace_id,
                    actor_channel_account_id,
                );
                let store = open_cli_memory_store(&config_manager, &root_overrides, store.as_deref()).await?;
                let out = match store.get(&key).await? {
                    Some(entry)
                        if entry.status == deepagents::memory::MemoryStatus::Active
                            && memory_entry_visible_to_actor(&entry, &actor, scope_id.as_deref()) =>
                    {
                        memory_entry_json(&entry)
                    }
                    _ => null_memory_get_json(&key),
                };
                print_json_value(out, pretty)?;
            }
            MemoryCmd::Edit {
                key,
                value,
                title,
                scope,
                scope_id,
                memory_type,
                confidence,
                salience,
                clear_tags,
                tag,
                actor_user_id,
                actor_thread_id,
                actor_workspace_id,
                actor_channel_account_id,
                store,
                pretty,
            } => {
                let actor = MemoryActorContext::new(
                    actor_user_id,
                    actor_thread_id,
                    actor_workspace_id,
                    actor_channel_account_id,
                );
                let store = open_cli_memory_store(&config_manager, &root_overrides, store.as_deref()).await?;
                let existing = store
                    .get(&key)
                    .await?
                    .ok_or_else(|| anyhow!("memory_not_found: {key}"))?;
                ensure_memory_mutation_allowed(&existing, &actor, scope_id.as_deref())?;
                let entry = apply_memory_mutation(
                    Some(existing),
                    key.clone(),
                    MemoryCommandDefaults {
                        scope: deepagents::memory::MemoryScope::User,
                        memory_type: deepagents::memory::MemoryType::Semantic,
                        pinned: false,
                    },
                    &actor,
                    MemoryMutation {
                        value,
                        title,
                        scope: scope
                            .as_deref()
                            .map(|value| parse_memory_scope(Some(value), deepagents::memory::MemoryScope::User))
                            .transpose()?,
                        scope_id,
                        memory_type: memory_type
                            .as_deref()
                            .map(|value| parse_memory_type(Some(value), deepagents::memory::MemoryType::Semantic))
                            .transpose()?,
                        tags: if clear_tags || !tag.is_empty() { Some(tag) } else { None },
                        confidence,
                        salience,
                        ..Default::default()
                    },
                )?;
                store.put(entry.clone()).await?;
                let report = store.evict_if_needed().await?;
                store.flush().await?;
                let _ = store.render_agents_md().await;
                let mut out = memory_entry_json(&entry);
                if let Some(object) = out.as_object_mut() {
                    object.insert("updated".to_string(), serde_json::Value::Bool(true));
                    object.insert("eviction".to_string(), serde_json::to_value(report)?);
                }
                print_json_value(out, pretty)?;
            }
            MemoryCmd::Pin {
                key,
                scope_id,
                actor_user_id,
                actor_thread_id,
                actor_workspace_id,
                actor_channel_account_id,
                store,
                pretty,
            } => {
                let actor = MemoryActorContext::new(
                    actor_user_id,
                    actor_thread_id,
                    actor_workspace_id,
                    actor_channel_account_id,
                );
                let store = open_cli_memory_store(&config_manager, &root_overrides, store.as_deref()).await?;
                let existing = store
                    .get(&key)
                    .await?
                    .ok_or_else(|| anyhow!("memory_not_found: {key}"))?;
                ensure_memory_mutation_allowed(&existing, &actor, scope_id.as_deref())?;
                if existing.status != deepagents::memory::MemoryStatus::Active {
                    return Err(anyhow!("invalid_request: cannot pin inactive memory"));
                }
                let entry = apply_memory_mutation(
                    Some(existing),
                    key,
                    MemoryCommandDefaults {
                        scope: deepagents::memory::MemoryScope::User,
                        memory_type: deepagents::memory::MemoryType::Semantic,
                        pinned: false,
                    },
                    &actor,
                    MemoryMutation {
                        pinned: Some(true),
                        ..Default::default()
                    },
                )?;
                store.put(entry.clone()).await?;
                store.flush().await?;
                let _ = store.render_agents_md().await;
                let mut out = memory_entry_json(&entry);
                if let Some(object) = out.as_object_mut() {
                    object.insert("updated".to_string(), serde_json::Value::Bool(true));
                }
                print_json_value(out, pretty)?;
            }
            MemoryCmd::Unpin {
                key,
                scope_id,
                actor_user_id,
                actor_thread_id,
                actor_workspace_id,
                actor_channel_account_id,
                store,
                pretty,
            } => {
                let actor = MemoryActorContext::new(
                    actor_user_id,
                    actor_thread_id,
                    actor_workspace_id,
                    actor_channel_account_id,
                );
                let store = open_cli_memory_store(&config_manager, &root_overrides, store.as_deref()).await?;
                let existing = store
                    .get(&key)
                    .await?
                    .ok_or_else(|| anyhow!("memory_not_found: {key}"))?;
                ensure_memory_mutation_allowed(&existing, &actor, scope_id.as_deref())?;
                if existing.status != deepagents::memory::MemoryStatus::Active {
                    return Err(anyhow!("invalid_request: cannot unpin inactive memory"));
                }
                let entry = apply_memory_mutation(
                    Some(existing),
                    key,
                    MemoryCommandDefaults {
                        scope: deepagents::memory::MemoryScope::User,
                        memory_type: deepagents::memory::MemoryType::Semantic,
                        pinned: false,
                    },
                    &actor,
                    MemoryMutation {
                        pinned: Some(false),
                        ..Default::default()
                    },
                )?;
                store.put(entry.clone()).await?;
                store.flush().await?;
                let _ = store.render_agents_md().await;
                let mut out = memory_entry_json(&entry);
                if let Some(object) = out.as_object_mut() {
                    object.insert("updated".to_string(), serde_json::Value::Bool(true));
                }
                print_json_value(out, pretty)?;
            }
            MemoryCmd::Delete {
                key,
                scope_id,
                actor_user_id,
                actor_thread_id,
                actor_workspace_id,
                actor_channel_account_id,
                store,
                pretty,
            } => {
                let actor = MemoryActorContext::new(
                    actor_user_id,
                    actor_thread_id,
                    actor_workspace_id,
                    actor_channel_account_id,
                );
                let store = open_cli_memory_store(&config_manager, &root_overrides, store.as_deref()).await?;
                let out = match store.get(&key).await? {
                    Some(existing) => {
                        ensure_memory_mutation_allowed(&existing, &actor, scope_id.as_deref())?;
                        let entry = apply_memory_mutation(
                            Some(existing),
                            key.clone(),
                            MemoryCommandDefaults {
                                scope: deepagents::memory::MemoryScope::User,
                                memory_type: deepagents::memory::MemoryType::Semantic,
                                pinned: false,
                            },
                            &actor,
                            MemoryMutation {
                                status: Some(deepagents::memory::MemoryStatus::Deleted),
                                ..Default::default()
                            },
                        )?;
                        store.put(entry).await?;
                        store.flush().await?;
                        let _ = store.render_agents_md().await;
                        serde_json::json!({ "deleted": true, "key": key })
                    }
                    None => serde_json::json!({ "deleted": false, "key": key }),
                };
                print_json_value(out, pretty)?;
            }
            MemoryCmd::Query {
                prefix,
                tag,
                scope,
                scope_id,
                memory_type,
                pinned,
                status,
                include_inactive,
                actor_user_id,
                actor_thread_id,
                actor_workspace_id,
                actor_channel_account_id,
                limit,
                store,
                pretty,
            } => {
                let store = open_cli_memory_store(&config_manager, &root_overrides, store.as_deref()).await?;
                let entries = store
                    .query(deepagents::memory::MemoryQuery {
                        prefix,
                        tag,
                        scope: scope
                            .as_deref()
                            .map(|value| parse_memory_scope(Some(value), deepagents::memory::MemoryScope::User))
                            .transpose()?,
                        scope_id,
                        memory_type: memory_type
                            .as_deref()
                            .map(|value| parse_memory_type(Some(value), deepagents::memory::MemoryType::Semantic))
                            .transpose()?,
                        pinned,
                        status: parse_memory_status(status.as_deref())?,
                        include_inactive,
                        actor_user_id,
                        actor_thread_id,
                        actor_workspace_id,
                        actor_channel_account_id,
                        limit: Some(limit),
                    })
                    .await?;
                let out = serde_json::json!({
                    "entries": entries.iter().map(memory_entry_json).collect::<Vec<_>>()
                });
                print_json_value(out, pretty)?;
            }
            MemoryCmd::Compact { store, pretty } => {
                let store = open_cli_memory_store(&config_manager, &root_overrides, store.as_deref()).await?;
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
            let out = serde_json::json!({
                "scope": scope,
                "config": config_list_for_cli(config_manager, scope)?,
            });
            print_json_value(out, pretty)?;
        }
        ConfigCmd::Get { key, scope, pretty } => {
            let scope = parse_config_scope(scope.as_deref(), ConfigScope::Effective)?;
            let key = ConfigKey::parse(key)?;
            let out = config_get_for_cli(config_manager, scope, &key)?;
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
            config_set_for_cli(config_manager, scope, &key, &value)?;
            let out = serde_json::json!({ "updated": true, "scope": scope, "key": key });
            print_json_value(out, pretty)?;
        }
        ConfigCmd::Unset { key, scope, pretty } => {
            let scope = parse_config_scope(scope.as_deref(), ConfigScope::Workspace)?;
            let key = ConfigKey::parse(key)?;
            config_unset_for_cli(config_manager, scope, &key)?;
            let out = serde_json::json!({ "deleted": true, "scope": scope, "key": key });
            print_json_value(out, pretty)?;
        }
        ConfigCmd::Schema { pretty } => {
            let out = config_schema_for_cli(config_manager)?;
            print_json_value(out, pretty)?;
        }
        ConfigCmd::Doctor { pretty } => {
            let report = config_manager.doctor(&ConfigOverrides::new())?;
            let out = serde_json::json!({ "diagnostics": report.issues });
            print_json_value(out, pretty)?;
        }
    }
    Ok(())
}

/// Returns the persisted config document for one storage scope.
fn config_document_for_scope(
    config_manager: &ConfigManager,
    scope: ConfigScope,
) -> Result<ConfigDocument> {
    let path = match scope {
        ConfigScope::Global => ConfigManager::global_config_path()?,
        ConfigScope::Workspace => config_manager.workspace_config_path(),
        ConfigScope::Effective => {
            return Err(anyhow!(
                "invalid_request: effective scope does not have a backing document"
            ))
        }
    };
    Ok(ConfigDocument::load(&path)?)
}

/// Persists one config document back to disk.
fn save_config_document_for_scope(
    config_manager: &ConfigManager,
    scope: ConfigScope,
    doc: &ConfigDocument,
) -> Result<()> {
    let path = match scope {
        ConfigScope::Global => ConfigManager::global_config_path()?,
        ConfigScope::Workspace => config_manager.workspace_config_path(),
        ConfigScope::Effective => {
            return Err(anyhow!(
                "invalid_request: effective scope does not have a backing document"
            ))
        }
    };
    doc.save(&path)?;
    Ok(())
}

/// Heuristically identifies secret-like keys for redaction on generic config
/// entries that are not part of the strict built-in schema.
fn is_secret_like_config_key(key: &ConfigKey) -> bool {
    key.as_str().contains("api_key")
        || key.as_str().contains("secret")
        || key.as_str().contains("token")
        || key.as_str().contains("password")
}

/// Parses CLI config values, using schema-aware typing when available and a
/// small generic inference fallback otherwise.
fn parse_config_value_for_cli(
    config_manager: &ConfigManager,
    key: &ConfigKey,
    raw: &str,
) -> Result<ConfigValue> {
    if config_manager.schema().field(key).is_some() {
        return Ok(config_manager.parse_cli_value(key, raw)?);
    }
    if raw.eq_ignore_ascii_case("true") {
        return Ok(ConfigValue::Boolean(true));
    }
    if raw.eq_ignore_ascii_case("false") {
        return Ok(ConfigValue::Boolean(false));
    }
    if let Ok(value) = raw.parse::<i64>() {
        return Ok(ConfigValue::Integer(value));
    }
    Ok(ConfigValue::String(raw.to_string()))
}

/// Resolves one config key for CLI output, supporting both schema-backed keys
/// and generic future-state keys stored directly in the documents.
fn config_get_for_cli(
    config_manager: &ConfigManager,
    scope: ConfigScope,
    key: &ConfigKey,
) -> Result<serde_json::Value> {
    if config_manager.schema().field(key).is_some() {
        let resolved = config_manager.get(scope, key, &ConfigOverrides::new())?;
        if is_secret_like_config_key(key) {
            return Ok(serde_json::json!({
                "key": resolved.key,
                "value": serde_json::Value::Null,
                "origin": resolved.origin,
                "secret_status": resolved.secret_status.unwrap_or_else(|| "set".to_string()),
            }));
        }
        return Ok(serde_json::to_value(resolved)?);
    }

    let global_doc = config_document_for_scope(config_manager, ConfigScope::Global)?;
    let workspace_doc = config_document_for_scope(config_manager, ConfigScope::Workspace)?;
    let global_value = global_doc.get(key)?;
    let workspace_value = workspace_doc.get(key)?;
    let (value, origin) = match scope {
        ConfigScope::Global => (global_value.clone(), "global"),
        ConfigScope::Workspace => (workspace_value.clone(), "workspace"),
        ConfigScope::Effective => {
            if let Some(value) = workspace_value.clone() {
                (Some(value), "workspace")
            } else {
                (global_value.clone(), "global")
            }
        }
    };
    let is_present = match scope {
        ConfigScope::Global => global_value.is_some(),
        ConfigScope::Workspace => workspace_value.is_some(),
        ConfigScope::Effective => workspace_value.is_some() || global_value.is_some(),
    };
    if !is_present {
        return Ok(serde_json::json!({
            "key": key.as_str(),
            "value": serde_json::Value::Null,
            "origin": "unset",
        }));
    }
    if is_secret_like_config_key(key) {
        return Ok(serde_json::json!({
            "key": key.as_str(),
            "value": serde_json::Value::Null,
            "origin": origin,
            "secret_status": "set",
        }));
    }
    Ok(serde_json::json!({
        "key": key.as_str(),
        "value": value,
        "origin": origin,
    }))
}

/// Persists one CLI config update, allowing future-state keys outside the
/// built-in schema while preserving schema validation for known keys.
fn config_set_for_cli(
    config_manager: &ConfigManager,
    scope: ConfigScope,
    key: &ConfigKey,
    raw: &str,
) -> Result<()> {
    if config_manager.schema().field(key).is_some() {
        let value = config_manager.parse_cli_value(key, raw)?;
        config_manager.set(scope, key, value)?;
        return Ok(());
    }
    let value = parse_config_value_for_cli(config_manager, key, raw)?;
    let mut doc = config_document_for_scope(config_manager, scope)?;
    doc.set(key, value)?;
    save_config_document_for_scope(config_manager, scope, &doc)
}

/// Removes one config key from either schema-backed or generic config storage.
fn config_unset_for_cli(
    config_manager: &ConfigManager,
    scope: ConfigScope,
    key: &ConfigKey,
) -> Result<()> {
    if config_manager.schema().field(key).is_some() {
        config_manager.unset(scope, key)?;
        return Ok(());
    }
    let mut doc = config_document_for_scope(config_manager, scope)?;
    doc.unset(key);
    save_config_document_for_scope(config_manager, scope, &doc)
}

/// Produces the future-state `config list` shape expected by the E2E suite.
fn config_list_for_cli(
    config_manager: &ConfigManager,
    scope: ConfigScope,
) -> Result<serde_json::Value> {
    let mut config = std::collections::BTreeMap::new();
    for entry in config_manager.list(scope, &ConfigOverrides::new())? {
        let value = if is_secret_like_config_key(&ConfigKey::parse(entry.key.clone())?) {
            serde_json::Value::Null
        } else {
            serde_json::to_value(entry.value)?
        };
        config.insert(entry.key, value);
    }

    let global_doc = config_document_for_scope(config_manager, ConfigScope::Global)?;
    let workspace_doc = config_document_for_scope(config_manager, ConfigScope::Workspace)?;
    let mut keys = std::collections::BTreeSet::new();
    keys.extend(global_doc.flatten_keys());
    keys.extend(workspace_doc.flatten_keys());

    for key in keys {
        let config_key = ConfigKey::parse(key.clone())?;
        if config_manager.schema().field(&config_key).is_some() {
            continue;
        }
        let value = match scope {
            ConfigScope::Global => global_doc.get(&config_key)?,
            ConfigScope::Workspace => workspace_doc.get(&config_key)?,
            ConfigScope::Effective => workspace_doc
                .get(&config_key)?
                .or(global_doc.get(&config_key)?),
        };
        if is_secret_like_config_key(&config_key) {
            config.insert(key, serde_json::Value::Null);
        } else {
            config.insert(key, serde_json::to_value(value)?);
        }
    }
    Ok(serde_json::to_value(config)?)
}

/// Converts the built-in schema into a JSON-schema-like document with a
/// `properties` field so CLI consumers can discover supported keys.
fn config_schema_for_cli(config_manager: &ConfigManager) -> Result<serde_json::Value> {
    let mut properties = serde_json::Map::new();
    for field in &config_manager.schema().fields {
        let value_type = match field.kind {
            deepagents::config::SchemaValueKind::String
            | deepagents::config::SchemaValueKind::Path
            | deepagents::config::SchemaValueKind::EnvVar
            | deepagents::config::SchemaValueKind::Enum => "string",
            deepagents::config::SchemaValueKind::Boolean => "boolean",
            deepagents::config::SchemaValueKind::Integer => "integer",
            deepagents::config::SchemaValueKind::StringList => "array",
        };
        properties.insert(
            field.key.to_string(),
            serde_json::json!({
                "type": value_type,
                "scopes": field.scopes,
                "default": field.default,
            }),
        );
    }
    Ok(serde_json::json!({
        "version": config_manager.schema().version,
        "type": "object",
        "properties": properties,
    }))
}

#[derive(Debug, Clone, Default)]
/// Actor context used by memory commands to enforce scope visibility.
struct MemoryActorContext {
    /// Canonical user identity for user-scoped memory.
    user_id: Option<String>,
    /// Active thread identity for thread-scoped memory.
    thread_id: Option<String>,
    /// Active workspace identity for workspace-scoped memory.
    workspace_id: Option<String>,
    /// Channel account identity for future cross-channel continuity.
    channel_account_id: Option<String>,
}

impl MemoryActorContext {
    /// Builds one actor context from CLI options.
    fn new(
        user_id: Option<String>,
        thread_id: Option<String>,
        workspace_id: Option<String>,
        channel_account_id: Option<String>,
    ) -> Self {
        Self {
            user_id,
            thread_id,
            workspace_id,
            channel_account_id,
        }
    }
}

#[derive(Debug, Clone, Copy)]
/// Defaults applied when creating a new memory item through one CLI command.
struct MemoryCommandDefaults {
    /// Default durable scope.
    scope: deepagents::memory::MemoryScope,
    /// Default memory classification.
    memory_type: deepagents::memory::MemoryType,
    /// Default pin flag.
    pinned: bool,
}

#[derive(Debug, Default)]
/// Partial mutation applied to an existing or newly created memory item.
struct MemoryMutation {
    /// Replacement body value.
    value: Option<String>,
    /// Replacement title.
    title: Option<String>,
    /// Replacement scope.
    scope: Option<deepagents::memory::MemoryScope>,
    /// Replacement scope identifier.
    scope_id: Option<String>,
    /// Replacement memory type.
    memory_type: Option<deepagents::memory::MemoryType>,
    /// Replacement pinned flag.
    pinned: Option<bool>,
    /// Replacement tag set.
    tags: Option<Vec<String>>,
    /// Replacement confidence score.
    confidence: Option<i64>,
    /// Replacement salience score.
    salience: Option<i64>,
    /// Replacement lifecycle status.
    status: Option<deepagents::memory::MemoryStatus>,
    /// Optional supersession pointer.
    supersedes: Option<String>,
}

/// Parses CLI memory scope values onto the durable memory enum.
fn parse_memory_scope(
    value: Option<&str>,
    default: deepagents::memory::MemoryScope,
) -> Result<deepagents::memory::MemoryScope> {
    match value.unwrap_or(match default {
        deepagents::memory::MemoryScope::User => "user",
        deepagents::memory::MemoryScope::Thread => "thread",
        deepagents::memory::MemoryScope::Workspace => "workspace",
        deepagents::memory::MemoryScope::System => "system",
    }) {
        "user" => Ok(deepagents::memory::MemoryScope::User),
        "thread" => Ok(deepagents::memory::MemoryScope::Thread),
        "workspace" => Ok(deepagents::memory::MemoryScope::Workspace),
        "system" => Ok(deepagents::memory::MemoryScope::System),
        other => Err(anyhow!(
            "invalid_arguments: memory scope must be one of user|thread|workspace|system, got {other}"
        )),
    }
}

/// Parses CLI memory type values onto the durable memory enum.
fn parse_memory_type(
    value: Option<&str>,
    default: deepagents::memory::MemoryType,
) -> Result<deepagents::memory::MemoryType> {
    match value.unwrap_or(match default {
        deepagents::memory::MemoryType::Semantic => "semantic",
        deepagents::memory::MemoryType::Procedural => "procedural",
        deepagents::memory::MemoryType::Episodic => "episodic",
        deepagents::memory::MemoryType::Pinned => "pinned",
        deepagents::memory::MemoryType::Profile => "profile",
    }) {
        "semantic" => Ok(deepagents::memory::MemoryType::Semantic),
        "procedural" => Ok(deepagents::memory::MemoryType::Procedural),
        "episodic" => Ok(deepagents::memory::MemoryType::Episodic),
        "pinned" => Ok(deepagents::memory::MemoryType::Pinned),
        "profile" => Ok(deepagents::memory::MemoryType::Profile),
        other => Err(anyhow!(
            "invalid_arguments: memory type must be one of semantic|procedural|episodic|pinned|profile, got {other}"
        )),
    }
}

/// Parses CLI memory lifecycle status filters.
fn parse_memory_status(
    value: Option<&str>,
) -> Result<Option<deepagents::memory::MemoryStatus>> {
    match value {
        None => Ok(None),
        Some("active") => Ok(Some(deepagents::memory::MemoryStatus::Active)),
        Some("deleted") => Ok(Some(deepagents::memory::MemoryStatus::Deleted)),
        Some("inactive") => Ok(Some(deepagents::memory::MemoryStatus::Inactive)),
        Some(other) => Err(anyhow!(
            "invalid_arguments: memory status must be one of active|deleted|inactive, got {other}"
        )),
    }
}

/// Validates CLI-facing confidence and salience scores.
fn validate_memory_score(name: &str, value: i64) -> Result<()> {
    if !(0..=100).contains(&value) {
        return Err(anyhow!(
            "invalid_arguments: {name} must be between 0 and 100"
        ));
    }
    Ok(())
}

/// Creates a new file-backed memory store for CLI commands.
async fn open_cli_memory_store(
    config_manager: &ConfigManager,
    root_overrides: &ConfigOverrides,
    store_override: Option<&str>,
) -> Result<deepagents::memory::FileMemoryStore> {
    let effective = config_manager.resolve_effective(root_overrides)?;
    let store_path = resolve_memory_store_path(config_manager, &effective, store_override);
    let store = deepagents::memory::FileMemoryStore::new(store_path);
    store.load().await?;
    Ok(store)
}

/// Builds a baseline memory entry with command-specific defaults.
fn default_memory_entry(
    key: String,
    value: String,
    defaults: MemoryCommandDefaults,
) -> deepagents::memory::MemoryEntry {
    deepagents::memory::MemoryEntry {
        key,
        value,
        title: None,
        scope: defaults.scope,
        scope_id: None,
        memory_type: defaults.memory_type,
        pinned: defaults.pinned,
        status: deepagents::memory::MemoryStatus::Active,
        confidence: None,
        salience: None,
        supersedes: None,
        owner_user_id: None,
        owner_workspace_id: None,
        owner_channel_account_id: None,
        tags: Vec::new(),
        created_at: String::new(),
        updated_at: String::new(),
        last_accessed_at: String::new(),
        access_count: 0,
    }
}

/// Applies one CLI mutation to an existing or new memory entry.
fn apply_memory_mutation(
    existing: Option<deepagents::memory::MemoryEntry>,
    key: String,
    defaults: MemoryCommandDefaults,
    actor: &MemoryActorContext,
    mutation: MemoryMutation,
) -> Result<deepagents::memory::MemoryEntry> {
    let mut entry = if let Some(existing) = existing {
        existing
    } else {
        default_memory_entry(
            key.clone(),
            mutation
                .value
                .clone()
                .ok_or_else(|| anyhow!("invalid_arguments: --value is required"))?,
            defaults,
        )
    };

    entry.key = key;
    if let Some(value) = mutation.value {
        entry.value = value;
    }
    if let Some(title) = mutation.title {
        entry.title = Some(title);
    }
    if let Some(tags) = mutation.tags {
        entry.tags = tags;
    }
    if let Some(confidence) = mutation.confidence {
        validate_memory_score("confidence", confidence)?;
        entry.confidence = Some(confidence);
    }
    if let Some(salience) = mutation.salience {
        validate_memory_score("salience", salience)?;
        entry.salience = Some(salience);
    }
    if let Some(supersedes) = mutation.supersedes {
        entry.supersedes = Some(supersedes);
    }
    if let Some(status) = mutation.status {
        entry.status = status;
    }
    if let Some(memory_type) = mutation.memory_type {
        entry.memory_type = memory_type;
    }
    if let Some(pinned) = mutation.pinned {
        entry.pinned = pinned;
    }

    let scope = mutation.scope.unwrap_or(entry.scope);
    entry.scope = scope;
    match scope {
        deepagents::memory::MemoryScope::User => {
            entry.scope_id = None;
            if actor.user_id.is_some() {
                entry.owner_user_id = actor.user_id.clone();
            }
            if actor.channel_account_id.is_some() {
                entry.owner_channel_account_id = actor.channel_account_id.clone();
            }
            entry.owner_workspace_id = None;
        }
        deepagents::memory::MemoryScope::Thread => {
            entry.scope_id = mutation
                .scope_id
                .or(entry.scope_id)
                .or(actor.thread_id.clone());
            if entry.scope_id.is_none() {
                return Err(anyhow!(
                    "invalid_arguments: thread scope requires --scope-id or --actor-thread-id"
                ));
            }
            entry.owner_workspace_id = None;
            entry.owner_user_id = None;
        }
        deepagents::memory::MemoryScope::Workspace => {
            entry.scope_id = None;
            if actor.workspace_id.is_some() {
                entry.owner_workspace_id = actor.workspace_id.clone();
            }
            entry.owner_user_id = None;
        }
        deepagents::memory::MemoryScope::System => {
            entry.scope_id = None;
            entry.owner_user_id = None;
            entry.owner_workspace_id = None;
        }
    }
    Ok(entry)
}

/// Applies the same scope visibility logic to `memory get/edit/delete` that the
/// store already uses for `query`.
fn memory_entry_visible_to_actor(
    entry: &deepagents::memory::MemoryEntry,
    actor: &MemoryActorContext,
    requested_scope_id: Option<&str>,
) -> bool {
    match entry.scope {
        deepagents::memory::MemoryScope::User => match entry.owner_user_id.as_deref() {
            Some(owner) => {
                if let Some(actor_user_id) = actor.user_id.as_deref() {
                    owner == actor_user_id
                } else if let Some(actor_channel) = actor.channel_account_id.as_deref() {
                    entry.owner_channel_account_id.as_deref() == Some(actor_channel)
                } else {
                    false
                }
            }
            None => true,
        },
        deepagents::memory::MemoryScope::Thread => match entry.scope_id.as_deref() {
            Some(thread_id) => actor
                .thread_id
                .as_deref()
                .or(requested_scope_id)
                .map(|value| value == thread_id)
                .unwrap_or(false),
            None => true,
        },
        deepagents::memory::MemoryScope::Workspace => match entry.owner_workspace_id.as_deref() {
            Some(workspace_id) => actor.workspace_id.as_deref() == Some(workspace_id),
            None => true,
        },
        deepagents::memory::MemoryScope::System => true,
    }
}

/// Rejects unauthorized mutations while still allowing unowned compatibility
/// records created without actor identity.
fn ensure_memory_mutation_allowed(
    entry: &deepagents::memory::MemoryEntry,
    actor: &MemoryActorContext,
    requested_scope_id: Option<&str>,
) -> Result<()> {
    if memory_entry_visible_to_actor(entry, actor, requested_scope_id) {
        return Ok(());
    }
    Err(anyhow!(
        "memory_permission_denied: actor cannot modify {}",
        entry.key
    ))
}

/// Renders a stable JSON representation for one memory item.
fn memory_entry_json(entry: &deepagents::memory::MemoryEntry) -> serde_json::Value {
    serde_json::json!({
        "key": entry.key,
        "value": entry.value,
        "title": entry.title,
        "scope": entry.scope,
        "scope_id": entry.scope_id,
        "memory_type": entry.memory_type,
        "pinned": entry.pinned,
        "status": entry.status,
        "confidence": entry.confidence,
        "salience": entry.salience,
        "supersedes": entry.supersedes,
        "tags": entry.tags,
        "created_at": entry.created_at,
        "updated_at": entry.updated_at,
        "last_accessed_at": entry.last_accessed_at,
        "access_count": entry.access_count,
    })
}

/// Produces the null-shaped `memory get` payload used for not-found or
/// unauthorized reads.
fn null_memory_get_json(key: &str) -> serde_json::Value {
    serde_json::json!({
        "key": key,
        "value": serde_json::Value::Null,
    })
}

/// Appends a lightweight JSONL record for each CLI run when `--audit-json` is
/// configured so observability tests can validate the run/audit contract.
fn append_run_audit_record(
    path: Option<&str>,
    root: &str,
    thread_id: &str,
    status: deepagents::runtime::RunStatus,
    error: Option<&deepagents::runtime::RuntimeError>,
) -> Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    let path = std::path::Path::new(path);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let payload = serde_json::json!({
        "event_type": "run",
        "timestamp_ms": now_ms(),
        "root": root,
        "thread_id": thread_id,
        "status": status,
        "error": error,
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    use std::io::Write as _;
    writeln!(file, "{}", serde_json::to_string(&payload)?)?;
    Ok(())
}

/// Removes ephemeral auto-generated thread IDs from stdout-only run responses
/// so `--pretty` and non-pretty runs stay semantically identical.
fn strip_transient_thread_id_for_output(
    out: &mut deepagents::runtime::RunOutput,
    explicit_thread_id: Option<&str>,
    state_file: Option<&str>,
) {
    if explicit_thread_id.is_some() || state_file.is_some() {
        return;
    }
    out.state.extra.remove("thread_id");
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
        allow_versionless_compat: true,
    };
    deepagents::skills::loader::load_skills(sources, options)
}

/// Returns the default file-backed registry directory rooted under the current
/// workspace.
fn default_skill_registry_dir(root: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(root)
        .join(".deepagents")
        .join("skills")
}

/// Resolves the registry path for registry-manipulating CLI commands.
fn resolve_skill_registry_dir(root: &str, registry: Option<&str>) -> std::path::PathBuf {
    registry
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| default_skill_registry_dir(root))
}

/// Resolves the optional registry path for run-time skill resolution.
fn resolve_run_skill_registry_dir(
    root: &str,
    registry: Option<&str>,
    force_default: bool,
) -> Option<String> {
    let path = resolve_skill_registry_dir(root, registry);
    if registry.is_some() || force_default || path.exists() {
        return Some(path.to_string_lossy().into_owned());
    }
    None
}

/// Parses the CLI-facing selection mode onto the library enum.
fn parse_skill_selection_mode(
    value: Option<&str>,
) -> Result<deepagents::skills::selection::SkillSelectionMode> {
    match value.unwrap_or("auto") {
        "auto" => Ok(deepagents::skills::selection::SkillSelectionMode::Auto),
        "manual" => Ok(deepagents::skills::selection::SkillSelectionMode::Manual),
        "off" => Ok(deepagents::skills::selection::SkillSelectionMode::Off),
        other => Err(anyhow!(
            "invalid_arguments: --skill-select must be one of auto|manual|off, got {other}"
        )),
    }
}

/// Builds a stable registry summary used by `skill install`, `skill status`,
/// and `skill versions`.
fn registry_summary_json(
    entries: &[deepagents::skills::SkillRegistryEntry],
    loaded: &deepagents::skills::LoadedSkills,
) -> serde_json::Value {
    let enabled = entries
        .iter()
        .filter(|entry| entry.lifecycle == deepagents::skills::SkillLifecycleState::Enabled)
        .count();
    let disabled = entries
        .iter()
        .filter(|entry| entry.lifecycle == deepagents::skills::SkillLifecycleState::Disabled)
        .count();
    let quarantined = entries
        .iter()
        .filter(|entry| entry.lifecycle == deepagents::skills::SkillLifecycleState::Quarantined)
        .count();
    serde_json::json!({
        "entries": entries.len(),
        "enabled": enabled,
        "disabled": disabled,
        "quarantined": quarantined,
        "skills": loaded.metadata.len(),
        "tools": loaded.tools.len(),
        "diagnostics": loaded.diagnostics.records.len(),
    })
}

/// Implements `skill install`.
fn skill_install_command(
    root: &str,
    registry: Option<&str>,
    sources: &[String],
) -> Result<serde_json::Value> {
    if sources.is_empty() {
        return Err(anyhow!("invalid_arguments: --source is required"));
    }
    let registry_dir = resolve_skill_registry_dir(root, registry);
    let report = deepagents::skills::registry::install_sources_into_registry(
        sources,
        &registry_dir,
        deepagents::skills::loader::SkillsLoadOptions {
            skip_invalid_sources: false,
            strict: true,
            allow_versionless_compat: false,
        },
    )?;
    let status = deepagents::skills::registry::registry_status(&registry_dir)?;
    let loaded = deepagents::skills::registry::registry_loaded_skills(&registry_dir)?;
    Ok(serde_json::json!({
        "ok": true,
        "command": "install",
        "registry": registry_dir.to_string_lossy().to_string(),
        "summary": registry_summary_json(&status, &loaded),
        "installed": report.installed,
        "unchanged": report.unchanged,
        "entries": status,
        "diagnostics": loaded.diagnostics,
    }))
}

/// Implements `skill status`.
fn skill_status_command(root: &str, registry: Option<&str>) -> Result<serde_json::Value> {
    let registry_dir = resolve_skill_registry_dir(root, registry);
    let status = deepagents::skills::registry::registry_status(&registry_dir)?;
    let loaded = deepagents::skills::registry::registry_loaded_skills(&registry_dir)?;
    Ok(serde_json::json!({
        "ok": true,
        "command": "status",
        "registry": registry_dir.to_string_lossy().to_string(),
        "summary": registry_summary_json(&status, &loaded),
        "entries": status,
        "diagnostics": loaded.diagnostics,
    }))
}

/// Implements `skill versions`.
fn skill_versions_command(
    root: &str,
    registry: Option<&str>,
    name: &str,
) -> Result<serde_json::Value> {
    let registry_dir = resolve_skill_registry_dir(root, registry);
    let versions = deepagents::skills::registry::registry_versions(&registry_dir, name)?;
    let loaded = deepagents::skills::registry::registry_loaded_skills(&registry_dir)?;
    Ok(serde_json::json!({
        "ok": true,
        "command": "versions",
        "registry": registry_dir.to_string_lossy().to_string(),
        "name": name,
        "summary": registry_summary_json(&versions, &loaded),
        "versions": versions,
    }))
}

/// Blocks lifecycle transitions that would violate governance invariants.
fn validate_registry_lifecycle_transition(
    root: &str,
    registry: Option<&str>,
    identity: &(String, Option<String>),
    lifecycle: deepagents::skills::SkillLifecycleState,
) -> Result<()> {
    if lifecycle != deepagents::skills::SkillLifecycleState::Enabled {
        return Ok(());
    }
    let registry_dir = resolve_skill_registry_dir(root, registry);
    let entries = deepagents::skills::registry::registry_status(&registry_dir)?;
    let violating = entries.into_iter().find(|entry| {
        entry.identity.name == identity.0
            && identity
                .1
                .as_deref()
                .is_none_or(|value| value == entry.identity.version)
            && (entry.governance.status == deepagents::skills::SkillGovernanceStatus::Fail
                || entry.lifecycle == deepagents::skills::SkillLifecycleState::Quarantined)
    });
    if let Some(entry) = violating {
        return Err(anyhow!(
            "governance_blocked: {} cannot be enabled because semantic review failed",
            entry.identity.as_key()
        ));
    }
    Ok(())
}

/// Implements `skill enable` and `skill disable`.
fn skill_lifecycle_command(
    root: &str,
    registry: Option<&str>,
    command: &'static str,
    identity: &str,
    lifecycle: deepagents::skills::SkillLifecycleState,
    reason: Option<String>,
) -> Result<serde_json::Value> {
    let parsed = deepagents::skills::registry::parse_identity_token(identity)?;
    validate_registry_lifecycle_transition(root, registry, &parsed, lifecycle)?;
    let registry_dir = resolve_skill_registry_dir(root, registry);
    let changed = deepagents::skills::registry::set_registry_lifecycle(
        &registry_dir,
        &parsed.0,
        parsed.1.as_deref(),
        lifecycle,
        reason,
    )?;
    let mut out = serde_json::json!({
        "ok": true,
        "command": command,
        "registry": registry_dir.to_string_lossy().to_string(),
        "changed": changed,
    });
    if let Some(object) = out.as_object_mut() {
        let status_key = match command {
            "enable" => "enabled",
            "disable" => "disabled",
            other => other,
        };
        object.insert(status_key.to_string(), serde_json::Value::Bool(true));
    }
    Ok(out)
}

/// Implements `skill quarantine`.
fn skill_quarantine_command(
    root: &str,
    registry: Option<&str>,
    identity: &str,
    reason: Option<String>,
) -> Result<serde_json::Value> {
    let parsed = deepagents::skills::registry::parse_identity_token(identity)?;
    let Some(version) = parsed.1.as_deref() else {
        return Err(anyhow!(
            "invalid_arguments: skill quarantine requires name@version"
        ));
    };
    let registry_dir = resolve_skill_registry_dir(root, registry);
    let changed = deepagents::skills::registry::set_registry_lifecycle(
        &registry_dir,
        &parsed.0,
        Some(version),
        deepagents::skills::SkillLifecycleState::Quarantined,
        Some(reason.unwrap_or_else(|| "quarantined_by_cli".to_string())),
    )?;
    Ok(serde_json::json!({
        "ok": true,
        "command": "quarantine",
        "registry": registry_dir.to_string_lossy().to_string(),
        "changed": changed,
        "quarantined": !changed.is_empty(),
    }))
}

/// Implements `skill remove`.
fn skill_remove_command(
    root: &str,
    registry: Option<&str>,
    identity: &str,
) -> Result<serde_json::Value> {
    let parsed = deepagents::skills::registry::parse_identity_token(identity)?;
    let Some(version) = parsed.1.as_deref() else {
        return Err(anyhow!("invalid_arguments: skill remove requires name@version"));
    };
    let registry_dir = resolve_skill_registry_dir(root, registry);
    let removed =
        deepagents::skills::registry::remove_registry_entry(&registry_dir, &parsed.0, version)?;
    Ok(serde_json::json!({
        "ok": true,
        "command": "remove",
        "registry": registry_dir.to_string_lossy().to_string(),
        "removed": true,
        "entry": removed,
    }))
}

/// Implements `skill resolve`.
fn skill_resolve_command(
    root: &str,
    registry: Option<&str>,
    input: &str,
    sources: &[String],
    skills: &[String],
    disabled_skills: &[String],
    skill_select: Option<&str>,
    skill_max_active: Option<usize>,
    refresh_skill_snapshot: bool,
) -> Result<serde_json::Value> {
    let registry_dir =
        resolve_run_skill_registry_dir(root, registry, !skills.is_empty() || !disabled_skills.is_empty());
    let mut diagnostics = deepagents::skills::SkillsDiagnostics::default();
    if let Some(registry_dir) = registry_dir.as_deref() {
        let loaded =
            deepagents::skills::registry::registry_loaded_skills(std::path::Path::new(registry_dir))?;
        diagnostics.records.extend(loaded.diagnostics.records);
        diagnostics.overrides.extend(loaded.diagnostics.overrides);
    }
    if !sources.is_empty() {
        let loaded = load_skills_from_sources(sources)?;
        diagnostics.sources.extend(loaded.diagnostics.sources);
        diagnostics.records.extend(loaded.diagnostics.records);
        diagnostics.overrides.extend(loaded.diagnostics.overrides);
    }

    let message = deepagents::types::Message {
        role: "user".to_string(),
        content: input.to_string(),
        content_blocks: None,
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        status: None,
    };
    let snapshot = deepagents::skills::selection::resolve_skill_snapshot(
        &[message],
        &deepagents::state::AgentState::default(),
        &deepagents::skills::selection::SkillResolverOptions {
            registry_dir,
            sources: sources.to_vec(),
            source_options: deepagents::skills::loader::SkillsLoadOptions {
                skip_invalid_sources: false,
                strict: true,
                allow_versionless_compat: true,
            },
            explicit_skills: skills.to_vec(),
            disabled_skills: disabled_skills.to_vec(),
            selection_mode: parse_skill_selection_mode(skill_select)?,
            max_active: skill_max_active.unwrap_or(3).max(1),
            refresh_snapshot: refresh_skill_snapshot,
        },
    )?;
    Ok(serde_json::json!({
        "ok": true,
        "command": "resolve",
        "summary": {
            "selected": snapshot.as_ref().map(|value| value.selection.selected.len()).unwrap_or(0),
            "skipped": snapshot.as_ref().map(|value| value.selection.skipped.len()).unwrap_or(0),
            "candidates": snapshot.as_ref().map(|value| value.selection.candidates.len()).unwrap_or(0),
        },
        "selection": snapshot.as_ref().map(|value| value.selection.clone()),
        "snapshot": snapshot,
        "diagnostics": diagnostics,
    }))
}

/// Returns the persisted audit file path for one thread.
fn skill_audit_path(root: &str, thread_id: &str) -> std::path::PathBuf {
    default_skill_registry_dir(root)
        .join("audit")
        .join(format!("{thread_id}.json"))
}

/// Implements `skill audit`.
fn skill_audit_command(root: &str, thread_id: &str) -> Result<serde_json::Value> {
    let path = skill_audit_path(root, thread_id);
    if !path.exists() {
        return Err(anyhow!("audit_not_found: {}", path.display()));
    }
    let bytes = std::fs::read(&path)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    Ok(serde_json::json!({
        "ok": true,
        "command": "audit",
        "thread_id": thread_id,
        "path": path.to_string_lossy().to_string(),
        "record": value,
    }))
}

/// Ensures every CLI `run` invocation carries a stable thread ID in state.
fn ensure_cli_thread_id(state: &mut deepagents::state::AgentState, explicit: Option<&str>) -> String {
    if let Some(thread_id) = explicit {
        state.extra.insert(
            "thread_id".to_string(),
            serde_json::Value::String(thread_id.to_string()),
        );
        return thread_id.to_string();
    }
    if let Some(thread_id) = state.extra.get("thread_id").and_then(|value| value.as_str()) {
        return thread_id.to_string();
    }
    let generated = format!("thread-{}", now_ms());
    state.extra.insert(
        "thread_id".to_string(),
        serde_json::Value::String(generated.clone()),
    );
    generated
}

/// Persists run state and writes a thread-scoped skill audit record for every
/// run so post-run audit lookup remains stable even when no skills were loaded.
fn persist_run_state_and_skill_audit(
    state_file: Option<&str>,
    root: &str,
    thread_id: &str,
    state: &deepagents::state::AgentState,
    trace: Option<&serde_json::Value>,
    status: deepagents::runtime::RunStatus,
    error: Option<&deepagents::runtime::RuntimeError>,
) -> Result<()> {
    if let Some(state_file) = state_file {
        save_state(state_file, state)?;
    }
    let snapshot = state
        .extra
        .get(deepagents::skills::SKILLS_SNAPSHOT_KEY)
        .cloned();
    let selection = state
        .extra
        .get(deepagents::skills::SKILLS_SELECTION_KEY)
        .cloned();
    let diagnostics = state
        .extra
        .get(deepagents::skills::SKILLS_DIAGNOSTICS_KEY)
        .cloned();
    let trace_skills = trace.and_then(|trace| trace.get("skills")).cloned();
    let path = skill_audit_path(root, thread_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let payload = serde_json::json!({
        "timestamp_ms": now_ms(),
        "thread_id": thread_id,
        "root": root,
        "status": status,
        "error": error,
        "trace": {
            "skills": trace_skills,
        },
        "snapshot": snapshot,
        "selection": selection,
        "diagnostics": diagnostics,
    });
    std::fs::write(path, serde_json::to_vec_pretty(&payload)?)?;
    Ok(())
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
            eprintln!("{}", format_error_chain(&error));
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
    if message.starts_with("invalid_identity:") {
        return "invalid_identity";
    }
    if message.starts_with("registry_conflict:") {
        return "registry_conflict";
    }
    if message.starts_with("registry_entry_not_found:") {
        return "registry_entry_not_found";
    }
    if message.starts_with("audit_not_found:") {
        return "audit_not_found";
    }
    if message.starts_with("governance_blocked:") {
        return "governance_blocked";
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
        "---\nname: {name}\nversion: 0.1.0\ndescription: Describe what this skill does and when to use it.\nallowed-tools: []\ntriggers:\n  keywords: []\nrisk-level: low\ndefault-enabled: true\nrequires-isolation: false\n---\n\n# {name}\n\n## Role\nDescribe the role this skill should play.\n\n## When to Use\n- Describe the requests that should activate this skill.\n\n## Inputs\n- Describe required inputs and assumptions.\n\n## Constraints\n- Describe policy, safety, and correctness constraints.\n\n## Workflow\n1. Describe the ordered workflow.\n\n## Output\n- Describe the expected output contract.\n\n## Examples\n- Add short examples that improve routing.\n\n## References\n- Add optional reference notes or package assets.\n",
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
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
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
