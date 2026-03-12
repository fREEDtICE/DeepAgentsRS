use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::time::{timeout, Duration as TokioDuration};

/// Prompt 缓存运行时（当前仅支持内存缓存）。
///
/// 核心目标：
/// - 为 provider 的一次 `step_output` 调用提供可选的两级缓存（L1/L2），并将命中/写入/淘汰/过期等信息写入 `AgentState`。
/// - 缓存键使用“稳定 JSON + SHA256”生成，尽量避免结构体序列化顺序/平台差异带来的抖动。
///
/// 缓存层级含义：
/// - L1：只基于 provider_id/system messages/tools 等“相对稳定的上下文”构建 key，
///   存入一个占位值 `()`，用于观察/统计上下文复用与过期情况（不缓存响应内容）。
/// - L2：在 L1 的基础上叠加“非 system messages + summarization_event”等更易变化的输入，缓存 `ProviderStepOutput`。
///
/// 分区（partition）：
/// - 通过 `partition_key(opts)` 将不同的 `partition/backend/ttl/max_entries` 隔离到不同缓存实例，
///   避免不同配置互相污染。
use crate::provider::{
    AgentProvider, AgentProviderEvent, AgentProviderEventCollector, AgentProviderRequest,
    AgentStep, AgentStepOutput, VecAgentProviderEventCollector,
};
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
    /// provider 内部错误（原样包装，便于上层区分超时/错误）。
    Provider(anyhow::Error),
    /// provider 调用超时。
    Timeout,
}

/// 单个分区对应的一组缓存实例。
///
/// - L1 只存占位，用于“上下文 key”的命中观察
/// - L2 存 `ProviderStepOutput`，可选启用
struct PartitionedPromptCache {
    l1: MemoryCacheStore<()>,
    l2: MemoryCacheStore<AgentStepOutput>,
}

/// 全局缓存注册表：按分区 key 懒加载/复用 `PartitionedPromptCache`。
struct PromptCacheRegistry {
    partitions: Mutex<HashMap<String, Arc<PartitionedPromptCache>>>,
}

impl PromptCacheRegistry {
    /// 进程级全局单例。
    fn global() -> &'static PromptCacheRegistry {
        static REG: OnceLock<PromptCacheRegistry> = OnceLock::new();
        REG.get_or_init(|| PromptCacheRegistry {
            partitions: Mutex::new(HashMap::new()),
        })
    }

    /// 获取（或创建）某个分区的缓存实例。
    ///
    /// - `ttl_ms/max_entries` 会做下限保护（至少 1），避免零值导致不可用或除零等问题。
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

/// 从 `AgentState.extra` 读取 prompt cache 配置。
///
/// 缓存配置由 `PromptCachingMiddleware` 写入；若未写入或解析失败，则返回“禁用”配置。
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

/// 仅提取 system role 的 messages（通常代表全局指令、约束等）。
fn extract_system_messages(messages: &[Message]) -> Vec<&Message> {
    messages.iter().filter(|m| m.role == "system").collect()
}

/// 提取非 system role 的 messages（通常是用户/助手对话内容）。
fn extract_non_system_messages(messages: &[Message]) -> Vec<&Message> {
    messages.iter().filter(|m| m.role != "system").collect()
}

/// 构建用于生成缓存 key 的组件集合。
///
/// 这里选择将不同“变化频率”的输入拆分成多个 hash，便于：
/// - L1/L2 复用（L2 key 依赖 L1 key）
/// - 事件上报时能携带更可解释的维度（components）
fn build_key_components(
    req: &AgentProviderRequest,
    state: &AgentState,
    opts: &PromptCacheOptions,
) -> CacheKeyComponents {
    // L0：只包含 provider_id，保证不同 provider 的缓存天然隔离。
    let l0_view = serde_json::json!({
        "provider_id": opts.provider_id,
    });
    let l0_hash = stable_json_sha256_hex(&l0_view);

    // system messages：通常较稳定，适合作为 L1 的一部分。
    let system_view = serde_json::to_value(extract_system_messages(&req.messages))
        .unwrap_or(serde_json::Value::Null);
    let system_hash = stable_json_sha256_hex(&system_view);

    // tools/skills：工具规格或技能集变化会显著影响模型输出，也纳入 L1。
    let tools_view = serde_json::json!({
        "tool_specs": req.tool_specs,
        "skills": req.skills,
    });
    let tools_hash = stable_json_sha256_hex(&tools_view);

    // 非 system messages：对话内容变化频繁，纳入 L2。
    let messages_view = serde_json::to_value(extract_non_system_messages(&req.messages))
        .unwrap_or(serde_json::Value::Null);
    let messages_hash = stable_json_sha256_hex(&messages_view);

    // summarization_event：摘要等“隐藏状态”会影响输出，因此也纳入 L2（存在时）。
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

/// L1 key：只包含相对稳定的输入维度。
fn l1_key_hash(components: &CacheKeyComponents) -> String {
    let v = serde_json::json!({
        "l0_hash": components.l0_hash,
        "system_hash": components.system_hash,
        "tools_hash": components.tools_hash,
    });
    stable_json_sha256_hex(&v)
}

/// L2 key：在 L1 的基础上叠加“对话内容/摘要状态”等更易变化的维度。
fn l2_key_hash(components: &CacheKeyComponents, l1_hash: &str) -> String {
    let v = serde_json::json!({
        "l1_hash": l1_hash,
        "messages_hash": components.messages_hash,
        "summarization_event_hash": components.summarization_event_hash,
    });
    stable_json_sha256_hex(&v)
}

/// 生成分区 key，用于隔离不同缓存配置（尤其是 ttl/max_entries）。
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

/// 带 prompt cache 的 provider step（不包含 provider 事件 collector）。
///
/// - 若缓存禁用或后端不是 Memory，则直接执行 provider 并仅做超时包装。
/// - 若启用：
///   - 先做 L1 事件记录（命中/写入/淘汰/过期）
///   - 若启用 L2 响应缓存，则尝试命中并返回；未命中时执行 provider，并在非 Error step 时写入缓存
pub async fn step_with_prompt_cache(
    provider: &Arc<dyn AgentProvider>,
    req: AgentProviderRequest,
    provider_timeout_ms: u64,
    state: &mut AgentState,
) -> Result<AgentStepOutput, CachedProviderError> {
    let opts = load_prompt_cache_options(state);
    if !opts.enabled || opts.backend != CacheBackend::Memory {
        // 兜底直通：不参与缓存逻辑，但仍统一将超时/错误映射到 CachedProviderError。
        return match timeout_with_step_output(provider, req, provider_timeout_ms).await {
            Ok(Ok(output)) => Ok(output),
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
        // L1 只存占位值：用于观察上下文复用、并在 L1 维度上产生一致的 cache 事件。
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
        if let Some(output) = l2_lookup.value {
            // L2 命中：直接返回输出，并记录命中事件。
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
            return Ok(output);
        }

        // L2 未命中：执行 provider；失败/超时需要先记录 L2 miss 事件再返回错误。
        let output = match timeout_with_step_output(provider, req, provider_timeout_ms).await {
            Ok(Ok(output)) => output,
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
        if !matches!(output.step, AgentStep::Error { .. }) {
            // 不缓存 Error step，避免将临时故障/拒绝等错误固化成“稳定结果”。
            let ins = cache.l2.insert(l2_hash.clone(), output.clone());
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
        return Ok(output);
    }

    match timeout_with_step_output(provider, req, provider_timeout_ms).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(CachedProviderError::Provider(e)),
        Err(_) => Err(CachedProviderError::Timeout),
    }
}

/// 带 prompt cache 的 provider step（包含 provider 事件 collector）。
///
/// 与 `step_with_prompt_cache` 的差异：
/// - provider 调用使用 `step_output_with_collector`，将 provider 产生的事件写入外部 collector
/// - 缓存事件仍写入 `AgentState`（供上层统一消费）
pub async fn step_with_prompt_cache_and_collector(
    provider: &Arc<dyn AgentProvider>,
    req: AgentProviderRequest,
    provider_timeout_ms: u64,
    state: &mut AgentState,
    collector: &mut dyn AgentProviderEventCollector,
) -> Result<AgentStepOutput, CachedProviderError> {
    let opts = load_prompt_cache_options(state);
    if !opts.enabled || opts.backend != CacheBackend::Memory {
        // 兜底直通：不参与缓存逻辑，但保留事件收集能力。
        return match timeout_with_provider_output_collector(
            provider,
            req,
            provider_timeout_ms,
            collector,
        )
        .await
        {
            Ok(Ok(output)) => Ok(output),
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
        // 与无 collector 版本一致：L1 只存占位。
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
        if let Some(output) = l2_lookup.value {
            // L2 命中：直接返回输出，并记录命中事件。
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
            return Ok(output);
        }

        // L2 未命中：执行 provider 并收集事件；失败/超时先记录 L2 miss 事件。
        let output = match timeout_with_provider_output_collector(
            provider,
            req,
            provider_timeout_ms,
            collector,
        )
        .await
        {
            Ok(Ok(output)) => output,
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
        if !matches!(output.step, AgentStep::Error { .. }) {
            // 不缓存 Error step，理由同上。
            let ins = cache.l2.insert(l2_hash.clone(), output.clone());
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
        return Ok(output);
    }

    match timeout_with_provider_output_collector(provider, req, provider_timeout_ms, collector)
        .await
    {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(CachedProviderError::Provider(e)),
        Err(_) => Err(CachedProviderError::Timeout),
    }
}

/// 便捷封装：执行带缓存的 step，并返回 provider 事件列表。
pub async fn step_with_prompt_cache_and_events(
    provider: &Arc<dyn AgentProvider>,
    req: AgentProviderRequest,
    provider_timeout_ms: u64,
    state: &mut AgentState,
) -> Result<(AgentStepOutput, Vec<AgentProviderEvent>), CachedProviderError> {
    let mut collector = VecAgentProviderEventCollector::new();
    let output = step_with_prompt_cache_and_collector(
        provider,
        req,
        provider_timeout_ms,
        state,
        &mut collector,
    )
    .await?;
    Ok((output, collector.into_events()))
}

/// 为 provider.step_output 增加 tokio timeout 包装。
async fn timeout_with_step_output(
    provider: &Arc<dyn AgentProvider>,
    req: AgentProviderRequest,
    provider_timeout_ms: u64,
) -> Result<Result<AgentStepOutput, anyhow::Error>, tokio::time::error::Elapsed> {
    timeout(TokioDuration::from_millis(provider_timeout_ms), async {
        provider.step_output(req).await
    })
    .await
}

/// 为 provider.step_output_with_collector 增加 tokio timeout 包装。
async fn timeout_with_provider_output_collector(
    provider: &Arc<dyn AgentProvider>,
    req: AgentProviderRequest,
    provider_timeout_ms: u64,
    collector: &mut dyn AgentProviderEventCollector,
) -> Result<Result<AgentStepOutput, anyhow::Error>, tokio::time::error::Elapsed> {
    timeout(TokioDuration::from_millis(provider_timeout_ms), async {
        provider.step_output_with_collector(req, collector).await
    })
    .await
}
