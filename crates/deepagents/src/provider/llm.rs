use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_stream::Stream;
use tokio_stream::StreamExt;

use crate::provider::prompt_guided::{validate_prompt_guided_tool_choice, PromptGuidedConfig};
use crate::provider::protocol::{
    Provider, ProviderEvent, ProviderEventCollector, ProviderRequest, ProviderStep,
    ProviderStepOutput, ProviderToolCall, ToolChoice,
};
use crate::runtime::ToolSpec;

fn is_false(value: &bool) -> bool {
    !*value
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

    pub fn supports_role(&self, role: &str) -> bool {
        match role {
            "user" => self.user,
            "assistant" => self.assistant,
            "tool" => self.tool,
            "system" => self.system,
            _ => false,
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

pub type LlmEventStream = Pin<Box<dyn Stream<Item = anyhow::Result<LlmEvent>> + Send + 'static>>;

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

/// Typed function-style tool metadata shared by provider adapters.
///
/// This keeps the stable tool contract strongly typed within Rust while
/// allowing each provider to map it to its own wire format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionTool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Provider-layer result of converting canonical `ToolSpec` values.
///
/// This type is intentionally provider-layer only. Runtime, CLI, and ACP
/// should only work with the unified `ToolChoice` plus canonical `ToolSpec`.
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
    FinalStep {
        output: ProviderStepOutput,
    },
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn capabilities(&self) -> LlmProviderCapabilities {
        LlmProviderCapabilities::default()
    }

    /// Convert canonical tool specs into provider-native tool payloads.
    ///
    /// This is intended to be the primary extension point for provider-native
    /// tool binding behavior.
    fn convert_tools(&self, tool_specs: &[ToolSpec]) -> anyhow::Result<ToolsPayload> {
        let _ = tool_specs;
        Ok(ToolsPayload::None)
    }

    async fn chat(&self, req: ProviderRequest) -> anyhow::Result<ProviderStepOutput>;

    async fn stream_chat(&self, req: ProviderRequest) -> anyhow::Result<LlmEventStream>;
}

pub struct LlmProviderAdapter {
    inner: Arc<dyn LlmProvider>,
}

impl LlmProviderAdapter {
    pub fn new(inner: Arc<dyn LlmProvider>) -> Self {
        Self { inner }
    }
}

struct AdaptedRequest {
    request: ProviderRequest,
    prompt_guided: Option<PromptGuidedConfig>,
}

#[async_trait]
impl Provider for LlmProviderAdapter {
    async fn step(&self, req: ProviderRequest) -> anyhow::Result<ProviderStep> {
        Ok(self.step_output(req).await?.step)
    }

    async fn step_output(&self, req: ProviderRequest) -> anyhow::Result<ProviderStepOutput> {
        let AdaptedRequest {
            request,
            prompt_guided,
        } = prepare_request(self.inner.as_ref(), req)?;
        let output = self.inner.chat(request).await?;
        parse_adapted_output(prompt_guided.as_ref(), output)
    }

    async fn step_with_collector(
        &self,
        req: ProviderRequest,
        collector: &mut dyn ProviderEventCollector,
    ) -> anyhow::Result<ProviderStep> {
        Ok(self.step_output_with_collector(req, collector).await?.step)
    }

    async fn step_output_with_collector(
        &self,
        req: ProviderRequest,
        collector: &mut dyn ProviderEventCollector,
    ) -> anyhow::Result<ProviderStepOutput> {
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
                        .emit(ProviderEvent::AssistantTextDelta { text })
                        .await?;
                }
                LlmEvent::ToolCallArgsDelta {
                    tool_call_id,
                    delta,
                } => {
                    collector
                        .emit(ProviderEvent::ToolCallArgsDelta {
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
                        .emit(ProviderEvent::Usage {
                            input_tokens,
                            output_tokens,
                            total_tokens,
                        })
                        .await?;
                }
                LlmEvent::FinalStep { output } => {
                    final_output = Some(parse_adapted_output(prompt_guided.as_ref(), output)?);
                }
            }
        }

        final_output.ok_or_else(|| anyhow::anyhow!("llm_stream_missing_final_step"))
    }
}

fn prepare_request(
    provider: &dyn LlmProvider,
    req: ProviderRequest,
) -> anyhow::Result<AdaptedRequest> {
    if let Some(spec) = req.structured_output.as_ref() {
        spec.validate()?;
        if !provider.capabilities().supports_structured_output {
            anyhow::bail!("provider_unsupported_structured_output");
        }
    }

    let tools_payload = provider.convert_tools(&req.tool_specs)?;
    validate_tool_choice_support(provider, &req, &tools_payload)?;

    let prompt_guided = match &tools_payload {
        ToolsPayload::PromptGuided { .. }
            if !req.tool_specs.is_empty() && !matches!(req.tool_choice, ToolChoice::None) =>
        {
            Some(PromptGuidedConfig::new(
                req.tool_choice.clone(),
                req.tool_specs.clone(),
            ))
        }
        _ => None,
    };
    let request = match (prompt_guided.as_ref(), tools_payload) {
        (Some(config), ToolsPayload::PromptGuided { instructions }) => {
            config.prepare_request(req, &instructions)
        }
        _ => req,
    };

    Ok(AdaptedRequest {
        request,
        prompt_guided,
    })
}

fn parse_adapted_output(
    prompt_guided: Option<&PromptGuidedConfig>,
    output: ProviderStepOutput,
) -> anyhow::Result<ProviderStepOutput> {
    let ProviderStepOutput {
        step,
        assistant_metadata,
    } = output;
    let step = match prompt_guided {
        Some(config) => config.parse_step(step)?,
        None => step,
    };
    Ok(ProviderStepOutput {
        step,
        assistant_metadata,
    })
}

fn validate_tool_choice_support(
    provider: &dyn LlmProvider,
    req: &ProviderRequest,
    tools_payload: &ToolsPayload,
) -> anyhow::Result<()> {
    let requires_tools = matches!(
        req.tool_choice,
        ToolChoice::Required | ToolChoice::Named { .. }
    );

    if !provider.capabilities().supports_tool_calling {
        return match tools_payload {
            ToolsPayload::PromptGuided { .. } => {
                validate_prompt_guided_tool_choice(&req.tool_choice, &req.tool_specs)
            }
            _ => match req.tool_choice {
                ToolChoice::Auto | ToolChoice::None => Ok(()),
                ToolChoice::Required | ToolChoice::Named { .. } => {
                    Err(anyhow::anyhow!("provider_unsupported_tool_calling"))
                }
            },
        };
    }

    if requires_tools && req.tool_specs.is_empty() {
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

#[derive(Clone)]
pub struct MockLlmProvider {
    events: Arc<Vec<LlmEvent>>,
    capabilities: LlmProviderCapabilities,
}

impl MockLlmProvider {
    pub fn new(events: Vec<LlmEvent>) -> Self {
        let mut capabilities = LlmProviderCapabilities::default();
        capabilities.supports_streaming = true;
        capabilities.reports_usage = events
            .iter()
            .any(|event| matches!(event, LlmEvent::Usage { .. }));
        capabilities.supports_tool_calling = events.iter().any(|event| {
            matches!(
                event,
                LlmEvent::ToolCallArgsDelta { .. }
                    | LlmEvent::FinalStep {
                        output: ProviderStepOutput {
                            step: ProviderStep::ToolCalls { .. }
                                | ProviderStep::AssistantMessageWithToolCalls { .. },
                            ..
                        }
                    }
            )
        });
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

    async fn chat(&self, _req: ProviderRequest) -> anyhow::Result<ProviderStepOutput> {
        self.events
            .iter()
            .find_map(|event| match event {
                LlmEvent::FinalStep { output } => Some(output.clone()),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("llm_stream_missing_final_step"))
    }

    async fn stream_chat(&self, _req: ProviderRequest) -> anyhow::Result<LlmEventStream> {
        let events = self.events.as_ref().clone();
        Ok(Box::pin(tokio_stream::iter(
            events.into_iter().map(Ok::<_, anyhow::Error>),
        )))
    }
}

pub fn final_text_step(text: &str) -> LlmEvent {
    LlmEvent::FinalStep {
        output: ProviderStepOutput::from(ProviderStep::FinalText {
            text: text.to_string(),
        }),
    }
}

pub fn tool_calls_step(calls: Vec<ProviderToolCall>) -> LlmEvent {
    LlmEvent::FinalStep {
        output: ProviderStepOutput::from(ProviderStep::ToolCalls { calls }),
    }
}
