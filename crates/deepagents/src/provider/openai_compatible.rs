use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use async_stream::try_stream;
use async_trait::async_trait;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use tokio_stream::Stream;
use tokio_stream::StreamExt;

use crate::provider::llm::{LlmEvent, LlmEventStream, LlmProvider, LlmProviderCapabilities};
use crate::provider::{ProviderRequest, ProviderStep, ProviderToolCall};

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleConfig {
    pub model: String,
    pub base_url: String,
    pub api_key: Option<String>,
}

impl OpenAiCompatibleConfig {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: None,
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

pub struct OpenAiCompatibleProvider {
    config: OpenAiCompatibleConfig,
    transport: Arc<dyn OpenAiCompatibleTransport>,
}

#[derive(Clone, Default)]
pub struct ReqwestOpenAiTransport {
    client: reqwest::Client,
}

impl OpenAiCompatibleProvider {
    pub fn new(
        config: OpenAiCompatibleConfig,
        transport: Arc<dyn OpenAiCompatibleTransport>,
    ) -> Self {
        Self { config, transport }
    }
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
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    fn capabilities(&self) -> LlmProviderCapabilities {
        LlmProviderCapabilities {
            supports_streaming: true,
            supports_tool_calling: true,
            reports_usage: true,
            supports_structured_output: false,
            supports_reasoning_content: false,
        }
    }

    async fn chat(&self, req: ProviderRequest) -> anyhow::Result<ProviderStep> {
        let request = build_chat_request(&self.config.model, &req, false);
        let response = self
            .transport
            .create_chat_completion(&self.config, request)
            .await?;
        parse_chat_response(response)
    }

    async fn stream_chat(&self, req: ProviderRequest) -> anyhow::Result<LlmEventStream> {
        let request = build_chat_request(&self.config.model, &req, true);
        let chunks = self
            .transport
            .stream_chat_completion(&self.config, request)
            .await?;
        Ok(Box::pin(stream_openai_chunks(chunks)))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAiChatRequest {
    pub model: String,
    pub messages: Vec<OpenAiMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OpenAiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAiMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
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
    pub content: Option<String>,
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

pub fn build_chat_request(model: &str, req: &ProviderRequest, stream: bool) -> OpenAiChatRequest {
    OpenAiChatRequest {
        model: model.to_string(),
        messages: req.messages.iter().map(convert_message).collect(),
        tools: req
            .tool_specs
            .iter()
            .map(|tool| OpenAiTool {
                kind: "function".to_string(),
                function: OpenAiFunctionSpec {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    parameters: tool.input_schema.clone(),
                },
            })
            .collect(),
        stream: if stream { Some(true) } else { None },
    }
}

fn convert_message(message: &crate::types::Message) -> OpenAiMessage {
    OpenAiMessage {
        role: message.role.clone(),
        content: if message.content.is_empty() && message.tool_calls.is_some() {
            None
        } else {
            Some(message.content.clone())
        },
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

pub fn parse_chat_response(response: OpenAiChatResponse) -> anyhow::Result<ProviderStep> {
    let Some(choice) = response.choices.into_iter().next() else {
        return Err(anyhow::anyhow!("openai_response_missing_choice"));
    };
    parse_openai_message(choice.message)
}

fn parse_openai_message(message: OpenAiMessage) -> anyhow::Result<ProviderStep> {
    if let Some(tool_calls) = message.tool_calls {
        let mut calls = Vec::with_capacity(tool_calls.len());
        for call in tool_calls {
            let arguments = if call.function.arguments.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&call.function.arguments)?
            };
            calls.push(ProviderToolCall {
                tool_name: call.function.name,
                arguments,
                call_id: Some(call.id),
            });
        }
        return Ok(ProviderStep::ToolCalls { calls });
    }

    Ok(ProviderStep::FinalText {
        text: message.content.unwrap_or_default(),
    })
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
                    if !text.is_empty() {
                        state.text.push_str(&text);
                        yield LlmEvent::AssistantTextDelta { text };
                    }
                }
                if let Some(tool_calls) = choice.delta.tool_calls {
                    for delta in tool_calls {
                        let entry = state.tool_calls.entry(delta.index).or_default();
                        if let Some(id) = delta.id {
                            entry.id = Some(id);
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

        let step = if state.tool_calls.is_empty() {
            ProviderStep::FinalText { text: state.text }
        } else {
            ProviderStep::ToolCalls {
                calls: assemble_tool_calls(&state.tool_calls)?,
            }
        };
        yield LlmEvent::FinalStep { step };
    }
}

fn assemble_tool_calls(
    tool_calls: &BTreeMap<usize, PartialToolCall>,
) -> anyhow::Result<Vec<ProviderToolCall>> {
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
        out.push(ProviderToolCall {
            tool_name: name,
            arguments,
            call_id: Some(id),
        });
    }
    Ok(out)
}

#[derive(Default)]
struct StreamAssemblyState {
    text: String,
    tool_calls: BTreeMap<usize, PartialToolCall>,
    finished: bool,
}

#[derive(Default)]
struct PartialToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
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
        Ok(Box::pin(parse_sse_response(response)))
    }
}

impl ReqwestOpenAiTransport {
    async fn send_request(
        &self,
        config: &OpenAiCompatibleConfig,
        request: OpenAiChatRequest,
        stream: bool,
    ) -> anyhow::Result<reqwest::Response> {
        let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
        let mut builder = self
            .client
            .post(url)
            .header(CONTENT_TYPE, "application/json")
            .header(
                ACCEPT,
                if stream {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )
            .json(&request);

        if let Some(api_key) = &config.api_key {
            builder = builder.header(AUTHORIZATION, format!("Bearer {api_key}"));
        }

        let response = builder.send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

        let body = response.text().await.unwrap_or_default();
        Err(anyhow::anyhow!("openai_http_error: {} {}", status, body))
    }
}

fn parse_sse_response(
    response: reqwest::Response,
) -> impl Stream<Item = anyhow::Result<OpenAiChatChunk>> + Send + 'static {
    try_stream! {
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let text = std::str::from_utf8(&chunk)?.replace("\r\n", "\n");
            buffer.push_str(&text);

            while let Some(idx) = buffer.find("\n\n") {
                let frame = buffer[..idx].to_string();
                buffer.drain(..idx + 2);

                let Some(data) = extract_sse_data(&frame) else {
                    continue;
                };
                if data == "[DONE]" {
                    return;
                }
                yield serde_json::from_str::<OpenAiChatChunk>(&data)?;
            }
        }

        if !buffer.trim().is_empty() {
            if let Some(data) = extract_sse_data(&buffer) {
                if data != "[DONE]" {
                    yield serde_json::from_str::<OpenAiChatChunk>(&data)?;
                }
            }
        }
    }
}

fn extract_sse_data(frame: &str) -> Option<String> {
    let lines: Vec<String> = frame
        .lines()
        .filter_map(|line| {
            line.strip_prefix("data:")
                .map(|value| value.trim_start().to_string())
        })
        .collect();
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}
