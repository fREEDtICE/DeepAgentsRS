use std::panic::AssertUnwindSafe;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use futures_util::FutureExt;
use tokio::time::{timeout, Duration};

use crate::approval::{redact_command, ApprovalDecision, ApprovalRequest, ExecutionMode};
use crate::audit::{AuditEvent, AuditSink};
use crate::runtime::{HandledToolCall, RuntimeMiddleware, ToolCallContext};
use crate::skills::loader::{load_skills, SkillsLoadOptions};
use crate::skills::validator::{
    classify_package_skill_step_tool, validate_package_skill_input, PackageSkillStepKind,
};
use crate::skills::{LoadedSkills, SkillMetadata, SkillToolSpec, SkillsDiagnostics};
use crate::state::AgentState;
use crate::types::Message;

pub struct SkillsMiddleware {
    sources: Vec<String>,
    options: SkillsLoadOptions,
    state: Arc<RwLock<LoadedSkills>>,
}

impl SkillsMiddleware {
    pub fn new(sources: Vec<String>, options: SkillsLoadOptions) -> Self {
        Self {
            sources,
            options,
            state: Arc::new(RwLock::new(LoadedSkills::default())),
        }
    }

    pub async fn loaded(&self) -> LoadedSkills {
        self.state.read().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl RuntimeMiddleware for SkillsMiddleware {
    async fn before_run(
        &self,
        mut messages: Vec<Message>,
        state: &mut AgentState,
    ) -> Result<Vec<Message>> {
        let loaded = if let Some(loaded) = restore_loaded_skills(state)? {
            loaded
        } else {
            let loaded = load_skills(&self.sources, self.options.clone())?;
            store_loaded_skills_snapshot(state, &loaded)?;
            loaded
        };
        *self.state.write().unwrap() = loaded.clone();

        let block = build_skills_block(&loaded);
        upsert_injection_message(&mut messages, block);

        Ok(messages)
    }

    async fn handle_tool_call(
        &self,
        ctx: &mut ToolCallContext<'_>,
    ) -> Result<Option<HandledToolCall>> {
        let loaded = self.state.read().unwrap().clone();
        let tool = match loaded
            .tools
            .iter()
            .find(|t| t.name == ctx.tool_call.tool_name)
        {
            Some(t) => t.clone(),
            None => return Ok(None),
        };

        if let Err(e) = validate_package_skill_input(&tool.input_schema, &ctx.tool_call.arguments) {
            return Ok(Some(HandledToolCall {
                output: serde_json::Value::Null,
                error: Some(format!("invalid_request: {}", e)),
            }));
        }

        if tool.steps.len() > tool.policy.max_steps {
            return Ok(Some(HandledToolCall {
                output: serde_json::Value::Null,
                error: Some(format!(
                    "skill_steps_exceeded: max={}",
                    tool.policy.max_steps
                )),
            }));
        }

        // Catch panics inside skill execution so a buggy package step is
        // surfaced as a tool error instead of unwinding the whole runner.
        let result = timeout(
            Duration::from_millis(tool.policy.timeout_ms),
            AssertUnwindSafe(execute_skill_steps(ctx, &tool)).catch_unwind(),
        )
        .await;
        match result {
            Ok(Ok(Ok(out))) => Ok(Some(HandledToolCall {
                output: out,
                error: None,
            })),
            Ok(Ok(Err(e))) => Ok(Some(HandledToolCall {
                output: serde_json::Value::Null,
                error: Some(e),
            })),
            Ok(Err(payload)) => Ok(Some(HandledToolCall {
                output: serde_json::Value::Null,
                error: Some(format!(
                    "skill_panic: {}",
                    panic_payload_message(payload.as_ref())
                )),
            })),
            Err(_) => Ok(Some(HandledToolCall {
                output: serde_json::Value::Null,
                error: Some(format!(
                    "skill_timeout: exceeded {}ms",
                    tool.policy.timeout_ms
                )),
            })),
        }
    }
}

async fn execute_skill_steps(
    ctx: &mut ToolCallContext<'_>,
    tool: &SkillToolSpec,
) -> Result<serde_json::Value, String> {
    let mut last_output = serde_json::Value::Null;
    for step in &tool.steps {
        validate_step_access(step, &tool.policy)?;
        if !step.arguments.is_object() {
            return Err(format!(
                "invalid_skill_definition: step {} arguments must be object",
                step.tool_name
            ));
        }
        let mut args = step.arguments.clone();
        args = merge_args(args, &ctx.tool_call.arguments);
        let output = if step.tool_name == "execute" {
            execute_with_approval(ctx, &args).await?
        } else {
            ctx.agent
                .call_tool_stateful(&step.tool_name, args, ctx.state)
                .await
                .map(|(out, _delta)| out.output)
                .map_err(|e| format!("skill_step_failed: {}: {}", step.tool_name, e))?
        };
        last_output = truncate_output(output, tool.policy.max_output_chars);
    }
    Ok(last_output)
}

async fn execute_with_approval(
    ctx: &mut ToolCallContext<'_>,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let cmd = args
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if let Some(policy) = ctx.approval {
        let req = ApprovalRequest {
            command: cmd.clone(),
            root: ctx.root.to_string(),
            mode: ctx.mode,
        };
        let decision = policy.decide(&req);
        match decision {
            ApprovalDecision::Allow { reason } => {
                let started = std::time::Instant::now();
                let result = ctx
                    .agent
                    .call_tool_stateful("execute", args.clone(), ctx.state)
                    .await;
                let duration_ms = started.elapsed().as_millis() as u64;
                match result {
                    Ok((out, _delta)) => {
                        let output = out.output;
                        record_audit(AuditRecordInput {
                            sink: ctx.audit,
                            cmd: &cmd,
                            root: ctx.root,
                            mode: ctx.mode,
                            decision: "allow",
                            decision_code: "allow",
                            decision_reason: reason,
                            duration_ms,
                            output: Some(&output),
                        });
                        Ok(output)
                    }
                    Err(e) => {
                        record_audit(AuditRecordInput {
                            sink: ctx.audit,
                            cmd: &cmd,
                            root: ctx.root,
                            mode: ctx.mode,
                            decision: "allow",
                            decision_code: "allow",
                            decision_reason: "allowed but execution failed".to_string(),
                            duration_ms,
                            output: None,
                        });
                        Err(format!("skill_step_failed: execute: {}", e))
                    }
                }
            }
            ApprovalDecision::Deny { code, reason } => {
                record_audit(AuditRecordInput {
                    sink: ctx.audit,
                    cmd: &cmd,
                    root: ctx.root,
                    mode: ctx.mode,
                    decision: "deny",
                    decision_code: &code,
                    decision_reason: reason.clone(),
                    duration_ms: 0,
                    output: None,
                });
                Err(format!("command_not_allowed: {}: {}", code, reason))
            }
            ApprovalDecision::RequireApproval { code, reason } => {
                record_audit(AuditRecordInput {
                    sink: ctx.audit,
                    cmd: &cmd,
                    root: ctx.root,
                    mode: ctx.mode,
                    decision: "require_approval",
                    decision_code: &code,
                    decision_reason: reason.clone(),
                    duration_ms: 0,
                    output: None,
                });
                Err(format!("command_not_allowed: {}: {}", code, reason))
            }
        }
    } else {
        ctx.agent
            .call_tool_stateful("execute", args.clone(), ctx.state)
            .await
            .map(|(out, _delta)| out.output)
            .map_err(|e| format!("skill_step_failed: execute: {}", e))
    }
}

struct AuditRecordInput<'a> {
    sink: Option<&'a Arc<dyn AuditSink>>,
    cmd: &'a str,
    root: &'a str,
    mode: ExecutionMode,
    decision: &'a str,
    decision_code: &'a str,
    decision_reason: String,
    duration_ms: u64,
    output: Option<&'a serde_json::Value>,
}

fn record_audit(input: AuditRecordInput<'_>) {
    let AuditRecordInput {
        sink,
        cmd,
        root,
        mode,
        decision,
        decision_code,
        decision_reason,
        duration_ms,
        output,
    } = input;
    let Some(sink) = sink else { return };
    let (exit_code, truncated) = output
        .and_then(|v| v.as_object())
        .map(|o| {
            (
                o.get("exit_code")
                    .and_then(|v| v.as_i64())
                    .map(|v| v as i32),
                o.get("truncated").and_then(|v| v.as_bool()),
            )
        })
        .unwrap_or((None, None));
    let _ = sink.record(AuditEvent {
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        root: root.to_string(),
        mode: mode_str(mode),
        command_redacted: redact_command(cmd),
        decision: decision.to_string(),
        decision_code: decision_code.to_string(),
        decision_reason,
        exit_code,
        truncated,
        duration_ms: Some(duration_ms),
    });
}

fn mode_str(mode: ExecutionMode) -> String {
    match mode {
        ExecutionMode::NonInteractive => "non_interactive".to_string(),
        ExecutionMode::Interactive => "interactive".to_string(),
    }
}

/// Enforces the deny-by-default execution boundary for package skill steps.
fn validate_step_access(
    step: &crate::skills::SkillToolStep,
    policy: &crate::skills::SkillToolPolicy,
) -> Result<(), String> {
    match classify_package_skill_step_tool(&step.tool_name).map_err(|error| error.to_string())? {
        PackageSkillStepKind::AgentOwned => Ok(()),
        PackageSkillStepKind::Filesystem => {
            if policy.allow_filesystem {
                Ok(())
            } else {
                Err(format!(
                    "permission_denied: {} requires allow_filesystem=true",
                    step.tool_name
                ))
            }
        }
        PackageSkillStepKind::Execute => {
            if policy.allow_execute {
                Ok(())
            } else {
                Err("permission_denied: execute requires allow_execute=true".to_string())
            }
        }
    }
}

/// Extracts a readable panic message so skill unwinds can be surfaced as
/// regular tool errors.
fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "skill step panicked".to_string()
}

fn build_skills_block(loaded: &LoadedSkills) -> String {
    let mut out = String::new();
    out.push_str("DEEPAGENTS_SKILLS_INJECTED_V1\n");
    out.push_str("## Skills\n");
    for skill in &loaded.metadata {
        out.push_str("- ");
        out.push_str(&skill.name);
        out.push_str(": ");
        out.push_str(&skill.description);
        out.push_str(" (source: ");
        out.push_str(&skill.source);
        out.push(')');
        if !skill.allowed_tools.is_empty() {
            out.push_str(" Allowed tools: ");
            out.push_str(&skill.allowed_tools.join(", "));
        }
        out.push('\n');
    }
    out
}

fn has_injection_marker(messages: &[Message]) -> bool {
    messages
        .iter()
        .any(|m| m.content.contains("DEEPAGENTS_SKILLS_INJECTED_V1"))
}

fn restore_loaded_skills(state: &mut AgentState) -> Result<Option<LoadedSkills>> {
    let Some(metadata) = state.extra.get("skills_metadata").cloned() else {
        return Ok(None);
    };
    let Some(tools) = state.extra.get("skills_tools").cloned() else {
        return Ok(None);
    };

    let metadata = match serde_json::from_value::<Vec<SkillMetadata>>(metadata) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(None),
    };
    let tools = match serde_json::from_value::<Vec<SkillToolSpec>>(tools) {
        Ok(tools) => tools,
        Err(_) => return Ok(None),
    };
    let diagnostics = match state.extra.get("skills_diagnostics").cloned() {
        Some(value) => match serde_json::from_value::<SkillsDiagnostics>(value) {
            Ok(diagnostics) => diagnostics,
            Err(_) => return Ok(None),
        },
        None => SkillsDiagnostics::default(),
    };

    let mut loaded = LoadedSkills {
        metadata,
        tools,
        diagnostics,
    };
    loaded.canonicalize();
    store_loaded_skills_snapshot(state, &loaded)?;
    Ok(Some(loaded))
}

fn store_loaded_skills_snapshot(state: &mut AgentState, loaded: &LoadedSkills) -> Result<()> {
    state.extra.insert(
        "skills_metadata".to_string(),
        serde_json::to_value(&loaded.metadata)?,
    );
    state.extra.insert(
        "skills_tools".to_string(),
        serde_json::to_value(&loaded.tools)?,
    );
    state.extra.insert(
        "skills_diagnostics".to_string(),
        serde_json::to_value(&loaded.diagnostics)?,
    );
    Ok(())
}

fn upsert_injection_message(messages: &mut Vec<Message>, block: String) {
    if !has_injection_marker(messages) {
        messages.insert(0, skills_injection_message(block));
        return;
    }

    let mut seen_marker = false;
    messages.retain(|message| {
        if !message.content.contains("DEEPAGENTS_SKILLS_INJECTED_V1") {
            return true;
        }
        if !seen_marker {
            seen_marker = true;
            return true;
        }
        false
    });

    if let Some(message) = messages
        .iter_mut()
        .find(|message| message.content.contains("DEEPAGENTS_SKILLS_INJECTED_V1"))
    {
        *message = skills_injection_message(block);
    }
}

fn skills_injection_message(block: String) -> Message {
    Message {
        role: "system".to_string(),
        content: block,
        content_blocks: None,
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        status: None,
    }
}

fn merge_args(base: serde_json::Value, overlay: &serde_json::Value) -> serde_json::Value {
    let Some(overlay_map) = overlay.as_object() else {
        return base;
    };
    let mut out = match base {
        serde_json::Value::Object(m) => m,
        other => return other,
    };
    for (k, v) in overlay_map {
        out.insert(k.clone(), v.clone());
    }
    serde_json::Value::Object(out)
}

fn truncate_output(value: serde_json::Value, max: usize) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => {
            if s.len() > max {
                serde_json::Value::String(s.chars().take(max).collect())
            } else {
                serde_json::Value::String(s)
            }
        }
        serde_json::Value::Object(mut map) => {
            if let Some(content) = map.get_mut("content") {
                if let Some(s) = content.as_str() {
                    if s.len() > max {
                        *content = serde_json::Value::String(s.chars().take(max).collect());
                        map.insert("truncated".to_string(), serde_json::Value::Bool(true));
                        return serde_json::Value::Object(map);
                    }
                }
            }
            serde_json::Value::Object(map)
        }
        other => {
            let raw = serde_json::to_string(&other).unwrap_or_default();
            if raw.len() > max {
                serde_json::json!({
                    "content": raw.chars().take(max).collect::<String>(),
                    "truncated": true
                })
            } else {
                other
            }
        }
    }
}
