use crate::provider::protocol::{ProviderStep, ProviderToolCall};
use crate::runtime::protocol::RuntimeMiddleware;
use crate::runtime::tool_compat::normalize_messages;
use crate::state::AgentState;
use crate::types::{Message, ToolCall};

pub fn sanitize_tool_call_id(id: &str) -> String {
    let mut out = String::with_capacity(id.len().min(128));
    let mut prev_dot = false;
    for ch in id.chars() {
        if out.len() >= 128 {
            break;
        }
        let ok = ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.';
        let mapped = if ok { ch } else { '_' };
        if mapped == '.' {
            if prev_dot {
                out.push('_');
                prev_dot = false;
            } else {
                out.push('.');
                prev_dot = true;
            }
        } else {
            prev_dot = false;
            out.push(mapped);
        }
    }
    if out.is_empty() {
        "_".to_string()
    } else {
        out
    }
}

pub fn patch_dangling_tool_calls(messages: Vec<Message>) -> Vec<Message> {
    if messages.is_empty() {
        return messages;
    }
    let mut out: Vec<Message> = Vec::new();
    let n = messages.len();
    for (i, msg) in messages.iter().cloned().enumerate() {
        let tool_calls = msg.tool_calls.clone();
        out.push(msg);
        if tool_calls.is_none() {
            continue;
        }
        let Some(tool_calls) = tool_calls else {
            continue;
        };
        for tc in tool_calls {
            let exists = messages
                .iter()
                .skip(i + 1)
                .take(n - (i + 1))
                .any(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some(tc.id.as_str()));
            if exists {
                continue;
            }
            let err = "tool_call_cancelled: missing tool result";
            let content = serde_json::json!({
                "tool_call_id": tc.id,
                "tool_name": tc.name,
                "status": "patched",
                "output": serde_json::Value::Null,
                "error": err,
                "content": format!(
                    "PATCHED_TOOL_CALL: Tool call {} ({}) was cancelled - missing tool result.",
                    tc.name, tc.id
                )
            })
            .to_string();
            out.push(Message {
                role: "tool".to_string(),
                content,
                tool_calls: None,
                tool_call_id: Some(tc.id),
                name: Some(tc.name),
                status: Some("patched".to_string()),
            });
        }
    }
    out
}

pub fn normalize_provider_tool_calls(
    calls: Vec<ProviderToolCall>,
    next_call_id: &mut u64,
) -> Vec<ProviderToolCall> {
    let mut out = Vec::new();
    for mut call in calls {
        if call.tool_name.trim().is_empty() {
            call.tool_name = "unknown".to_string();
        }
        if call.call_id.is_none() {
            let id = format!("call-{}", *next_call_id);
            *next_call_id += 1;
            call.call_id = Some(id);
        }
        match &call.arguments {
            serde_json::Value::Object(_) => {}
            serde_json::Value::Null => {
                call.arguments = serde_json::json!({});
            }
            serde_json::Value::String(s) => {
                match serde_json::from_str::<serde_json::Value>(s) {
                    Ok(v) if v.is_object() => call.arguments = v,
                    _ => {}
                }
            }
            _ => {}
        }
        out.push(call);
    }
    out
}

pub struct PatchToolCallsMiddleware;

impl PatchToolCallsMiddleware {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl RuntimeMiddleware for PatchToolCallsMiddleware {
    async fn before_run(&self, messages: Vec<Message>, _state: &mut AgentState) -> anyhow::Result<Vec<Message>> {
        let messages = normalize_messages(messages);
        Ok(patch_dangling_tool_calls(messages))
    }

    async fn patch_provider_step(
        &self,
        step: ProviderStep,
        next_call_id: &mut u64,
    ) -> anyhow::Result<ProviderStep> {
        match step {
            ProviderStep::ToolCalls { calls } => {
                let calls = normalize_provider_tool_calls(calls, next_call_id);
                Ok(ProviderStep::ToolCalls { calls })
            }
            other => Ok(other),
        }
    }
}

pub fn tool_calls_from_provider_calls(calls: &[ProviderToolCall]) -> Vec<ToolCall> {
    let mut out = Vec::new();
    for c in calls {
        let id = c.call_id.clone().unwrap_or_default();
        out.push(ToolCall {
            id,
            name: c.tool_name.clone(),
            arguments: c.arguments.clone(),
        });
    }
    out
}
