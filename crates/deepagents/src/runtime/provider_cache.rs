use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state::AgentState;

pub const PROVIDER_CACHE_EVENTS_KEY: &str = "_provider_cache_events";
pub const PROMPT_CACHE_OPTIONS_KEY: &str = "_prompt_cache_options";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptCacheOptions {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "PromptCacheOptions::default_backend")]
    pub backend: CacheBackend,
    #[serde(default)]
    pub enable_l2_response_cache: bool,
    #[serde(default = "PromptCacheOptions::default_ttl_ms")]
    pub ttl_ms: u64,
    #[serde(default = "PromptCacheOptions::default_max_entries")]
    pub max_entries: usize,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub partition: String,
}

impl PromptCacheOptions {
    fn default_backend() -> CacheBackend {
        CacheBackend::Memory
    }

    fn default_ttl_ms() -> u64 {
        5 * 60 * 1000
    }

    fn default_max_entries() -> usize {
        1024
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CacheBackend {
    #[serde(rename = "memory")]
    Memory,
    #[serde(rename = "disk")]
    Disk,
    #[serde(rename = "remote")]
    Remote,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CacheLevel {
    #[serde(rename = "L0")]
    L0,
    #[serde(rename = "L1")]
    L1,
    #[serde(rename = "L2")]
    L2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheKeyComponents {
    pub l0_hash: String,
    pub system_hash: String,
    pub tools_hash: String,
    pub messages_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summarization_event_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderCacheEvent {
    ProviderCache {
        cache_backend: CacheBackend,
        cache_level: CacheLevel,
        lookup_hit: bool,
        cache_key_hash: String,
        components: CacheKeyComponents,
        #[serde(skip_serializing_if = "Option::is_none")]
        inserted: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        evicted: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        expired: Option<bool>,
    },
}

pub fn push_provider_cache_event(state: &mut AgentState, event: ProviderCacheEvent) {
    let Ok(value) = serde_json::to_value(event) else {
        return;
    };
    match state.extra.get_mut(PROVIDER_CACHE_EVENTS_KEY) {
        Some(Value::Array(items)) => {
            items.push(value);
        }
        Some(_) => {
            state.extra.insert(
                PROVIDER_CACHE_EVENTS_KEY.to_string(),
                Value::Array(vec![value]),
            );
        }
        None => {
            state.extra.insert(
                PROVIDER_CACHE_EVENTS_KEY.to_string(),
                Value::Array(vec![value]),
            );
        }
    }
}

pub fn take_provider_cache_events(state: &mut AgentState) -> Option<Value> {
    state.extra.remove(PROVIDER_CACHE_EVENTS_KEY)
}

pub fn attach_provider_cache_events_to_trace(
    trace: Option<Value>,
    state: &mut AgentState,
) -> Option<Value> {
    let Some(events) = take_provider_cache_events(state) else {
        return trace;
    };

    let mut trace = trace.unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    let Value::Object(map) = &mut trace else {
        return Some(trace);
    };
    map.insert("provider_cache_events".to_string(), events);
    Some(trace)
}
