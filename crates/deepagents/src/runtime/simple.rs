use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::time::{timeout, Duration};

use crate::approval::{redact_command, ApprovalDecision, ApprovalPolicy, ApprovalRequest, ExecutionMode};
use crate::audit::{AuditEvent, AuditSink};
use crate::provider::{Provider, ProviderRequest, ProviderStep, ProviderToolCall};
use crate::runtime::protocol::{
    HandledToolCall, RunOutput, Runtime, RuntimeConfig, RuntimeError, RuntimeMiddleware, ToolCallContext, ToolCallRecord,
    ToolResultRecord, ToolSpec,
};
use crate::runtime::patch_tool_calls::tool_calls_from_provider_calls;
use crate::runtime::tool_compat::{normalize_messages, normalize_tool_call_for_execution, tool_results_from_messages, NormalizedToolCall};
use crate::skills::{SkillCall, SkillPlugin};
use crate::state::AgentState;
use crate::types::Message;
use crate::DeepAgent;

fn mode_str(mode: ExecutionMode) -> String {
    match mode {
        ExecutionMode::NonInteractive => "non_interactive".to_string(),
        ExecutionMode::Interactive => "interactive".to_string(),
    }
}

pub struct SimpleRuntime {
    agent: DeepAgent,
    provider: Arc<dyn Provider>,
    skills: Vec<Arc<dyn SkillPlugin>>,
    config: RuntimeConfig,
    approval: Option<Arc<dyn ApprovalPolicy>>,
    audit: Option<Arc<dyn AuditSink>>,
    root: String,
    mode: ExecutionMode,
    runtime_middlewares: Vec<Arc<dyn RuntimeMiddleware>>,
    initial_state: AgentState,
    task_depth: usize,
}

pub struct SimpleRuntimeOptions {
    pub config: RuntimeConfig,
    pub approval: Option<Arc<dyn ApprovalPolicy>>,
    pub audit: Option<Arc<dyn AuditSink>>,
    pub root: String,
    pub mode: ExecutionMode,
}

impl SimpleRuntime {
    pub fn new(
        agent: DeepAgent,
        provider: Arc<dyn Provider>,
        skills: Vec<Arc<dyn SkillPlugin>>,
        options: SimpleRuntimeOptions,
    ) -> Self {
        let SimpleRuntimeOptions {
            config,
            approval,
            audit,
            root,
            mode,
        } = options;
        Self {
            agent,
            provider,
            skills,
            config,
            approval,
            audit,
            root,
            mode,
            runtime_middlewares: Vec::new(),
            initial_state: AgentState::default(),
            task_depth: 0,
        }
    }

    pub fn with_runtime_middlewares(mut self, middlewares: Vec<Arc<dyn RuntimeMiddleware>>) -> Self {
        self.runtime_middlewares = middlewares;
        self
    }

    pub fn with_initial_state(mut self, state: AgentState) -> Self {
        self.initial_state = state;
        self
    }

    pub fn with_task_depth(mut self, depth: usize) -> Self {
        self.task_depth = depth;
        self
    }
}

#[async_trait]
impl Runtime for SimpleRuntime {
    async fn run(&self, mut messages: Vec<Message>) -> RunOutput {
        messages = normalize_messages(messages);
        let mut state = self.initial_state.clone();
        let mut tool_calls: Vec<ToolCallRecord> = Vec::new();
        let mut tool_results: Vec<ToolResultRecord> = Vec::new();
        let mut next_call_id = 1u64;

        if !self.runtime_middlewares.is_empty() {
            for mw in &self.runtime_middlewares {
                match mw.before_run(messages, &mut state).await {
                    Ok(m) => messages = m,
                    Err(e) => {
                        return RunOutput {
                            final_text: String::new(),
                            tool_calls,
                            tool_results,
                            state: state.clone(),
                            error: Some(RuntimeError {
                                code: "middleware_error".to_string(),
                                message: e.to_string(),
                            }),
                            summarization_events: state.extra.get("_summarization_events").cloned(),
                            trace: Some(serde_json::json!({ "terminated_at_step": 0, "reason": "middleware_before_run_error" })),
                        };
                    }
                }
            }
        }

        tool_results = tool_results_from_messages(&messages);

        for step_idx in 0..self.config.max_steps {
            let tool_specs = self.agent_tools(&state);
            let skill_specs = self
                .skills
                .iter()
                .flat_map(|p| p.list_skills())
                .collect::<Vec<_>>();
            let skill_specs_for_req = skill_specs.clone();

            let mut provider_messages = messages.clone();
            if !self.runtime_middlewares.is_empty() {
                for mw in &self.runtime_middlewares {
                    match mw.before_provider_step(provider_messages, &mut state).await {
                        Ok(m) => provider_messages = m,
                        Err(e) => {
                            return RunOutput {
                                final_text: String::new(),
                                tool_calls,
                                tool_results,
                                state: state.clone(),
                                error: Some(RuntimeError {
                                    code: "middleware_error".to_string(),
                                    message: e.to_string(),
                                }),
                                summarization_events: state.extra.get("_summarization_events").cloned(),
                                trace: Some(serde_json::json!({ "terminated_at_step": step_idx, "reason": "middleware_before_provider_step_error" })),
                            };
                        }
                    }
                }
            }

            let req = ProviderRequest {
                messages: provider_messages.clone(),
                tool_specs,
                skills: skill_specs_for_req,
                state: state.clone(),
                last_tool_results: tool_results.clone(),
            };

            let mut provider_step = match timeout(
                Duration::from_millis(self.config.provider_timeout_ms),
                self.provider.step(req),
            )
            .await
            {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    return RunOutput {
                        final_text: String::new(),
                        tool_calls,
                        tool_results,
                        state: state.clone(),
                        error: Some(RuntimeError {
                            code: "provider_error".to_string(),
                            message: e.to_string(),
                        }),
                        summarization_events: state.extra.get("_summarization_events").cloned(),
                        trace: Some(serde_json::json!({ "terminated_at_step": step_idx, "reason": "provider_error" })),
                    };
                }
                Err(_) => {
                    return RunOutput {
                        final_text: String::new(),
                        tool_calls,
                        tool_results,
                        state: state.clone(),
                        error: Some(RuntimeError {
                            code: "provider_timeout".to_string(),
                            message: "provider timed out".to_string(),
                        }),
                        summarization_events: state.extra.get("_summarization_events").cloned(),
                        trace: Some(serde_json::json!({ "terminated_at_step": step_idx, "reason": "provider_timeout" })),
                    };
                }
            };

            if let ProviderStep::Error { error } = &provider_step {
                if error.code == "context_overflow" {
                    state
                        .extra
                        .insert("_summarization_force".to_string(), serde_json::Value::Bool(true));
                    let mut overflow_messages = messages.clone();
                    if !self.runtime_middlewares.is_empty() {
                        for mw in &self.runtime_middlewares {
                            match mw.before_provider_step(overflow_messages, &mut state).await {
                                Ok(m) => overflow_messages = m,
                                Err(e) => {
                                    return RunOutput {
                                        final_text: String::new(),
                                        tool_calls,
                                        tool_results,
                                        state: state.clone(),
                                        error: Some(RuntimeError {
                                            code: "middleware_error".to_string(),
                                            message: e.to_string(),
                                        }),
                                        summarization_events: state.extra.get("_summarization_events").cloned(),
                                        trace: Some(serde_json::json!({ "terminated_at_step": step_idx, "reason": "middleware_before_provider_step_error" })),
                                    };
                                }
                            }
                        }
                    }
                    state.extra.remove("_summarization_force");
                    let retry_req = ProviderRequest {
                        messages: overflow_messages,
                        tool_specs: self.agent_tools(&state),
                        skills: skill_specs.clone(),
                        state: state.clone(),
                        last_tool_results: tool_results.clone(),
                    };
                    provider_step = match timeout(
                        Duration::from_millis(self.config.provider_timeout_ms),
                        self.provider.step(retry_req),
                    )
                    .await
                    {
                        Ok(Ok(s)) => s,
                        Ok(Err(e)) => {
                            return RunOutput {
                                final_text: String::new(),
                                tool_calls,
                                tool_results,
                                state: state.clone(),
                                error: Some(RuntimeError {
                                    code: "provider_error".to_string(),
                                    message: e.to_string(),
                                }),
                                summarization_events: state.extra.get("_summarization_events").cloned(),
                                trace: Some(serde_json::json!({ "terminated_at_step": step_idx, "reason": "provider_error" })),
                            };
                        }
                        Err(_) => {
                            return RunOutput {
                                final_text: String::new(),
                                tool_calls,
                                tool_results,
                                state: state.clone(),
                                error: Some(RuntimeError {
                                    code: "provider_timeout".to_string(),
                                    message: "provider timed out".to_string(),
                                }),
                                summarization_events: state.extra.get("_summarization_events").cloned(),
                                trace: Some(serde_json::json!({ "terminated_at_step": step_idx, "reason": "provider_timeout" })),
                            };
                        }
                    };
                }
            }

            if !self.runtime_middlewares.is_empty() {
                for mw in &self.runtime_middlewares {
                    match mw.patch_provider_step(provider_step, &mut next_call_id).await {
                        Ok(s) => provider_step = s,
                        Err(e) => {
                            return RunOutput {
                                final_text: String::new(),
                                tool_calls,
                                tool_results,
                                state: state.clone(),
                                error: Some(RuntimeError {
                                    code: "middleware_error".to_string(),
                                    message: e.to_string(),
                                }),
                                summarization_events: state.extra.get("_summarization_events").cloned(),
                                trace: Some(serde_json::json!({ "terminated_at_step": step_idx, "reason": "middleware_patch_provider_step_error" })),
                            };
                        }
                    }
                }
            }

            match provider_step {
                ProviderStep::AssistantMessage { text } => {
                    messages.push(Message {
                        role: "assistant".to_string(),
                        content: text,
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                        status: None,
                    });
                }
                ProviderStep::FinalText { text } => {
                    return RunOutput {
                        final_text: text,
                        tool_calls,
                        tool_results,
                        state: state.clone(),
                        error: None,
                        summarization_events: state.extra.get("_summarization_events").cloned(),
                        trace: Some(serde_json::json!({ "terminated_at_step": step_idx, "reason": "final_text" })),
                    };
                }
                ProviderStep::Error { error } => {
                    return RunOutput {
                        final_text: String::new(),
                        tool_calls,
                        tool_results,
                        state: state.clone(),
                        error: Some(RuntimeError {
                            code: error.code,
                            message: error.message,
                        }),
                        summarization_events: state.extra.get("_summarization_events").cloned(),
                        trace: Some(serde_json::json!({ "terminated_at_step": step_idx, "reason": "provider_step_error" })),
                    };
                }
                ProviderStep::SkillCall { name, input, call_id } => {
                    let call = SkillCall { name, input, call_id };
                    let calls = match self.expand_skill(call).await {
                        Ok(c) => c,
                        Err(e) => {
                            return RunOutput {
                                final_text: String::new(),
                                tool_calls,
                                tool_results,
                                state: state.clone(),
                                error: Some(RuntimeError {
                                    code: e.code,
                                    message: e.message,
                                }),
                                summarization_events: state.extra.get("_summarization_events").cloned(),
                                trace: Some(serde_json::json!({ "terminated_at_step": step_idx, "reason": "skill_error" })),
                            };
                        }
                    };
                    self.execute_calls(
                        calls,
                        &mut messages,
                        &mut state,
                        &mut tool_calls,
                        &mut tool_results,
                        &mut next_call_id,
                    )
                    .await;
                }
                ProviderStep::ToolCalls { calls } => {
                    messages.push(Message {
                        role: "assistant".to_string(),
                        content: String::new(),
                        tool_calls: Some(tool_calls_from_provider_calls(&calls)),
                        tool_call_id: None,
                        name: None,
                        status: None,
                    });
                    self.execute_calls(
                        calls,
                        &mut messages,
                        &mut state,
                        &mut tool_calls,
                        &mut tool_results,
                        &mut next_call_id,
                    )
                    .await;
                }
            }
        }

        RunOutput {
            final_text: String::new(),
            tool_calls,
            tool_results,
            state: state.clone(),
            error: Some(RuntimeError {
                code: "max_steps_exceeded".to_string(),
                message: "runtime exceeded max_steps".to_string(),
            }),
            summarization_events: state.extra.get("_summarization_events").cloned(),
            trace: Some(serde_json::json!({ "terminated_at_step": self.config.max_steps, "reason": "max_steps_exceeded" })),
        }
    }
}

impl SimpleRuntime {
    fn agent_tools(&self, state: &AgentState) -> Vec<ToolSpec> {
        let mut out = Vec::new();
        for (name, desc) in [
            ("ls", "Lists files and directories in a given path."),
            ("read_file", "Reads a file from the local filesystem and returns output."),
            ("write_file", "Writes a new file to the filesystem."),
            ("edit_file", "Edits an existing file by replacing a literal string."),
            ("delete_file", "Deletes a file from the filesystem."),
            ("glob", "Glob match file paths."),
            ("grep", "Search for a literal text pattern across files."),
            ("execute", "Executes a shell command in an isolated sandbox environment."),
            ("task", "Launch a sub-agent and assign a task to it."),
            ("compact_conversation", "Compacts conversation history by summarizing older messages."),
        ] {
            out.push(ToolSpec {
                name: name.to_string(),
                description: desc.to_string(),
            });
        }
        if let Some(v) = state.extra.get("skills_tools") {
            if let Ok(skills) = serde_json::from_value::<Vec<crate::skills::SkillToolSpec>>(v.clone()) {
                for s in skills {
                    out.push(ToolSpec {
                        name: s.name,
                        description: s.description,
                    });
                }
            }
        }
        out
    }

    async fn expand_skill(&self, call: SkillCall) -> Result<Vec<ProviderToolCall>, crate::skills::SkillError> {
        for p in &self.skills {
            let names: Vec<String> = p.list_skills().into_iter().map(|s| s.name).collect();
            if names.iter().any(|n| n == &call.name) {
                return p.call(call).await;
            }
        }
        Err(crate::skills::SkillError {
            code: "skill_not_found".to_string(),
            message: format!("skill not found: {}", call.name),
        })
    }

    async fn execute_calls(
        &self,
        calls: Vec<ProviderToolCall>,
        messages: &mut Vec<Message>,
        state: &mut AgentState,
        tool_calls: &mut Vec<ToolCallRecord>,
        tool_results: &mut Vec<ToolResultRecord>,
        next_call_id: &mut u64,
    ) {
        for call in calls {
            let normalized = normalize_tool_call_for_execution(call, next_call_id);
            let (call, error) = match normalized {
                NormalizedToolCall::Valid(c) => (c, None),
                NormalizedToolCall::Invalid { call, error } => (call, Some(error)),
            };

            let call_id = call.call_id.clone().unwrap_or_default();
            let tool_name = call.tool_name.clone();

            if let Some(err) = error {
                tool_calls.push(ToolCallRecord {
                    tool_name: tool_name.clone(),
                    arguments: call.arguments.clone(),
                    call_id: Some(call_id.clone()),
                });
                tool_results.push(ToolResultRecord {
                    tool_name: tool_name.clone(),
                    call_id: Some(call_id.clone()),
                    output: serde_json::Value::Null,
                    error: Some(err.clone()),
                    status: Some("error".to_string()),
                });
                messages.push(Message {
                    role: "tool".to_string(),
                    content: serde_json::to_string(&serde_json::json!({
                        "tool_call_id": call_id.clone(),
                        "tool_name": tool_name.clone(),
                        "status": "error",
                        "error": err.clone(),
                        "content": err,
                    }))
                    .unwrap_or_default(),
                    tool_calls: None,
                    tool_call_id: Some(call_id.clone()),
                    name: Some(tool_name),
                    status: Some("error".to_string()),
                });
                continue;
            }

            tool_calls.push(ToolCallRecord {
                tool_name: tool_name.clone(),
                arguments: call.arguments.clone(),
                call_id: Some(call_id.clone()),
            });

            if !self.runtime_middlewares.is_empty() {
                let mut ctx = ToolCallContext {
                    agent: &self.agent,
                    tool_call: &call,
                    call_id: &call_id,
                    messages,
                    state,
                    root: &self.root,
                    mode: self.mode,
                    approval: self.approval.as_ref(),
                    audit: self.audit.as_ref(),
                    runtime_middlewares: &self.runtime_middlewares,
                    task_depth: self.task_depth,
                };
                let mut handled: Option<HandledToolCall> = None;
                for mw in &self.runtime_middlewares {
                    match mw.handle_tool_call(&mut ctx).await {
                        Ok(Some(h)) => {
                            handled = Some(h);
                            break;
                        }
                        Ok(None) => {}
                        Err(e) => {
                            handled = Some(HandledToolCall {
                                output: serde_json::Value::Null,
                                error: Some(format!("middleware_error: {e}")),
                            });
                            break;
                        }
                    }
                }
                if let Some(HandledToolCall { output, error }) = handled {
                    let status = if error.is_some() { "error" } else { "success" }.to_string();
                    let tool_name = tool_name.clone();
                    let cid = call_id.clone();
                    let content = if let Some(e) = &error {
                        e.clone()
                    } else if let Some(s) = output.get("content").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
                        s.to_string()
                    } else {
                        serde_json::to_string(&output).unwrap_or_default()
                    };
                    tool_results.push(ToolResultRecord {
                        tool_name: tool_name.clone(),
                        call_id: Some(cid.clone()),
                        output: output.clone(),
                        error: error.clone(),
                        status: Some(status.clone()),
                    });
                    messages.push(Message {
                        role: "tool".to_string(),
                        content: serde_json::to_string(&serde_json::json!({
                            "tool_call_id": cid.clone(),
                            "tool_name": tool_name.clone(),
                            "status": status.clone(),
                            "output": if error.is_some() { serde_json::Value::Null } else { output },
                            "error": error,
                            "content": content,
                        }))
                        .unwrap_or_default(),
                        tool_calls: None,
                        tool_call_id: Some(call_id.clone()),
                        name: Some(tool_name.clone()),
                        status: Some(status.clone()),
                    });
                    continue;
                }
            }

            if call.tool_name == "execute" {
                if let Some(policy) = &self.approval {
                    let cmd = call
                        .arguments
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let req = ApprovalRequest {
                        command: cmd.clone(),
                        root: self.root.clone(),
                        mode: self.mode,
                    };
                    let decision = policy.decide(&req);
                    match decision {
                        ApprovalDecision::Allow { reason } => {
                            let started = std::time::Instant::now();
                            let result = self
                                .agent
                                .call_tool_stateful(&call.tool_name, call.arguments.clone(), state)
                                .await;

                            let duration_ms = started.elapsed().as_millis() as u64;
                            match result {
                                Ok((out, _delta)) => {
                                    if let Some(sink) = &self.audit {
                                        let _ = sink.record(AuditEvent {
                                            timestamp_ms: Utc::now().timestamp_millis(),
                                            root: self.root.clone(),
                                            mode: mode_str(self.mode),
                                            command_redacted: redact_command(&cmd),
                                            decision: "allow".to_string(),
                                            decision_code: "allow".to_string(),
                                            decision_reason: reason,
                                            exit_code: out.get("exit_code").and_then(|v| v.as_i64()).map(|v| v as i32),
                                            truncated: out.get("truncated").and_then(|v| v.as_bool()),
                                            duration_ms: Some(duration_ms),
                                        });
                                    }
                                    let content = if let Some(s) = out.get("content").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
                                        s.to_string()
                                    } else {
                                        serde_json::to_string(&out).unwrap_or_default()
                                    };
                                    tool_results.push(ToolResultRecord {
                                        tool_name: tool_name.clone(),
                                        call_id: Some(call_id.clone()),
                                        output: out.clone(),
                                        error: None,
                                        status: Some("success".to_string()),
                                    });
                                    messages.push(Message {
                                        role: "tool".to_string(),
                                        content: serde_json::to_string(&serde_json::json!({
                                            "tool_call_id": call_id.clone(),
                                            "tool_name": tool_name.clone(),
                                            "status": "success",
                                            "output": out,
                                            "content": content,
                                        }))
                                        .unwrap_or_default(),
                                        tool_calls: None,
                                        tool_call_id: Some(call_id.clone()),
                                        name: Some(tool_name.clone()),
                                        status: Some("success".to_string()),
                                    });
                                }
                                Err(e) => {
                                    if let Some(sink) = &self.audit {
                                        let _ = sink.record(AuditEvent {
                                            timestamp_ms: Utc::now().timestamp_millis(),
                                            root: self.root.clone(),
                                            mode: mode_str(self.mode),
                                            command_redacted: redact_command(&cmd),
                                            decision: "allow".to_string(),
                                            decision_code: "allow".to_string(),
                                            decision_reason: "allowed but execution failed".to_string(),
                                            exit_code: None,
                                            truncated: None,
                                            duration_ms: Some(duration_ms),
                                        });
                                    }
                                    let err = e.to_string();
                                    tool_results.push(ToolResultRecord {
                                        tool_name: tool_name.clone(),
                                        call_id: Some(call_id.clone()),
                                        output: serde_json::Value::Null,
                                        error: Some(err.clone()),
                                        status: Some("error".to_string()),
                                    });
                                    messages.push(Message {
                                        role: "tool".to_string(),
                                        content: serde_json::to_string(&serde_json::json!({
                                            "tool_call_id": call_id.clone(),
                                            "tool_name": tool_name.clone(),
                                            "status": "error",
                                            "error": err.clone(),
                                            "content": err,
                                        }))
                                        .unwrap_or_default(),
                                        tool_calls: None,
                                        tool_call_id: Some(call_id.clone()),
                                        name: Some(tool_name.clone()),
                                        status: Some("error".to_string()),
                                    });
                                }
                            }
                            continue;
                        }
                        ApprovalDecision::Deny { code, reason } => {
                            if let Some(sink) = &self.audit {
                                let _ = sink.record(AuditEvent {
                                    timestamp_ms: Utc::now().timestamp_millis(),
                                    root: self.root.clone(),
                                    mode: mode_str(self.mode),
                                    command_redacted: redact_command(&cmd),
                                    decision: "deny".to_string(),
                                    decision_code: code.clone(),
                                    decision_reason: reason.clone(),
                                    exit_code: None,
                                    truncated: None,
                                    duration_ms: None,
                                });
                            }
                            let err = format!("command_not_allowed: {}: {}", code, reason);
                            tool_results.push(ToolResultRecord {
                                tool_name: tool_name.clone(),
                                call_id: Some(call_id.clone()),
                                output: serde_json::Value::Null,
                                error: Some(err.clone()),
                                status: Some("error".to_string()),
                            });
                            messages.push(Message {
                                role: "tool".to_string(),
                                content: serde_json::to_string(&serde_json::json!({
                                    "tool_call_id": call_id.clone(),
                                    "tool_name": tool_name.clone(),
                                    "status": "error",
                                    "error": err.clone(),
                                    "content": err,
                                }))
                                .unwrap_or_default(),
                                tool_calls: None,
                                tool_call_id: Some(call_id.clone()),
                                name: Some(tool_name.clone()),
                                status: Some("error".to_string()),
                            });
                            continue;
                        }
                        ApprovalDecision::RequireApproval { code, reason } => {
                            if let Some(sink) = &self.audit {
                                let _ = sink.record(AuditEvent {
                                    timestamp_ms: Utc::now().timestamp_millis(),
                                    root: self.root.clone(),
                                    mode: mode_str(self.mode),
                                    command_redacted: redact_command(&cmd),
                                    decision: "require_approval".to_string(),
                                    decision_code: code.clone(),
                                    decision_reason: reason.clone(),
                                    exit_code: None,
                                    truncated: None,
                                    duration_ms: None,
                                });
                            }
                            let err = format!("command_not_allowed: {}: {}", code, reason);
                            tool_results.push(ToolResultRecord {
                                tool_name: tool_name.clone(),
                                call_id: Some(call_id.clone()),
                                output: serde_json::Value::Null,
                                error: Some(err.clone()),
                                status: Some("error".to_string()),
                            });
                            messages.push(Message {
                                role: "tool".to_string(),
                                content: serde_json::to_string(&serde_json::json!({
                                    "tool_call_id": call_id.clone(),
                                    "tool_name": tool_name.clone(),
                                    "status": "error",
                                    "error": err.clone(),
                                    "content": err,
                                }))
                                .unwrap_or_default(),
                                tool_calls: None,
                                tool_call_id: Some(call_id.clone()),
                                name: Some(tool_name.clone()),
                                status: Some("error".to_string()),
                            });
                            continue;
                        }
                    }
                }
            }

            let result = self
                .agent
                .call_tool_stateful(&call.tool_name, call.arguments.clone(), state)
                .await;

            match result {
                Ok((out, _delta)) => {
                    let content = if let Some(s) = out.get("content").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
                        s.to_string()
                    } else {
                        serde_json::to_string(&out).unwrap_or_default()
                    };
                    tool_results.push(ToolResultRecord {
                        tool_name: tool_name.clone(),
                        call_id: Some(call_id.clone()),
                        output: out.clone(),
                        error: None,
                        status: Some("success".to_string()),
                    });
                    messages.push(Message {
                        role: "tool".to_string(),
                        content: serde_json::to_string(&serde_json::json!({
                            "tool_call_id": call_id.clone(),
                            "tool_name": tool_name.clone(),
                            "status": "success",
                            "output": out,
                            "content": content,
                        }))
                        .unwrap_or_default(),
                        tool_calls: None,
                        tool_call_id: Some(call_id.clone()),
                        name: Some(tool_name.clone()),
                        status: Some("success".to_string()),
                    });
                }
                Err(e) => {
                    let err = e.to_string();
                    tool_results.push(ToolResultRecord {
                        tool_name: tool_name.clone(),
                        call_id: Some(call_id.clone()),
                        output: serde_json::Value::Null,
                        error: Some(err.clone()),
                        status: Some("error".to_string()),
                    });
                    messages.push(Message {
                        role: "tool".to_string(),
                        content: serde_json::to_string(&serde_json::json!({
                            "tool_call_id": call_id.clone(),
                            "tool_name": tool_name.clone(),
                            "status": "error",
                            "error": err.clone(),
                            "content": err,
                        }))
                        .unwrap_or_default(),
                        tool_calls: None,
                        tool_call_id: Some(call_id.clone()),
                        name: Some(tool_name.clone()),
                        status: Some("error".to_string()),
                    });
                }
            }
        }
    }
}
