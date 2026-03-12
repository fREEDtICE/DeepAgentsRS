use std::sync::Arc;

use async_trait::async_trait;

use crate::provider::mock::{MockProvider, MockScript, MockStep};
use crate::provider::AgentToolCall;
use crate::runtime::simple::SimpleRuntime;
use crate::runtime::{RunStatus, Runtime, RuntimeConfig};
use crate::subagents::protocol::{CompiledSubAgent, SubAgentRunOutput, SubAgentRunRequest};
use crate::subagents::registry::InMemorySubAgentRegistry;
use crate::subagents::SubAgentRegistry;

pub fn default_registry() -> anyhow::Result<Arc<dyn SubAgentRegistry>> {
    let reg: Arc<InMemorySubAgentRegistry> = Arc::new(InMemorySubAgentRegistry::new());
    reg.register(Arc::new(FixedTextSubAgent::new(
        "general-purpose",
        "General purpose sub-agent",
        "HI",
    )))?;
    reg.register(Arc::new(EchoSubAgent::new(
        "echo-subagent",
        "Echo sub-agent (for isolation assertions)",
    )))?;
    reg.register(Arc::new(ExtraStateSubAgent::new(
        "state-extra-subagent",
        "Inject extra state keys (for merge assertions)",
    )))?;
    reg.register(Arc::new(BrokenSubAgent::new(
        "broken-subagent",
        "Broken sub-agent (returns empty output)",
    )))?;
    reg.register(Arc::new(runtime_mock_subagent(
        "write-file-subagent",
        "Writes /child.txt and returns DONE",
        MockScript {
            steps: vec![
                MockStep::ToolCalls {
                    calls: vec![AgentToolCall {
                        tool_name: "write_file".to_string(),
                        arguments: serde_json::json!({ "file_path": "child.txt", "content": "hi\n" }),
                        call_id: Some("w1".to_string()),
                    }],
                },
                MockStep::FinalText {
                    text: "DONE".to_string(),
                },
            ],
        },
    )))?;
    reg.register(Arc::new(runtime_mock_subagent(
        "multi-message-subagent",
        "Emits multiple assistant messages then returns final",
        MockScript {
            steps: vec![
                MockStep::AssistantMessage {
                    text: "step1".to_string(),
                },
                MockStep::AssistantMessage {
                    text: "step2".to_string(),
                },
                MockStep::FinalText {
                    text: "final".to_string(),
                },
            ],
        },
    )))?;
    reg.register(Arc::new(runtime_mock_subagent(
        "nested-task-subagent",
        "Calls task inside child to test nesting",
        MockScript {
            steps: vec![
                MockStep::ToolCalls {
                    calls: vec![AgentToolCall {
                        tool_name: "task".to_string(),
                        arguments: serde_json::json!({ "description": "inner", "subagent_type": "general-purpose" }),
                        call_id: Some("inner-task".to_string()),
                    }],
                },
                MockStep::FinalFromLastToolFirstLine { prefix: None },
            ],
        },
    )))?;
    reg.register(Arc::new(runtime_last_tool_error_subagent(
        "root-escape-subagent",
        "Tries reading outside root and reports error",
        MockScript {
            steps: vec![MockStep::ToolCalls {
                calls: vec![AgentToolCall {
                    tool_name: "read_file".to_string(),
                    arguments: serde_json::json!({ "file_path": "../secret.txt", "limit": 1 }),
                    call_id: Some("r1".to_string()),
                }],
            }],
        },
    )))?;
    reg.register(Arc::new(runtime_last_tool_error_subagent(
        "execute-deny-subagent",
        "Tries execute and reports policy error",
        MockScript {
            steps: vec![MockStep::ToolCalls {
                calls: vec![AgentToolCall {
                    tool_name: "execute".to_string(),
                    arguments: serde_json::json!({ "command": "echo hi" }),
                    call_id: Some("e1".to_string()),
                }],
            }],
        },
    )))?;
    Ok(reg)
}

struct EchoSubAgent {
    subagent_type: String,
    description: String,
}

impl EchoSubAgent {
    fn new(subagent_type: &str, description: &str) -> Self {
        Self {
            subagent_type: subagent_type.to_string(),
            description: description.to_string(),
        }
    }
}

#[async_trait]
impl CompiledSubAgent for EchoSubAgent {
    fn subagent_type(&self) -> &str {
        &self.subagent_type
    }

    fn description(&self) -> &str {
        &self.description
    }

    async fn run(&self, req: SubAgentRunRequest) -> anyhow::Result<SubAgentRunOutput> {
        let mut keys: Vec<String> = req.state.extra.keys().cloned().collect();
        keys.sort();
        let first = req.messages.first().cloned();
        let saw_secret = req
            .messages
            .iter()
            .any(|m| m.content.contains("SECRET_IN_MAIN"));
        let payload = serde_json::json!({
            "messages_len": req.messages.len(),
            "first_message": first.map(|m| serde_json::json!({"role": m.role, "content": m.content })),
            "state_extra_keys": keys,
            "saw_secret_in_messages": saw_secret
        });
        Ok(SubAgentRunOutput {
            final_text: serde_json::to_string(&payload)?,
            state: req.state,
        })
    }
}

struct FixedTextSubAgent {
    subagent_type: String,
    description: String,
    text: String,
}

impl FixedTextSubAgent {
    fn new(subagent_type: &str, description: &str, text: &str) -> Self {
        Self {
            subagent_type: subagent_type.to_string(),
            description: description.to_string(),
            text: text.to_string(),
        }
    }
}

#[async_trait]
impl CompiledSubAgent for FixedTextSubAgent {
    fn subagent_type(&self) -> &str {
        &self.subagent_type
    }

    fn description(&self) -> &str {
        &self.description
    }

    async fn run(&self, req: SubAgentRunRequest) -> anyhow::Result<SubAgentRunOutput> {
        Ok(SubAgentRunOutput {
            final_text: self.text.clone(),
            state: req.state,
        })
    }
}

struct ExtraStateSubAgent {
    subagent_type: String,
    description: String,
}

impl ExtraStateSubAgent {
    fn new(subagent_type: &str, description: &str) -> Self {
        Self {
            subagent_type: subagent_type.to_string(),
            description: description.to_string(),
        }
    }
}

#[async_trait]
impl CompiledSubAgent for ExtraStateSubAgent {
    fn subagent_type(&self) -> &str {
        &self.subagent_type
    }

    fn description(&self) -> &str {
        &self.description
    }

    async fn run(&self, mut req: SubAgentRunRequest) -> anyhow::Result<SubAgentRunOutput> {
        req.state
            .extra
            .insert("allowed_key".to_string(), serde_json::json!({"x": 1}));
        req.state
            .extra
            .insert("todos".to_string(), serde_json::json!([1, 2, 3]));
        req.state
            .extra
            .insert("memory_contents".to_string(), serde_json::json!("MEM"));
        req.state
            .extra
            .insert("skills_metadata".to_string(), serde_json::json!({"k":"v"}));
        req.state.extra.insert(
            "structured_response".to_string(),
            serde_json::json!({"ok": true}),
        );
        req.state
            .extra
            .insert("messages".to_string(), serde_json::json!(["bad"]));
        Ok(SubAgentRunOutput {
            final_text: "OK".to_string(),
            state: req.state,
        })
    }
}

struct BrokenSubAgent {
    subagent_type: String,
    description: String,
}

impl BrokenSubAgent {
    fn new(subagent_type: &str, description: &str) -> Self {
        Self {
            subagent_type: subagent_type.to_string(),
            description: description.to_string(),
        }
    }
}

#[async_trait]
impl CompiledSubAgent for BrokenSubAgent {
    fn subagent_type(&self) -> &str {
        &self.subagent_type
    }

    fn description(&self) -> &str {
        &self.description
    }

    async fn run(&self, req: SubAgentRunRequest) -> anyhow::Result<SubAgentRunOutput> {
        Ok(SubAgentRunOutput {
            final_text: String::new(),
            state: req.state,
        })
    }
}

struct RuntimeMockSubAgent {
    subagent_type: String,
    description: String,
    script: Arc<MockScript>,
    output_mode: RuntimeMockOutputMode,
}

#[derive(Clone, Copy)]
enum RuntimeMockOutputMode {
    FinalText,
    LastToolError,
}

fn runtime_mock_subagent(
    subagent_type: &str,
    description: &str,
    script: MockScript,
) -> RuntimeMockSubAgent {
    RuntimeMockSubAgent {
        subagent_type: subagent_type.to_string(),
        description: description.to_string(),
        script: Arc::new(script),
        output_mode: RuntimeMockOutputMode::FinalText,
    }
}

fn runtime_last_tool_error_subagent(
    subagent_type: &str,
    description: &str,
    script: MockScript,
) -> RuntimeMockSubAgent {
    RuntimeMockSubAgent {
        subagent_type: subagent_type.to_string(),
        description: description.to_string(),
        script: Arc::new(script),
        output_mode: RuntimeMockOutputMode::LastToolError,
    }
}

#[async_trait]
impl CompiledSubAgent for RuntimeMockSubAgent {
    fn subagent_type(&self) -> &str {
        &self.subagent_type
    }

    fn description(&self) -> &str {
        &self.description
    }

    async fn run(&self, req: SubAgentRunRequest) -> anyhow::Result<SubAgentRunOutput> {
        let provider: Arc<dyn crate::provider::AgentProvider> =
            Arc::new(MockProvider::from_script((*self.script).clone()));
        let runtime = SimpleRuntime::new(
            req.agent,
            provider,
            Vec::new(),
            crate::runtime::simple::SimpleRuntimeOptions {
                config: RuntimeConfig {
                    max_steps: 8,
                    provider_timeout_ms: 1000,
                },
                approval: req.approval,
                audit: req.audit,
                root: req.root,
                mode: req.mode,
            },
        )
        .with_runtime_middlewares(req.runtime_middlewares)
        .with_initial_state(req.state)
        .with_task_depth(req.task_depth);

        let out = runtime.run(req.messages).await;
        let text = match self.output_mode {
            RuntimeMockOutputMode::FinalText => out.final_text,
            RuntimeMockOutputMode::LastToolError => {
                if let Some(err) = out.tool_results.iter().rev().find_map(|r| r.error.clone()) {
                    err
                } else if let Some(error) = out.error.as_ref() {
                    format!("runtime_error: {}: {}", error.code, error.message)
                } else if out.status == RunStatus::Interrupted {
                    out.interrupts
                        .first()
                        .map(|i| {
                            format!(
                                "runtime_interrupted: tool={} tool_call_id={}",
                                i.tool_name, i.tool_call_id
                            )
                        })
                        .unwrap_or_else(|| "runtime_interrupted".to_string())
                } else {
                    "no_error".to_string()
                }
            }
        };

        Ok(SubAgentRunOutput {
            final_text: text,
            state: out.state,
        })
    }
}
