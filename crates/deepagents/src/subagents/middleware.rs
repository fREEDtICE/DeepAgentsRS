use std::sync::Arc;

use async_trait::async_trait;

use crate::runtime::{HandledToolCall, RuntimeMiddleware, ToolCallContext};
use crate::subagents::protocol::{
    filter_state_for_child, merge_child_state, SubAgentRunRequest, TaskInput,
};
use crate::subagents::SubAgentRegistry;
use crate::types::Message;

pub struct SubAgentMiddleware {
    registry: Arc<dyn SubAgentRegistry>,
    max_task_depth: usize,
    default_subagent_type: String,
}

impl SubAgentMiddleware {
    pub fn new(registry: Arc<dyn SubAgentRegistry>) -> Self {
        Self {
            registry,
            max_task_depth: 2,
            default_subagent_type: "general-purpose".to_string(),
        }
    }

    pub fn with_max_task_depth(mut self, max: usize) -> Self {
        self.max_task_depth = max.max(1);
        self
    }

    pub fn with_default_subagent_type(mut self, t: impl Into<String>) -> Self {
        self.default_subagent_type = t.into();
        self
    }
}

#[async_trait]
impl RuntimeMiddleware for SubAgentMiddleware {
    async fn handle_tool_call(
        &self,
        ctx: &mut ToolCallContext<'_>,
    ) -> anyhow::Result<Option<HandledToolCall>> {
        if ctx.tool_call.tool_name != "task" {
            return Ok(None);
        }

        let input: TaskInput = match serde_json::from_value(ctx.tool_call.arguments.clone()) {
            Ok(v) => v,
            Err(e) => {
                return Ok(Some(HandledToolCall {
                    output: serde_json::Value::Null,
                    error: Some(format!("invalid_tool_call: {e}")),
                }));
            }
        };

        if input.description.len() > 8192 {
            return Ok(Some(HandledToolCall {
                output: serde_json::Value::Null,
                error: Some("invalid_request: description too long".to_string()),
            }));
        }
        if input.subagent_type.len() > 128 {
            return Ok(Some(HandledToolCall {
                output: serde_json::Value::Null,
                error: Some("invalid_request: subagent_type too long".to_string()),
            }));
        }

        let t = input.subagent_type.trim();
        let subagent_type = if t.is_empty() {
            self.default_subagent_type.clone()
        } else {
            t.to_string()
        };

        if ctx.task_depth + 1 > self.max_task_depth {
            return Ok(Some(HandledToolCall {
                output: serde_json::Value::Null,
                error: Some(format!(
                    "max_task_depth_exceeded: max={}",
                    self.max_task_depth
                )),
            }));
        }

        let subagent = match self.registry.resolve(&subagent_type) {
            Some(s) => s,
            None => {
                return Ok(Some(HandledToolCall {
                    output: serde_json::Value::Null,
                    error: Some(format!("subagent_not_found: {subagent_type}")),
                }));
            }
        };

        let child_state = filter_state_for_child(ctx.state);
        let child_messages = vec![Message {
            role: "user".to_string(),
            content: input.description.clone(),
            content_blocks: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }];

        let req = SubAgentRunRequest {
            description: input.description,
            messages: child_messages,
            state: child_state,
            agent: ctx.agent.clone(),
            root: ctx.root.to_string(),
            mode: ctx.mode,
            approval: ctx.approval.cloned(),
            audit: ctx.audit.cloned(),
            runtime_middlewares: ctx.runtime_middlewares.to_vec(),
            task_depth: ctx.task_depth + 1,
        };

        let out = match subagent.run(req).await {
            Ok(v) => v,
            Err(e) => {
                return Ok(Some(HandledToolCall {
                    output: serde_json::Value::Null,
                    error: Some(format!("subagent_failed: {e}")),
                }));
            }
        };

        if out.final_text.trim().is_empty() {
            return Ok(Some(HandledToolCall {
                output: serde_json::Value::Null,
                error: Some("subagent_invalid_output: empty".to_string()),
            }));
        }

        merge_child_state(ctx.state, &out.state);

        Ok(Some(HandledToolCall {
            output: serde_json::json!({ "content": out.final_text }),
            error: None,
        }))
    }
}
