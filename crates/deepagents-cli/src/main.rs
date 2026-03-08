use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use deepagents::approval::{redact_command, ApprovalDecision, ApprovalRequest, DefaultApprovalPolicy, ExecutionMode};
use deepagents::audit::{AuditEvent, AuditSink};
use deepagents::runtime::Runtime;
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
        mock_script: Option<String>,
        #[arg(long)]
        plugin: Vec<String>,
        #[arg(long, default_value_t = 8)]
        max_steps: usize,
        #[arg(long, default_value_t = 1000)]
        provider_timeout_ms: u64,
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
                    json.get("command").and_then(|v| v.as_str()).map(|s| s.to_string())
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
                                    exit_code: out.get("exit_code").and_then(|v| v.as_i64()).map(|v| v as i32),
                                    truncated: out.get("truncated").and_then(|v| v.as_bool()),
                                    duration_ms: Some(started.elapsed().as_millis() as u64),
                                });
                            }
                        }
                        let resp = serde_json::json!({
                            "output": out,
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
                    json.get("command").and_then(|v| v.as_str()).map(|s| s.to_string())
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
                            exit_code: out.get("exit_code").and_then(|v| v.as_i64()).map(|v| v as i32),
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
            mock_script,
            plugin,
            max_steps,
            provider_timeout_ms,
            pretty,
        } => {
            let provider: std::sync::Arc<dyn deepagents::provider::Provider> = match provider.as_str() {
                "mock" => {
                    let path = mock_script.ok_or_else(|| anyhow!("--mock-script is required for --provider mock"))?;
                    let script = deepagents::provider::mock::MockProvider::load_from_file(&path)?;
                    std::sync::Arc::new(deepagents::provider::mock::MockProvider::from_script(script))
                }
                "mock2" => {
                    let path = mock_script.ok_or_else(|| anyhow!("--mock-script is required for --provider mock2"))?;
                    let script = deepagents::provider::mock::MockProvider::load_from_file(&path)?;
                    std::sync::Arc::new(deepagents::provider::mock::MockProvider::from_script_without_call_ids(script))
                }
                other => return Err(anyhow!("unknown --provider: {other}")),
            };

            let mut skills: Vec<std::sync::Arc<dyn deepagents::skills::SkillPlugin>> = Vec::new();
            for p in plugin {
                skills.push(std::sync::Arc::new(
                    deepagents::skills::declarative::DeclarativeSkillPlugin::load_from_file(&p)?,
                ));
            }

            let runtime = deepagents::runtime::simple::SimpleRuntime::new(
                agent,
                provider,
                skills,
                deepagents::runtime::RuntimeConfig {
                    max_steps,
                    provider_timeout_ms,
                },
                Some(policy),
                audit_sink,
                root.clone(),
                mode,
            );

            let out = runtime
                .run(vec![deepagents::types::Message {
                    role: "user".to_string(),
                    content: input,
                }])
                .await;

            let ok = out.error.is_none();
            if pretty {
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("{}", serde_json::to_string(&out)?);
            }
            if !ok {
                return Err(anyhow!("runtime_error"));
            }
        }
    }
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
        out.extend(v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
    }
    if let Ok(p) = std::env::var("DEEPAGENTS_SHELL_ALLOW_FILE") {
        out.extend(read_allow_file(&p).unwrap_or_default());
    }
    normalize_allow_list(out)
}

fn read_allow_file(path: &str) -> Result<Vec<String>> {
    let content = std::fs::read_to_string(path)?;
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
