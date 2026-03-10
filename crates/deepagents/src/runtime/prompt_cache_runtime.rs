use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::time::{timeout, Duration as TokioDuration};

use crate::provider::{Provider, ProviderRequest, ProviderStep};
use crate::runtime::cache_store::{CacheStore, MemoryCacheStore};
use crate::runtime::stable_hash::stable_json_sha256_hex;
use crate::runtime::{
    push_provider_cache_event, CacheBackend, CacheKeyComponents, CacheLevel, PromptCacheOptions,
    ProviderCacheEvent, PROMPT_CACHE_OPTIONS_KEY,
};
use crate::state::AgentState;
use crate::types::Message;

#[derive(Debug)]
pub enum CachedProviderError {
    Provider(anyhow::Error),
    Timeout,
}

struct PartitionedPromptCache {
    l1: MemoryCacheStore<()>,
    l2: MemoryCacheStore<ProviderStep>,
}

struct PromptCacheRegistry {
    partitions: Mutex<HashMap<String, Arc<PartitionedPromptCache>>>,
}

impl PromptCacheRegistry {
    fn global() -> &'static PromptCacheRegistry {
        static REG: OnceLock<PromptCacheRegistry> = OnceLock::new();
        REG.get_or_init(|| PromptCacheRegistry {
            partitions: Mutex::new(HashMap::new()),
        })
    }

    fn partition(&self, key: String, opts: &PromptCacheOptions) -> Arc<PartitionedPromptCache> {
        let mut guard = self.partitions.lock().unwrap();
        if let Some(p) = guard.get(&key) {
            return p.clone();
        }
        let ttl = Duration::from_millis(opts.ttl_ms.max(1));
        let max_entries = opts.max_entries.max(1);
        let p = Arc::new(PartitionedPromptCache {
            l1: MemoryCacheStore::new(ttl, max_entries),
            l2: MemoryCacheStore::new(ttl, max_entries),
        });
        guard.insert(key, p.clone());
        p
    }
}

fn load_prompt_cache_options(state: &AgentState) -> PromptCacheOptions {
    let Some(v) = state.extra.get(PROMPT_CACHE_OPTIONS_KEY) else {
        return PromptCacheOptions {
            enabled: false,
            backend: CacheBackend::Memory,
            enable_l2_response_cache: false,
            ttl_ms: 0,
            max_entries: 0,
            provider_id: String::new(),
            partition: String::new(),
        };
    };
    serde_json::from_value(v.clone()).unwrap_or(PromptCacheOptions {
        enabled: false,
        backend: CacheBackend::Memory,
        enable_l2_response_cache: false,
        ttl_ms: 0,
        max_entries: 0,
        provider_id: String::new(),
        partition: String::new(),
    })
}

fn extract_system_messages(messages: &[Message]) -> Vec<&Message> {
    messages.iter().filter(|m| m.role == "system").collect()
}

fn extract_non_system_messages(messages: &[Message]) -> Vec<&Message> {
    messages.iter().filter(|m| m.role != "system").collect()
}

fn build_key_components(
    req: &ProviderRequest,
    state: &AgentState,
    opts: &PromptCacheOptions,
) -> CacheKeyComponents {
    let l0_view = serde_json::json!({
        "provider_id": opts.provider_id,
    });
    let l0_hash = stable_json_sha256_hex(&l0_view);

    let system_view = serde_json::to_value(extract_system_messages(&req.messages))
        .unwrap_or(serde_json::Value::Null);
    let system_hash = stable_json_sha256_hex(&system_view);

    let tools_view = serde_json::json!({
        "tool_specs": req.tool_specs,
        "skills": req.skills,
    });
    let tools_hash = stable_json_sha256_hex(&tools_view);

    let messages_view = serde_json::to_value(extract_non_system_messages(&req.messages))
        .unwrap_or(serde_json::Value::Null);
    let messages_hash = stable_json_sha256_hex(&messages_view);

    let summarization_event_hash = state.extra.get("_summarization_event").and_then(|v| {
        if v.is_null() {
            None
        } else {
            Some(stable_json_sha256_hex(v))
        }
    });

    CacheKeyComponents {
        l0_hash,
        system_hash,
        tools_hash,
        messages_hash,
        summarization_event_hash,
    }
}

fn l1_key_hash(components: &CacheKeyComponents) -> String {
    let v = serde_json::json!({
        "l0_hash": components.l0_hash,
        "system_hash": components.system_hash,
        "tools_hash": components.tools_hash,
    });
    stable_json_sha256_hex(&v)
}

fn l2_key_hash(components: &CacheKeyComponents, l1_hash: &str) -> String {
    let v = serde_json::json!({
        "l1_hash": l1_hash,
        "messages_hash": components.messages_hash,
        "summarization_event_hash": components.summarization_event_hash,
    });
    stable_json_sha256_hex(&v)
}

fn partition_key(opts: &PromptCacheOptions) -> String {
    format!(
        "{}:{}:{}:{}",
        opts.partition,
        match opts.backend {
            CacheBackend::Memory => "memory",
            CacheBackend::Disk => "disk",
            CacheBackend::Remote => "remote",
        },
        opts.ttl_ms,
        opts.max_entries
    )
}

pub async fn step_with_prompt_cache(
    provider: &Arc<dyn Provider>,
    req: ProviderRequest,
    provider_timeout_ms: u64,
    state: &mut AgentState,
) -> Result<ProviderStep, CachedProviderError> {
    let opts = load_prompt_cache_options(state);
    if !opts.enabled || opts.backend != CacheBackend::Memory {
        return match timeout(
            TokioDuration::from_millis(provider_timeout_ms),
            provider.step(req),
        )
        .await
        {
            Ok(Ok(s)) => Ok(s),
            Ok(Err(e)) => Err(CachedProviderError::Provider(e)),
            Err(_) => Err(CachedProviderError::Timeout),
        };
    }

    let components = build_key_components(&req, state, &opts);
    let l1_hash = l1_key_hash(&components);
    let pkey = partition_key(&opts);
    let cache = PromptCacheRegistry::global().partition(pkey, &opts);

    let l1_lookup = cache.l1.get(&l1_hash);
    let mut l1_event_inserted: Option<bool> = None;
    let mut l1_event_evicted: Option<u64> = None;
    if l1_lookup.value.is_none() {
        let ins = cache.l1.insert(l1_hash.clone(), ());
        l1_event_inserted = Some(ins.inserted);
        if ins.evicted > 0 {
            l1_event_evicted = Some(ins.evicted);
        }
    }
    push_provider_cache_event(
        state,
        ProviderCacheEvent::ProviderCache {
            cache_backend: CacheBackend::Memory,
            cache_level: CacheLevel::L1,
            lookup_hit: l1_lookup.value.is_some(),
            cache_key_hash: l1_hash.clone(),
            components: components.clone(),
            inserted: l1_event_inserted,
            evicted: l1_event_evicted,
            expired: if l1_lookup.expired { Some(true) } else { None },
        },
    );

    if opts.enable_l2_response_cache {
        let l2_hash = l2_key_hash(&components, &l1_hash);
        let l2_lookup = cache.l2.get(&l2_hash);
        if let Some(step) = l2_lookup.value {
            push_provider_cache_event(
                state,
                ProviderCacheEvent::ProviderCache {
                    cache_backend: CacheBackend::Memory,
                    cache_level: CacheLevel::L2,
                    lookup_hit: true,
                    cache_key_hash: l2_hash,
                    components,
                    inserted: None,
                    evicted: None,
                    expired: if l2_lookup.expired { Some(true) } else { None },
                },
            );
            return Ok(step);
        }

        let step = match timeout(
            TokioDuration::from_millis(provider_timeout_ms),
            provider.step(req),
        )
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                push_provider_cache_event(
                    state,
                    ProviderCacheEvent::ProviderCache {
                        cache_backend: CacheBackend::Memory,
                        cache_level: CacheLevel::L2,
                        lookup_hit: false,
                        cache_key_hash: l2_hash,
                        components,
                        inserted: Some(false),
                        evicted: None,
                        expired: if l2_lookup.expired { Some(true) } else { None },
                    },
                );
                return Err(CachedProviderError::Provider(e));
            }
            Err(_) => {
                push_provider_cache_event(
                    state,
                    ProviderCacheEvent::ProviderCache {
                        cache_backend: CacheBackend::Memory,
                        cache_level: CacheLevel::L2,
                        lookup_hit: false,
                        cache_key_hash: l2_hash,
                        components,
                        inserted: Some(false),
                        evicted: None,
                        expired: if l2_lookup.expired { Some(true) } else { None },
                    },
                );
                return Err(CachedProviderError::Timeout);
            }
        };

        let mut inserted = Some(false);
        let mut evicted = None;
        if !matches!(step, ProviderStep::Error { .. }) {
            let ins = cache.l2.insert(l2_hash.clone(), step.clone());
            inserted = Some(ins.inserted);
            if ins.evicted > 0 {
                evicted = Some(ins.evicted);
            }
        }
        push_provider_cache_event(
            state,
            ProviderCacheEvent::ProviderCache {
                cache_backend: CacheBackend::Memory,
                cache_level: CacheLevel::L2,
                lookup_hit: false,
                cache_key_hash: l2_hash,
                components,
                inserted,
                evicted,
                expired: if l2_lookup.expired { Some(true) } else { None },
            },
        );
        return Ok(step);
    }

    match timeout(
        TokioDuration::from_millis(provider_timeout_ms),
        provider.step(req),
    )
    .await
    {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(e)) => Err(CachedProviderError::Provider(e)),
        Err(_) => Err(CachedProviderError::Timeout),
    }
}
