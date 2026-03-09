use std::sync::{Arc, RwLock};

use anyhow::Result;
use tokio::time::{timeout, Duration};

use crate::approval::{redact_command, ApprovalDecision, ApprovalRequest, ExecutionMode};
use crate::audit::{AuditEvent, AuditSink};
use crate::runtime::{HandledToolCall, RuntimeMiddleware, ToolCallContext};
use crate::skills::loader::{load_skills, SkillsLoadOptions};
use crate::skills::{LoadedSkills, SkillToolSpec};
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
    async fn before_run(&self, mut messages: Vec<Message>, state: &mut AgentState) -> Result<Vec<Message>> {
        let mut should_load = true;
        if let (Some(meta), Some(tools)) = (
            state.extra.get("skills_metadata"),
            state.extra.get("skills_tools"),
        ) {
            let meta_ok = serde_json::from_value::<Vec<crate::skills::SkillMetadata>>(meta.clone()).is_ok();
            let tools_ok = serde_json::from_value::<Vec<SkillToolSpec>>(tools.clone()).is_ok();
            if meta_ok && tools_ok {
                should_load = false;
            }
        }

        if should_load {
            let loaded = load_skills(&self.sources, self.options.clone())?;
            state
                .extra
                .insert("skills_metadata".to_string(), serde_json::to_value(&loaded.metadata)?);
            state
                .extra
                .insert("skills_tools".to_string(), serde_json::to_value(&loaded.tools)?);
            state
                .extra
                .insert("skills_diagnostics".to_string(), serde_json::to_value(&loaded.diagnostics)?);
            *self.state.write().unwrap() = loaded;
        } else {
            let meta = state
                .extra
                .get("skills_metadata")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let tools = state
                .extra
                .get("skills_tools")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let diagnostics = state
                .extra
                .get("skills_diagnostics")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let loaded = LoadedSkills {
                metadata: serde_json::from_value(meta).unwrap_or_default(),
                tools: serde_json::from_value(tools).unwrap_or_default(),
                diagnostics: serde_json::from_value(diagnostics).unwrap_or_default(),
            };
            *self.state.write().unwrap() = loaded;
        }

        if !has_injection_marker(&messages) {
            let block = build_skills_block(&self.state.read().unwrap());
            messages.insert(
                0,
                Message {
                    role: "system".to_string(),
                    content: block,
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    status: None,
                },
            );
        }

        Ok(messages)
    }

    async fn handle_tool_call(&self, ctx: &mut ToolCallContext<'_>) -> Result<Option<HandledToolCall>> {
        let loaded = self.state.read().unwrap().clone();
        let tool = match loaded.tools.iter().find(|t| t.name == ctx.tool_call.tool_name) {
            Some(t) => t.clone(),
            None => return Ok(None),
        };

        if let Err(e) = validate_input_schema(&tool.input_schema, &ctx.tool_call.arguments) {
            return Ok(Some(HandledToolCall {
                output: serde_json::Value::Null,
                error: Some(format!("invalid_request: {e}")),
            }));
        }

        if tool.steps.len() > tool.policy.max_steps {
            return Ok(Some(HandledToolCall {
                output: serde_json::Value::Null,
                error: Some(format!("skill_steps_exceeded: max={}", tool.policy.max_steps)),
            }));
        }

        let result = timeout(
            Duration::from_millis(tool.policy.timeout_ms),
            execute_skill_steps(ctx, &tool),
        )
        .await;
        match result {
            Ok(Ok(out)) => Ok(Some(HandledToolCall {
                output: out,
                error: None,
            })),
            Ok(Err(e)) => Ok(Some(HandledToolCall {
                output: serde_json::Value::Null,
                error: Some(e),
            })),
            Err(_) => Ok(Some(HandledToolCall {
                output: serde_json::Value::Null,
                error: Some("skill_timeout".to_string()),
            })),
        }
    }
}

async fn execute_skill_steps(ctx: &mut ToolCallContext<'_>, tool: &SkillToolSpec) -> Result<serde_json::Value, String> {
    let mut last_output = serde_json::Value::Null;
    for step in &tool.steps {
        if !is_step_allowed(step, &tool.policy) {
            return Err("permission_denied: tool not allowed".to_string());
        }
        let mut args = step.arguments.clone();
        args = merge_args(args, &ctx.tool_call.arguments);
        let output = if step.tool_name == "execute" {
            execute_with_approval(ctx, &args).await?
        } else {
            ctx.agent
                .call_tool_stateful(&step.tool_name, args, ctx.state)
                .await
                .map(|(out, _delta)| out)
                .map_err(|e| format!("skill_step_failed: {}: {}", step.tool_name, e))?
        };
        last_output = truncate_output(output, tool.policy.max_output_chars);
    }
    Ok(last_output)
}

async fn execute_with_approval(ctx: &mut ToolCallContext<'_>, args: &serde_json::Value) -> Result<serde_json::Value, String> {
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
                        record_audit(ctx.audit, &cmd, ctx.root, ctx.mode, "allow", "allow", reason, duration_ms, Some(&out));
                        Ok(out)
                    }
                    Err(e) => {
                        record_audit(
                            ctx.audit,
                            &cmd,
                            ctx.root,
                            ctx.mode,
                            "allow",
                            "allow",
                            "allowed but execution failed".to_string(),
                            duration_ms,
                            None,
                        );
                        Err(format!("skill_step_failed: execute: {}", e))
                    }
                }
            }
            ApprovalDecision::Deny { code, reason } => {
                record_audit(ctx.audit, &cmd, ctx.root, ctx.mode, "deny", &code, reason.clone(), 0, None);
                Err(format!("command_not_allowed: {}: {}", code, reason))
            }
            ApprovalDecision::RequireApproval { code, reason } => {
                record_audit(
                    ctx.audit,
                    &cmd,
                    ctx.root,
                    ctx.mode,
                    "require_approval",
                    &code,
                    reason.clone(),
                    0,
                    None,
                );
                Err(format!("command_not_allowed: {}: {}", code, reason))
            }
        }
    } else {
        ctx.agent
            .call_tool_stateful("execute", args.clone(), ctx.state)
            .await
            .map(|(out, _delta)| out)
            .map_err(|e| format!("skill_step_failed: execute: {}", e))
    }
}

fn record_audit(
    sink: Option<&Arc<dyn AuditSink>>,
    cmd: &str,
    root: &str,
    mode: ExecutionMode,
    decision: &str,
    decision_code: &str,
    decision_reason: String,
    duration_ms: u64,
    output: Option<&serde_json::Value>,
) {
    let Some(sink) = sink else { return };
    let (exit_code, truncated) = output
        .and_then(|v| v.as_object())
        .map(|o| {
            (
                o.get("exit_code").and_then(|v| v.as_i64()).map(|v| v as i32),
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

fn is_step_allowed(step: &crate::skills::SkillToolStep, policy: &crate::skills::SkillToolPolicy) -> bool {
    if step.tool_name == "execute" {
        return policy.allow_execute;
    }
    if is_filesystem_tool(&step.tool_name) {
        return policy.allow_filesystem;
    }
    true
}

fn is_filesystem_tool(name: &str) -> bool {
    matches!(
        name,
        "ls" | "read_file" | "write_file" | "edit_file" | "delete_file" | "glob" | "grep"
    )
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
        out.push_str(")");
        if !skill.allowed_tools.is_empty() {
            out.push_str(" Allowed tools: ");
            out.push_str(&skill.allowed_tools.join(", "));
        }
        out.push('\n');
    }
    out
}

fn has_injection_marker(messages: &[Message]) -> bool {
    messages.iter().any(|m| m.content.contains("DEEPAGENTS_SKILLS_INJECTED_V1"))
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

fn validate_input_schema(schema: &serde_json::Value, input: &serde_json::Value) -> Result<(), String> {
    let Some(schema_obj) = schema.as_object() else {
        return Err("schema must be object".to_string());
    };
    let typ = schema_obj.get("type").and_then(|v| v.as_str()).unwrap_or("object");
    if typ != "object" {
        return Err("schema type must be object".to_string());
    }
    let input_obj = input
        .as_object()
        .ok_or_else(|| "input must be object".to_string())?;
    if let Some(required) = schema_obj.get("required").and_then(|v| v.as_array()) {
        for r in required {
            let key = r.as_str().ok_or_else(|| "required must be string".to_string())?;
            if !input_obj.contains_key(key) {
                return Err(format!("missing required field: {}", key));
            }
        }
    }
    if let Some(props) = schema_obj.get("properties").and_then(|v| v.as_object()) {
        for (k, prop) in props {
            if let Some(value) = input_obj.get(k) {
                if let Some(t) = prop.get("type").and_then(|v| v.as_str()) {
                    if !matches_type(value, t) {
                        return Err(format!("field {} must be {}", k, t));
                    }
                }
            }
        }
    }
    Ok(())
}

fn matches_type(value: &serde_json::Value, typ: &str) -> bool {
    match typ {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        _ => true,
    }
}
