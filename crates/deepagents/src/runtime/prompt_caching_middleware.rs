use anyhow::Result;

use crate::runtime::RuntimeMiddleware;
use crate::runtime::{PromptCacheOptions, PROMPT_CACHE_OPTIONS_KEY, PROVIDER_CACHE_EVENTS_KEY};
use crate::state::AgentState;
use crate::types::Message;

/// Prompt 缓存相关的运行时中间件。
///
/// 作用：
/// - 在运行前/每次调用 provider 前，将 `PromptCacheOptions` 写入 `AgentState.extra`，
///   作为 provider 层读取缓存配置的统一入口。
/// - 在一次 run 开始前清理 `PROVIDER_CACHE_EVENTS_KEY`，避免上一次 run 的缓存事件污染本次。
#[derive(Debug, Clone)]
pub struct PromptCachingMiddleware {
    /// Prompt 缓存配置（是否启用、后端、TTL、分区等）。
    options: PromptCacheOptions,
}

impl PromptCachingMiddleware {
    /// 创建一个启用/配置化的 Prompt 缓存中间件。
    pub fn new(options: PromptCacheOptions) -> Self {
        Self { options }
    }

    /// 创建一个“禁用”的中间件实例。
    ///
    /// 适用于：需要显式关闭缓存，但仍希望在运行时保留该中间件（便于统一装配/切换）。
    pub fn disabled() -> Self {
        Self {
            options: PromptCacheOptions {
                enabled: false,
                backend: crate::runtime::CacheBackend::Memory,
                native: crate::runtime::PromptCacheNativeMode::Auto,
                layout: crate::runtime::PromptCacheLayoutMode::Auto,
                enable_l2_response_cache: false,
                ttl_ms: 0,
                max_entries: 0,
                provider_id: String::new(),
                model_id: String::new(),
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
        // 每次 run 开始前，清空 provider 侧缓存事件，避免跨 run 残留。
        state.extra.remove(PROVIDER_CACHE_EVENTS_KEY);
        // 将缓存配置写入 state.extra，供后续 provider/step 读取。
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
        // 有些调用链可能绕过 before_run（或中间件顺序不同），这里兜底补齐配置。
        if !state.extra.contains_key(PROMPT_CACHE_OPTIONS_KEY) {
            if let Ok(v) = serde_json::to_value(&self.options) {
                state.extra.insert(PROMPT_CACHE_OPTIONS_KEY.to_string(), v);
            }
        }
        Ok(messages)
    }
}
