use anyhow::Result;

use crate::runtime::RuntimeMiddleware;
use crate::runtime::{PromptCacheOptions, PROMPT_CACHE_OPTIONS_KEY, PROVIDER_CACHE_EVENTS_KEY};
use crate::state::AgentState;
use crate::types::Message;

#[derive(Debug, Clone)]
pub struct PromptCachingMiddleware {
    options: PromptCacheOptions,
}

impl PromptCachingMiddleware {
    pub fn new(options: PromptCacheOptions) -> Self {
        Self { options }
    }

    pub fn disabled() -> Self {
        Self {
            options: PromptCacheOptions {
                enabled: false,
                backend: crate::runtime::CacheBackend::Memory,
                enable_l2_response_cache: false,
                ttl_ms: 0,
                max_entries: 0,
                provider_id: String::new(),
                partition: String::new(),
            },
        }
    }
}

#[async_trait::async_trait]
impl RuntimeMiddleware for PromptCachingMiddleware {
    async fn before_run(
        &self,
        messages: Vec<Message>,
        state: &mut AgentState,
    ) -> Result<Vec<Message>> {
        state.extra.remove(PROVIDER_CACHE_EVENTS_KEY);
        if let Ok(v) = serde_json::to_value(&self.options) {
            state.extra.insert(PROMPT_CACHE_OPTIONS_KEY.to_string(), v);
        }
        Ok(messages)
    }

    async fn before_provider_step(
        &self,
        messages: Vec<Message>,
        state: &mut AgentState,
    ) -> Result<Vec<Message>> {
        if !state.extra.contains_key(PROMPT_CACHE_OPTIONS_KEY) {
            if let Ok(v) = serde_json::to_value(&self.options) {
                state.extra.insert(PROMPT_CACHE_OPTIONS_KEY.to_string(), v);
            }
        }
        Ok(messages)
    }
}
