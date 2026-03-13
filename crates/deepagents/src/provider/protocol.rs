use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::llm::{AssistantMessageMetadata, StructuredOutputSpec, ToolChoice};
use crate::provider::prompt_cache::{
    PromptCachePlan, ProviderPromptCacheHint, ProviderPromptCacheObservation,
};
use crate::state::AgentState;
use crate::types::Message;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolCall {
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
pub enum AgentStep {
    AssistantMessage {
        text: String,
    },
    AssistantMessageWithToolCalls {
        text: String,
        calls: Vec<AgentToolCall>,
    },
    FinalText {
        text: String,
    },
    ToolCalls {
        calls: Vec<AgentToolCall>,
    },
    Error {
        error: AgentProviderError,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProviderError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStepOutput {
    pub step: AgentStep,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_metadata: Option<AssistantMessageMetadata>,
}

impl AgentStepOutput {
    pub fn new(step: AgentStep) -> Self {
        Self {
            step,
            assistant_metadata: None,
        }
    }

    pub fn with_assistant_metadata(mut self, metadata: AssistantMessageMetadata) -> Self {
        if !metadata.is_empty() {
            self.assistant_metadata = Some(metadata);
        }
        self
    }
}

impl From<AgentStep> for AgentStepOutput {
    fn from(step: AgentStep) -> Self {
        Self::new(step)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProviderRequest {
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tool_specs: Vec<crate::runtime::ToolSpec>,
    #[serde(default)]
    pub tool_choice: ToolChoice,
    #[serde(default)]
    pub state: AgentState,
    #[serde(default)]
    pub last_tool_results: Vec<crate::runtime::ToolResultRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<StructuredOutputSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentProviderEvent {
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
pub trait AgentProviderEventCollector: Send {
    async fn emit(&mut self, event: AgentProviderEvent) -> anyhow::Result<()>;
}

#[derive(Debug, Default)]
pub struct VecAgentProviderEventCollector {
    events: Vec<AgentProviderEvent>,
}

impl VecAgentProviderEventCollector {
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    pub fn into_events(self) -> Vec<AgentProviderEvent> {
        self.events
    }
}

#[async_trait]
impl AgentProviderEventCollector for VecAgentProviderEventCollector {
    async fn emit(&mut self, event: AgentProviderEvent) -> anyhow::Result<()> {
        self.events.push(event);
        Ok(())
    }
}

#[async_trait]
pub trait AgentProvider: Send + Sync {
    async fn step(&self, req: AgentProviderRequest) -> anyhow::Result<AgentStep>;

    async fn step_output(&self, req: AgentProviderRequest) -> anyhow::Result<AgentStepOutput> {
        Ok(self.step(req).await?.into())
    }

    async fn step_with_collector(
        &self,
        req: AgentProviderRequest,
        _collector: &mut dyn AgentProviderEventCollector,
    ) -> anyhow::Result<AgentStep> {
        Ok(self.step_output(req).await?.step)
    }

    async fn step_output_with_collector(
        &self,
        req: AgentProviderRequest,
        _collector: &mut dyn AgentProviderEventCollector,
    ) -> anyhow::Result<AgentStepOutput> {
        self.step_output(req).await
    }

    fn prompt_cache_plan(&self, req: &AgentProviderRequest) -> anyhow::Result<PromptCachePlan> {
        Ok(PromptCachePlan::from_agent_request(req))
    }

    fn apply_prompt_cache_hint(
        &self,
        req: AgentProviderRequest,
        _hint: &ProviderPromptCacheHint,
    ) -> AgentProviderRequest {
        req
    }

    fn observe_prompt_cache_result(
        &self,
        _output: &AgentStepOutput,
        _events: &[AgentProviderEvent],
    ) -> Option<ProviderPromptCacheObservation> {
        None
    }
}

pub use AgentProvider as Provider;
pub use AgentProviderError as ProviderError;
pub use AgentProviderEvent as ProviderEvent;
pub use AgentProviderEventCollector as ProviderEventCollector;
pub use AgentProviderRequest as ProviderRequest;
pub use AgentStep as ProviderStep;
pub use AgentStepOutput as ProviderStepOutput;
pub use AgentToolCall as ProviderToolCall;
pub use VecAgentProviderEventCollector as VecProviderEventCollector;
