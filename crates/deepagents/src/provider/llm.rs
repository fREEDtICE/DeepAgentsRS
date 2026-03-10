use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_stream::Stream;
use tokio_stream::StreamExt;

use crate::provider::protocol::{
    Provider, ProviderEvent, ProviderEventCollector, ProviderRequest, ProviderStep,
    ProviderToolCall,
};

pub type LlmEventStream =
    Pin<Box<dyn Stream<Item = anyhow::Result<LlmEvent>> + Send + 'static>>;

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
        let mut stream = self.inner.stream_chat(req).await?;
        let mut final_step = None;

        while let Some(event) = stream.next().await {
            match event? {
                LlmEvent::AssistantTextDelta { text } => {
                    collector
                        .emit(ProviderEvent::AssistantTextDelta { text })
                        .await?;
                }
                LlmEvent::ToolCallArgsDelta { tool_call_id, delta } => {
                    collector
                        .emit(ProviderEvent::ToolCallArgsDelta { tool_call_id, delta })
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
}

impl MockLlmProvider {
    pub fn new(events: Vec<LlmEvent>) -> Self {
        Self {
            events: Arc::new(events),
        }
    }
}

#[async_trait]
impl LlmProvider for MockLlmProvider {
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
