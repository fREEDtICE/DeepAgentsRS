use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::provider::ProviderStep;
use crate::runtime::{HitlInterrupt, RunStatus};
use crate::state::AgentState;
use crate::types::Message;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStepKind {
    AssistantMessage,
    FinalText,
    ToolCalls,
    SkillCall,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageSummary {
    pub role: String,
    pub content_preview: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_call_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    RunStarted {
        resumed_from_interrupt: bool,
    },
    ModelRequestBuilt {
        step_index: usize,
        tool_names: Vec<String>,
        skills: Vec<String>,
        message_count: usize,
        message_summary: Vec<MessageSummary>,
    },
    ProviderStepReceived {
        step_index: usize,
        step_type: ProviderStepKind,
    },
    AssistantTextDelta {
        step_index: usize,
        text: String,
    },
    AssistantMessage {
        step_index: usize,
        message: Message,
    },
    ToolCallStarted {
        step_index: usize,
        tool_name: String,
        tool_call_id: String,
        arguments_preview: Value,
    },
    ToolCallArgsDelta {
        step_index: usize,
        tool_call_id: String,
        delta: String,
    },
    ToolCallFinished {
        step_index: usize,
        tool_name: String,
        tool_call_id: String,
        output_preview: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    ToolMessageAppended {
        step_index: usize,
        tool_call_id: String,
        content_preview: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    StateUpdated {
        step_index: usize,
        updated_keys: Vec<String>,
    },
    Interrupt {
        step_index: usize,
        interrupt: HitlInterrupt,
    },
    UsageReported {
        step_index: usize,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        total_tokens: Option<u64>,
    },
    RunFinished {
        status: RunStatus,
        reason: String,
        final_text: String,
        step_count: usize,
        tool_call_count: usize,
        tool_error_count: usize,
    },
}

#[async_trait]
pub trait RunEventSink: Send {
    async fn emit(&mut self, event: RunEvent) -> Result<()>;
}

pub struct NoopRunEventSink;

#[async_trait]
impl RunEventSink for NoopRunEventSink {
    async fn emit(&mut self, _event: RunEvent) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct VecRunEventSink {
    events: Vec<RunEvent>,
}

impl VecRunEventSink {
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    pub fn events(&self) -> &[RunEvent] {
        &self.events
    }

    pub fn into_events(self) -> Vec<RunEvent> {
        self.events
    }
}

#[async_trait]
impl RunEventSink for VecRunEventSink {
    async fn emit(&mut self, event: RunEvent) -> Result<()> {
        self.events.push(event);
        Ok(())
    }
}

pub fn provider_step_kind(step: &ProviderStep) -> ProviderStepKind {
    match step {
        ProviderStep::AssistantMessage { .. } => ProviderStepKind::AssistantMessage,
        ProviderStep::FinalText { .. } => ProviderStepKind::FinalText,
        ProviderStep::ToolCalls { .. } => ProviderStepKind::ToolCalls,
        ProviderStep::SkillCall { .. } => ProviderStepKind::SkillCall,
        ProviderStep::Error { .. } => ProviderStepKind::Error,
    }
}

pub fn preview_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    format!("{head}...")
}

pub fn preview_json(value: &Value) -> Value {
    const MAX_KEYS: usize = 8;
    const MAX_TEXT_CHARS: usize = 240;

    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (idx, (k, v)) in map.iter().enumerate() {
                if idx >= MAX_KEYS {
                    out.insert("_truncated".to_string(), Value::Bool(true));
                    break;
                }
                out.insert(k.clone(), preview_json(v));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().take(4).map(preview_json).collect()),
        Value::String(s) => Value::String(preview_text(s, MAX_TEXT_CHARS)),
        _ => value.clone(),
    }
}

pub fn summarize_messages(messages: &[Message]) -> Vec<MessageSummary> {
    messages
        .iter()
        .map(|message| MessageSummary {
            role: message.role.clone(),
            content_preview: preview_text(&message.content, 120),
            tool_call_ids: message
                .tool_calls
                .as_ref()
                .map(|calls| calls.iter().map(|c| c.id.clone()).collect())
                .unwrap_or_default(),
            tool_call_id: message.tool_call_id.clone(),
            status: message.status.clone(),
        })
        .collect()
}

pub fn diff_state_keys(before: &AgentState, after: &AgentState) -> Vec<String> {
    let mut keys = Vec::new();

    if serde_json::to_value(&before.filesystem).ok() != serde_json::to_value(&after.filesystem).ok()
    {
        keys.push("filesystem".to_string());
    }
    if before.todos != after.todos {
        keys.push("todos".to_string());
    }
    if before.extra != after.extra {
        keys.push("extra".to_string());
    }

    keys
}
