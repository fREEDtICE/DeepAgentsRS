pub(crate) mod common;
pub mod openai_compatible;
pub mod openrouter;
pub mod protocol;

pub use openai_compatible::{
    MockOpenAiTransport, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
    OpenAiCompatibleTransport, ReqwestOpenAiTransport,
};
pub use openrouter::{
    MockOpenRouterTransport, OpenRouterConfig, OpenRouterProvider, ReqwestOpenRouterTransport,
};
pub use protocol::{
    final_text_step, tool_calls_step, AssistantMessageMetadata, ChatMessage, ChatRequest,
    ChatResponse, ChatRole, FunctionTool, LlmEvent, LlmEventStream, LlmProvider,
    LlmProviderCapabilities, MockLlmProvider, MultimodalCapabilities, MultimodalInputRoles,
    StructuredOutputSpec, TokenUsage, ToolCall, ToolChoice, ToolSpec, ToolsPayload,
};
