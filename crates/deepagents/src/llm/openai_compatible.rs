use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use async_stream::try_stream;
use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde::{Deserialize, Serialize};
use tokio_stream::Stream;
use tokio_stream::StreamExt;

use crate::llm::common::{
    build_data_url, finalize_assistant_text, openai_chat_completions_url,
    parse_image_content_block, parse_sse_json_response, send_openai_compatible_request,
};
use crate::llm::{
    AssistantMessageMetadata, ChatMessage, ChatRequest, ChatResponse, ChatRole, FunctionTool,
    LlmEvent, LlmEventStream, LlmProvider, LlmProviderCapabilities, MultimodalCapabilities,
    MultimodalInputRoles, TokenUsage, ToolCall as LlmToolCall, ToolChoice, ToolSpec, ToolsPayload,
};
use crate::types::{fallback_text_for_content_blocks, ContentBlock};

#[derive(Debug, Clone)]
pub enum OpenAiAuthStyle {
    Bearer,
    XApiKey,
    Custom(String),
}

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleConfig {
    pub model: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub auth_style: OpenAiAuthStyle,
    pub user_agent: Option<String>,
    pub merge_system_into_user: bool,
    pub multimodal_input_roles: MultimodalInputRoles,
}

impl OpenAiCompatibleConfig {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: None,
            auth_style: OpenAiAuthStyle::Bearer,
            user_agent: None,
            merge_system_into_user: false,
            multimodal_input_roles: MultimodalInputRoles::user_only(),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    pub fn with_auth_style(mut self, auth_style: OpenAiAuthStyle) -> Self {
        self.auth_style = auth_style;
        self
    }

    pub fn with_user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = Some(user_agent.into());
        self
    }

    pub fn with_merge_system_into_user(mut self, enabled: bool) -> Self {
        self.merge_system_into_user = enabled;
        self
    }

    pub fn with_multimodal_input_roles(mut self, roles: MultimodalInputRoles) -> Self {
        self.multimodal_input_roles = roles;
        self
    }

    pub(crate) fn chat_completions_url(&self) -> String {
        openai_chat_completions_url(&self.base_url)
    }

    pub(crate) fn request_headers(&self) -> anyhow::Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        if let Some(api_key) = self.api_key.as_deref() {
            let (name, value) = match &self.auth_style {
                OpenAiAuthStyle::Bearer => (
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {api_key}"))?,
                ),
                OpenAiAuthStyle::XApiKey => (
                    HeaderName::from_static("x-api-key"),
                    HeaderValue::from_str(api_key)?,
                ),
                OpenAiAuthStyle::Custom(header) => (
                    HeaderName::from_bytes(header.as_bytes())?,
                    HeaderValue::from_str(api_key)?,
                ),
            };
            headers.insert(name, value);
        }
        if let Some(user_agent) = self.user_agent.as_deref() {
            headers.insert(USER_AGENT, HeaderValue::from_str(user_agent)?);
        }
        Ok(headers)
    }
}

pub struct OpenAiCompatibleProvider {
    config: OpenAiCompatibleConfig,
    transport: Arc<dyn OpenAiCompatibleTransport>,
}

impl OpenAiCompatibleProvider {
    pub fn new(
        config: OpenAiCompatibleConfig,
        transport: Arc<dyn OpenAiCompatibleTransport>,
    ) -> Self {
        Self { config, transport }
    }

    fn llm_capabilities(&self) -> LlmProviderCapabilities {
        LlmProviderCapabilities {
            supports_streaming: true,
            supports_tool_calling: true,
            reports_usage: true,
            supports_structured_output: true,
            supports_reasoning_content: true,
            multimodal: MultimodalCapabilities::image_input_output(
                self.config.multimodal_input_roles,
            ),
        }
    }

    fn prepare_request(&self, mut req: ChatRequest) -> ChatRequest {
        if self.config.merge_system_into_user {
            req.messages = flatten_system_messages(&req.messages);
        }
        req
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    fn capabilities(&self) -> LlmProviderCapabilities {
        self.llm_capabilities()
    }

    fn convert_tools(&self, tool_specs: &[ToolSpec]) -> anyhow::Result<ToolsPayload> {
        if tool_specs.is_empty() {
            return Ok(ToolsPayload::None);
        }

        let tools = tool_specs
            .iter()
            .map(|tool| FunctionTool {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.input_schema.clone(),
            })
            .collect();
        Ok(ToolsPayload::FunctionTools { tools })
    }

    fn prompt_cache_payload(
        &self,
        req: &ChatRequest,
        tools_payload: &ToolsPayload,
    ) -> anyhow::Result<serde_json::Value> {
        let req = self.prepare_request(req.clone());
        let request = build_chat_request(
            &self.config.model,
            &req,
            tools_payload,
            self.config.multimodal_input_roles,
            false,
        )?;
        serde_json::to_value(request).map_err(|err| anyhow::anyhow!(err))
    }

    async fn chat(&self, req: ChatRequest) -> anyhow::Result<ChatResponse> {
        let req = self.prepare_request(req);
        let tools_payload = self.convert_tools(&req.tool_specs)?;
        let request = build_chat_request(
            &self.config.model,
            &req,
            &tools_payload,
            self.config.multimodal_input_roles,
            false,
        )?;
        let response = self
            .transport
            .create_chat_completion(&self.config, request)
            .await?;
        parse_chat_response(response)
    }

    async fn stream_chat(&self, req: ChatRequest) -> anyhow::Result<LlmEventStream> {
        let req = self.prepare_request(req);
        let tools_payload = self.convert_tools(&req.tool_specs)?;
        let request = build_chat_request(
            &self.config.model,
            &req,
            &tools_payload,
            self.config.multimodal_input_roles,
            true,
        )?;
        let chunks = self
            .transport
            .stream_chat_completion(&self.config, request)
            .await?;
        Ok(Box::pin(stream_openai_chunks(chunks)))
    }
}

pub type OpenAiChunkStream =
    Pin<Box<dyn Stream<Item = anyhow::Result<OpenAiChatChunk>> + Send + 'static>>;

#[async_trait]
pub trait OpenAiCompatibleTransport: Send + Sync {
    async fn create_chat_completion(
        &self,
        config: &OpenAiCompatibleConfig,
        request: OpenAiChatRequest,
    ) -> anyhow::Result<OpenAiChatResponse>;

    async fn stream_chat_completion(
        &self,
        config: &OpenAiCompatibleConfig,
        request: OpenAiChatRequest,
    ) -> anyhow::Result<OpenAiChunkStream>;
}

#[derive(Clone)]
pub struct MockOpenAiTransport {
    response: Option<OpenAiChatResponse>,
    chunks: Vec<OpenAiChatChunk>,
}

impl MockOpenAiTransport {
    pub fn for_response(response: OpenAiChatResponse) -> Self {
        Self {
            response: Some(response),
            chunks: Vec::new(),
        }
    }

    pub fn for_chunks(chunks: Vec<OpenAiChatChunk>) -> Self {
        Self {
            response: None,
            chunks,
        }
    }
}

#[async_trait]
impl OpenAiCompatibleTransport for MockOpenAiTransport {
    async fn create_chat_completion(
        &self,
        _config: &OpenAiCompatibleConfig,
        _request: OpenAiChatRequest,
    ) -> anyhow::Result<OpenAiChatResponse> {
        self.response
            .clone()
            .ok_or_else(|| anyhow::anyhow!("mock_openai_missing_response"))
    }

    async fn stream_chat_completion(
        &self,
        _config: &OpenAiCompatibleConfig,
        _request: OpenAiChatRequest,
    ) -> anyhow::Result<OpenAiChunkStream> {
        Ok(Box::pin(tokio_stream::iter(
            self.chunks.clone().into_iter().map(Ok::<_, anyhow::Error>),
        )))
    }
}

#[derive(Clone, Default)]
pub struct ReqwestOpenAiTransport {
    client: reqwest::Client,
}

impl ReqwestOpenAiTransport {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }

    async fn send_request(
        &self,
        config: &OpenAiCompatibleConfig,
        request: OpenAiChatRequest,
        stream: bool,
    ) -> anyhow::Result<reqwest::Response> {
        let request_url = config.chat_completions_url();
        send_openai_compatible_request(
            &self.client,
            &request_url,
            &request,
            stream,
            config.request_headers()?,
            "openai_http_error",
        )
        .await
    }
}

#[async_trait]
impl OpenAiCompatibleTransport for ReqwestOpenAiTransport {
    async fn create_chat_completion(
        &self,
        config: &OpenAiCompatibleConfig,
        request: OpenAiChatRequest,
    ) -> anyhow::Result<OpenAiChatResponse> {
        let response = self.send_request(config, request, false).await?;
        Ok(response.json::<OpenAiChatResponse>().await?)
    }

    async fn stream_chat_completion(
        &self,
        config: &OpenAiCompatibleConfig,
        request: OpenAiChatRequest,
    ) -> anyhow::Result<OpenAiChunkStream> {
        let response = self.send_request(config, request, true).await?;
        Ok(Box::pin(parse_sse_json_response(response)))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAiChatRequest {
    pub model: String,
    pub messages: Vec<OpenAiMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OpenAiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<OpenAiToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<OpenAiResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAiMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<OpenAiMessageContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum OpenAiMessageContent {
    Text(String),
    Parts(Vec<OpenAiContentPart>),
}

impl From<String> for OpenAiMessageContent {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for OpenAiMessageContent {
    fn from(value: &str) -> Self {
        Self::Text(value.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAiContentPart {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<OpenAiImageUrl>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
}

impl OpenAiContentPart {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            kind: "text".to_string(),
            text: Some(text.into()),
            image_url: None,
            refusal: None,
        }
    }

    pub fn image_url(url: impl Into<String>) -> Self {
        Self {
            kind: "image_url".to_string(),
            text: None,
            image_url: Some(OpenAiImageUrl {
                url: url.into(),
                detail: None,
            }),
            refusal: None,
        }
    }

    pub fn refusal(refusal: impl Into<String>) -> Self {
        Self {
            kind: "refusal".to_string(),
            text: None,
            image_url: None,
            refusal: Some(refusal.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAiImageUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAiTool {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: OpenAiFunctionSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAiFunctionSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum OpenAiToolChoice {
    Mode(String),
    Named {
        #[serde(rename = "type")]
        kind: String,
        function: OpenAiToolChoiceFunction,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAiToolChoiceFunction {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAiResponseFormat {
    #[serde(rename = "type")]
    pub kind: String,
    pub json_schema: OpenAiJsonSchemaResponseFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAiJsonSchemaResponseFormat {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAiToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: OpenAiFunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAiFunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiChatResponse {
    #[serde(default)]
    pub choices: Vec<OpenAiChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<OpenAiUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiChoice {
    pub message: OpenAiMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiChatChunk {
    #[serde(default)]
    pub choices: Vec<OpenAiChunkChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<OpenAiUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiChunkChoice {
    pub delta: OpenAiDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OpenAiDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<OpenAiMessageContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAiToolCallDelta>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiToolCallDelta {
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<OpenAiFunctionCallDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiFunctionCallDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

fn build_chat_request(
    model: &str,
    req: &ChatRequest,
    tools_payload: &ToolsPayload,
    multimodal_input_roles: MultimodalInputRoles,
    stream: bool,
) -> anyhow::Result<OpenAiChatRequest> {
    let function_tools = match tools_payload {
        ToolsPayload::None => Vec::new(),
        ToolsPayload::FunctionTools { tools } => tools.clone(),
        ToolsPayload::PromptGuided { .. } => {
            anyhow::bail!("openai_prompt_guided_tools_unsupported")
        }
    };

    let tool_names = function_tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();

    Ok(OpenAiChatRequest {
        model: model.to_string(),
        messages: req
            .messages
            .iter()
            .map(|message| convert_message(message, multimodal_input_roles))
            .collect(),
        tools: function_tools
            .into_iter()
            .map(|tool| OpenAiTool {
                kind: "function".to_string(),
                function: OpenAiFunctionSpec {
                    name: tool.name,
                    description: tool.description,
                    parameters: tool.parameters,
                },
            })
            .collect(),
        tool_choice: convert_openai_tool_choice(&req.tool_choice, &tool_names)?,
        response_format: req
            .structured_output
            .as_ref()
            .map(convert_openai_response_format),
        stream: if stream { Some(true) } else { None },
    })
}

fn convert_openai_tool_choice(
    tool_choice: &ToolChoice,
    tool_names: &[String],
) -> anyhow::Result<Option<OpenAiToolChoice>> {
    let value = match tool_choice {
        ToolChoice::Auto => None,
        ToolChoice::None => Some(OpenAiToolChoice::Mode("none".to_string())),
        ToolChoice::Required => Some(OpenAiToolChoice::Mode("required".to_string())),
        ToolChoice::Named { name } => {
            if !tool_names.iter().any(|tool_name| tool_name == name) {
                return Err(anyhow::anyhow!("openai_unknown_tool_choice: {name}"));
            }
            Some(OpenAiToolChoice::Named {
                kind: "function".to_string(),
                function: OpenAiToolChoiceFunction { name: name.clone() },
            })
        }
    };
    Ok(value)
}

fn convert_openai_response_format(spec: &crate::llm::StructuredOutputSpec) -> OpenAiResponseFormat {
    OpenAiResponseFormat {
        kind: "json_schema".to_string(),
        json_schema: OpenAiJsonSchemaResponseFormat {
            name: spec.name.clone(),
            description: spec.description.clone(),
            schema: spec.schema.clone(),
            strict: Some(spec.strict),
        },
    }
}

fn flatten_system_messages(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    let system_content = messages
        .iter()
        .filter(|message| message.role == ChatRole::System)
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if system_content.is_empty() {
        return messages.to_vec();
    }
    let mut result = messages
        .iter()
        .filter(|message| message.role != ChatRole::System)
        .cloned()
        .collect::<Vec<_>>();
    if let Some(first_user) = result
        .iter_mut()
        .find(|message| message.role == ChatRole::User)
    {
        first_user.content = if first_user.content.is_empty() {
            system_content
        } else {
            format!("{system_content}\n\n{}", first_user.content)
        };
    } else {
        result.insert(0, ChatMessage::user(system_content));
    }
    result
}

fn convert_message(
    message: &ChatMessage,
    multimodal_input_roles: MultimodalInputRoles,
) -> OpenAiMessage {
    OpenAiMessage {
        role: message.role.as_str().to_string(),
        content: convert_openai_content(message, multimodal_input_roles),
        reasoning_content: message.reasoning_content.clone(),
        tool_calls: message.tool_calls.as_ref().map(|calls| {
            calls
                .iter()
                .map(|call| OpenAiToolCall {
                    id: call.id.clone(),
                    kind: "function".to_string(),
                    function: OpenAiFunctionCall {
                        name: call.name.clone(),
                        arguments: serde_json::to_string(&call.arguments)
                            .unwrap_or_else(|_| "{}".to_string()),
                    },
                })
                .collect()
        }),
        tool_call_id: message.tool_call_id.clone(),
    }
}

fn convert_openai_content(
    message: &ChatMessage,
    multimodal_input_roles: MultimodalInputRoles,
) -> Option<OpenAiMessageContent> {
    let blocks = message.content_blocks.as_deref().unwrap_or(&[]);
    if blocks.is_empty() {
        return fallback_openai_text_content(message);
    }

    if !multimodal_input_roles.supports_role(&message.role) {
        return fallback_openai_text_content(message)
            .or_else(|| fallback_text_for_content_blocks(blocks).map(OpenAiMessageContent::from));
    }

    let mut parts = Vec::new();
    if !message.content.is_empty() {
        parts.push(OpenAiContentPart::text(message.content.clone()));
    }
    for block in blocks {
        if let Some(image) = block.as_image_base64() {
            parts.push(OpenAiContentPart::image_url(build_data_url(
                image.mime_type,
                image.base64,
            )));
        } else if let Some(image) = block.as_image_url() {
            parts.push(OpenAiContentPart::image_url(image.url));
        }
    }

    if parts.is_empty() {
        fallback_openai_text_content(message)
            .or_else(|| fallback_text_for_content_blocks(blocks).map(OpenAiMessageContent::from))
    } else {
        Some(OpenAiMessageContent::Parts(parts))
    }
}

fn fallback_openai_text_content(message: &ChatMessage) -> Option<OpenAiMessageContent> {
    if !message.content.is_empty() {
        return Some(OpenAiMessageContent::from(message.content.clone()));
    }
    if let Some(fallback) = message
        .content_blocks
        .as_deref()
        .and_then(fallback_text_for_content_blocks)
    {
        return Some(OpenAiMessageContent::from(fallback));
    }
    if message.tool_calls.is_some() {
        return None;
    }
    Some(OpenAiMessageContent::from(String::new()))
}

pub fn parse_chat_response(response: OpenAiChatResponse) -> anyhow::Result<ChatResponse> {
    let usage = response.usage.as_ref().map(|usage| TokenUsage {
        input_tokens: usage.prompt_tokens,
        output_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
    });
    let Some(choice) = response.choices.into_iter().next() else {
        return Err(anyhow::anyhow!("openai_response_missing_choice"));
    };
    let mut parsed = parse_openai_message(choice.message)?;
    if let Some(usage) = usage {
        parsed = parsed.with_usage(usage);
    }
    Ok(parsed)
}

fn parse_openai_message(message: OpenAiMessage) -> anyhow::Result<ChatResponse> {
    let parsed_content = parse_openai_message_content(message.content);
    let mut text = strip_think_tags(&finalize_assistant_text(
        parsed_content.text,
        &parsed_content.content_blocks,
        parsed_content.saw_multimodal_content,
    ));
    let reasoning_content = message
        .reasoning_content
        .as_deref()
        .map(strip_think_tags)
        .filter(|value| !value.is_empty());
    if text.is_empty() {
        if let Some(reasoning) = reasoning_content.as_deref() {
            text = reasoning.to_string();
        }
    }
    let metadata = AssistantMessageMetadata {
        content_blocks: (!parsed_content.content_blocks.is_empty())
            .then_some(parsed_content.content_blocks.clone()),
        reasoning_content: reasoning_content.clone(),
    };
    if let Some(tool_calls) = message.tool_calls {
        let mut calls = Vec::with_capacity(tool_calls.len());
        for call in tool_calls {
            let arguments = if call.function.arguments.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&call.function.arguments)?
            };
            calls.push(LlmToolCall {
                id: call.id,
                name: call.function.name,
                arguments,
            });
        }
        return Ok(ChatResponse::new(text)
            .with_tool_calls(calls)
            .with_assistant_metadata(metadata));
    }

    Ok(ChatResponse::new(text).with_assistant_metadata(metadata))
}

fn stream_openai_chunks(
    chunks: OpenAiChunkStream,
) -> impl Stream<Item = anyhow::Result<LlmEvent>> + Send + 'static {
    try_stream! {
        let mut chunks = chunks;
        let mut state = StreamAssemblyState::default();

        while let Some(chunk) = chunks.next().await {
            let chunk = chunk?;
            if let Some(usage) = chunk.usage {
                yield LlmEvent::Usage {
                    input_tokens: usage.prompt_tokens,
                    output_tokens: usage.completion_tokens,
                    total_tokens: usage.total_tokens,
                };
            }

            for choice in chunk.choices {
                if let Some(text) = choice.delta.content {
                    let parsed_content = parse_openai_message_content(Some(text));
                    if !parsed_content.text.is_empty() {
                        state.text.push_str(&parsed_content.text);
                        yield LlmEvent::AssistantTextDelta {
                            text: parsed_content.text,
                        };
                    }
                    if !parsed_content.content_blocks.is_empty() {
                        state.content_blocks.extend(parsed_content.content_blocks);
                    }
                    state.saw_multimodal_content |= parsed_content.saw_multimodal_content;
                }
                if let Some(reasoning_content) = choice.delta.reasoning_content {
                    state.reasoning_content.push_str(&reasoning_content);
                }
                if let Some(tool_calls) = choice.delta.tool_calls {
                    for delta in tool_calls {
                        let entry = state.tool_calls.entry(delta.index).or_default();
                        if let Some(id) = delta.id {
                            let needs_flush = entry.id.is_none()
                                && !entry.arguments.is_empty()
                                && entry.emitted_args_len == 0;
                            entry.id = Some(id.clone());
                            if needs_flush {
                                yield LlmEvent::ToolCallArgsDelta {
                                    tool_call_id: id,
                                    delta: entry.arguments.clone(),
                                };
                                entry.emitted_args_len = entry.arguments.len();
                            }
                        }
                        if let Some(function) = delta.function {
                            if let Some(name) = function.name {
                                entry.name = Some(name);
                            }
                            if let Some(arguments) = function.arguments {
                                entry.arguments.push_str(&arguments);
                                if let Some(id) = entry.id.clone() {
                                    yield LlmEvent::ToolCallArgsDelta {
                                        tool_call_id: id,
                                        delta: arguments,
                                    };
                                    entry.emitted_args_len = entry.arguments.len();
                                }
                            }
                        }
                    }
                }
                if choice.finish_reason.is_some() {
                    state.finished = true;
                }
            }
        }

        if !state.finished {
            Err(anyhow::anyhow!("openai_stream_missing_finish"))?;
        }

        let calls = assemble_tool_calls(&state.tool_calls)?;
        let reasoning_content = strip_think_tags(&state.reasoning_content);
        let mut text = strip_think_tags(&finalize_assistant_text(
            state.text,
            &state.content_blocks,
            state.saw_multimodal_content,
        ));
        if text.is_empty() && !reasoning_content.is_empty() {
            text = reasoning_content.clone();
        }
        let response = ChatResponse::new(text)
        .with_tool_calls(calls)
        .with_assistant_metadata(AssistantMessageMetadata {
            content_blocks: (!state.content_blocks.is_empty()).then_some(state.content_blocks),
            reasoning_content: if reasoning_content.is_empty() {
                None
            } else {
                Some(reasoning_content)
            },
        });
        yield LlmEvent::FinalResponse { response };
    }
}

fn assemble_tool_calls(
    tool_calls: &BTreeMap<usize, PartialToolCall>,
) -> anyhow::Result<Vec<LlmToolCall>> {
    let mut out = Vec::with_capacity(tool_calls.len());
    for partial in tool_calls.values() {
        let id = partial
            .id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("openai_stream_tool_call_missing_id"))?;
        let name = partial
            .name
            .clone()
            .ok_or_else(|| anyhow::anyhow!("openai_stream_tool_call_missing_name"))?;
        let arguments = if partial.arguments.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&partial.arguments)?
        };
        out.push(LlmToolCall {
            id,
            name,
            arguments,
        });
    }
    Ok(out)
}

#[derive(Default)]
struct StreamAssemblyState {
    text: String,
    content_blocks: Vec<ContentBlock>,
    saw_multimodal_content: bool,
    reasoning_content: String,
    tool_calls: BTreeMap<usize, PartialToolCall>,
    finished: bool,
}

#[derive(Default)]
struct PartialToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
    emitted_args_len: usize,
}

struct ParsedOpenAiContent {
    text: String,
    content_blocks: Vec<ContentBlock>,
    saw_multimodal_content: bool,
}

fn strip_think_tags(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut rest = input;
    loop {
        if let Some(start) = rest.find("<think>") {
            result.push_str(&rest[..start]);
            if let Some(end) = rest[start..].find("</think>") {
                rest = &rest[start + end + "</think>".len()..];
            } else {
                break;
            }
        } else {
            result.push_str(rest);
            break;
        }
    }
    result.trim().to_string()
}

fn parse_openai_message_content(content: Option<OpenAiMessageContent>) -> ParsedOpenAiContent {
    let Some(content) = content else {
        return ParsedOpenAiContent {
            text: String::new(),
            content_blocks: Vec::new(),
            saw_multimodal_content: false,
        };
    };

    match content {
        OpenAiMessageContent::Text(text) => ParsedOpenAiContent {
            text,
            content_blocks: Vec::new(),
            saw_multimodal_content: false,
        },
        OpenAiMessageContent::Parts(parts) => {
            let mut text = String::new();
            let mut content_blocks = Vec::new();
            let mut saw_multimodal_content = false;

            for part in parts {
                match part.kind.as_str() {
                    "text" => {
                        if let Some(value) = part.text {
                            text.push_str(&value);
                        }
                    }
                    "refusal" => {
                        if let Some(value) = part.refusal {
                            text.push_str(&value);
                        }
                    }
                    "image_url" => {
                        saw_multimodal_content = true;
                        if let Some(image_url) = part.image_url {
                            content_blocks.push(parse_image_content_block(&image_url.url));
                        }
                    }
                    _ => {}
                }
            }

            ParsedOpenAiContent {
                text,
                content_blocks,
                saw_multimodal_content,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::common::parse_sse_json_bytes_stream;
    use bytes::Bytes;

    #[tokio::test]
    async fn parse_sse_json_bytes_stream_handles_utf8_split_across_chunks() {
        let json = serde_json::json!({
            "choices": [{
                "delta": { "content": "你" },
                "finish_reason": null
            }]
        });
        let frame = format!("data: {}\n\n", json);
        let bytes = frame.into_bytes();

        let needle = "你".as_bytes();
        let pos = bytes
            .windows(needle.len())
            .position(|w| w == needle)
            .expect("needle present");
        let split = pos + 1;

        let parts: Vec<Result<Bytes, std::io::Error>> = vec![
            Ok(Bytes::from(bytes[..split].to_vec())),
            Ok(Bytes::from(bytes[split..].to_vec())),
            Ok(Bytes::from_static(b"data: [DONE]\n\n")),
        ];

        let stream =
            parse_sse_json_bytes_stream::<_, _, OpenAiChatChunk>(tokio_stream::iter(parts));
        tokio::pin!(stream);
        let mut out = Vec::new();
        while let Some(chunk) = stream.next().await {
            out.push(chunk.unwrap());
        }

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].choices.len(), 1);
        assert_eq!(
            out[0].choices[0].delta.content,
            Some(OpenAiMessageContent::from("你"))
        );
    }

    #[test]
    fn chat_completions_url_uses_full_endpoint_when_provided() {
        let config = OpenAiCompatibleConfig::new("test")
            .with_base_url("https://my-api.example.com/v2/chat/completions");
        assert_eq!(
            config.chat_completions_url(),
            "https://my-api.example.com/v2/chat/completions"
        );
    }

    #[test]
    fn request_headers_support_x_api_key_and_user_agent() {
        let config = OpenAiCompatibleConfig::new("test")
            .with_api_key("k")
            .with_auth_style(OpenAiAuthStyle::XApiKey)
            .with_user_agent("deepagents-test/1.0");
        let headers = config.request_headers().expect("headers should be valid");
        assert_eq!(
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("k")
        );
        assert_eq!(
            headers
                .get(reqwest::header::USER_AGENT)
                .and_then(|value| value.to_str().ok()),
            Some("deepagents-test/1.0")
        );
        assert!(headers.get(reqwest::header::AUTHORIZATION).is_none());
    }

    #[test]
    fn flatten_system_messages_merges_into_first_user_turn() {
        let messages = vec![
            ChatMessage::system("policy"),
            ChatMessage::assistant("ack"),
            ChatMessage::system("constraints"),
            ChatMessage::user("hello"),
        ];
        let flattened = flatten_system_messages(&messages);
        assert_eq!(flattened.len(), 2);
        assert_eq!(flattened[0].role, ChatRole::Assistant);
        assert_eq!(flattened[1].role, ChatRole::User);
        assert_eq!(flattened[1].content, "policy\n\nconstraints\n\nhello");
    }

    #[test]
    fn parse_openai_message_uses_reasoning_when_content_is_think_only() {
        let message = OpenAiMessage {
            role: "assistant".to_string(),
            content: Some(OpenAiMessageContent::Text(
                "<think>internal chain</think>".to_string(),
            )),
            reasoning_content: Some("final answer".to_string()),
            tool_calls: None,
            tool_call_id: None,
        };
        let response = parse_openai_message(message).expect("should parse");
        assert_eq!(response.text, "final answer");
        assert_eq!(
            response
                .assistant_metadata
                .and_then(|metadata| metadata.reasoning_content),
            Some("final answer".to_string())
        );
    }
}
