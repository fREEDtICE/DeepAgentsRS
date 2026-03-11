use std::sync::Arc;

use crate::llm::{
    openai_compatible::{OpenAiCompatibleConfig, OpenAiCompatibleProvider, ReqwestOpenAiTransport},
    openrouter::OpenRouterConfig,
    LlmProvider, OpenRouterProvider,
};

use super::llm::{LlmProviderAdapter, ProviderDiagnostics, ProviderSurfaceCapabilities};
use super::mock::{MockProvider, MockScript};
use super::protocol::AgentProvider;

#[derive(Clone)]
pub struct ProviderInitBundle {
    pub provider: Arc<dyn AgentProvider>,
    pub diagnostics: ProviderDiagnostics,
}

#[derive(Debug, Clone)]
pub enum ProviderInitSpec {
    Mock {
        script: MockScript,
        omit_call_ids: bool,
    },
    OpenAiCompatible {
        config: OpenAiCompatibleConfig,
    },
    OpenRouter {
        config: OpenRouterConfig,
    },
}

pub fn build_provider_bundle(
    provider_id: impl Into<String>,
    spec: ProviderInitSpec,
) -> ProviderInitBundle {
    let provider_id = provider_id.into();
    match spec {
        ProviderInitSpec::Mock {
            script,
            omit_call_ids,
        } => {
            let provider: Arc<dyn AgentProvider> = if omit_call_ids {
                Arc::new(MockProvider::from_script_without_call_ids(script))
            } else {
                Arc::new(MockProvider::from_script(script))
            };
            ProviderInitBundle {
                provider,
                diagnostics: ProviderDiagnostics::new(
                    provider_id,
                    ProviderSurfaceCapabilities {
                        supports_tool_choice: true,
                        supports_structured_output: true,
                        ..Default::default()
                    },
                    None,
                ),
            }
        }
        ProviderInitSpec::OpenAiCompatible { config } => {
            let llm_provider = Arc::new(OpenAiCompatibleProvider::new(
                config,
                Arc::new(ReqwestOpenAiTransport::new()),
            ));
            let llm_capabilities = LlmProvider::capabilities(llm_provider.as_ref());
            ProviderInitBundle {
                provider: Arc::new(LlmProviderAdapter::new(llm_provider)),
                diagnostics: ProviderDiagnostics::new(
                    provider_id,
                    ProviderSurfaceCapabilities {
                        supports_provider_streaming: llm_capabilities.supports_streaming,
                        supports_tool_choice: true,
                        reports_usage: llm_capabilities.reports_usage,
                        supports_structured_output: llm_capabilities.supports_structured_output,
                    },
                    Some(llm_capabilities),
                ),
            }
        }
        ProviderInitSpec::OpenRouter { config } => {
            let llm_provider = Arc::new(OpenRouterProvider::new(config));
            let llm_capabilities = LlmProvider::capabilities(llm_provider.as_ref());
            ProviderInitBundle {
                provider: Arc::new(LlmProviderAdapter::new(llm_provider)),
                diagnostics: ProviderDiagnostics::new(
                    provider_id,
                    ProviderSurfaceCapabilities {
                        supports_provider_streaming: llm_capabilities.supports_streaming,
                        supports_tool_choice: true,
                        reports_usage: llm_capabilities.reports_usage,
                        supports_structured_output: llm_capabilities.supports_structured_output,
                    },
                    Some(llm_capabilities),
                ),
            }
        }
    }
}
