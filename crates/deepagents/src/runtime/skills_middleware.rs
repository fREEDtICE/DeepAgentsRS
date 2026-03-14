use std::panic::AssertUnwindSafe;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use futures_util::FutureExt;
use tokio::time::{timeout, Duration};

use crate::approval::{redact_command, ApprovalDecision, ApprovalRequest, ExecutionMode};
use crate::audit::{AuditEvent, AuditSink};
use crate::provider::AgentToolCall;
use crate::runtime::{HandledToolCall, RuntimeMiddleware, ToolCallContext};
use crate::skills::loader::{load_skills, SkillsLoadOptions};
use crate::skills::selection::{resolve_skill_snapshot, SkillResolverOptions, SkillSelectionMode};
use crate::skills::validator::{
    classify_package_skill_step_tool, validate_package_skill_input, PackageSkillStepKind,
};
use crate::skills::{
    restore_resolved_snapshot, store_resolved_snapshot, store_skills_diagnostics,
    ResolvedSkillSnapshot, SkillExecutionMode, SkillGovernanceSeverity, SkillPackage,
    SkillSelectedRecord, SkillToolPolicy, SkillToolSpec, SkillsDiagnostics,
};
use crate::state::AgentState;
use crate::types::Message;

/// Runtime middleware that resolves selected skills into a per-run snapshot and
/// intercepts calls to selected skill tools.
pub struct SkillsMiddleware {
    sources: Vec<String>,
    options: SkillsLoadOptions,
    resolver: SkillResolverOptions,
    state: Arc<RwLock<Option<ResolvedSkillSnapshot>>>,
}

impl SkillsMiddleware {
    /// Creates a middleware instance backed by source-based skill loading.
    pub fn new(sources: Vec<String>, options: SkillsLoadOptions) -> Self {
        Self {
            resolver: SkillResolverOptions {
                sources: sources.clone(),
                source_options: options.clone(),
                ..Default::default()
            },
            sources,
            options,
            state: Arc::new(RwLock::new(None)),
        }
    }

    /// Adds a registry root used for versioned lifecycle resolution.
    pub fn with_registry_dir(mut self, registry_dir: impl Into<String>) -> Self {
        self.resolver.registry_dir = Some(registry_dir.into());
        self
    }

    /// Sets explicit pinned skills for manual or auto selection.
    pub fn with_explicit_skills(mut self, explicit_skills: Vec<String>) -> Self {
        self.resolver.explicit_skills = explicit_skills;
        self
    }

    /// Sets explicitly disabled skills for the run.
    pub fn with_disabled_skills(mut self, disabled_skills: Vec<String>) -> Self {
        self.resolver.disabled_skills = disabled_skills;
        self
    }

    /// Sets the selection mode used at run start.
    pub fn with_selection_mode(mut self, selection_mode: SkillSelectionMode) -> Self {
        self.resolver.selection_mode = selection_mode;
        self
    }

    /// Sets the maximum number of active skills for one run.
    pub fn with_max_active(mut self, max_active: usize) -> Self {
        self.resolver.max_active = max_active.max(1);
        self
    }

    /// Forces snapshot recomputation instead of using a sticky thread snapshot.
    pub fn with_refresh_snapshot(mut self, refresh_snapshot: bool) -> Self {
        self.resolver.refresh_snapshot = refresh_snapshot;
        self
    }

    /// Returns the currently resolved snapshot for inspection in tests.
    pub async fn loaded(&self) -> Option<ResolvedSkillSnapshot> {
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
        let snapshot = resolve_skill_snapshot(&messages, state, &self.resolver)?;
        if let Some(snapshot) = snapshot {
            store_resolved_snapshot(state, &snapshot)?;
            let diagnostics = self.collect_diagnostics(&snapshot)?;
            store_skills_diagnostics(state, &diagnostics)?;
            *self.state.write().unwrap() = Some(snapshot.clone());
            upsert_injection_message(&mut messages, snapshot.injection_block);
        } else {
            *self.state.write().unwrap() = None;
        }
        Ok(messages)
    }

    async fn handle_tool_call(
        &self,
        ctx: &mut ToolCallContext<'_>,
    ) -> Result<Option<HandledToolCall>> {
        let snapshot = self
            .state
            .read()
            .unwrap()
            .clone()
            .or_else(|| restore_resolved_snapshot(ctx.state));
        let Some(snapshot) = snapshot else {
            return Ok(None);
        };
        let Some(tool) = snapshot
            .tools
            .iter()
            .find(|tool| tool.name == ctx.tool_call.tool_name)
            .cloned()
        else {
            return Ok(None);
        };

        if tool.requires_isolation {
            let selected = selected_record_for_tool(&snapshot, &tool);
            let package = snapshot
                .packages
                .iter()
                .find(|package| {
                    package.manifest.identity.name == tool.skill_name
                        && package.manifest.identity.version == tool.skill_version
                })
                .cloned();
            let out = execute_subagent_skill(ctx, &tool, selected, package.as_ref()).await;
            return Ok(Some(match out {
                Ok(value) => HandledToolCall {
                    output: value,
                    error: None,
                },
                Err(error) => HandledToolCall {
                    output: serde_json::Value::Null,
                    error: Some(error),
                },
            }));
        }

        if let Err(error) =
            validate_package_skill_input(&tool.input_schema, &ctx.tool_call.arguments)
        {
            return Ok(Some(HandledToolCall {
                output: serde_json::Value::Null,
                error: Some(format!("invalid_request: {}", error)),
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
            Ok(Ok(Err(error))) => Ok(Some(HandledToolCall {
                output: serde_json::Value::Null,
                error: Some(error),
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
) -> std::result::Result<serde_json::Value, String> {
    let mut last_output = serde_json::Value::Null;
    for step in &tool.steps {
        validate_step_access(step, &tool.policy)?;
        if !step.arguments.is_object() {
            return Err(format!(
                "invalid_skill_definition: step {} arguments must be object",
                step.tool_name
            ));
        }
        let args = merge_args(step.arguments.clone(), &ctx.tool_call.arguments);
        let output = if step.tool_name == "execute" {
            execute_with_approval(ctx, &args).await?
        } else {
            ctx.agent
                .call_tool_stateful(&step.tool_name, args, ctx.state)
                .await
                .map(|(out, _delta)| out.output)
                .map_err(|error| format!("skill_step_failed: {}: {}", step.tool_name, error))?
        };
        last_output = truncate_output(output, tool.policy.max_output_chars);
    }
    Ok(last_output)
}

async fn execute_subagent_skill(
    ctx: &mut ToolCallContext<'_>,
    tool: &SkillToolSpec,
    selected: Option<&SkillSelectedRecord>,
    package: Option<&SkillPackage>,
) -> std::result::Result<serde_json::Value, String> {
    let description = build_context_capsule(
        tool,
        selected,
        package,
        &ctx.tool_call.arguments,
        ctx.messages,
    );
    let task_call = AgentToolCall {
        tool_name: "task".to_string(),
        arguments: serde_json::json!({
            "description": description,
            "subagent_type": tool.subagent_type.clone().unwrap_or_else(|| "general-purpose".to_string())
        }),
        call_id: ctx.tool_call.call_id.clone(),
    };

    for middleware in ctx.runtime_middlewares {
        let mut sub_ctx = ToolCallContext {
            agent: ctx.agent,
            tool_call: &task_call,
            call_id: ctx.call_id,
            messages: ctx.messages,
            state: ctx.state,
            root: ctx.root,
            mode: ctx.mode,
            approval: ctx.approval,
            audit: ctx.audit,
            runtime_middlewares: ctx.runtime_middlewares,
            task_depth: ctx.task_depth,
        };
        match middleware.handle_tool_call(&mut sub_ctx).await {
            Ok(Some(handled)) => {
                return match handled.error {
                    Some(error) => Err(error),
                    None => Ok(truncate_output(
                        handled.output,
                        tool.policy.max_output_chars,
                    )),
                };
            }
            Ok(None) => continue,
            Err(error) => return Err(format!("skill_subagent_failed: {}", error)),
        }
    }

    Err("skill_subagent_failed: no runtime middleware handled task".to_string())
}

async fn execute_with_approval(
    ctx: &mut ToolCallContext<'_>,
    args: &serde_json::Value,
) -> std::result::Result<serde_json::Value, String> {
    let cmd = args
        .get("command")
        .and_then(|value| value.as_str())
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
                            _state: ctx.state,
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
                    Err(error) => {
                        record_audit(AuditRecordInput {
                            sink: ctx.audit,
                            _state: ctx.state,
                            cmd: &cmd,
                            root: ctx.root,
                            mode: ctx.mode,
                            decision: "allow",
                            decision_code: "allow",
                            decision_reason: "allowed but execution failed".to_string(),
                            duration_ms,
                            output: None,
                        });
                        Err(format!("skill_step_failed: execute: {}", error))
                    }
                }
            }
            ApprovalDecision::Deny { code, reason } => {
                record_audit(AuditRecordInput {
                    sink: ctx.audit,
                    _state: ctx.state,
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
                    _state: ctx.state,
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
            .map_err(|error| format!("skill_step_failed: execute: {}", error))
    }
}

struct AuditRecordInput<'a> {
    sink: Option<&'a Arc<dyn AuditSink>>,
    _state: &'a AgentState,
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
        _state,
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
        .and_then(|value| value.as_object())
        .map(|object| {
            (
                object
                    .get("exit_code")
                    .and_then(|value| value.as_i64())
                    .map(|value| value as i32),
                object.get("truncated").and_then(|value| value.as_bool()),
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

fn validate_step_access(
    step: &crate::skills::SkillToolStep,
    policy: &SkillToolPolicy,
) -> std::result::Result<(), String> {
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

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "skill step panicked".to_string()
}

fn selected_record_for_tool<'a>(
    snapshot: &'a ResolvedSkillSnapshot,
    tool: &SkillToolSpec,
) -> Option<&'a SkillSelectedRecord> {
    snapshot.selection.selected.iter().find(|record| {
        record.identity.name == tool.skill_name && record.identity.version == tool.skill_version
    })
}

fn build_context_capsule(
    tool: &SkillToolSpec,
    selected: Option<&SkillSelectedRecord>,
    package: Option<&SkillPackage>,
    input: &serde_json::Value,
    messages: &[Message],
) -> String {
    let objective = selected
        .map(|record| record.description.clone())
        .unwrap_or_else(|| tool.description.clone());
    let mut out = String::new();
    out.push_str("Skill objective:\n");
    out.push_str(&objective);
    out.push_str("\n\nTool input:\n");
    out.push_str(&serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string()));
    if let Some(selected) = selected {
        out.push_str("\n\nSelected skill:\n");
        out.push_str(&selected.identity.as_key());
        out.push_str("\nExecution mode:\n");
        out.push_str(match selected.execution_mode {
            SkillExecutionMode::Inline => "inline",
            SkillExecutionMode::Subagent => "subagent",
        });
        out.push_str("\nSelected fragments:\n");
        out.push_str(&selected.fragments.join(", "));
    }
    if let Some(package) = package {
        for fragment in ["role", "constraints", "workflow", "output"] {
            if let Some(content) = package.fragments.get(fragment) {
                out.push_str("\n\n");
                out.push_str(fragment);
                out.push_str(":\n");
                out.push_str(content.trim());
            }
        }
    }
    let recent_messages = messages
        .iter()
        .rev()
        .filter(|message| message.role == "user")
        .take(2)
        .collect::<Vec<_>>();
    if !recent_messages.is_empty() {
        out.push_str("\n\nRecent context:\n");
        for message in recent_messages.into_iter().rev() {
            out.push_str("- ");
            out.push_str(message.content.trim());
            out.push('\n');
        }
    }
    out
}

fn upsert_injection_message(messages: &mut Vec<Message>, block: String) {
    const MARKER_V1: &str = "DEEPAGENTS_SKILLS_INJECTED_V1";
    const MARKER_V2: &str = "DEEPAGENTS_SKILLS_INJECTED_V2";

    messages.retain(|message| {
        !message.content.contains(MARKER_V1) && !message.content.contains(MARKER_V2)
    });
    messages.insert(0, skills_injection_message(block));
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
        serde_json::Value::Object(map) => map,
        other => return other,
    };
    for (key, value) in overlay_map {
        out.insert(key.clone(), value.clone());
    }
    serde_json::Value::Object(out)
}

fn truncate_output(value: serde_json::Value, max: usize) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => {
            if text.chars().count() > max {
                serde_json::Value::String(text.chars().take(max).collect())
            } else {
                serde_json::Value::String(text)
            }
        }
        serde_json::Value::Object(mut object) => {
            if let Some(content) = object.get_mut("content") {
                if let Some(text) = content.as_str() {
                    if text.chars().count() > max {
                        *content = serde_json::Value::String(text.chars().take(max).collect());
                        object.insert("truncated".to_string(), serde_json::Value::Bool(true));
                        return serde_json::Value::Object(object);
                    }
                }
            }
            serde_json::Value::Object(object)
        }
        other => {
            let raw = serde_json::to_string(&other).unwrap_or_default();
            if raw.chars().count() > max {
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

impl SkillsMiddleware {
    fn collect_diagnostics(&self, snapshot: &ResolvedSkillSnapshot) -> Result<SkillsDiagnostics> {
        if !self.sources.is_empty() {
            return Ok(load_skills(&self.sources, self.options.clone())?.diagnostics);
        }
        let mut diagnostics = SkillsDiagnostics::default();
        for package in &snapshot.packages {
            for finding in &package.governance.findings {
                diagnostics
                    .records
                    .push(crate::skills::SkillDiagnosticRecord {
                        name: package.manifest.identity.name.clone(),
                        version: package.manifest.identity.version.clone(),
                        source: package.manifest.source.clone(),
                        severity: match finding.severity {
                            SkillGovernanceSeverity::Warn => "warn".to_string(),
                            SkillGovernanceSeverity::Fail => "fail".to_string(),
                        },
                        code: finding.code.clone(),
                        message: finding.message.clone(),
                    });
            }
        }
        Ok(diagnostics)
    }
}
