use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::skills::SkillSpec;
use crate::state::AgentState;
use crate::types::{ContentBlock, Message};

fn default_structured_output_strict() -> bool {
    true
}

/// Selects how the provider should expose tools to the model.
///
/// This trait-facing enum is intended to remain provider-agnostic so each
/// provider can map it to its native tool-choice surface.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
    Required,
    Named {
        name: String,
    },
}

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
    AssistantMessageWithToolCalls {
        text: String,
        calls: Vec<ProviderToolCall>,
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

/// Supplemental assistant-message fields that should be preserved in history
/// without changing runtime control flow semantics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssistantMessageMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_blocks: Option<Vec<ContentBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl AssistantMessageMetadata {
    pub fn is_empty(&self) -> bool {
        self.content_blocks.as_ref().map_or(true, Vec::is_empty) && self.reasoning_content.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderStepOutput {
    pub step: ProviderStep,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_metadata: Option<AssistantMessageMetadata>,
}

impl ProviderStepOutput {
    pub fn new(step: ProviderStep) -> Self {
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

impl From<ProviderStep> for ProviderStepOutput {
    fn from(step: ProviderStep) -> Self {
        Self::new(step)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StructuredOutputSpec {
    pub name: String,
    pub schema: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_structured_output_strict")]
    pub strict: bool,
}

impl StructuredOutputSpec {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.name.trim().is_empty() {
            anyhow::bail!("structured_output_invalid_name");
        }
        if !self.schema.is_object() {
            anyhow::bail!("structured_output_invalid_schema");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tool_specs: Vec<crate::runtime::ToolSpec>,
    #[serde(default)]
    pub tool_choice: ToolChoice,
    #[serde(default)]
    pub skills: Vec<SkillSpec>,
    #[serde(default)]
    pub state: AgentState,
    #[serde(default)]
    pub last_tool_results: Vec<crate::runtime::ToolResultRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<StructuredOutputSpec>,
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

    async fn step_output(&self, req: ProviderRequest) -> anyhow::Result<ProviderStepOutput> {
        Ok(self.step(req).await?.into())
    }

    async fn step_with_collector(
        &self,
        req: ProviderRequest,
        _collector: &mut dyn ProviderEventCollector,
    ) -> anyhow::Result<ProviderStep> {
        Ok(self.step_output(req).await?.step)
    }

    async fn step_output_with_collector(
        &self,
        req: ProviderRequest,
        _collector: &mut dyn ProviderEventCollector,
    ) -> anyhow::Result<ProviderStepOutput> {
        self.step_output(req).await
    }
}
