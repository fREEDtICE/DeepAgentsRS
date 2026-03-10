use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use deepagents::approval::ExecutionMode;
use deepagents::provider::{Provider, ProviderRequest, ProviderStep};
use deepagents::runtime::simple::{SimpleRuntime, SimpleRuntimeOptions};
use deepagents::runtime::{
    CacheBackend, PromptCacheOptions, PromptCachingMiddleware, RunStatus, Runtime, RuntimeConfig,
    RuntimeMiddlewareAssembler, RuntimeMiddlewareSlot,
};
use deepagents::types::Message;

struct StaticProvider;

#[async_trait]
impl Provider for StaticProvider {
    async fn step(&self, _req: ProviderRequest) -> anyhow::Result<ProviderStep> {
        Ok(ProviderStep::FinalText {
            text: "OK".to_string(),
        })
    }
}

struct SingleCallProvider {
    calls: AtomicUsize,
}

impl SingleCallProvider {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Provider for SingleCallProvider {
    async fn step(&self, _req: ProviderRequest) -> anyhow::Result<ProviderStep> {
        let idx = self.calls.fetch_add(1, Ordering::SeqCst);
        if idx >= 1 {
            anyhow::bail!("provider should not be called on cache hit");
        }
        Ok(ProviderStep::FinalText {
            text: "OK".to_string(),
        })
    }
}

fn build_runtime(
    root: &str,
    provider: Arc<dyn Provider>,
    cache_options: PromptCacheOptions,
) -> SimpleRuntime {
    let agent = deepagents::create_deep_agent(root).unwrap();
    let mut asm = RuntimeMiddlewareAssembler::new();
    asm.push(
        RuntimeMiddlewareSlot::PromptCaching,
        "prompt_caching",
        Arc::new(PromptCachingMiddleware::new(cache_options)),
    );
    let mws = asm.build().unwrap();
    SimpleRuntime::new(
        agent,
        provider,
        vec![],
        SimpleRuntimeOptions {
            config: RuntimeConfig {
                max_steps: 2,
                provider_timeout_ms: 1000,
            },
            approval: None,
            audit: None,
            root: root.to_string(),
            mode: ExecutionMode::NonInteractive,
        },
    )
    .with_runtime_middlewares(mws)
}

fn build_runtime_with_state(
    root: &str,
    provider: Arc<dyn Provider>,
    cache_options: PromptCacheOptions,
    state: deepagents::state::AgentState,
) -> SimpleRuntime {
    build_runtime(root, provider, cache_options).with_initial_state(state)
}

fn provider_cache_events(out: &deepagents::runtime::RunOutput) -> Vec<serde_json::Value> {
    out.trace
        .as_ref()
        .and_then(|t| t.get("provider_cache_events"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

fn first_cache_event_of_level(
    out: &deepagents::runtime::RunOutput,
    level: &str,
) -> serde_json::Value {
    provider_cache_events(out)
        .into_iter()
        .find(|e| e.get("cache_level").and_then(|v| v.as_str()) == Some(level))
        .unwrap()
}

#[tokio::test]
async fn pc_01_off_produces_no_cache_events() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let provider: Arc<dyn Provider> = Arc::new(StaticProvider);
    let rt = build_runtime(
        &root,
        provider,
        PromptCacheOptions {
            enabled: false,
            backend: CacheBackend::Memory,
            enable_l2_response_cache: false,
            ttl_ms: 300000,
            max_entries: 1024,
            provider_id: "static".to_string(),
            partition: "t".to_string(),
        },
    );

    let out = rt
        .run(vec![Message {
            role: "system".to_string(),
            content: "SECRET_SHOULD_NOT_LEAK".to_string(),
            content_blocks: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }])
        .await;
    assert_eq!(out.status, RunStatus::Completed);
    let events = provider_cache_events(&out);
    assert!(events.is_empty());
    let trace_str = serde_json::to_string(&out.trace).unwrap();
    assert!(!trace_str.contains("SECRET_SHOULD_NOT_LEAK"));
}

#[tokio::test]
async fn pc_02_l2_hit_short_circuits_provider_on_second_run() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let provider = Arc::new(SingleCallProvider::new());
    let provider_dyn: Arc<dyn Provider> = provider.clone();
    let rt = build_runtime(
        &root,
        provider_dyn,
        PromptCacheOptions {
            enabled: true,
            backend: CacheBackend::Memory,
            enable_l2_response_cache: true,
            ttl_ms: 300000,
            max_entries: 1024,
            provider_id: "single".to_string(),
            partition: "t".to_string(),
        },
    );

    let msgs = vec![
        Message {
            role: "system".to_string(),
            content: "SYS".to_string(),
            content_blocks: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        },
        Message {
            role: "user".to_string(),
            content: "hi".to_string(),
            content_blocks: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        },
    ];
    let out1 = rt.run(msgs.clone()).await;
    assert_eq!(out1.status, RunStatus::Completed);
    assert_eq!(provider.calls(), 1);

    let out2 = rt.run(msgs).await;
    assert_eq!(out2.status, RunStatus::Completed);
    assert_eq!(provider.calls(), 1);

    let e2 = first_cache_event_of_level(&out2, "L2");
    assert_eq!(e2.get("lookup_hit").and_then(|v| v.as_bool()), Some(true));
}

#[tokio::test]
async fn pk_01_l1_hits_when_only_user_message_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let provider: Arc<dyn Provider> = Arc::new(StaticProvider);
    let rt = build_runtime(
        &root,
        provider,
        PromptCacheOptions {
            enabled: true,
            backend: CacheBackend::Memory,
            enable_l2_response_cache: false,
            ttl_ms: 300000,
            max_entries: 1024,
            provider_id: "static".to_string(),
            partition: "t".to_string(),
        },
    );

    let base = vec![Message {
        role: "system".to_string(),
        content: "SYS".to_string(),
        content_blocks: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        status: None,
    }];

    let out1 = rt
        .run(
            [
                base.clone(),
                vec![Message {
                    role: "user".to_string(),
                    content: "a".to_string(),
                    content_blocks: None,
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    status: None,
                }],
            ]
            .concat(),
        )
        .await;
    let out2 = rt
        .run(
            [
                base,
                vec![Message {
                    role: "user".to_string(),
                    content: "b".to_string(),
                    content_blocks: None,
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    status: None,
                }],
            ]
            .concat(),
        )
        .await;

    let e1 = first_cache_event_of_level(&out1, "L1");
    let e2 = first_cache_event_of_level(&out2, "L1");
    assert_eq!(e2.get("lookup_hit").and_then(|v| v.as_bool()), Some(true));
    let c1 = e1.get("components").unwrap();
    let c2 = e2.get("components").unwrap();
    assert_eq!(
        c1.get("system_hash").and_then(|v| v.as_str()),
        c2.get("system_hash").and_then(|v| v.as_str())
    );
    assert_eq!(
        c1.get("tools_hash").and_then(|v| v.as_str()),
        c2.get("tools_hash").and_then(|v| v.as_str())
    );
    assert_ne!(
        c1.get("messages_hash").and_then(|v| v.as_str()),
        c2.get("messages_hash").and_then(|v| v.as_str())
    );
}

#[tokio::test]
async fn pc_03_tools_change_causes_l1_miss() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();

    let options = PromptCacheOptions {
        enabled: true,
        backend: CacheBackend::Memory,
        enable_l2_response_cache: false,
        ttl_ms: 300000,
        max_entries: 1024,
        provider_id: "static".to_string(),
        partition: "t".to_string(),
    };

    let provider: Arc<dyn Provider> = Arc::new(StaticProvider);
    let rt1 = build_runtime(&root, provider.clone(), options.clone());

    let mut state2 = deepagents::state::AgentState::default();
    let st = deepagents::skills::SkillToolSpec {
        name: "extra_tool".to_string(),
        description: "extra".to_string(),
        input_schema: serde_json::json!({}),
        steps: vec![],
        policy: deepagents::skills::SkillToolPolicy::default(),
        skill_name: "s".to_string(),
        source: "x".to_string(),
    };
    state2.extra.insert(
        "skills_tools".to_string(),
        serde_json::to_value(vec![st]).unwrap(),
    );
    let rt2 = build_runtime_with_state(&root, provider, options, state2);

    let msgs = vec![
        Message {
            role: "system".to_string(),
            content: "SYS".to_string(),
            content_blocks: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        },
        Message {
            role: "user".to_string(),
            content: "hi".to_string(),
            content_blocks: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        },
    ];
    let out1 = rt1.run(msgs.clone()).await;
    let out2 = rt2.run(msgs).await;

    let e2 = first_cache_event_of_level(&out2, "L1");
    assert_eq!(e2.get("lookup_hit").and_then(|v| v.as_bool()), Some(false));

    let c1 = first_cache_event_of_level(&out1, "L1")
        .get("components")
        .unwrap()
        .clone();
    let c2 = e2.get("components").unwrap().clone();
    assert_ne!(
        c1.get("tools_hash").and_then(|v| v.as_str()),
        c2.get("tools_hash").and_then(|v| v.as_str())
    );
}

#[tokio::test]
async fn pk_03_system_change_causes_l1_miss() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let provider: Arc<dyn Provider> = Arc::new(StaticProvider);
    let rt = build_runtime(
        &root,
        provider,
        PromptCacheOptions {
            enabled: true,
            backend: CacheBackend::Memory,
            enable_l2_response_cache: false,
            ttl_ms: 300000,
            max_entries: 1024,
            provider_id: "static".to_string(),
            partition: "t".to_string(),
        },
    );

    let out1 = rt
        .run(vec![
            Message {
                role: "system".to_string(),
                content: "A".to_string(),
                content_blocks: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                status: None,
            },
            Message {
                role: "user".to_string(),
                content: "hi".to_string(),
                content_blocks: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                status: None,
            },
        ])
        .await;
    let out2 = rt
        .run(vec![
            Message {
                role: "system".to_string(),
                content: "B".to_string(),
                content_blocks: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                status: None,
            },
            Message {
                role: "user".to_string(),
                content: "hi".to_string(),
                content_blocks: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                status: None,
            },
        ])
        .await;

    let e1 = first_cache_event_of_level(&out1, "L1");
    let e2 = first_cache_event_of_level(&out2, "L1");
    assert_eq!(e2.get("lookup_hit").and_then(|v| v.as_bool()), Some(false));
    assert_ne!(
        e1.get("components")
            .and_then(|c| c.get("system_hash"))
            .and_then(|v| v.as_str()),
        e2.get("components")
            .and_then(|c| c.get("system_hash"))
            .and_then(|v| v.as_str())
    );
}

#[tokio::test]
async fn pc_07_secret_never_appears_in_cache_events() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let provider: Arc<dyn Provider> = Arc::new(StaticProvider);
    let rt = build_runtime(
        &root,
        provider,
        PromptCacheOptions {
            enabled: true,
            backend: CacheBackend::Memory,
            enable_l2_response_cache: true,
            ttl_ms: 300000,
            max_entries: 1024,
            provider_id: "static".to_string(),
            partition: "t".to_string(),
        },
    );

    let out = rt
        .run(vec![
            Message {
                role: "system".to_string(),
                content: "SECRET_TOKEN_ABC123".to_string(),
                content_blocks: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                status: None,
            },
            Message {
                role: "user".to_string(),
                content: "hi".to_string(),
                content_blocks: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                status: None,
            },
        ])
        .await;
    assert_eq!(out.status, RunStatus::Completed);
    let trace_str = serde_json::to_string(&out.trace).unwrap();
    assert!(!trace_str.contains("SECRET_TOKEN_ABC123"));
}
