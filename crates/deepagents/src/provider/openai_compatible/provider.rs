use std::collections::BTreeMap;
use std::sync::Arc;

use async_stream::try_stream;
use async_trait::async_trait;
use tokio_stream::Stream;
use tokio_stream::StreamExt;

use crate::provider::llm::{
    FunctionTool, LlmEvent, LlmEventStream, LlmProvider, LlmProviderCapabilities,
    MultimodalCapabilities, MultimodalInputRoles, ToolsPayload,
};
use crate::provider::{
    AssistantMessageMetadata, ProviderRequest, ProviderStep, ProviderStepOutput, ProviderToolCall,
    ToolChoice,
};
use crate::runtime::ToolSpec;
use crate::types::{fallback_text_for_content_blocks, ContentBlock, Message};

use super::transport::{OpenAiChunkStream, OpenAiCompatibleTransport};
use super::wire::{
    OpenAiChatRequest, OpenAiChatResponse, OpenAiContentPart, OpenAiFunctionCall,
    OpenAiFunctionSpec, OpenAiJsonSchemaResponseFormat, OpenAiMessage, OpenAiMessageContent,
    OpenAiResponseFormat, OpenAiTool, OpenAiToolCall, OpenAiToolChoice, OpenAiToolChoiceFunction,
};

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleConfig {
    pub model: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub multimodal_input_roles: MultimodalInputRoles,
}

impl OpenAiCompatibleConfig {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: None,
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

    pub fn with_multimodal_input_roles(mut self, roles: MultimodalInputRoles) -> Self {
        self.multimodal_input_roles = roles;
        self
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

    async fn chat(&self, req: ProviderRequest) -> anyhow::Result<ProviderStepOutput> {
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

    async fn stream_chat(&self, req: ProviderRequest) -> anyhow::Result<LlmEventStream> {
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

fn build_chat_request(
    model: &str,
    req: &ProviderRequest,
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

fn convert_openai_response_format(
    spec: &crate::provider::StructuredOutputSpec,
) -> OpenAiResponseFormat {
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

fn convert_message(
    message: &Message,
    multimodal_input_roles: MultimodalInputRoles,
) -> OpenAiMessage {
    OpenAiMessage {
        role: message.role.clone(),
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
    message: &Message,
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

fn fallback_openai_text_content(message: &Message) -> Option<OpenAiMessageContent> {
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

fn build_data_url(mime_type: &str, base64: &str) -> String {
    format!("data:{mime_type};base64,{base64}")
}

pub fn parse_chat_response(response: OpenAiChatResponse) -> anyhow::Result<ProviderStepOutput> {
    let Some(choice) = response.choices.into_iter().next() else {
        return Err(anyhow::anyhow!("openai_response_missing_choice"));
    };
    parse_openai_message(choice.message)
}

fn parse_openai_message(message: OpenAiMessage) -> anyhow::Result<ProviderStepOutput> {
    let parsed_content = parse_openai_message_content(message.content);
    let text = finalized_assistant_text(
        parsed_content.text,
        &parsed_content.content_blocks,
        parsed_content.saw_multimodal_content,
    );
    let metadata = AssistantMessageMetadata {
        content_blocks: (!parsed_content.content_blocks.is_empty())
            .then_some(parsed_content.content_blocks.clone()),
        reasoning_content: message.reasoning_content.clone(),
    };
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
        if text.is_empty() {
            return Ok(ProviderStepOutput::from(ProviderStep::ToolCalls { calls })
                .with_assistant_metadata(metadata));
        }
        return Ok(
            ProviderStepOutput::from(ProviderStep::AssistantMessageWithToolCalls { text, calls })
                .with_assistant_metadata(metadata),
        );
    }

    Ok(
        ProviderStepOutput::from(ProviderStep::FinalText { text })
            .with_assistant_metadata(metadata),
    )
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

        let step = if state.tool_calls.is_empty() {
            ProviderStep::FinalText {
                text: finalized_assistant_text(
                    state.text,
                    &state.content_blocks,
                    state.saw_multimodal_content,
                ),
            }
        } else {
            let calls = assemble_tool_calls(&state.tool_calls)?;
            let text = finalized_assistant_text(
                state.text,
                &state.content_blocks,
                state.saw_multimodal_content,
            );
            if text.is_empty() {
                ProviderStep::ToolCalls { calls }
            } else {
                ProviderStep::AssistantMessageWithToolCalls {
                    text,
                    calls,
                }
            }
        };
        let output = ProviderStepOutput::from(step).with_assistant_metadata(AssistantMessageMetadata {
            content_blocks: (!state.content_blocks.is_empty()).then_some(state.content_blocks),
            reasoning_content: if state.reasoning_content.is_empty() {
                None
            } else {
                Some(state.reasoning_content)
            },
        });
        yield LlmEvent::FinalStep { output };
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
                            content_blocks.push(parse_openai_image_content_block(&image_url.url));
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

fn parse_openai_image_content_block(url: &str) -> ContentBlock {
    parse_data_url_content_block(url).unwrap_or_else(|| ContentBlock::image_url(url))
}

fn parse_data_url_content_block(url: &str) -> Option<ContentBlock> {
    let payload = url.strip_prefix("data:")?;
    let (meta, base64) = payload.split_once(",")?;
    let mime_type = meta.strip_suffix(";base64")?;
    Some(ContentBlock::image_base64(mime_type, base64))
}

fn finalized_assistant_text(
    text: String,
    content_blocks: &[ContentBlock],
    saw_multimodal_content: bool,
) -> String {
    if !text.is_empty() {
        return text;
    }
    if let Some(fallback) = fallback_text_for_content_blocks(content_blocks) {
        return fallback;
    }
    if saw_multimodal_content {
        return "(assistant returned multimodal content)".to_string();
    }
    String::new()
}
