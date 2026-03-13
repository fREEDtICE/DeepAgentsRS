use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::{timeout, Duration as TokioDuration};

/// Prompt 缓存运行时（当前仅支持内存缓存）。
///
/// 核心目标：
/// - 为 provider 的一次 `step_output` 调用提供可选的两级缓存（L1/L2），并将命中/写入/淘汰/过期等信息写入 `AgentState`。
/// - 缓存键使用“稳定 JSON + SHA256”生成，尽量避免结构体序列化顺序/平台差异带来的抖动。
///
/// 缓存层级含义：
/// - L1：基于 provider_id/system messages/tools 等“相对稳定的上下文”构建 key，
///   存入 `PromptPrefixArtifact`，用于追踪本地前缀复用与 provider-native 句柄。
/// - L2：在 L1 的基础上叠加“非 system messages + summarization_event”等更易变化的输入，缓存 `ProviderStepOutput`。
///
/// 分区（partition）：
/// - 通过 `partition_key(opts)` 将不同的 `partition/backend/ttl/max_entries` 隔离到不同缓存实例，
///   避免不同配置互相污染。
use crate::provider::{
    AgentProvider, AgentProviderEvent, AgentProviderEventCollector, AgentProviderRequest,
    AgentStep, AgentStepOutput, PromptCachePlan, PromptPrefixArtifact,
    ProviderPromptCacheObservation, ProviderPromptCacheSource, ProviderPromptCacheStatus,
    ProviderPromptCacheStrategy, VecAgentProviderEventCollector,
};
use crate::runtime::cache_store::{CacheStore, MemoryCacheStore};
use crate::runtime::stable_hash::stable_json_sha256_hex;
use crate::runtime::{
    push_provider_cache_event, CacheBackend, CacheKeyComponents, CacheLevel, PromptCacheLayoutMode,
    PromptCacheNativeMode, PromptCacheOptions, ProviderCacheEvent, PROMPT_CACHE_OPTIONS_KEY,
};
use crate::state::AgentState;

#[derive(Debug)]
pub enum CachedProviderError {
    /// provider 内部错误（原样包装，便于上层区分超时/错误）。
    Provider(anyhow::Error),
    /// provider 调用超时。
    Timeout,
    /// prompt cache 原生模式要求无法满足。
    PromptCache(anyhow::Error),
}

/// 单个分区对应的一组缓存实例。
///
/// - L1 只存占位，用于“上下文 key”的命中观察
/// - L2 存 `ProviderStepOutput`，可选启用
struct PartitionedPromptCache {
    l1: MemoryCacheStore<PromptPrefixArtifact>,
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

struct ForwardingCaptureCollector<'a> {
    inner: &'a mut dyn AgentProviderEventCollector,
    events: Vec<AgentProviderEvent>,
}

impl<'a> ForwardingCaptureCollector<'a> {
    fn new(inner: &'a mut dyn AgentProviderEventCollector) -> Self {
        Self {
            inner,
            events: Vec::new(),
        }
    }

    fn into_events(self) -> Vec<AgentProviderEvent> {
        self.events
    }
}

#[async_trait]
impl<'a> AgentProviderEventCollector for ForwardingCaptureCollector<'a> {
    async fn emit(&mut self, event: AgentProviderEvent) -> anyhow::Result<()> {
        self.events.push(event.clone());
        self.inner.emit(event).await
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
            native: PromptCacheNativeMode::Auto,
            layout: PromptCacheLayoutMode::Auto,
            enable_l2_response_cache: false,
            ttl_ms: 0,
            max_entries: 0,
            provider_id: String::new(),
            model_id: String::new(),
            partition: String::new(),
        };
    };
    serde_json::from_value(v.clone()).unwrap_or(PromptCacheOptions {
        enabled: false,
        backend: CacheBackend::Memory,
        native: PromptCacheNativeMode::Auto,
        layout: PromptCacheLayoutMode::Auto,
        enable_l2_response_cache: false,
        ttl_ms: 0,
        max_entries: 0,
        provider_id: String::new(),
        model_id: String::new(),
        partition: String::new(),
    })
}

fn build_prompt_cache_plan(
    provider: &Arc<dyn AgentProvider>,
    req: &AgentProviderRequest,
) -> PromptCachePlan {
    provider
        .prompt_cache_plan(req)
        .unwrap_or_else(|_| PromptCachePlan::from_agent_request(req))
}

fn l0_hash_with_provider(opts: &PromptCacheOptions, plan: &PromptCachePlan) -> String {
    let v = serde_json::json!({
        "provider_id": opts.provider_id,
        "model_id": opts.model_id,
        "l0_hash": plan.l0_hash,
    });
    stable_json_sha256_hex(&v)
}

fn l1_hash_with_l0(l0_hash: &str, plan: &PromptCachePlan) -> String {
    let v = serde_json::json!({
        "l0_hash": l0_hash,
        "l1_hash": plan.l1_hash,
    });
    stable_json_sha256_hex(&v)
}

fn l2_hash_with_l1(l1_hash: &str, plan: &PromptCachePlan) -> String {
    let v = serde_json::json!({
        "l1_hash": l1_hash,
        "l2_hash": plan.l2_hash,
    });
    stable_json_sha256_hex(&v)
}

fn fallback_provider_observation(
    opts: &PromptCacheOptions,
    plan: &PromptCachePlan,
) -> Option<ProviderPromptCacheObservation> {
    if opts.native == PromptCacheNativeMode::Off {
        return None;
    }
    Some(ProviderPromptCacheObservation {
        cache_source: ProviderPromptCacheSource::Local,
        provider_strategy: plan.provider_strategy,
        provider_cache_status: ProviderPromptCacheStatus::Unsupported,
        provider_handle_hash: None,
        provider_handle: None,
    })
}

fn event_fields(
    observation: Option<&ProviderPromptCacheObservation>,
    local: bool,
    default_strategy: Option<ProviderPromptCacheStrategy>,
) -> (
    Option<ProviderPromptCacheSource>,
    Option<ProviderPromptCacheStrategy>,
    Option<ProviderPromptCacheStatus>,
    Option<String>,
) {
    if let Some(obs) = observation {
        let provider_handle_hash = obs
            .provider_handle_hash
            .clone()
            .or_else(|| obs.provider_handle.as_ref().map(|h| h.hash()));
        return (
            Some(obs.cache_source),
            Some(obs.provider_strategy),
            Some(obs.provider_cache_status),
            provider_handle_hash,
        );
    }
    if local {
        return (
            Some(ProviderPromptCacheSource::Local),
            default_strategy,
            None,
            None,
        );
    }
    (None, default_strategy, None, None)
}

/// 在 native=required 时，如果 provider 根本没有声明任何原生策略，则直接失败。
fn fail_if_native_required_without_strategy(
    opts: &PromptCacheOptions,
    plan: &PromptCachePlan,
) -> Result<(), CachedProviderError> {
    if opts.native == PromptCacheNativeMode::Required
        && plan.provider_strategy == ProviderPromptCacheStrategy::None
    {
        return Err(CachedProviderError::PromptCache(anyhow::anyhow!(
            "prompt_cache_native_required_but_provider_has_no_native_strategy"
        )));
    }
    Ok(())
}

/// 原生前缀缓存即使还没有 handle，也可能需要在首个请求上打 hint。
fn apply_native_hint(
    provider: &Arc<dyn AgentProvider>,
    req: AgentProviderRequest,
    opts: &PromptCacheOptions,
    artifact: Option<&PromptPrefixArtifact>,
) -> AgentProviderRequest {
    if opts.native == PromptCacheNativeMode::Off {
        return req;
    }
    match artifact {
        Some(artifact) => provider.apply_prompt_cache_hint(req, &artifact.hint()),
        None => req,
    }
}

/// provider 执行完成后统一提取原生缓存观测；auto 模式允许 unsupported，required 模式会报错。
fn observe_native_result(
    provider: &Arc<dyn AgentProvider>,
    output: &AgentStepOutput,
    events: &[AgentProviderEvent],
    opts: &PromptCacheOptions,
    plan: &PromptCachePlan,
) -> Result<Option<ProviderPromptCacheObservation>, CachedProviderError> {
    let observation = provider
        .observe_prompt_cache_result(output, events)
        .or_else(|| fallback_provider_observation(opts, plan));

    if opts.native == PromptCacheNativeMode::Required {
        let supported = observation
            .as_ref()
            .map(|obs| obs.provider_cache_status != ProviderPromptCacheStatus::Unsupported)
            .unwrap_or(false);
        if !supported {
            return Err(CachedProviderError::PromptCache(anyhow::anyhow!(
                "prompt_cache_native_required_but_provider_did_not_observe_native_cache"
            )));
        }
    }

    Ok(observation)
}

/// L1 只在 provider 返回新的原生句柄时刷新，避免覆盖已知句柄。
fn refresh_l1_artifact(
    cache: &PartitionedPromptCache,
    l1_hash: &str,
    observation: Option<&ProviderPromptCacheObservation>,
) {
    let Some(observation) = observation else {
        return;
    };
    let Some(handle) = observation.provider_handle.clone() else {
        return;
    };
    let artifact = PromptPrefixArtifact::new(
        l1_hash.to_string(),
        observation.provider_strategy,
        Some(handle),
    );
    cache.l1.insert(l1_hash.to_string(), artifact);
}

/// 统一写入 L1 事件，保证本地 lookup 与 provider-native 状态进入同一条 trace 流。
#[allow(clippy::too_many_arguments)]
fn push_l1_event(
    state: &mut AgentState,
    opts: &PromptCacheOptions,
    plan: &PromptCachePlan,
    l0_hash: &str,
    l1_hash: &str,
    lookup_hit: bool,
    inserted: Option<bool>,
    evicted: Option<u64>,
    expired: bool,
    observation: Option<&ProviderPromptCacheObservation>,
) {
    let components = CacheKeyComponents {
        l0_hash: l0_hash.to_string(),
        l1_hash: l1_hash.to_string(),
        l2_hash: None,
    };
    let (cache_source, provider_strategy, provider_cache_status, provider_handle_hash) =
        event_fields(observation, true, Some(plan.provider_strategy));
    push_provider_cache_event(
        state,
        ProviderCacheEvent::ProviderCache {
            cache_backend: opts.backend,
            cache_level: CacheLevel::L1,
            lookup_hit,
            cache_key_hash: l1_hash.to_string(),
            components,
            cache_source,
            provider_strategy,
            provider_cache_status,
            provider_handle_hash,
            inserted,
            evicted,
            expired: if expired { Some(true) } else { None },
        },
    );
}

/// L2 事件只在启用响应缓存时写入；provider-native 观测会附加到 miss/insert 事件上。
#[allow(clippy::too_many_arguments)]
fn push_l2_event(
    state: &mut AgentState,
    opts: &PromptCacheOptions,
    plan: &PromptCachePlan,
    l0_hash: &str,
    l1_hash: &str,
    l2_hash: &str,
    lookup_hit: bool,
    inserted: Option<bool>,
    evicted: Option<u64>,
    expired: bool,
    observation: Option<&ProviderPromptCacheObservation>,
) {
    let components = CacheKeyComponents {
        l0_hash: l0_hash.to_string(),
        l1_hash: l1_hash.to_string(),
        l2_hash: Some(l2_hash.to_string()),
    };
    let (cache_source, provider_strategy, provider_cache_status, provider_handle_hash) =
        event_fields(observation, !lookup_hit, Some(plan.provider_strategy));
    push_provider_cache_event(
        state,
        ProviderCacheEvent::ProviderCache {
            cache_backend: opts.backend,
            cache_level: CacheLevel::L2,
            lookup_hit,
            cache_key_hash: l2_hash.to_string(),
            components,
            cache_source,
            provider_strategy,
            provider_cache_status,
            provider_handle_hash,
            inserted,
            evicted,
            expired: if expired { Some(true) } else { None },
        },
    );
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

    let plan = build_prompt_cache_plan(provider, &req);
    let l0_hash = l0_hash_with_provider(&opts, &plan);
    let l1_hash = l1_hash_with_l0(&l0_hash, &plan);
    let l2_hash = l2_hash_with_l1(&l1_hash, &plan);
    let pkey = partition_key(&opts);
    let cache = PromptCacheRegistry::global().partition(pkey, &opts);

    let l1_lookup = cache.l1.get(&l1_hash);
    let mut l1_event_inserted: Option<bool> = None;
    let mut l1_event_evicted: Option<u64> = None;
    let mut l1_artifact = l1_lookup.value.clone();
    if l1_artifact.is_none() {
        let artifact = PromptPrefixArtifact::new(l1_hash.clone(), plan.provider_strategy, None);
        let ins = cache.l1.insert(l1_hash.clone(), artifact.clone());
        l1_artifact = Some(artifact);
        l1_event_inserted = Some(ins.inserted);
        if ins.evicted > 0 {
            l1_event_evicted = Some(ins.evicted);
        }
    }

    if let Err(err) = fail_if_native_required_without_strategy(&opts, &plan) {
        let observation = fallback_provider_observation(&opts, &plan);
        push_l1_event(
            state,
            &opts,
            &plan,
            &l0_hash,
            &l1_hash,
            l1_lookup.value.is_some(),
            l1_event_inserted,
            l1_event_evicted,
            l1_lookup.expired,
            observation.as_ref(),
        );
        return Err(err);
    }

    let mut l2_lookup_expired = false;
    if opts.enable_l2_response_cache {
        let l2_lookup = cache.l2.get(&l2_hash);
        l2_lookup_expired = l2_lookup.expired;
        if let Some(output) = l2_lookup.value {
            push_l1_event(
                state,
                &opts,
                &plan,
                &l0_hash,
                &l1_hash,
                l1_lookup.value.is_some(),
                l1_event_inserted,
                l1_event_evicted,
                l1_lookup.expired,
                None,
            );
            push_l2_event(
                state,
                &opts,
                &plan,
                &l0_hash,
                &l1_hash,
                &l2_hash,
                true,
                None,
                None,
                l2_lookup.expired,
                None,
            );
            return Ok(output);
        }
    }

    let req_to_send = apply_native_hint(provider, req, &opts, l1_artifact.as_ref());
    let mut collector = VecAgentProviderEventCollector::new();
    let output = match timeout_with_provider_output_collector(
        provider,
        req_to_send,
        provider_timeout_ms,
        &mut collector,
    )
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            let observation = fallback_provider_observation(&opts, &plan);
            push_l1_event(
                state,
                &opts,
                &plan,
                &l0_hash,
                &l1_hash,
                l1_lookup.value.is_some(),
                l1_event_inserted,
                l1_event_evicted,
                l1_lookup.expired,
                observation.as_ref(),
            );
            if opts.enable_l2_response_cache {
                push_l2_event(
                    state,
                    &opts,
                    &plan,
                    &l0_hash,
                    &l1_hash,
                    &l2_hash,
                    false,
                    Some(false),
                    None,
                    l2_lookup_expired,
                    observation.as_ref(),
                );
            }
            return Err(CachedProviderError::Provider(e));
        }
        Err(_) => {
            let observation = fallback_provider_observation(&opts, &plan);
            push_l1_event(
                state,
                &opts,
                &plan,
                &l0_hash,
                &l1_hash,
                l1_lookup.value.is_some(),
                l1_event_inserted,
                l1_event_evicted,
                l1_lookup.expired,
                observation.as_ref(),
            );
            if opts.enable_l2_response_cache {
                push_l2_event(
                    state,
                    &opts,
                    &plan,
                    &l0_hash,
                    &l1_hash,
                    &l2_hash,
                    false,
                    Some(false),
                    None,
                    l2_lookup_expired,
                    observation.as_ref(),
                );
            }
            return Err(CachedProviderError::Timeout);
        }
    };

    let events = collector.into_events();
    let observation = match observe_native_result(provider, &output, &events, &opts, &plan) {
        Ok(observation) => observation,
        Err(err) => {
            let observation = fallback_provider_observation(&opts, &plan);
            push_l1_event(
                state,
                &opts,
                &plan,
                &l0_hash,
                &l1_hash,
                l1_lookup.value.is_some(),
                l1_event_inserted,
                l1_event_evicted,
                l1_lookup.expired,
                observation.as_ref(),
            );
            return Err(err);
        }
    };

    refresh_l1_artifact(&cache, &l1_hash, observation.as_ref());
    push_l1_event(
        state,
        &opts,
        &plan,
        &l0_hash,
        &l1_hash,
        l1_lookup.value.is_some(),
        l1_event_inserted,
        l1_event_evicted,
        l1_lookup.expired,
        observation.as_ref(),
    );

    if opts.enable_l2_response_cache {
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
        push_l2_event(
            state,
            &opts,
            &plan,
            &l0_hash,
            &l1_hash,
            &l2_hash,
            false,
            inserted,
            evicted,
            l2_lookup_expired,
            observation.as_ref(),
        );
    }

    Ok(output)
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

    let plan = build_prompt_cache_plan(provider, &req);
    let l0_hash = l0_hash_with_provider(&opts, &plan);
    let l1_hash = l1_hash_with_l0(&l0_hash, &plan);
    let l2_hash = l2_hash_with_l1(&l1_hash, &plan);
    let pkey = partition_key(&opts);
    let cache = PromptCacheRegistry::global().partition(pkey, &opts);

    let l1_lookup = cache.l1.get(&l1_hash);
    let mut l1_event_inserted: Option<bool> = None;
    let mut l1_event_evicted: Option<u64> = None;
    let mut l1_artifact = l1_lookup.value.clone();
    if l1_artifact.is_none() {
        let artifact = PromptPrefixArtifact::new(l1_hash.clone(), plan.provider_strategy, None);
        let ins = cache.l1.insert(l1_hash.clone(), artifact.clone());
        l1_artifact = Some(artifact);
        l1_event_inserted = Some(ins.inserted);
        if ins.evicted > 0 {
            l1_event_evicted = Some(ins.evicted);
        }
    }
    if let Err(err) = fail_if_native_required_without_strategy(&opts, &plan) {
        let observation = fallback_provider_observation(&opts, &plan);
        push_l1_event(
            state,
            &opts,
            &plan,
            &l0_hash,
            &l1_hash,
            l1_lookup.value.is_some(),
            l1_event_inserted,
            l1_event_evicted,
            l1_lookup.expired,
            observation.as_ref(),
        );
        return Err(err);
    }

    let mut l2_lookup_expired = false;
    if opts.enable_l2_response_cache {
        let l2_lookup = cache.l2.get(&l2_hash);
        l2_lookup_expired = l2_lookup.expired;
        if let Some(output) = l2_lookup.value {
            push_l1_event(
                state,
                &opts,
                &plan,
                &l0_hash,
                &l1_hash,
                l1_lookup.value.is_some(),
                l1_event_inserted,
                l1_event_evicted,
                l1_lookup.expired,
                None,
            );
            push_l2_event(
                state,
                &opts,
                &plan,
                &l0_hash,
                &l1_hash,
                &l2_hash,
                true,
                None,
                None,
                l2_lookup.expired,
                None,
            );
            return Ok(output);
        }
    }

    let req_to_send = apply_native_hint(provider, req, &opts, l1_artifact.as_ref());
    let mut forwarding_collector = ForwardingCaptureCollector::new(collector);
    let output = match timeout_with_provider_output_collector(
        provider,
        req_to_send,
        provider_timeout_ms,
        &mut forwarding_collector,
    )
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            let observation = fallback_provider_observation(&opts, &plan);
            push_l1_event(
                state,
                &opts,
                &plan,
                &l0_hash,
                &l1_hash,
                l1_lookup.value.is_some(),
                l1_event_inserted,
                l1_event_evicted,
                l1_lookup.expired,
                observation.as_ref(),
            );
            if opts.enable_l2_response_cache {
                push_l2_event(
                    state,
                    &opts,
                    &plan,
                    &l0_hash,
                    &l1_hash,
                    &l2_hash,
                    false,
                    Some(false),
                    None,
                    l2_lookup_expired,
                    observation.as_ref(),
                );
            }
            return Err(CachedProviderError::Provider(e));
        }
        Err(_) => {
            let observation = fallback_provider_observation(&opts, &plan);
            push_l1_event(
                state,
                &opts,
                &plan,
                &l0_hash,
                &l1_hash,
                l1_lookup.value.is_some(),
                l1_event_inserted,
                l1_event_evicted,
                l1_lookup.expired,
                observation.as_ref(),
            );
            if opts.enable_l2_response_cache {
                push_l2_event(
                    state,
                    &opts,
                    &plan,
                    &l0_hash,
                    &l1_hash,
                    &l2_hash,
                    false,
                    Some(false),
                    None,
                    l2_lookup_expired,
                    observation.as_ref(),
                );
            }
            return Err(CachedProviderError::Timeout);
        }
    };

    let events = forwarding_collector.into_events();
    let observation = match observe_native_result(provider, &output, &events, &opts, &plan) {
        Ok(observation) => observation,
        Err(err) => {
            let observation = fallback_provider_observation(&opts, &plan);
            push_l1_event(
                state,
                &opts,
                &plan,
                &l0_hash,
                &l1_hash,
                l1_lookup.value.is_some(),
                l1_event_inserted,
                l1_event_evicted,
                l1_lookup.expired,
                observation.as_ref(),
            );
            return Err(err);
        }
    };

    refresh_l1_artifact(&cache, &l1_hash, observation.as_ref());
    push_l1_event(
        state,
        &opts,
        &plan,
        &l0_hash,
        &l1_hash,
        l1_lookup.value.is_some(),
        l1_event_inserted,
        l1_event_evicted,
        l1_lookup.expired,
        observation.as_ref(),
    );

    if opts.enable_l2_response_cache {
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
        push_l2_event(
            state,
            &opts,
            &plan,
            &l0_hash,
            &l1_hash,
            &l2_hash,
            false,
            inserted,
            evicted,
            l2_lookup_expired,
            observation.as_ref(),
        );
    }

    Ok(output)
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
