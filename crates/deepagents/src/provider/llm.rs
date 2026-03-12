use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;

use crate::llm::{
    ChatMessage, ChatRequest, ChatResponse, ChatRole, LlmEvent, LlmProvider,
    LlmProviderCapabilities, ToolCall as LlmToolCall, ToolChoice, ToolSpec as LlmToolSpec,
    ToolsPayload,
};
use crate::provider::prompt_guided::{validate_prompt_guided_tool_choice, PromptGuidedConfig};
use crate::provider::protocol::{
    AgentProvider, AgentProviderEvent, AgentProviderEventCollector, AgentProviderRequest,
    AgentStep, AgentStepOutput, AgentToolCall,
};
use crate::types::{Message, ToolCall};

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSurfaceCapabilities {
    #[serde(default, skip_serializing_if = "is_false")]
    pub supports_provider_streaming: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub supports_tool_choice: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reports_usage: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub supports_structured_output: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDiagnostics {
    pub provider_id: String,
    #[serde(
        default,
        skip_serializing_if = "ProviderSurfaceCapabilities::is_disabled"
    )]
    pub surface_capabilities: ProviderSurfaceCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_capabilities: Option<LlmProviderCapabilities>,
}

impl ProviderSurfaceCapabilities {
    pub fn is_disabled(&self) -> bool {
        !self.supports_provider_streaming
            && !self.supports_tool_choice
            && !self.reports_usage
            && !self.supports_structured_output
    }
}

impl ProviderDiagnostics {
    pub fn new(
        provider_id: impl Into<String>,
        surface_capabilities: ProviderSurfaceCapabilities,
        llm_capabilities: Option<LlmProviderCapabilities>,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            surface_capabilities,
            llm_capabilities,
        }
    }

    pub fn supports_provider_streaming(&self) -> bool {
        self.surface_capabilities.supports_provider_streaming
    }

    pub fn supports_tool_choice(&self) -> bool {
        self.surface_capabilities.supports_tool_choice
    }

    pub fn reports_usage(&self) -> bool {
        self.surface_capabilities.reports_usage
    }

    pub fn supports_structured_output(&self) -> bool {
        self.surface_capabilities.supports_structured_output
    }
}

pub type AgentProviderFromLlm = LlmProviderAdapter;

pub struct LlmProviderAdapter {
    inner: Arc<dyn LlmProvider>,
}

impl LlmProviderAdapter {
    pub fn new(inner: Arc<dyn LlmProvider>) -> Self {
        Self { inner }
    }
}

struct AdaptedRequest {
    request: ChatRequest,
    prompt_guided: Option<PromptGuidedConfig>,
}

#[async_trait]
impl AgentProvider for LlmProviderAdapter {
    async fn step(&self, req: AgentProviderRequest) -> anyhow::Result<AgentStep> {
        Ok(self.step_output(req).await?.step)
    }

    async fn step_output(&self, req: AgentProviderRequest) -> anyhow::Result<AgentStepOutput> {
        let AdaptedRequest {
            request,
            prompt_guided,
        } = prepare_request(self.inner.as_ref(), req)?;
        let output = self.inner.chat(request).await?;
        parse_adapted_output(prompt_guided.as_ref(), output)
    }

    async fn step_with_collector(
        &self,
        req: AgentProviderRequest,
        collector: &mut dyn AgentProviderEventCollector,
    ) -> anyhow::Result<AgentStep> {
        Ok(self.step_output_with_collector(req, collector).await?.step)
    }

    async fn step_output_with_collector(
        &self,
        req: AgentProviderRequest,
        collector: &mut dyn AgentProviderEventCollector,
    ) -> anyhow::Result<AgentStepOutput> {
        let AdaptedRequest {
            request,
            prompt_guided,
        } = prepare_request(self.inner.as_ref(), req)?;
        if prompt_guided.is_some() || !self.inner.capabilities().supports_streaming {
            let output = self.inner.chat(request).await?;
            return parse_adapted_output(prompt_guided.as_ref(), output);
        }

        let mut stream = self.inner.stream_chat(request).await?;
        let mut final_output = None;

        while let Some(event) = stream.next().await {
            match event? {
                LlmEvent::AssistantTextDelta { text } => {
                    collector
                        .emit(AgentProviderEvent::AssistantTextDelta { text })
                        .await?;
                }
                LlmEvent::ToolCallArgsDelta {
                    tool_call_id,
                    delta,
                } => {
                    collector
                        .emit(AgentProviderEvent::ToolCallArgsDelta {
                            tool_call_id,
                            delta,
                        })
                        .await?;
                }
                LlmEvent::Usage {
                    input_tokens,
                    output_tokens,
                    total_tokens,
                } => {
                    collector
                        .emit(AgentProviderEvent::Usage {
                            input_tokens,
                            output_tokens,
                            total_tokens,
                        })
                        .await?;
                }
                LlmEvent::FinalResponse { response } => {
                    final_output = Some(parse_adapted_output(prompt_guided.as_ref(), response)?);
                }
            }
        }

        final_output.ok_or_else(|| anyhow::anyhow!("llm_stream_missing_final_response"))
    }
}

fn prepare_request(
    provider: &dyn LlmProvider,
    req: AgentProviderRequest,
) -> anyhow::Result<AdaptedRequest> {
    let AgentProviderRequest {
        messages,
        tool_specs,
        tool_choice,
        skills: _,
        state: _,
        last_tool_results: _,
        structured_output,
    } = req;

    if let Some(spec) = structured_output.as_ref() {
        spec.validate()?;
        if !provider.capabilities().supports_structured_output {
            anyhow::bail!("provider_unsupported_structured_output");
        }
    }

    let tool_specs = tool_specs
        .into_iter()
        .map(convert_tool_spec)
        .collect::<Vec<_>>();
    let tools_payload = provider.convert_tools(&tool_specs)?;
    validate_tool_choice_support(provider, &tool_specs, &tool_choice, &tools_payload)?;

    let prompt_guided = match &tools_payload {
        ToolsPayload::PromptGuided { .. }
            if !tool_specs.is_empty() && !matches!(tool_choice, ToolChoice::None) =>
        {
            Some(PromptGuidedConfig::new(
                tool_choice.clone(),
                tool_specs.clone(),
            ))
        }
        _ => None,
    };

    let mut request = ChatRequest {
        messages: messages.into_iter().map(convert_message).collect(),
        tool_specs,
        tool_choice,
        structured_output,
    };

    if let (Some(config), ToolsPayload::PromptGuided { instructions }) =
        (prompt_guided.as_ref(), &tools_payload)
    {
        request = config.prepare_request(request, instructions);
    }

    Ok(AdaptedRequest {
        request,
        prompt_guided,
    })
}

fn parse_adapted_output(
    prompt_guided: Option<&PromptGuidedConfig>,
    output: ChatResponse,
) -> anyhow::Result<AgentStepOutput> {
    let ChatResponse {
        text,
        tool_calls,
        usage: _,
        assistant_metadata,
    } = output;

    let calls = tool_calls
        .into_iter()
        .map(convert_llm_tool_call)
        .collect::<Vec<_>>();
    let mut step = if calls.is_empty() {
        AgentStep::FinalText { text }
    } else if text.is_empty() {
        AgentStep::ToolCalls { calls }
    } else {
        AgentStep::AssistantMessageWithToolCalls { text, calls }
    };

    if let Some(config) = prompt_guided {
        step = config.parse_step(step)?;
    }

    Ok(AgentStepOutput {
        step,
        assistant_metadata: assistant_metadata.filter(|metadata| !metadata.is_empty()),
    })
}

fn validate_tool_choice_support(
    provider: &dyn LlmProvider,
    tool_specs: &[LlmToolSpec],
    tool_choice: &ToolChoice,
    tools_payload: &ToolsPayload,
) -> anyhow::Result<()> {
    let requires_tools = matches!(tool_choice, ToolChoice::Required | ToolChoice::Named { .. });

    if !provider.capabilities().supports_tool_calling {
        return match tools_payload {
            ToolsPayload::PromptGuided { .. } => {
                validate_prompt_guided_tool_choice(tool_choice, tool_specs)
            }
            _ => match tool_choice {
                ToolChoice::Auto | ToolChoice::None => Ok(()),
                ToolChoice::Required | ToolChoice::Named { .. } => {
                    Err(anyhow::anyhow!("provider_unsupported_tool_calling"))
                }
            },
        };
    }

    if requires_tools && tool_specs.is_empty() {
        anyhow::bail!("tool_choice_requires_tools");
    }

    match tools_payload {
        ToolsPayload::PromptGuided { .. } => Ok(()),
        ToolsPayload::FunctionTools { tools } => {
            if requires_tools && tools.is_empty() {
                anyhow::bail!("tool_choice_requires_tools");
            }
            Ok(())
        }
        ToolsPayload::None => {
            if requires_tools {
                anyhow::bail!("tool_choice_requires_tools");
            }
            Ok(())
        }
    }
}

fn convert_tool_spec(tool: crate::runtime::ToolSpec) -> LlmToolSpec {
    LlmToolSpec {
        name: tool.name,
        description: tool.description,
        input_schema: tool.input_schema,
    }
}

fn convert_message(message: Message) -> ChatMessage {
    ChatMessage {
        role: ChatRole::from(message.role),
        content: message.content,
        content_blocks: message.content_blocks,
        reasoning_content: message.reasoning_content,
        tool_calls: message
            .tool_calls
            .map(|calls| calls.into_iter().map(convert_message_tool_call).collect()),
        tool_call_id: message.tool_call_id,
        name: message.name,
        status: message.status,
    }
}

fn convert_message_tool_call(call: ToolCall) -> LlmToolCall {
    LlmToolCall {
        id: call.id,
        name: call.name,
        arguments: call.arguments,
    }
}

fn convert_llm_tool_call(call: LlmToolCall) -> AgentToolCall {
    AgentToolCall {
        tool_name: call.name,
        arguments: call.arguments,
        call_id: Some(call.id),
    }
}

pub use crate::llm::{final_text_step, tool_calls_step, LlmEventStream, MockLlmProvider};
