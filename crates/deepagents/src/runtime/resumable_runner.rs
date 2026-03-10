use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::Utc;

use crate::approval::{
    redact_command, ApprovalDecision, ApprovalPolicy, ApprovalRequest, ExecutionMode,
};
use crate::audit::{AuditEvent, AuditSink};
use crate::provider::{Provider, ProviderRequest, ProviderStep, ProviderToolCall};
use crate::runtime::attach_provider_cache_events_to_trace;
use crate::runtime::events::{
    diff_state_keys, preview_json, preview_text, provider_step_kind, summarize_messages, RunEvent,
    RunEventSink,
};
use crate::runtime::filesystem_runtime_middleware::{
    LargeToolResultOffloadOptions, LARGE_TOOL_RESULT_OFFLOAD_OPTIONS_KEY,
};
use crate::runtime::patch_tool_calls::{sanitize_tool_call_id, tool_calls_from_provider_calls};
use crate::runtime::prompt_cache_runtime::{step_with_prompt_cache_and_events, CachedProviderError};
use crate::runtime::protocol::{
    HandledToolCall, HitlDecision, HitlInterrupt, RunOutput, RunStatus, RuntimeError,
    RuntimeMiddleware, ToolCallContext, ToolCallRecord, ToolResultRecord,
};
use crate::runtime::tool_compat::{
    normalize_tool_call_for_execution, tool_results_from_messages, NormalizedToolCall,
};
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

fn load_large_tool_result_offload_options(
    state: &AgentState,
) -> Option<LargeToolResultOffloadOptions> {
    let v = state.extra.get(LARGE_TOOL_RESULT_OFFLOAD_OPTIONS_KEY)?;
    serde_json::from_value(v.clone()).ok()
}

fn preview_head_tail_lines(text: &str, max_lines: usize) -> (String, String) {
    let max_lines = max_lines.max(1);
    let mut head: Vec<&str> = Vec::new();
    let mut tail: std::collections::VecDeque<&str> =
        std::collections::VecDeque::with_capacity(max_lines);
    for line in text.lines() {
        if head.len() < max_lines {
            head.push(line);
        }
        if tail.len() == max_lines {
            tail.pop_front();
        }
        tail.push_back(line);
    }
    (
        head.join("\n"),
        tail.into_iter().collect::<Vec<_>>().join("\n"),
    )
}

fn finalize_run_output(mut out: RunOutput) -> RunOutput {
    out.trace = attach_provider_cache_events_to_trace(out.trace, &mut out.state);
    out.summarization_events = out.state.extra.get("_summarization_events").cloned();
    out
}

#[derive(Clone)]
pub struct ResumableRunnerOptions {
    pub config: crate::runtime::RuntimeConfig,
    pub approval: Option<Arc<dyn ApprovalPolicy>>,
    pub audit: Option<Arc<dyn AuditSink>>,
    pub root: String,
    pub mode: ExecutionMode,
    pub interrupt_on: BTreeMap<String, bool>,
}

#[derive(Clone)]
struct PendingInterrupt {
    interrupt: HitlInterrupt,
    call: ProviderToolCall,
    remaining_calls: Vec<ProviderToolCall>,
}

pub struct ResumableRunner {
    agent: DeepAgent,
    provider: Arc<dyn Provider>,
    skills: Vec<Arc<dyn SkillPlugin>>,
    config: crate::runtime::RuntimeConfig,
    approval: Option<Arc<dyn ApprovalPolicy>>,
    audit: Option<Arc<dyn AuditSink>>,
    root: String,
    mode: ExecutionMode,
    interrupt_on: BTreeMap<String, bool>,
    runtime_middlewares: Vec<Arc<dyn RuntimeMiddleware>>,
    initialized: bool,
    messages: Vec<Message>,
    state: AgentState,
    tool_calls: Vec<ToolCallRecord>,
    tool_results: Vec<ToolResultRecord>,
    next_call_id: u64,
    step_counter: usize,
    pending: Option<PendingInterrupt>,
    task_depth: usize,
}

impl ResumableRunner {
    pub fn new(
        agent: DeepAgent,
        provider: Arc<dyn Provider>,
        skills: Vec<Arc<dyn SkillPlugin>>,
        options: ResumableRunnerOptions,
    ) -> Self {
        Self {
            agent,
            provider,
            skills,
            config: options.config,
            approval: options.approval,
            audit: options.audit,
            root: options.root,
            mode: options.mode,
            interrupt_on: options.interrupt_on,
            runtime_middlewares: Vec::new(),
            initialized: false,
            messages: Vec::new(),
            state: AgentState::default(),
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            next_call_id: 1,
            step_counter: 0,
            pending: None,
            task_depth: 0,
        }
    }

    pub fn with_runtime_middlewares(
        mut self,
        middlewares: Vec<Arc<dyn RuntimeMiddleware>>,
    ) -> Self {
        self.runtime_middlewares = middlewares;
        self
    }

    pub fn with_initial_state(mut self, state: AgentState) -> Self {
        self.state = state;
        self
    }

    pub fn with_initial_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }

    pub fn with_task_depth(mut self, depth: usize) -> Self {
        self.task_depth = depth;
        self
    }

    pub fn pending_interrupt(&self) -> Option<&HitlInterrupt> {
        self.pending.as_ref().map(|p| &p.interrupt)
    }

    pub fn state(&self) -> &AgentState {
        &self.state
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn tool_calls(&self) -> &[ToolCallRecord] {
        &self.tool_calls
    }

    pub fn tool_results(&self) -> &[ToolResultRecord] {
        &self.tool_results
    }

    pub fn push_user_input(&mut self, input: String) {
        self.messages.push(Message {
            role: "user".to_string(),
            content: input,
            content_blocks: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        });
    }

    async fn emit_event(&self, sink: &mut dyn RunEventSink, event: RunEvent) {
        let _ = sink.emit(event).await;
    }

    async fn emit_run_finished(
        &self,
        sink: &mut dyn RunEventSink,
        status: RunStatus,
        reason: &str,
        final_text: String,
        step_count: usize,
    ) {
        self.emit_event(
            sink,
            RunEvent::RunFinished {
                status,
                reason: reason.to_string(),
                final_text,
                step_count,
                tool_call_count: self.tool_calls.len(),
                tool_error_count: self
                    .tool_results
                    .iter()
                    .filter(|r| r.error.is_some() || r.status.as_deref() == Some("error"))
                    .count(),
            },
        )
        .await;
    }

    async fn finish_with_output(
        &self,
        sink: &mut dyn RunEventSink,
        out: RunOutput,
        reason: &str,
        step_count: usize,
    ) -> RunOutput {
        self.emit_run_finished(sink, out.status, reason, out.final_text.clone(), step_count)
            .await;
        out
    }

    async fn emit_state_updated_if_any(
        &self,
        step_index: usize,
        before: &AgentState,
        sink: &mut dyn RunEventSink,
    ) {
        let updated_keys = diff_state_keys(before, &self.state);
        if !updated_keys.is_empty() {
            self.emit_event(
                sink,
                RunEvent::StateUpdated {
                    step_index,
                    updated_keys,
                },
            )
            .await;
        }
    }

    async fn emit_provider_events(
        &self,
        step_index: usize,
        events: &[crate::provider::ProviderEvent],
        sink: &mut dyn RunEventSink,
    ) {
        for event in events {
            match event {
                crate::provider::ProviderEvent::AssistantTextDelta { text } => {
                    self.emit_event(
                        sink,
                        RunEvent::AssistantTextDelta {
                            step_index,
                            text: text.clone(),
                        },
                    )
                    .await;
                }
                crate::provider::ProviderEvent::ToolCallArgsDelta { tool_call_id, delta } => {
                    self.emit_event(
                        sink,
                        RunEvent::ToolCallArgsDelta {
                            step_index,
                            tool_call_id: tool_call_id.clone(),
                            delta: delta.clone(),
                        },
                    )
                    .await;
                }
                crate::provider::ProviderEvent::Usage {
                    input_tokens,
                    output_tokens,
                    total_tokens,
                } => {
                    self.emit_event(
                        sink,
                        RunEvent::UsageReported {
                            step_index,
                            input_tokens: *input_tokens,
                            output_tokens: *output_tokens,
                            total_tokens: *total_tokens,
                        },
                    )
                    .await;
                }
            }
        }
    }

    async fn push_tool_result_and_message(
        &mut self,
        step_index: usize,
        before_state: &AgentState,
        tool_name: String,
        call_id: String,
        output: serde_json::Value,
        error: Option<String>,
        status: String,
        content: String,
        content_blocks: Option<Vec<crate::types::ContentBlock>>,
        sink: &mut dyn RunEventSink,
        custom_message: Option<serde_json::Value>,
    ) {
        self.tool_results.push(ToolResultRecord {
            tool_name: tool_name.clone(),
            call_id: Some(call_id.clone()),
            output: output.clone(),
            error: error.clone(),
            status: Some(status.clone()),
        });
        self.emit_event(
            sink,
            RunEvent::ToolCallFinished {
                step_index,
                tool_name: tool_name.clone(),
                tool_call_id: call_id.clone(),
                output_preview: preview_json(&output),
                error: error.clone(),
                status: Some(status.clone()),
            },
        )
        .await;

        let content_json = custom_message.unwrap_or_else(|| {
            serde_json::json!({
                "tool_call_id": call_id.clone(),
                "tool_name": tool_name.clone(),
                "status": status.clone(),
                "output": if error.is_some() { serde_json::Value::Null } else { output },
                "error": error.clone(),
                "content": content.clone(),
            })
        });
        self.messages.push(Message {
            role: "tool".to_string(),
            content: content_json.to_string(),
            content_blocks,
            tool_calls: None,
            tool_call_id: Some(call_id.clone()),
            name: Some(tool_name),
            status: Some(status.clone()),
        });
        self.emit_event(
            sink,
            RunEvent::ToolMessageAppended {
                step_index,
                tool_call_id: call_id,
                content_preview: preview_text(&content, 240),
                status: Some(status),
            },
        )
        .await;
        self.emit_state_updated_if_any(step_index, before_state, sink)
            .await;
    }

    pub async fn run(&mut self) -> RunOutput {
        let mut sink = crate::runtime::NoopRunEventSink;
        self.run_with_events(&mut sink).await
    }

    pub async fn run_with_events(&mut self, sink: &mut dyn RunEventSink) -> RunOutput {
        if self.pending.is_some() {
            if let Some(interrupt) = self.pending_interrupt().cloned() {
                self.emit_event(
                    sink,
                    RunEvent::Interrupt {
                        step_index: self.step_counter,
                        interrupt,
                    },
                )
                .await;
            }
            let out = self.pending_output();
            self.emit_run_finished(
                sink,
                out.status,
                "interrupt_pending",
                out.final_text.clone(),
                self.step_counter,
            )
            .await;
            return out;
        }

        self.emit_event(
            sink,
            RunEvent::RunStarted {
                resumed_from_interrupt: false,
            },
        )
        .await;

        if !self.initialized {
            self.messages =
                crate::runtime::tool_compat::normalize_messages(std::mem::take(&mut self.messages));
            if !self.runtime_middlewares.is_empty() {
                let mut messages = std::mem::take(&mut self.messages);
                for mw in &self.runtime_middlewares {
                    match mw.before_run(messages, &mut self.state).await {
                        Ok(m) => messages = m,
                        Err(e) => {
                            self.messages = Vec::new();
                            let out = finalize_run_output(RunOutput {
                                status: RunStatus::Error,
                                interrupts: Vec::new(),
                                final_text: String::new(),
                                tool_calls: self.tool_calls.clone(),
                                tool_results: self.tool_results.clone(),
                                state: self.state.clone(),
                                error: Some(RuntimeError {
                                    code: "middleware_error".to_string(),
                                    message: e.to_string(),
                                }),
                                summarization_events: self
                                    .state
                                    .extra
                                    .get("_summarization_events")
                                    .cloned(),
                                trace: Some(serde_json::json!({
                                    "terminated_at_step": 0,
                                    "reason": "middleware_before_run_error"
                                })),
                            });
                            return self
                                .finish_with_output(sink, out, "middleware_before_run_error", 0)
                                .await;
                        }
                    }
                }
                self.messages = messages;
            }
            self.tool_results = tool_results_from_messages(&self.messages);
            self.initialized = true;
        }

        for step_idx in 0..self.config.max_steps {
            let event_step_idx = self.step_counter;
            self.step_counter = self.step_counter.saturating_add(1);
            let tool_specs = self.agent_tools(&self.state);
            let skill_specs = self
                .skills
                .iter()
                .flat_map(|p| p.list_skills())
                .collect::<Vec<_>>();
            let skill_specs_for_req = skill_specs.clone();

            let mut provider_messages = self.messages.clone();
            if !self.runtime_middlewares.is_empty() {
                for mw in &self.runtime_middlewares {
                    match mw
                        .before_provider_step(provider_messages, &mut self.state)
                        .await
                    {
                        Ok(m) => provider_messages = m,
                        Err(e) => {
                            let out = finalize_run_output(RunOutput {
                                status: RunStatus::Error,
                                interrupts: Vec::new(),
                                final_text: String::new(),
                                tool_calls: self.tool_calls.clone(),
                                tool_results: self.tool_results.clone(),
                                state: self.state.clone(),
                                error: Some(RuntimeError {
                                    code: "middleware_error".to_string(),
                                    message: e.to_string(),
                                }),
                                summarization_events: self
                                    .state
                                    .extra
                                    .get("_summarization_events")
                                    .cloned(),
                                trace: Some(serde_json::json!({
                                    "terminated_at_step": step_idx,
                                    "reason": "middleware_before_provider_step_error"
                                })),
                            });
                            return self
                                .finish_with_output(
                                    sink,
                                    out,
                                    "middleware_before_provider_step_error",
                                    event_step_idx + 1,
                                )
                                .await;
                        }
                    }
                }
            }

            self.emit_event(
                sink,
                RunEvent::ModelRequestBuilt {
                    step_index: event_step_idx,
                    tool_names: tool_specs.iter().map(|t| t.name.clone()).collect(),
                    skills: skill_specs_for_req.iter().map(|s| s.name.clone()).collect(),
                    message_count: provider_messages.len(),
                    message_summary: summarize_messages(&provider_messages),
                },
            )
            .await;

            let req = ProviderRequest {
                messages: provider_messages.clone(),
                tool_specs,
                skills: skill_specs_for_req,
                state: self.state.clone(),
                last_tool_results: self.tool_results.clone(),
            };

            let (mut provider_step, mut provider_events) = match step_with_prompt_cache_and_events(
                &self.provider,
                req,
                self.config.provider_timeout_ms,
                &mut self.state,
            )
            .await
            {
                Ok(s) => s,
                Err(CachedProviderError::Provider(e)) => {
                    let out = finalize_run_output(RunOutput {
                        status: RunStatus::Error,
                        interrupts: Vec::new(),
                        final_text: String::new(),
                        tool_calls: self.tool_calls.clone(),
                        tool_results: self.tool_results.clone(),
                        state: self.state.clone(),
                        error: Some(RuntimeError {
                            code: "provider_error".to_string(),
                            message: e.to_string(),
                        }),
                        summarization_events: self
                            .state
                            .extra
                            .get("_summarization_events")
                            .cloned(),
                        trace: Some(serde_json::json!({
                            "terminated_at_step": step_idx,
                            "reason": "provider_error"
                        })),
                    });
                    return self
                        .finish_with_output(sink, out, "provider_error", event_step_idx + 1)
                        .await;
                }
                Err(CachedProviderError::Timeout) => {
                    let out = finalize_run_output(RunOutput {
                        status: RunStatus::Error,
                        interrupts: Vec::new(),
                        final_text: String::new(),
                        tool_calls: self.tool_calls.clone(),
                        tool_results: self.tool_results.clone(),
                        state: self.state.clone(),
                        error: Some(RuntimeError {
                            code: "provider_timeout".to_string(),
                            message: "provider timed out".to_string(),
                        }),
                        summarization_events: self
                            .state
                            .extra
                            .get("_summarization_events")
                            .cloned(),
                        trace: Some(serde_json::json!({
                            "terminated_at_step": step_idx,
                            "reason": "provider_timeout"
                        })),
                    });
                    return self
                        .finish_with_output(sink, out, "provider_timeout", event_step_idx + 1)
                        .await;
                }
            };

            if let ProviderStep::Error { error } = &provider_step {
                if error.code == "context_overflow" {
                    self.state.extra.insert(
                        "_summarization_force".to_string(),
                        serde_json::Value::Bool(true),
                    );
                    let mut overflow_messages = self.messages.clone();
                    if !self.runtime_middlewares.is_empty() {
                        for mw in &self.runtime_middlewares {
                            match mw
                                .before_provider_step(overflow_messages, &mut self.state)
                                .await
                            {
                                Ok(m) => overflow_messages = m,
                                Err(e) => {
                                    self.state.extra.remove("_summarization_force");
                                    let out = finalize_run_output(RunOutput {
                                        status: RunStatus::Error,
                                        interrupts: Vec::new(),
                                        final_text: String::new(),
                                        tool_calls: self.tool_calls.clone(),
                                        tool_results: self.tool_results.clone(),
                                        state: self.state.clone(),
                                        error: Some(RuntimeError {
                                            code: "middleware_error".to_string(),
                                            message: e.to_string(),
                                        }),
                                        summarization_events: self
                                            .state
                                            .extra
                                            .get("_summarization_events")
                                            .cloned(),
                                        trace: Some(serde_json::json!({
                                            "terminated_at_step": step_idx,
                                            "reason": "middleware_before_provider_step_error"
                                        })),
                                    });
                                    return self
                                        .finish_with_output(
                                            sink,
                                            out,
                                            "middleware_before_provider_step_error",
                                            event_step_idx + 1,
                                        )
                                        .await;
                                }
                            }
                        }
                    }
                    self.state.extra.remove("_summarization_force");
                    let retry_req = ProviderRequest {
                        messages: overflow_messages,
                        tool_specs: self.agent_tools(&self.state),
                        skills: skill_specs.clone(),
                        state: self.state.clone(),
                        last_tool_results: self.tool_results.clone(),
                    };
                    (provider_step, provider_events) = match step_with_prompt_cache_and_events(
                        &self.provider,
                        retry_req,
                        self.config.provider_timeout_ms,
                        &mut self.state,
                    )
                    .await
                    {
                        Ok(s) => s,
                        Err(CachedProviderError::Provider(e)) => {
                            let out = finalize_run_output(RunOutput {
                                status: RunStatus::Error,
                                interrupts: Vec::new(),
                                final_text: String::new(),
                                tool_calls: self.tool_calls.clone(),
                                tool_results: self.tool_results.clone(),
                                state: self.state.clone(),
                                error: Some(RuntimeError {
                                    code: "provider_error".to_string(),
                                    message: e.to_string(),
                                }),
                                summarization_events: self
                                    .state
                                    .extra
                                    .get("_summarization_events")
                                    .cloned(),
                                trace: Some(serde_json::json!({
                                    "terminated_at_step": step_idx,
                                    "reason": "provider_error"
                                })),
                            });
                            return self
                                .finish_with_output(sink, out, "provider_error", event_step_idx + 1)
                                .await;
                        }
                        Err(CachedProviderError::Timeout) => {
                            let out = finalize_run_output(RunOutput {
                                status: RunStatus::Error,
                                interrupts: Vec::new(),
                                final_text: String::new(),
                                tool_calls: self.tool_calls.clone(),
                                tool_results: self.tool_results.clone(),
                                state: self.state.clone(),
                                error: Some(RuntimeError {
                                    code: "provider_timeout".to_string(),
                                    message: "provider timed out".to_string(),
                                }),
                                summarization_events: self
                                    .state
                                    .extra
                                    .get("_summarization_events")
                                    .cloned(),
                                trace: Some(serde_json::json!({
                                    "terminated_at_step": step_idx,
                                    "reason": "provider_timeout"
                                })),
                            });
                            return self
                                .finish_with_output(
                                    sink,
                                    out,
                                    "provider_timeout",
                                    event_step_idx + 1,
                                )
                                .await;
                        }
                    };
                }
            }

            if !self.runtime_middlewares.is_empty() {
                for mw in &self.runtime_middlewares {
                    match mw
                        .patch_provider_step(provider_step, &mut self.next_call_id)
                        .await
                    {
                        Ok(s) => provider_step = s,
                        Err(e) => {
                            let out = finalize_run_output(RunOutput {
                                status: RunStatus::Error,
                                interrupts: Vec::new(),
                                final_text: String::new(),
                                tool_calls: self.tool_calls.clone(),
                                tool_results: self.tool_results.clone(),
                                state: self.state.clone(),
                                error: Some(RuntimeError {
                                    code: "middleware_error".to_string(),
                                    message: e.to_string(),
                                }),
                                summarization_events: self
                                    .state
                                    .extra
                                    .get("_summarization_events")
                                    .cloned(),
                                trace: Some(serde_json::json!({
                                    "terminated_at_step": step_idx,
                                    "reason": "middleware_patch_provider_step_error"
                                })),
                            });
                            return self
                                .finish_with_output(
                                    sink,
                                    out,
                                    "middleware_patch_provider_step_error",
                                    event_step_idx + 1,
                                )
                                .await;
                        }
                    }
                }
            }

            self.emit_event(
                sink,
                RunEvent::ProviderStepReceived {
                    step_index: event_step_idx,
                    step_type: provider_step_kind(&provider_step),
                },
            )
            .await;
            self.emit_provider_events(event_step_idx, &provider_events, sink)
                .await;

            match provider_step {
                ProviderStep::AssistantMessage { text } => {
                    let message = Message {
                        role: "assistant".to_string(),
                        content: text,
                        content_blocks: None,
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                        status: None,
                    };
                    self.messages.push(message.clone());
                    self.emit_event(
                        sink,
                        RunEvent::AssistantMessage {
                            step_index: event_step_idx,
                            message,
                        },
                    )
                    .await;
                }
                ProviderStep::FinalText { text } => {
                    let message = Message {
                        role: "assistant".to_string(),
                        content: text.clone(),
                        content_blocks: None,
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                        status: None,
                    };
                    self.messages.push(message.clone());
                    self.emit_event(
                        sink,
                        RunEvent::AssistantMessage {
                            step_index: event_step_idx,
                            message,
                        },
                    )
                    .await;
                    let out = finalize_run_output(RunOutput {
                        status: RunStatus::Completed,
                        interrupts: Vec::new(),
                        final_text: text,
                        tool_calls: self.tool_calls.clone(),
                        tool_results: self.tool_results.clone(),
                        state: self.state.clone(),
                        error: None,
                        summarization_events: self
                            .state
                            .extra
                            .get("_summarization_events")
                            .cloned(),
                        trace: Some(serde_json::json!({
                            "terminated_at_step": step_idx,
                            "reason": "final_text"
                        })),
                    });
                    self.emit_run_finished(
                        sink,
                        out.status,
                        "final_text",
                        out.final_text.clone(),
                        event_step_idx + 1,
                    )
                    .await;
                    return out;
                }
                ProviderStep::Error { error } => {
                    let out = finalize_run_output(RunOutput {
                        status: RunStatus::Error,
                        interrupts: Vec::new(),
                        final_text: String::new(),
                        tool_calls: self.tool_calls.clone(),
                        tool_results: self.tool_results.clone(),
                        state: self.state.clone(),
                        error: Some(RuntimeError {
                            code: error.code,
                            message: error.message,
                        }),
                        summarization_events: self
                            .state
                            .extra
                            .get("_summarization_events")
                            .cloned(),
                        trace: Some(serde_json::json!({
                            "terminated_at_step": step_idx,
                            "reason": "provider_step_error"
                        })),
                    });
                    self.emit_run_finished(
                        sink,
                        out.status,
                        "provider_step_error",
                        out.final_text.clone(),
                        event_step_idx + 1,
                    )
                    .await;
                    return out;
                }
                ProviderStep::SkillCall {
                    name,
                    input,
                    call_id,
                } => {
                    let call = SkillCall {
                        name,
                        input,
                        call_id,
                    };
                    let calls = match self.expand_skill(call).await {
                        Ok(c) => c,
                        Err(e) => {
                            let out = finalize_run_output(RunOutput {
                                status: RunStatus::Error,
                                interrupts: Vec::new(),
                                final_text: String::new(),
                                tool_calls: self.tool_calls.clone(),
                                tool_results: self.tool_results.clone(),
                                state: self.state.clone(),
                                error: Some(RuntimeError {
                                    code: e.code,
                                    message: e.message,
                                }),
                                summarization_events: self
                                    .state
                                    .extra
                                    .get("_summarization_events")
                                    .cloned(),
                                trace: Some(serde_json::json!({
                                    "terminated_at_step": step_idx,
                                    "reason": "skill_error"
                                })),
                            });
                            return self
                                .finish_with_output(sink, out, "skill_error", event_step_idx + 1)
                                .await;
                        }
                    };
                    if self.execute_calls(event_step_idx, calls, sink).await.is_some() {
                        let out = self.pending_output();
                        self.emit_run_finished(
                            sink,
                            out.status,
                            "interrupt",
                            out.final_text.clone(),
                            event_step_idx + 1,
                        )
                        .await;
                        return out;
                    }
                }
                ProviderStep::ToolCalls { calls } => {
                    let calls = crate::runtime::patch_tool_calls::normalize_provider_tool_calls(
                        calls,
                        &mut self.next_call_id,
                    );
                    let message = Message {
                        role: "assistant".to_string(),
                        content: String::new(),
                        content_blocks: None,
                        tool_calls: Some(tool_calls_from_provider_calls(&calls)),
                        tool_call_id: None,
                        name: None,
                        status: None,
                    };
                    self.messages.push(message.clone());
                    self.emit_event(
                        sink,
                        RunEvent::AssistantMessage {
                            step_index: event_step_idx,
                            message,
                        },
                    )
                    .await;
                    if self.execute_calls(event_step_idx, calls, sink).await.is_some() {
                        let out = self.pending_output();
                        self.emit_run_finished(
                            sink,
                            out.status,
                            "interrupt",
                            out.final_text.clone(),
                            event_step_idx + 1,
                        )
                        .await;
                        return out;
                    }
                }
            }
        }

        let out = finalize_run_output(RunOutput {
            status: RunStatus::Error,
            interrupts: Vec::new(),
            final_text: String::new(),
            tool_calls: self.tool_calls.clone(),
            tool_results: self.tool_results.clone(),
            state: self.state.clone(),
            error: Some(RuntimeError {
                code: "max_steps_exceeded".to_string(),
                message: "runtime exceeded max_steps".to_string(),
            }),
            summarization_events: self.state.extra.get("_summarization_events").cloned(),
            trace: Some(serde_json::json!({
                "terminated_at_step": self.config.max_steps,
                "reason": "max_steps_exceeded"
            })),
        });
        self.emit_run_finished(
            sink,
            out.status,
            "max_steps_exceeded",
            out.final_text.clone(),
            self.step_counter,
        )
        .await;
        out
    }

    pub async fn resume(&mut self, interrupt_id: &str, decision: HitlDecision) -> RunOutput {
        let mut sink = crate::runtime::NoopRunEventSink;
        self.resume_with_events(interrupt_id, decision, &mut sink).await
    }

    pub async fn resume_with_events(
        &mut self,
        interrupt_id: &str,
        decision: HitlDecision,
        sink: &mut dyn RunEventSink,
    ) -> RunOutput {
        let Some(p) = self.pending.clone() else {
            let out = finalize_run_output(RunOutput {
                status: RunStatus::Error,
                interrupts: Vec::new(),
                final_text: String::new(),
                tool_calls: self.tool_calls.clone(),
                tool_results: self.tool_results.clone(),
                state: self.state.clone(),
                error: Some(RuntimeError {
                    code: "interrupt_not_found".to_string(),
                    message: "no pending interrupt".to_string(),
                }),
                summarization_events: self.state.extra.get("_summarization_events").cloned(),
                trace: None,
            });
            return self
                .finish_with_output(sink, out, "interrupt_not_found", self.step_counter)
                .await;
        };

        if p.interrupt.interrupt_id != interrupt_id {
            let out = finalize_run_output(RunOutput {
                status: RunStatus::Error,
                interrupts: vec![p.interrupt],
                final_text: String::new(),
                tool_calls: self.tool_calls.clone(),
                tool_results: self.tool_results.clone(),
                state: self.state.clone(),
                error: Some(RuntimeError {
                    code: "interrupt_not_found".to_string(),
                    message: "interrupt_id mismatch".to_string(),
                }),
                summarization_events: self.state.extra.get("_summarization_events").cloned(),
                trace: None,
            });
            return self
                .finish_with_output(sink, out, "interrupt_not_found", self.step_counter)
                .await;
        }

        match decision.clone() {
            HitlDecision::Approve => {
                self.pending = None;
                self.execute_pending_call(self.step_counter, p.call, None, None, sink)
                    .await;
            }
            HitlDecision::Reject { reason } => {
                self.pending = None;
                self.inject_rejected_tool_message(self.step_counter, &p.call, reason, sink)
                    .await;
            }
            HitlDecision::Edit { args } => {
                if let Err(msg) = validate_edit_args(&p.call.tool_name, &args) {
                    let out = finalize_run_output(RunOutput {
                        status: RunStatus::Error,
                        interrupts: vec![p.interrupt],
                        final_text: String::new(),
                        tool_calls: self.tool_calls.clone(),
                        tool_results: self.tool_results.clone(),
                        state: self.state.clone(),
                        error: Some(RuntimeError {
                            code: "invalid_resume".to_string(),
                            message: msg,
                        }),
                        summarization_events: self
                            .state
                            .extra
                            .get("_summarization_events")
                            .cloned(),
                        trace: None,
                    });
                    return self
                        .finish_with_output(sink, out, "invalid_resume", self.step_counter)
                        .await;
                }
                self.pending = None;
                self.execute_pending_call(
                    self.step_counter,
                    p.call,
                    Some(args),
                    Some(p.interrupt.proposed_args.clone()),
                    sink,
                )
                .await;
            }
        }

        if self
            .execute_calls(self.step_counter, p.remaining_calls, sink)
            .await
            .is_some()
        {
            let out = self.pending_output();
            self.emit_run_finished(
                sink,
                out.status,
                "interrupt",
                out.final_text.clone(),
                self.step_counter,
            )
            .await;
            return out;
        }

        self.run_with_events(sink).await
    }

    fn pending_output(&self) -> RunOutput {
        let interrupts = self
            .pending
            .as_ref()
            .map(|p| vec![p.interrupt.clone()])
            .unwrap_or_default();
        finalize_run_output(RunOutput {
            status: RunStatus::Interrupted,
            interrupts,
            final_text: String::new(),
            tool_calls: self.tool_calls.clone(),
            tool_results: self.tool_results.clone(),
            state: self.state.clone(),
            error: None,
            summarization_events: self.state.extra.get("_summarization_events").cloned(),
            trace: None,
        })
    }

    fn agent_tools(&self, state: &AgentState) -> Vec<crate::runtime::ToolSpec> {
        let mut out = Vec::new();
        for (name, desc) in [
            ("ls", "Lists files and directories in a given path."),
            (
                "read_file",
                "Reads a file from the local filesystem and returns output.",
            ),
            ("write_file", "Writes a new file to the filesystem."),
            (
                "edit_file",
                "Edits an existing file by replacing a literal string.",
            ),
            ("delete_file", "Deletes a file from the filesystem."),
            ("glob", "Glob match file paths."),
            ("grep", "Search for a literal text pattern across files."),
            (
                "execute",
                "Executes a shell command in an isolated sandbox environment.",
            ),
            ("task", "Launch a sub-agent and assign a task to it."),
            (
                "compact_conversation",
                "Compacts conversation history by summarizing older messages.",
            ),
        ] {
            out.push(crate::runtime::ToolSpec {
                name: name.to_string(),
                description: desc.to_string(),
            });
        }
        if let Some(v) = state.extra.get("skills_tools") {
            if let Ok(skills) =
                serde_json::from_value::<Vec<crate::skills::SkillToolSpec>>(v.clone())
            {
                for s in skills {
                    out.push(crate::runtime::ToolSpec {
                        name: s.name,
                        description: s.description,
                    });
                }
            }
        }
        out
    }

    async fn maybe_offload_large_tool_result(
        agent: &DeepAgent,
        state: &mut AgentState,
        tool_name: &str,
        call_id: &str,
        output: serde_json::Value,
    ) -> (serde_json::Value, String) {
        let content = if let Some(s) = output
            .get("content")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            s.to_string()
        } else {
            serde_json::to_string(&output).unwrap_or_default()
        };

        let Some(opts) = load_large_tool_result_offload_options(state) else {
            return (output, content);
        };
        if !opts.enabled {
            return (output, content);
        }
        if opts.excluded_tools.iter().any(|t| t == tool_name) {
            return (output, content);
        }

        let full_text = if let Some(s) = output.get("content").and_then(|v| v.as_str()) {
            s.to_string()
        } else {
            serde_json::to_string(&output).unwrap_or_default()
        };
        let total_chars = full_text.chars().count();
        if total_chars < opts.threshold_chars {
            return (output, content);
        }

        let id = sanitize_tool_call_id(call_id);
        let prefix = opts.prefix.trim_end_matches('/');
        let path = format!("{prefix}/{id}");

        let backend = agent.backend();
        let write_ok = match backend.write_file(&path, &full_text).await {
            Ok(wr) => wr.error.as_deref().is_none() || wr.error.as_deref() == Some("file_exists"),
            Err(_) => false,
        };
        if !write_ok {
            return (output, content);
        }

        let (head, tail) = preview_head_tail_lines(&full_text, opts.preview_max_lines);
        let content2 = format!(
            "TOOL_OUTPUT_OFFLOADED: Full output written to {}. Use read_file with offset/limit to paginate.\n<preview_head>\n{}\n</preview_head>\n<preview_tail>\n{}\n</preview_tail>",
            path, head, tail
        );

        let output2 = match output {
            serde_json::Value::Object(mut obj) => {
                obj.insert("offloaded".to_string(), serde_json::Value::Bool(true));
                obj.insert(
                    "offload_path".to_string(),
                    serde_json::Value::String(path.clone()),
                );
                obj.insert(
                    "offload_total_chars".to_string(),
                    serde_json::Value::Number(total_chars.into()),
                );
                obj.insert(
                    "offload_head".to_string(),
                    serde_json::Value::String(head.clone()),
                );
                obj.insert(
                    "offload_tail".to_string(),
                    serde_json::Value::String(tail.clone()),
                );
                obj.insert(
                    "content".to_string(),
                    serde_json::Value::String(format!("(offloaded to {}; use read_file)", path)),
                );
                serde_json::Value::Object(obj)
            }
            other => serde_json::json!({
                "offloaded": true,
                "offload_path": path,
                "offload_total_chars": total_chars,
                "offload_head": head,
                "offload_tail": tail,
                "output_type": match other {
                    serde_json::Value::Null => "null",
                    serde_json::Value::Bool(_) => "bool",
                    serde_json::Value::Number(_) => "number",
                    serde_json::Value::String(_) => "string",
                    serde_json::Value::Array(_) => "array",
                    serde_json::Value::Object(_) => "object",
                },
                "content": format!("(offloaded; use read_file)"),
            }),
        };

        state.extra.insert(
            "_large_tool_result_offload_event".to_string(),
            serde_json::json!({
                "tool_name": tool_name,
                "call_id": call_id,
                "path": output2.get("offload_path").cloned().unwrap_or(serde_json::Value::Null),
                "total_chars": total_chars,
            }),
        );

        (output2, content2)
    }

    async fn expand_skill(
        &self,
        call: SkillCall,
    ) -> Result<Vec<ProviderToolCall>, crate::skills::SkillError> {
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
        &mut self,
        step_index: usize,
        calls: Vec<ProviderToolCall>,
        sink: &mut dyn RunEventSink,
    ) -> Option<HitlInterrupt> {
        let mut queue: std::collections::VecDeque<ProviderToolCall> =
            std::collections::VecDeque::from(calls);
        let write_todos_count = queue
            .iter()
            .filter(|c| c.tool_name == "write_todos")
            .count();

        while let Some(call0) = queue.pop_front() {
            let normalized = normalize_tool_call_for_execution(call0, &mut self.next_call_id);
            let (call, error) = match normalized {
                NormalizedToolCall::Valid(c) => (c, None),
                NormalizedToolCall::Invalid { call, error } => (call, Some(error)),
            };

            let call_id = call.call_id.clone().unwrap_or_default();
            let tool_name = call.tool_name.clone();

            if let Some(err) = error {
                self.tool_calls.push(ToolCallRecord {
                    tool_name: tool_name.clone(),
                    arguments: call.arguments.clone(),
                    call_id: Some(call_id.clone()),
                });
                self.emit_event(
                    sink,
                    RunEvent::ToolCallStarted {
                        step_index,
                        tool_name: tool_name.clone(),
                        tool_call_id: call_id.clone(),
                        arguments_preview: preview_json(&call.arguments),
                    },
                )
                .await;
                let before_state = self.state.clone();
                self.push_tool_result_and_message(
                    step_index,
                    &before_state,
                    tool_name,
                    call_id,
                    serde_json::Value::Null,
                    Some(err.clone()),
                    "error".to_string(),
                    err,
                    None,
                    sink,
                    None,
                )
                .await;
                continue;
            }

            self.tool_calls.push(ToolCallRecord {
                tool_name: tool_name.clone(),
                arguments: call.arguments.clone(),
                call_id: Some(call_id.clone()),
            });

            if write_todos_count > 1 && call.tool_name == "write_todos" {
                let err = "Error: The `write_todos` tool should never be called multiple times in parallel. Please call it only once per model invocation to update the todo list.".to_string();
                self.emit_event(
                    sink,
                    RunEvent::ToolCallStarted {
                        step_index,
                        tool_name: tool_name.clone(),
                        tool_call_id: call_id.clone(),
                        arguments_preview: preview_json(&call.arguments),
                    },
                )
                .await;
                let before_state = self.state.clone();
                self.push_tool_result_and_message(
                    step_index,
                    &before_state,
                    tool_name,
                    call_id,
                    serde_json::Value::Null,
                    Some(err.clone()),
                    "error".to_string(),
                    err,
                    None,
                    sink,
                    None,
                )
                .await;
                continue;
            }

            if tool_name == "execute" {
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
                        ApprovalDecision::Allow { .. } => {}
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
                            self.tool_results.push(ToolResultRecord {
                                tool_name: tool_name.clone(),
                                call_id: Some(call_id.clone()),
                                output: serde_json::Value::Null,
                                error: Some(err.clone()),
                                status: Some("error".to_string()),
                            });
                            self.messages.push(Message {
                                role: "tool".to_string(),
                                content: serde_json::to_string(&serde_json::json!({
                                    "tool_call_id": call_id.clone(),
                                    "tool_name": tool_name.clone(),
                                    "status": "error",
                                    "error": err.clone(),
                                    "content": err,
                                }))
                                .unwrap_or_default(),
                                content_blocks: None,
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
                                    decision_reason: reason,
                                    exit_code: None,
                                    truncated: None,
                                    duration_ms: None,
                                });
                            }
                            let interrupt = HitlInterrupt {
                                interrupt_id: call_id.clone(),
                                tool_name: tool_name.clone(),
                                tool_call_id: call_id.clone(),
                                proposed_args: call.arguments.clone(),
                                policy: Default::default(),
                                hints: None,
                            };
                            let remaining_calls = queue.into_iter().collect::<Vec<_>>();
                            self.emit_event(
                                sink,
                                RunEvent::Interrupt {
                                    step_index,
                                    interrupt: interrupt.clone(),
                                },
                            )
                            .await;
                            self.pending = Some(PendingInterrupt {
                                interrupt: interrupt.clone(),
                                call,
                                remaining_calls,
                            });
                            return Some(interrupt);
                        }
                    }
                }
            }

            if self.interrupt_on.get(&tool_name).copied().unwrap_or(false) {
                let interrupt = HitlInterrupt {
                    interrupt_id: call_id.clone(),
                    tool_name: tool_name.clone(),
                    tool_call_id: call_id.clone(),
                    proposed_args: call.arguments.clone(),
                    policy: Default::default(),
                    hints: None,
                };
                let remaining_calls = queue.into_iter().collect::<Vec<_>>();
                self.emit_event(
                    sink,
                    RunEvent::Interrupt {
                        step_index,
                        interrupt: interrupt.clone(),
                    },
                )
                .await;
                self.pending = Some(PendingInterrupt {
                    interrupt: interrupt.clone(),
                    call,
                    remaining_calls,
                });
                return Some(interrupt);
            }

            self.emit_event(
                sink,
                RunEvent::ToolCallStarted {
                    step_index,
                    tool_name: tool_name.clone(),
                    tool_call_id: call_id.clone(),
                    arguments_preview: preview_json(&call.arguments),
                },
            )
            .await;

            if !self.runtime_middlewares.is_empty() {
                let before_state = self.state.clone();
                let mut ctx = ToolCallContext {
                    agent: &self.agent,
                    tool_call: &call,
                    call_id: &call_id,
                    messages: &mut self.messages,
                    state: &mut self.state,
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
                    let (output, content) = if let Some(e) = &error {
                        (output, e.clone())
                    } else {
                        ResumableRunner::maybe_offload_large_tool_result(
                            &self.agent,
                            &mut self.state,
                            &tool_name,
                            &cid,
                            output,
                        )
                        .await
                    };
                    self.push_tool_result_and_message(
                        step_index,
                        &before_state,
                        tool_name.clone(),
                        cid,
                        output,
                        error,
                        status,
                        content,
                        None,
                        sink,
                        None,
                    )
                    .await;
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
                            let before_state = self.state.clone();
                            let started = std::time::Instant::now();
                            let result = self
                                .agent
                                .call_tool_stateful(
                                    &call.tool_name,
                                    call.arguments.clone(),
                                    &mut self.state,
                                )
                                .await;
                            let duration_ms = started.elapsed().as_millis() as u64;
                            match result {
                                Ok((out, _delta)) => {
                                    let crate::tools::ToolResult {
                                        output,
                                        content_blocks,
                                    } = out;
                                    let exit_code = output
                                        .get("exit_code")
                                        .and_then(|v| v.as_i64())
                                        .map(|v| v as i32);
                                    let truncated =
                                        output.get("truncated").and_then(|v| v.as_bool());
                                    if let Some(sink) = &self.audit {
                                        let _ = sink.record(AuditEvent {
                                            timestamp_ms: Utc::now().timestamp_millis(),
                                            root: self.root.clone(),
                                            mode: mode_str(self.mode),
                                            command_redacted: redact_command(&cmd),
                                            decision: "allow".to_string(),
                                            decision_code: "allow".to_string(),
                                            decision_reason: reason,
                                            exit_code,
                                            truncated,
                                            duration_ms: Some(duration_ms),
                                        });
                                    }
                                    let (out, content) =
                                        ResumableRunner::maybe_offload_large_tool_result(
                                            &self.agent,
                                            &mut self.state,
                                            &tool_name,
                                            &call_id,
                                            output,
                                        )
                                        .await;
                                    self.push_tool_result_and_message(
                                        step_index,
                                        &before_state,
                                        tool_name.clone(),
                                        call_id.clone(),
                                        out,
                                        None,
                                        "success".to_string(),
                                        content,
                                        content_blocks,
                                        sink,
                                        None,
                                    )
                                    .await;
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
                                            decision_reason: "allowed but execution failed"
                                                .to_string(),
                                            exit_code: None,
                                            truncated: None,
                                            duration_ms: Some(duration_ms),
                                        });
                                    }
                                    let err = e.to_string();
                                    let before_state = self.state.clone();
                                    self.push_tool_result_and_message(
                                        step_index,
                                        &before_state,
                                        tool_name.clone(),
                                        call_id.clone(),
                                        serde_json::Value::Null,
                                        Some(err.clone()),
                                        "error".to_string(),
                                        err,
                                        None,
                                        sink,
                                        None,
                                    )
                                    .await;
                                }
                            }
                            continue;
                        }
                        ApprovalDecision::Deny { code, reason } => {
                            if let Some(sink) = &self.audit {
                                let cmd = call
                                    .arguments
                                    .get("command")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
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
                            let before_state = self.state.clone();
                            self.push_tool_result_and_message(
                                step_index,
                                &before_state,
                                tool_name.clone(),
                                call_id.clone(),
                                serde_json::Value::Null,
                                Some(err.clone()),
                                "error".to_string(),
                                err,
                                None,
                                sink,
                                None,
                            )
                            .await;
                            continue;
                        }
                        ApprovalDecision::RequireApproval { code, reason } => {
                            if let Some(sink) = &self.audit {
                                let cmd = call
                                    .arguments
                                    .get("command")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
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
                            let interrupt = HitlInterrupt {
                                interrupt_id: call_id.clone(),
                                tool_name: tool_name.clone(),
                                tool_call_id: call_id.clone(),
                                proposed_args: call.arguments.clone(),
                                policy: Default::default(),
                                hints: None,
                            };
                            let remaining_calls = queue.into_iter().collect::<Vec<_>>();
                            self.emit_event(
                                sink,
                                RunEvent::Interrupt {
                                    step_index,
                                    interrupt: interrupt.clone(),
                                },
                            )
                            .await;
                            self.pending = Some(PendingInterrupt {
                                interrupt: interrupt.clone(),
                                call,
                                remaining_calls,
                            });
                            return Some(interrupt);
                        }
                    }
                }
            }

            let before_state = self.state.clone();
            let result = self
                .agent
                .call_tool_stateful(&call.tool_name, call.arguments.clone(), &mut self.state)
                .await;
            match result {
                Ok((out, _delta)) => {
                    let crate::tools::ToolResult {
                        output,
                        content_blocks,
                    } = out;
                    let (out, content) = ResumableRunner::maybe_offload_large_tool_result(
                        &self.agent,
                        &mut self.state,
                        &tool_name,
                        &call_id,
                        output,
                    )
                    .await;
                    self.push_tool_result_and_message(
                        step_index,
                        &before_state,
                        tool_name.clone(),
                        call_id.clone(),
                        out,
                        None,
                        "success".to_string(),
                        content,
                        content_blocks,
                        sink,
                        None,
                    )
                    .await;
                }
                Err(e) => {
                    let err = e.to_string();
                    let before_state = self.state.clone();
                    self.push_tool_result_and_message(
                        step_index,
                        &before_state,
                        tool_name.clone(),
                        call_id.clone(),
                        serde_json::Value::Null,
                        Some(err.clone()),
                        "error".to_string(),
                        err,
                        None,
                        sink,
                        None,
                    )
                    .await;
                }
            }
        }
        None
    }

    async fn execute_pending_call(
        &mut self,
        step_index: usize,
        call: ProviderToolCall,
        edited_args: Option<serde_json::Value>,
        original_args: Option<serde_json::Value>,
        sink: &mut dyn RunEventSink,
    ) {
        let call_id = call.call_id.clone().unwrap_or_default();
        let tool_name = call.tool_name.clone();
        let args = edited_args
            .clone()
            .unwrap_or_else(|| call.arguments.clone());

        self.emit_event(
            sink,
            RunEvent::ToolCallStarted {
                step_index,
                tool_name: tool_name.clone(),
                tool_call_id: call_id.clone(),
                arguments_preview: preview_json(&args),
            },
        )
        .await;

        if tool_name == "execute" {
            if let Some(policy) = &self.approval {
                let cmd = args
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let req = ApprovalRequest {
                    command: cmd.clone(),
                    root: self.root.clone(),
                    mode: self.mode,
                };
                match policy.decide(&req) {
                    ApprovalDecision::Allow { reason } => {
                        let before_state = self.state.clone();
                        let started = std::time::Instant::now();
                        let result = self
                            .agent
                            .call_tool_stateful(&tool_name, args.clone(), &mut self.state)
                            .await;
                        let duration_ms = started.elapsed().as_millis() as u64;
                        match result {
                            Ok((out, _delta)) => {
                                let crate::tools::ToolResult {
                                    output,
                                    content_blocks,
                                } = out;
                                let exit_code = output
                                    .get("exit_code")
                                    .and_then(|v| v.as_i64())
                                    .map(|v| v as i32);
                                let truncated = output.get("truncated").and_then(|v| v.as_bool());
                                if let Some(sink) = &self.audit {
                                    let _ = sink.record(AuditEvent {
                                        timestamp_ms: Utc::now().timestamp_millis(),
                                        root: self.root.clone(),
                                        mode: mode_str(self.mode),
                                        command_redacted: redact_command(&cmd),
                                        decision: "allow".to_string(),
                                        decision_code: "allow".to_string(),
                                        decision_reason: reason,
                                        exit_code,
                                        truncated,
                                        duration_ms: Some(duration_ms),
                                    });
                                }
                                let (output, content) =
                                    ResumableRunner::maybe_offload_large_tool_result(
                                        &self.agent,
                                        &mut self.state,
                                        &tool_name,
                                        &call_id,
                                        output,
                                    )
                                    .await;
                                let status = if edited_args.is_some() {
                                    "edited"
                                } else {
                                    "success"
                                };
                                let mut message_json = serde_json::json!({
                                    "tool_call_id": call_id.clone(),
                                    "tool_name": tool_name.clone(),
                                    "status": status,
                                    "output": output.clone(),
                                    "content": content.clone(),
                                });
                                if edited_args.is_some() {
                                    if let serde_json::Value::Object(map) = &mut message_json {
                                        map.insert("edited".to_string(), serde_json::Value::Bool(true));
                                        if let Some(orig) = original_args {
                                            map.insert("original_args".to_string(), orig);
                                        }
                                        map.insert("effective_args".to_string(), args.clone());
                                    }
                                }
                                self.push_tool_result_and_message(
                                    step_index,
                                    &before_state,
                                    tool_name.clone(),
                                    call_id.clone(),
                                    output,
                                    None,
                                    status.to_string(),
                                    content,
                                    content_blocks,
                                    sink,
                                    Some(message_json),
                                )
                                .await;
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
                                self.push_tool_result_and_message(
                                    step_index,
                                    &before_state,
                                    tool_name.clone(),
                                    call_id.clone(),
                                    serde_json::Value::Null,
                                    Some(err.clone()),
                                    "error".to_string(),
                                    err,
                                    None,
                                    sink,
                                    None,
                                )
                                .await;
                            }
                        }
                        return;
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
                        let before_state = self.state.clone();
                        let err = format!("command_not_allowed: {}: {}", code, reason);
                        self.push_tool_result_and_message(
                            step_index,
                            &before_state,
                            tool_name.clone(),
                            call_id.clone(),
                            serde_json::Value::Null,
                            Some(err.clone()),
                            "error".to_string(),
                            err,
                            None,
                            sink,
                            None,
                        )
                        .await;
                        return;
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
                        let before_state = self.state.clone();
                        let err = format!("command_not_allowed: {}: {}", code, reason);
                        self.push_tool_result_and_message(
                            step_index,
                            &before_state,
                            tool_name.clone(),
                            call_id.clone(),
                            serde_json::Value::Null,
                            Some(err.clone()),
                            "error".to_string(),
                            err,
                            None,
                            sink,
                            None,
                        )
                        .await;
                        return;
                    }
                }
            }
        }

        let before_state = self.state.clone();
        let result = self
            .agent
            .call_tool_stateful(&tool_name, args.clone(), &mut self.state)
            .await;

        match result {
            Ok((out, _delta)) => {
                let crate::tools::ToolResult {
                    output,
                    content_blocks,
                } = out;
                let (output, content) = ResumableRunner::maybe_offload_large_tool_result(
                    &self.agent,
                    &mut self.state,
                    &tool_name,
                    &call_id,
                    output,
                )
                .await;
                let status = if edited_args.is_some() {
                    "edited"
                } else {
                    "success"
                };
                let mut message_json = serde_json::json!({
                    "tool_call_id": call_id.clone(),
                    "tool_name": tool_name.clone(),
                    "status": status,
                    "output": output.clone(),
                    "content": content.clone(),
                });
                if edited_args.is_some() {
                    if let serde_json::Value::Object(map) = &mut message_json {
                        map.insert("edited".to_string(), serde_json::Value::Bool(true));
                        if let Some(orig) = original_args {
                            map.insert("original_args".to_string(), orig);
                        }
                        map.insert("effective_args".to_string(), args);
                    }
                }
                self.push_tool_result_and_message(
                    step_index,
                    &before_state,
                    tool_name.clone(),
                    call_id.clone(),
                    output,
                    None,
                    status.to_string(),
                    content,
                    content_blocks,
                    sink,
                    Some(message_json),
                )
                .await;
            }
            Err(e) => {
                let err = e.to_string();
                self.push_tool_result_and_message(
                    step_index,
                    &before_state,
                    tool_name.clone(),
                    call_id.clone(),
                    serde_json::Value::Null,
                    Some(err.clone()),
                    "error".to_string(),
                    err,
                    None,
                    sink,
                    None,
                )
                .await;
            }
        }
    }

    async fn inject_rejected_tool_message(
        &mut self,
        step_index: usize,
        call: &ProviderToolCall,
        reason: Option<String>,
        sink: &mut dyn RunEventSink,
    ) {
        let call_id = call.call_id.clone().unwrap_or_default();
        let tool_name = call.tool_name.clone();
        let reason2 = reason.unwrap_or_else(|| "rejected".to_string());
        let err = format!("tool_call_rejected: {}", reason2);
        let before_state = self.state.clone();
        self.emit_event(
            sink,
            RunEvent::ToolCallStarted {
                step_index,
                tool_name: tool_name.clone(),
                tool_call_id: call_id.clone(),
                arguments_preview: preview_json(&call.arguments),
            },
        )
        .await;
        self.push_tool_result_and_message(
            step_index,
            &before_state,
            tool_name,
            call_id,
            serde_json::Value::Null,
            Some(err.clone()),
            "rejected".to_string(),
            err,
            None,
            sink,
            None,
        )
        .await;
    }
}

#[async_trait::async_trait]
impl crate::runtime::StreamingRuntime for ResumableRunner {
    async fn run_with_events(&mut self, sink: &mut dyn crate::runtime::RunEventSink) -> RunOutput {
        ResumableRunner::run_with_events(self, sink).await
    }
}

fn validate_edit_args(tool_name: &str, args: &serde_json::Value) -> Result<(), String> {
    let Some(obj) = args.as_object() else {
        return Err("args must be a JSON object".to_string());
    };

    let has_str = |k: &str| -> bool { obj.get(k).and_then(|v| v.as_str()).is_some() };

    match tool_name {
        "write_file" => {
            if !has_str("file_path") {
                return Err("missing required field: file_path".to_string());
            }
            if !has_str("content") {
                return Err("missing required field: content".to_string());
            }
        }
        "edit_file" => {
            if !has_str("file_path") {
                return Err("missing required field: file_path".to_string());
            }
            if !has_str("old_string") {
                return Err("missing required field: old_string".to_string());
            }
            if !has_str("new_string") {
                return Err("missing required field: new_string".to_string());
            }
        }
        "delete_file" | "read_file" => {
            if !has_str("file_path") {
                return Err("missing required field: file_path".to_string());
            }
        }
        "execute" => {
            if !has_str("command") {
                return Err("missing required field: command".to_string());
            }
        }
        _ => {}
    }
    Ok(())
}
