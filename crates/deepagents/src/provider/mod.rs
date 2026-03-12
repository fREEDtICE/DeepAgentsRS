mod init;
pub mod llm;
pub mod mock;
pub mod openai_compatible;
mod prompt_guided;
pub mod protocol;

pub use init::{build_provider_bundle, ProviderInitBundle, ProviderInitSpec};
pub use llm::{
    AgentProviderFromLlm, LlmProviderAdapter, ProviderDiagnostics, ProviderSurfaceCapabilities,
};
pub use protocol::{
    AgentProvider, AgentProviderError, AgentProviderEvent, AgentProviderEventCollector,
    AgentProviderRequest, AgentStep, AgentStepOutput, AgentToolCall, Provider, ProviderError,
    ProviderEvent, ProviderEventCollector, ProviderRequest, ProviderStep, ProviderStepOutput,
    ProviderToolCall, VecAgentProviderEventCollector, VecProviderEventCollector,
};
