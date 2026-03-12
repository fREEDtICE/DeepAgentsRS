use crate::provider::protocol::{AgentStep, AgentToolCall};
use crate::runtime::protocol::RuntimeMiddleware;
use crate::runtime::tool_compat::normalize_messages;
use crate::state::AgentState;
use crate::types::{Message, ToolCall};

/// 将 tool_call_id 规范化为“安全可携带”的形式。
///
/// 目的：
/// - provider/上游可能产生包含空格、特殊字符、超长等不稳定/不兼容的 id
/// - 这里将其映射到 `[A-Za-z0-9_.-]`，并限制最大长度，避免在协议/日志/下游解析中出问题
/// - 对连续 `..` 做处理，避免某些下游把它当作路径语义或触发边界行为
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

/// 修补“悬空的 tool call”：存在 tool_calls，但后续缺少对应的 tool role 消息。
///
/// 背景：
/// - 某些模型/代理会输出 tool_calls，但因为取消、崩溃、截断等原因没有产生 tool result
/// - 不同 provider 对这种不一致容忍度不同，可能导致后续对话无法继续
///
/// 策略：
/// - 对每个缺失结果的 tool call，追加一条 role=tool 的“patched”消息，显式声明取消/缺失
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
            // 在当前位置之后查找是否已经存在对应的 tool 结果消息。
            let exists = messages
                .iter()
                .skip(i + 1)
                .take(n - (i + 1))
                .any(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some(tc.id.as_str()));
            if exists {
                continue;
            }
            // 缺失结果：追加一个 tool role 的“patched”占位输出，保持消息序列一致性。
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
                content_blocks: None,
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: Some(tc.id),
                name: Some(tc.name),
                status: Some("patched".to_string()),
            });
        }
    }
    out
}

/// 规范化 provider 侧产出的 tool calls，确保下游可以稳定处理。
///
/// 修复点：
/// - tool_name 为空：填充为 "unknown"
/// - call_id 缺失：生成递增 id（call-<n>）
/// - arguments 不是对象：将 null 变成 `{}`；若为字符串且可解析为对象则转换
pub fn normalize_provider_tool_calls(
    calls: Vec<AgentToolCall>,
    next_call_id: &mut u64,
) -> Vec<AgentToolCall> {
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
            serde_json::Value::String(s) => match serde_json::from_str::<serde_json::Value>(s) {
                Ok(v) if v.is_object() => call.arguments = v,
                _ => {}
            },
            _ => {}
        }
        out.push(call);
    }
    out
}

/// 运行时中间件：在进入运行/每个 provider step 前，修补悬空 tool calls；
/// 并在 provider step 输出中规范化 tool call 结构。
pub struct PatchToolCallsMiddleware;

impl PatchToolCallsMiddleware {
    /// 创建中间件实例（无配置）。
    pub fn new() -> Self {
        Self
    }
}

impl Default for PatchToolCallsMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl RuntimeMiddleware for PatchToolCallsMiddleware {
    async fn before_run(
        &self,
        messages: Vec<Message>,
        _state: &mut AgentState,
    ) -> anyhow::Result<Vec<Message>> {
        // 先做消息兼容性规范化，再补齐缺失的 tool result。
        let messages = normalize_messages(messages);
        Ok(patch_dangling_tool_calls(messages))
    }

    async fn before_provider_step(
        &self,
        messages: Vec<Message>,
        _state: &mut AgentState,
    ) -> anyhow::Result<Vec<Message>> {
        // 兜底：即使 before_run 未生效或消息被改写，也确保 provider step 输入一致。
        let messages = normalize_messages(messages);
        Ok(patch_dangling_tool_calls(messages))
    }

    async fn patch_provider_step(
        &self,
        step: AgentStep,
        next_call_id: &mut u64,
    ) -> anyhow::Result<AgentStep> {
        match step {
            AgentStep::AssistantMessageWithToolCalls { text, calls } => {
                // 规范化 tool calls，确保 id/name/arguments 满足协议约束。
                let calls = normalize_provider_tool_calls(calls, next_call_id);
                Ok(AgentStep::AssistantMessageWithToolCalls { text, calls })
            }
            AgentStep::ToolCalls { calls } => {
                // 同上：ToolCalls 形态也需要统一规范化。
                let calls = normalize_provider_tool_calls(calls, next_call_id);
                Ok(AgentStep::ToolCalls { calls })
            }
            other => Ok(other),
        }
    }
}

/// 将 provider 侧 tool call 转换为运行时通用 `ToolCall`（用于对话消息中的 tool_calls 字段）。
pub fn tool_calls_from_provider_calls(calls: &[AgentToolCall]) -> Vec<ToolCall> {
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
