use std::convert::Infallible;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tokio_stream::Stream;

use crate::types::ContentBlock;

fn default_structured_output_strict() -> bool {
    true
}

fn default_tool_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {},
        "additionalProperties": true
    })
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
    Other(String),
}

impl ChatRole {
    pub fn as_str(&self) -> &str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
            Self::Other(role) => role.as_str(),
        }
    }
}

impl From<&str> for ChatRole {
    fn from(value: &str) -> Self {
        match value {
            "system" => Self::System,
            "user" => Self::User,
            "assistant" => Self::Assistant,
            "tool" => Self::Tool,
            other => Self::Other(other.to_string()),
        }
    }
}

impl From<String> for ChatRole {
    fn from(value: String) -> Self {
        match value.as_str() {
            "system" => Self::System,
            "user" => Self::User,
            "assistant" => Self::Assistant,
            "tool" => Self::Tool,
            _ => Self::Other(value),
        }
    }
}

impl FromStr for ChatRole {
    type Err = Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(value))
    }
}

impl Serialize for ChatRole {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ChatRole {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        ChatRole::from_str(&value).map_err(|_| D::Error::custom("invalid chat role"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    #[serde(default, alias = "args", alias = "input")]
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssistantMessageMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_blocks: Option<Vec<ContentBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl AssistantMessageMetadata {
    pub fn is_empty(&self) -> bool {
        self.content_blocks.as_ref().is_none_or(Vec::is_empty) && self.reasoning_content.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    #[serde(default = "default_tool_input_schema")]
    pub input_schema: serde_json::Value,
}

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultimodalInputRoles {
    #[serde(default, skip_serializing_if = "is_false")]
    pub user: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub assistant: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub tool: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub system: bool,
}

impl MultimodalInputRoles {
    pub const fn user_only() -> Self {
        Self {
            user: true,
            assistant: false,
            tool: false,
            system: false,
        }
    }

    pub const fn user_and_tool() -> Self {
        Self {
            user: true,
            assistant: false,
            tool: true,
            system: false,
        }
    }

    pub fn supports_role(&self, role: &ChatRole) -> bool {
        match role {
            ChatRole::User => self.user,
            ChatRole::Assistant => self.assistant,
            ChatRole::Tool => self.tool,
            ChatRole::System => self.system,
            ChatRole::Other(_) => false,
        }
    }

    pub fn is_empty(&self) -> bool {
        !(self.user || self.assistant || self.tool || self.system)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultimodalCapabilities {
    #[serde(default, skip_serializing_if = "MultimodalInputRoles::is_empty")]
    pub input_image_roles: MultimodalInputRoles,
    #[serde(default, skip_serializing_if = "is_false")]
    pub supports_output_image_blocks: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub supports_remote_image_urls: bool,
}

impl MultimodalCapabilities {
    pub const fn image_input_output(input_image_roles: MultimodalInputRoles) -> Self {
        Self {
            input_image_roles,
            supports_output_image_blocks: true,
            supports_remote_image_urls: true,
        }
    }

    pub fn is_disabled(&self) -> bool {
        self.input_image_roles.is_empty()
            && !self.supports_output_image_blocks
            && !self.supports_remote_image_urls
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmProviderCapabilities {
    pub supports_streaming: bool,
    pub supports_tool_calling: bool,
    pub reports_usage: bool,
    pub supports_structured_output: bool,
    pub supports_reasoning_content: bool,
    #[serde(default, skip_serializing_if = "MultimodalCapabilities::is_disabled")]
    pub multimodal: MultimodalCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionTool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolsPayload {
    #[default]
    None,
    FunctionTools {
        tools: Vec<FunctionTool>,
    },
    PromptGuided {
        instructions: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_blocks: Option<Vec<ContentBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "call_id",
        alias = "toolUseId",
        alias = "tool_use_id"
    )]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::new(ChatRole::System, content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new(ChatRole::User, content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(ChatRole::Assistant, content)
    }

    pub fn tool(content: impl Into<String>) -> Self {
        Self::new(ChatRole::Tool, content)
    }

    pub fn new(role: ChatRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            content_blocks: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub tool_specs: Vec<ToolSpec>,
    #[serde(default)]
    pub tool_choice: ToolChoice,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<StructuredOutputSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatResponse {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_metadata: Option<AssistantMessageMetadata>,
}

impl ChatResponse {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tool_calls: Vec::new(),
            usage: None,
            assistant_metadata: None,
        }
    }

    pub fn with_tool_calls(mut self, tool_calls: Vec<ToolCall>) -> Self {
        self.tool_calls = tool_calls;
        self
    }

    pub fn with_usage(mut self, usage: TokenUsage) -> Self {
        self.usage = Some(usage);
        self
    }

    pub fn with_assistant_metadata(mut self, metadata: AssistantMessageMetadata) -> Self {
        if !metadata.is_empty() {
            self.assistant_metadata = Some(metadata);
        }
        self
    }

    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

pub type LlmEventStream = Pin<Box<dyn Stream<Item = anyhow::Result<LlmEvent>> + Send + 'static>>;

#[derive(Debug, Clone)]
pub enum LlmEvent {
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
    FinalResponse {
        response: ChatResponse,
    },
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn capabilities(&self) -> LlmProviderCapabilities {
        LlmProviderCapabilities::default()
    }

    fn convert_tools(&self, tool_specs: &[ToolSpec]) -> anyhow::Result<ToolsPayload> {
        let _ = tool_specs;
        Ok(ToolsPayload::None)
    }

    fn prompt_cache_payload(
        &self,
        req: &ChatRequest,
        tools_payload: &ToolsPayload,
    ) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::json!({
            "messages": req.messages,
            "tool_choice": req.tool_choice,
            "structured_output": req.structured_output,
            "tools_payload": tools_payload,
        }))
    }

    async fn chat(&self, req: ChatRequest) -> anyhow::Result<ChatResponse>;

    async fn stream_chat(&self, req: ChatRequest) -> anyhow::Result<LlmEventStream>;
}

#[derive(Clone)]
pub struct MockLlmProvider {
    events: Arc<Vec<LlmEvent>>,
    capabilities: LlmProviderCapabilities,
}

impl MockLlmProvider {
    pub fn new(events: Vec<LlmEvent>) -> Self {
        let reports_usage = events.iter().any(|event| match event {
            LlmEvent::Usage { .. } => true,
            LlmEvent::FinalResponse { response } => response.usage.is_some(),
            _ => false,
        });
        let supports_tool_calling = events.iter().any(|event| match event {
            LlmEvent::ToolCallArgsDelta { .. } => true,
            LlmEvent::FinalResponse { response } => response.has_tool_calls(),
            _ => false,
        });
        let capabilities = LlmProviderCapabilities {
            supports_streaming: true,
            reports_usage,
            supports_tool_calling,
            ..Default::default()
        };
        Self {
            events: Arc::new(events),
            capabilities,
        }
    }

    pub fn with_capabilities(mut self, capabilities: LlmProviderCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }
}

#[async_trait]
impl LlmProvider for MockLlmProvider {
    fn capabilities(&self) -> LlmProviderCapabilities {
        self.capabilities
    }

    async fn chat(&self, _req: ChatRequest) -> anyhow::Result<ChatResponse> {
        self.events
            .iter()
            .find_map(|event| match event {
                LlmEvent::FinalResponse { response } => Some(response.clone()),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("llm_stream_missing_final_response"))
    }

    async fn stream_chat(&self, _req: ChatRequest) -> anyhow::Result<LlmEventStream> {
        let events = self.events.as_ref().clone();
        Ok(Box::pin(tokio_stream::iter(
            events.into_iter().map(Ok::<_, anyhow::Error>),
        )))
    }
}

pub fn final_text_step(text: &str) -> LlmEvent {
    LlmEvent::FinalResponse {
        response: ChatResponse::new(text),
    }
}

pub fn tool_calls_step(calls: Vec<ToolCall>) -> LlmEvent {
    LlmEvent::FinalResponse {
        response: ChatResponse::new("").with_tool_calls(calls),
    }
}
