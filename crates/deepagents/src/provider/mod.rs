pub mod catalog;
mod init;
pub mod llm;
pub mod mock;
pub mod openai_compatible;
pub mod prompt_cache;
mod prompt_guided;
pub mod protocol;

pub use catalog::{
    default_model_level_catalog, resolve_model_level_selection,
    resolve_model_level_selection_with_catalog, ModelLevel, ModelLevelIntent,
    ModelLevelResolutionDiagnostics, ModelLevelResolutionError, ProviderBasicConfig,
    ProviderCatalogEntry, ProviderLevelMap, ProviderLevelTarget, ProviderSurfaceKind,
    ResolvedProviderSelection,
};
pub use init::{build_provider_bundle, ProviderInitBundle, ProviderInitSpec};
pub use llm::{
    AgentProviderFromLlm, LlmProviderAdapter, ProviderDiagnostics, ProviderSurfaceCapabilities,
};
pub use prompt_cache::{
    PromptCachePlan, PromptPrefixArtifact, ProviderPromptCacheHandle, ProviderPromptCacheHint,
    ProviderPromptCacheObservation, ProviderPromptCacheSource, ProviderPromptCacheStatus,
    ProviderPromptCacheStrategy,
};
pub use protocol::{
    AgentProvider, AgentProviderError, AgentProviderEvent, AgentProviderEventCollector,
    AgentProviderRequest, AgentStep, AgentStepOutput, AgentToolCall, Provider, ProviderError,
    ProviderEvent, ProviderEventCollector, ProviderRequest, ProviderStep, ProviderStepOutput,
    ProviderToolCall, VecAgentProviderEventCollector, VecProviderEventCollector,
};
