mod init;
pub mod llm;
pub mod mock;
pub mod openai_compatible;
mod prompt_guided;
pub mod protocol;

pub use init::{build_provider_bundle, ProviderInitBundle, ProviderInitSpec};
pub use llm::{
    final_text_step, tool_calls_step, LlmEvent, LlmEventStream, LlmProvider, LlmProviderAdapter,
    LlmProviderCapabilities, MockLlmProvider, MultimodalCapabilities, MultimodalInputRoles,
    ProviderDiagnostics, ProviderSurfaceCapabilities, ToolsPayload,
};
pub use openai_compatible::{
    MockOpenAiTransport, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
    OpenAiCompatibleTransport, ReqwestOpenAiTransport,
};
pub use protocol::{
    AssistantMessageMetadata, Provider, ProviderError, ProviderEvent, ProviderEventCollector,
    ProviderRequest, ProviderStep, ProviderStepOutput, ProviderToolCall, StructuredOutputSpec,
    ToolChoice, VecProviderEventCollector,
};
