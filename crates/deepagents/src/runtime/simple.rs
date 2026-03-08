use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::time::{timeout, Duration};

use crate::approval::{redact_command, ApprovalDecision, ApprovalPolicy, ApprovalRequest, ExecutionMode};
use crate::audit::{AuditEvent, AuditSink};
use crate::provider::{Provider, ProviderRequest, ProviderStep, ProviderToolCall};
use crate::runtime::protocol::{
    RunOutput, Runtime, RuntimeConfig, RuntimeError, ToolCallRecord, ToolResultRecord, ToolSpec,
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

pub struct SimpleRuntime {
    agent: DeepAgent,
    provider: Arc<dyn Provider>,
    skills: Vec<Arc<dyn SkillPlugin>>,
    config: RuntimeConfig,
    approval: Option<Arc<dyn ApprovalPolicy>>,
    audit: Option<Arc<dyn AuditSink>>,
    root: String,
    mode: ExecutionMode,
}

impl SimpleRuntime {
    pub fn new(
        agent: DeepAgent,
        provider: Arc<dyn Provider>,
        skills: Vec<Arc<dyn SkillPlugin>>,
        config: RuntimeConfig,
        approval: Option<Arc<dyn ApprovalPolicy>>,
        audit: Option<Arc<dyn AuditSink>>,
        root: String,
        mode: ExecutionMode,
    ) -> Self {
        Self {
            agent,
            provider,
            skills,
            config,
            approval,
            audit,
            root,
            mode,
        }
    }
}

#[async_trait]
impl Runtime for SimpleRuntime {
    async fn run(&self, mut messages: Vec<Message>) -> RunOutput {
        let mut state = AgentState::default();
        let mut tool_calls: Vec<ToolCallRecord> = Vec::new();
        let mut tool_results: Vec<ToolResultRecord> = Vec::new();
        let mut next_call_id = 1u64;

        for step_idx in 0..self.config.max_steps {
            let tool_specs = self.agent_tools();
            let skill_specs = self
                .skills
                .iter()
                .flat_map(|p| p.list_skills())
                .collect::<Vec<_>>();

            let req = ProviderRequest {
                messages: messages.clone(),
                tool_specs,
                skills: skill_specs,
                state: state.clone(),
                last_tool_results: tool_results.clone(),
            };

            let provider_step = match timeout(
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
                        state,
                        error: Some(RuntimeError {
                            code: "provider_error".to_string(),
                            message: e.to_string(),
                        }),
                        trace: Some(serde_json::json!({ "terminated_at_step": step_idx, "reason": "provider_error" })),
                    };
                }
                Err(_) => {
                    return RunOutput {
                        final_text: String::new(),
                        tool_calls,
                        tool_results,
                        state,
                        error: Some(RuntimeError {
                            code: "provider_timeout".to_string(),
                            message: "provider timed out".to_string(),
                        }),
                        trace: Some(serde_json::json!({ "terminated_at_step": step_idx, "reason": "provider_timeout" })),
                    };
                }
            };

            match provider_step {
                ProviderStep::FinalText { text } => {
                    return RunOutput {
                        final_text: text,
                        tool_calls,
                        tool_results,
                        state,
                        error: None,
                        trace: Some(serde_json::json!({ "terminated_at_step": step_idx, "reason": "final_text" })),
                    };
                }
                ProviderStep::Error { error } => {
                    return RunOutput {
                        final_text: String::new(),
                        tool_calls,
                        tool_results,
                        state,
                        error: Some(RuntimeError {
                            code: error.code,
                            message: error.message,
                        }),
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
                                state,
                                error: Some(RuntimeError {
                                    code: e.code,
                                    message: e.message,
                                }),
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
            state,
            error: Some(RuntimeError {
                code: "max_steps_exceeded".to_string(),
                message: "runtime exceeded max_steps".to_string(),
            }),
            trace: Some(serde_json::json!({ "terminated_at_step": self.config.max_steps, "reason": "max_steps_exceeded" })),
        }
    }
}

impl SimpleRuntime {
    fn agent_tools(&self) -> Vec<ToolSpec> {
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
        ] {
            out.push(ToolSpec {
                name: name.to_string(),
                description: desc.to_string(),
            });
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
            let call_id = call
                .call_id
                .clone()
                .unwrap_or_else(|| {
                    let id = format!("call-{}", *next_call_id);
                    *next_call_id += 1;
                    id
                });

            if !call.arguments.is_object() {
                tool_calls.push(ToolCallRecord {
                    tool_name: call.tool_name.clone(),
                    arguments: call.arguments.clone(),
                    call_id: Some(call_id.clone()),
                });
                tool_results.push(ToolResultRecord {
                    tool_name: call.tool_name.clone(),
                    call_id: Some(call_id),
                    output: serde_json::Value::Null,
                    error: Some("invalid_tool_call: arguments must be object".to_string()),
                });
                continue;
            }

            tool_calls.push(ToolCallRecord {
                tool_name: call.tool_name.clone(),
                arguments: call.arguments.clone(),
                call_id: Some(call_id.clone()),
            });

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
                                    tool_results.push(ToolResultRecord {
                                        tool_name: call.tool_name.clone(),
                                        call_id: Some(call_id.clone()),
                                        output: out.clone(),
                                        error: None,
                                    });
                                    messages.push(Message {
                                        role: "tool".to_string(),
                                        content: serde_json::to_string(&serde_json::json!({
                                            "tool_name": call.tool_name,
                                            "call_id": call_id,
                                            "output": out
                                        }))
                                        .unwrap_or_default(),
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
                                    tool_results.push(ToolResultRecord {
                                        tool_name: call.tool_name.clone(),
                                        call_id: Some(call_id.clone()),
                                        output: serde_json::Value::Null,
                                        error: Some(e.to_string()),
                                    });
                                    messages.push(Message {
                                        role: "tool".to_string(),
                                        content: serde_json::to_string(&serde_json::json!({
                                            "tool_name": call.tool_name,
                                            "call_id": call_id,
                                            "error": e.to_string()
                                        }))
                                        .unwrap_or_default(),
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
                                tool_name: call.tool_name.clone(),
                                call_id: Some(call_id.clone()),
                                output: serde_json::Value::Null,
                                error: Some(err.clone()),
                            });
                            messages.push(Message {
                                role: "tool".to_string(),
                                content: serde_json::to_string(&serde_json::json!({
                                    "tool_name": call.tool_name,
                                    "call_id": call_id,
                                    "error": err
                                }))
                                .unwrap_or_default(),
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
                                tool_name: call.tool_name.clone(),
                                call_id: Some(call_id.clone()),
                                output: serde_json::Value::Null,
                                error: Some(err.clone()),
                            });
                            messages.push(Message {
                                role: "tool".to_string(),
                                content: serde_json::to_string(&serde_json::json!({
                                    "tool_name": call.tool_name,
                                    "call_id": call_id,
                                    "error": err
                                }))
                                .unwrap_or_default(),
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
                    tool_results.push(ToolResultRecord {
                        tool_name: call.tool_name.clone(),
                        call_id: Some(call_id.clone()),
                        output: out.clone(),
                        error: None,
                    });
                    messages.push(Message {
                        role: "tool".to_string(),
                        content: serde_json::to_string(&serde_json::json!({
                            "tool_name": call.tool_name,
                            "call_id": call_id,
                            "output": out
                        }))
                        .unwrap_or_default(),
                    });
                }
                Err(e) => {
                    tool_results.push(ToolResultRecord {
                        tool_name: call.tool_name.clone(),
                        call_id: Some(call_id.clone()),
                        output: serde_json::Value::Null,
                        error: Some(e.to_string()),
                    });
                    messages.push(Message {
                        role: "tool".to_string(),
                        content: serde_json::to_string(&serde_json::json!({
                            "tool_name": call.tool_name,
                            "call_id": call_id,
                            "error": e.to_string()
                        }))
                        .unwrap_or_default(),
                    });
                }
            }
        }
    }
}
