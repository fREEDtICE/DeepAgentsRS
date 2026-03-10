use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_stream::Stream;
use tokio_stream::StreamExt;

use crate::provider::protocol::{
    Provider, ProviderEvent, ProviderEventCollector, ProviderRequest, ProviderStep,
    ProviderToolCall,
};

pub type LlmEventStream = Pin<Box<dyn Stream<Item = anyhow::Result<LlmEvent>> + Send + 'static>>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmProviderCapabilities {
    pub supports_streaming: bool,
    pub supports_tool_calling: bool,
    pub reports_usage: bool,
    pub supports_structured_output: bool,
    pub supports_reasoning_content: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDiagnostics {
    pub provider_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_capabilities: Option<LlmProviderCapabilities>,
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
        step: ProviderStep,
    },
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn capabilities(&self) -> LlmProviderCapabilities {
        LlmProviderCapabilities::default()
    }

    async fn chat(&self, req: ProviderRequest) -> anyhow::Result<ProviderStep>;

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

#[async_trait]
impl Provider for LlmProviderAdapter {
    async fn step(&self, req: ProviderRequest) -> anyhow::Result<ProviderStep> {
        self.inner.chat(req).await
    }

    async fn step_with_collector(
        &self,
        req: ProviderRequest,
        collector: &mut dyn ProviderEventCollector,
    ) -> anyhow::Result<ProviderStep> {
        if !self.inner.capabilities().supports_streaming {
            return self.inner.chat(req).await;
        }

        let mut stream = self.inner.stream_chat(req).await?;
        let mut final_step = None;

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
                LlmEvent::FinalStep { step } => {
                    final_step = Some(step);
                }
            }
        }

        final_step.ok_or_else(|| anyhow::anyhow!("llm_stream_missing_final_step"))
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
                        step: ProviderStep::ToolCalls { .. }
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

    async fn chat(&self, _req: ProviderRequest) -> anyhow::Result<ProviderStep> {
        self.events
            .iter()
            .find_map(|event| match event {
                LlmEvent::FinalStep { step } => Some(step.clone()),
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
        step: ProviderStep::FinalText {
            text: text.to_string(),
        },
    }
}

pub fn tool_calls_step(calls: Vec<ProviderToolCall>) -> LlmEvent {
    LlmEvent::FinalStep {
        step: ProviderStep::ToolCalls { calls },
    }
}
