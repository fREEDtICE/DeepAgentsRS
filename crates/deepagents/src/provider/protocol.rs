use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::skills::SkillSpec;
use crate::state::AgentState;
use crate::types::Message;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderToolCall {
    #[serde(alias = "name", alias = "tool")]
    pub tool_name: String,
    #[serde(default, alias = "args", alias = "input")]
    pub arguments: serde_json::Value,
    #[serde(
        skip_serializing_if = "Option::is_none",
        alias = "id",
        alias = "tool_call_id",
        alias = "tool_use_id",
        alias = "toolUseId"
    )]
    pub call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderStep {
    AssistantMessage {
        text: String,
    },
    FinalText {
        text: String,
    },
    ToolCalls {
        calls: Vec<ProviderToolCall>,
    },
    SkillCall {
        name: String,
        #[serde(default)]
        input: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
    },
    Error {
        error: ProviderError,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tool_specs: Vec<crate::runtime::ToolSpec>,
    #[serde(default)]
    pub skills: Vec<SkillSpec>,
    #[serde(default)]
    pub state: AgentState,
    #[serde(default)]
    pub last_tool_results: Vec<crate::runtime::ToolResultRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderEvent {
    AssistantTextDelta {
        text: String,
    },
    ToolCallArgsDelta {
        tool_call_id: String,
        delta: String,
    },
    Usage {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        total_tokens: Option<u64>,
    },
}

#[async_trait]
pub trait ProviderEventCollector: Send {
    async fn emit(&mut self, event: ProviderEvent) -> anyhow::Result<()>;
}

#[derive(Debug, Default)]
pub struct VecProviderEventCollector {
    events: Vec<ProviderEvent>,
}

impl VecProviderEventCollector {
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    pub fn into_events(self) -> Vec<ProviderEvent> {
        self.events
    }
}

#[async_trait]
impl ProviderEventCollector for VecProviderEventCollector {
    async fn emit(&mut self, event: ProviderEvent) -> anyhow::Result<()> {
        self.events.push(event);
        Ok(())
    }
}

#[async_trait]
pub trait Provider: Send + Sync {
    async fn step(&self, req: ProviderRequest) -> anyhow::Result<ProviderStep>;

    async fn step_with_collector(
        &self,
        req: ProviderRequest,
        _collector: &mut dyn ProviderEventCollector,
    ) -> anyhow::Result<ProviderStep> {
        self.step(req).await
    }
}
