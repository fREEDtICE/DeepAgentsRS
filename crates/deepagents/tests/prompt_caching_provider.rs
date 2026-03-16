use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use deepagents::approval::ExecutionMode;
use deepagents::provider::{
    AgentProvider, AgentProviderEvent, AgentProviderEventCollector, AgentProviderRequest,
    AgentStep, AgentStepOutput, PromptCachePlan, ProviderPromptCacheHandle,
    ProviderPromptCacheHint, ProviderPromptCacheObservation, ProviderPromptCacheSource,
    ProviderPromptCacheStatus, ProviderPromptCacheStrategy,
};
use deepagents::runtime::simple::{SimpleRuntime, SimpleRuntimeOptions};
use deepagents::runtime::skills_middleware::SkillsMiddleware;
use deepagents::runtime::{
    CacheBackend, PromptCacheLayoutMode, PromptCacheNativeMode, PromptCacheOptions,
    PromptCachingMiddleware, ProviderStepKind, RunEvent, RunStatus, Runtime, RuntimeConfig,
    RuntimeMiddlewareAssembler, RuntimeMiddlewareSlot, VecRunEventSink,
};
use deepagents::skills::loader::SkillsLoadOptions;
use deepagents::types::Message;

struct StaticProvider;

#[async_trait]
impl AgentProvider for StaticProvider {
    async fn step(&self, _req: AgentProviderRequest) -> anyhow::Result<AgentStep> {
        Ok(AgentStep::FinalText {
            text: "OK".to_string(),
        })
    }
}

#[derive(Default)]
struct NativeHintProvider {
    step_calls: AtomicUsize,
    observe_calls: AtomicUsize,
    applied_handle_presence: Mutex<Vec<bool>>,
    observed_event_counts: Mutex<Vec<usize>>,
}

impl NativeHintProvider {
    fn step_calls(&self) -> usize {
        self.step_calls.load(Ordering::SeqCst)
    }

    fn applied_handle_presence(&self) -> Vec<bool> {
        self.applied_handle_presence.lock().unwrap().clone()
    }

    fn observed_event_counts(&self) -> Vec<usize> {
        self.observed_event_counts.lock().unwrap().clone()
    }
}

#[async_trait]
impl AgentProvider for NativeHintProvider {
    async fn step(&self, _req: AgentProviderRequest) -> anyhow::Result<AgentStep> {
        Ok(AgentStep::FinalText {
            text: "OK".to_string(),
        })
    }

    async fn step_output_with_collector(
        &self,
        req: AgentProviderRequest,
        collector: &mut dyn AgentProviderEventCollector,
    ) -> anyhow::Result<AgentStepOutput> {
        self.step_calls.fetch_add(1, Ordering::SeqCst);
        collector
            .emit(AgentProviderEvent::Usage {
                input_tokens: Some(3),
                output_tokens: Some(1),
                total_tokens: Some(4),
            })
            .await?;
        Ok(AgentStepOutput::from(AgentStep::FinalText {
            text: req
                .messages
                .last()
                .map(|message| message.content.clone())
                .unwrap_or_else(|| "OK".to_string()),
        }))
    }

    fn prompt_cache_plan(&self, req: &AgentProviderRequest) -> anyhow::Result<PromptCachePlan> {
        let prefix_len = req
            .messages
            .iter()
            .take_while(|message| message.role == "system" || message.role == "developer")
            .count();
        Ok(PromptCachePlan::new(
            serde_json::json!({
                "tool_choice": req.tool_choice,
            }),
            serde_json::json!({
                "prefix_messages": req.messages.iter().take(prefix_len).cloned().collect::<Vec<_>>(),
                "tools": req.tool_specs,
            }),
            serde_json::json!({
                "messages": req.messages.iter().skip(prefix_len).cloned().collect::<Vec<_>>(),
            }),
            ProviderPromptCacheStrategy::CacheControl,
        ))
    }

    fn apply_prompt_cache_hint(
        &self,
        req: AgentProviderRequest,
        hint: &ProviderPromptCacheHint,
    ) -> AgentProviderRequest {
        self.applied_handle_presence
            .lock()
            .unwrap()
            .push(hint.handle.is_some());
        req
    }

    fn observe_prompt_cache_result(
        &self,
        _output: &AgentStepOutput,
        events: &[AgentProviderEvent],
    ) -> Option<ProviderPromptCacheObservation> {
        self.observed_event_counts
            .lock()
            .unwrap()
            .push(events.len());
        let observe_idx = self.observe_calls.fetch_add(1, Ordering::SeqCst);
        Some(ProviderPromptCacheObservation {
            cache_source: if observe_idx == 0 {
                ProviderPromptCacheSource::Provider
            } else {
                ProviderPromptCacheSource::Hybrid
            },
            provider_strategy: ProviderPromptCacheStrategy::CacheControl,
            provider_cache_status: if observe_idx == 0 {
                ProviderPromptCacheStatus::Applied
            } else {
                ProviderPromptCacheStatus::Hit
            },
            provider_handle_hash: None,
            provider_handle: Some(ProviderPromptCacheHandle {
                payload: serde_json::json!({
                    "cache_id": "native-prefix-1",
                }),
            }),
        })
    }
}

struct RequiredUnsupportedProvider {
    calls: AtomicUsize,
}

impl RequiredUnsupportedProvider {
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
impl AgentProvider for RequiredUnsupportedProvider {
    async fn step(&self, _req: AgentProviderRequest) -> anyhow::Result<AgentStep> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(AgentStep::FinalText {
            text: "OK".to_string(),
        })
    }

    fn prompt_cache_plan(&self, req: &AgentProviderRequest) -> anyhow::Result<PromptCachePlan> {
        let prefix_len = req
            .messages
            .iter()
            .take_while(|message| message.role == "system" || message.role == "developer")
            .count();
        Ok(PromptCachePlan::new(
            serde_json::json!({}),
            serde_json::json!({
                "prefix_messages": req.messages.iter().take(prefix_len).cloned().collect::<Vec<_>>(),
            }),
            serde_json::json!({
                "messages": req.messages.iter().skip(prefix_len).cloned().collect::<Vec<_>>(),
            }),
            ProviderPromptCacheStrategy::StablePrefix,
        ))
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
impl AgentProvider for SingleCallProvider {
    async fn step(&self, _req: AgentProviderRequest) -> anyhow::Result<AgentStep> {
        let idx = self.calls.fetch_add(1, Ordering::SeqCst);
        if idx >= 1 {
            anyhow::bail!("provider should not be called on cache hit");
        }
        Ok(AgentStep::FinalText {
            text: "OK".to_string(),
        })
    }
}

#[derive(Default)]
struct StreamingSingleCallProvider {
    step_calls: AtomicUsize,
    collector_calls: AtomicUsize,
}

#[async_trait]
impl AgentProvider for StreamingSingleCallProvider {
    async fn step(&self, _req: AgentProviderRequest) -> anyhow::Result<AgentStep> {
        self.step_calls.fetch_add(1, Ordering::SeqCst);
        Ok(AgentStep::FinalText {
            text: "OK".to_string(),
        })
    }

    async fn step_with_collector(
        &self,
        _req: AgentProviderRequest,
        collector: &mut dyn AgentProviderEventCollector,
    ) -> anyhow::Result<AgentStep> {
        let idx = self.collector_calls.fetch_add(1, Ordering::SeqCst);
        if idx >= 1 {
            anyhow::bail!("provider should not be called on cache hit");
        }
        collector
            .emit(
                deepagents::provider::AgentProviderEvent::AssistantTextDelta {
                    text: "OK".to_string(),
                },
            )
            .await?;
        collector
            .emit(deepagents::provider::AgentProviderEvent::Usage {
                input_tokens: Some(2),
                output_tokens: Some(1),
                total_tokens: Some(3),
            })
            .await?;
        Ok(AgentStep::FinalText {
            text: "OK".to_string(),
        })
    }

    async fn step_output_with_collector(
        &self,
        req: AgentProviderRequest,
        collector: &mut dyn AgentProviderEventCollector,
    ) -> anyhow::Result<AgentStepOutput> {
        Ok(self.step_with_collector(req, collector).await?.into())
    }
}

fn build_runtime(
    root: &str,
    provider: Arc<dyn AgentProvider>,
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
    provider: Arc<dyn AgentProvider>,
    cache_options: PromptCacheOptions,
    state: deepagents::state::AgentState,
) -> SimpleRuntime {
    build_runtime(root, provider, cache_options).with_initial_state(state)
}

fn build_runtime_with_skills(
    root: &str,
    provider: Arc<dyn AgentProvider>,
    cache_options: PromptCacheOptions,
    skills_source: String,
) -> SimpleRuntime {
    let agent = deepagents::create_deep_agent(root).unwrap();
    let mut asm = RuntimeMiddlewareAssembler::new();
    asm.push(
        RuntimeMiddlewareSlot::Skills,
        "skills",
        Arc::new(SkillsMiddleware::new(
            vec![skills_source],
            SkillsLoadOptions::default(),
        )),
    );
    asm.push(
        RuntimeMiddlewareSlot::PromptCaching,
        "prompt_caching",
        Arc::new(PromptCachingMiddleware::new(cache_options)),
    );
    let mws = asm.build().unwrap();
    SimpleRuntime::new(
        agent,
        provider,
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

fn system_and_user_messages(user: &str) -> Vec<Message> {
    vec![
        Message {
            role: "system".to_string(),
            content: "SYS".to_string(),
            content_blocks: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        },
        Message {
            role: "user".to_string(),
            content: user.to_string(),
            content_blocks: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        },
    ]
}

fn write_skill_package(
    source_dir: &std::path::Path,
    skill_name: &str,
    description: &str,
    tool_name: &str,
) {
    let skill_dir = source_dir.join(skill_name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: {skill_name}\ndescription: {description}\n---\n\n# {skill_name}\n"),
    )
    .unwrap();
    std::fs::write(
        skill_dir.join("tools.json"),
        format!(
            r#"{{
                "tools": [{{
                    "name": "{tool_name}",
                    "description": "{description}",
                    "input_schema": {{ "type": "object", "properties": {{}}, "required": [] }},
                    "steps": [],
                    "policy": {{}}
                }}]
            }}"#
        ),
    )
    .unwrap();
}

#[tokio::test]
async fn pc_01_off_produces_no_cache_events() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let provider: Arc<dyn AgentProvider> = Arc::new(StaticProvider);
    let rt = build_runtime(
        &root,
        provider,
        PromptCacheOptions {
            enabled: false,
            backend: CacheBackend::Memory,
            native: PromptCacheNativeMode::Auto,
            layout: PromptCacheLayoutMode::Auto,
            enable_l2_response_cache: false,
            ttl_ms: 300000,
            max_entries: 1024,
            provider_id: "static".to_string(),
            model_id: String::new(),
            partition: "t".to_string(),
        },
    );

    let out = rt
        .run(vec![Message {
            role: "system".to_string(),
            content: "SECRET_SHOULD_NOT_LEAK".to_string(),
            content_blocks: None,
            reasoning_content: None,
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
    let provider_dyn: Arc<dyn AgentProvider> = provider.clone();
    let rt = build_runtime(
        &root,
        provider_dyn,
        PromptCacheOptions {
            enabled: true,
            backend: CacheBackend::Memory,
            native: PromptCacheNativeMode::Auto,
            layout: PromptCacheLayoutMode::Auto,
            enable_l2_response_cache: true,
            ttl_ms: 300000,
            max_entries: 1024,
            provider_id: "single".to_string(),
            model_id: String::new(),
            partition: "t".to_string(),
        },
    );

    let msgs = vec![
        Message {
            role: "system".to_string(),
            content: "SYS".to_string(),
            content_blocks: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        },
        Message {
            role: "user".to_string(),
            content: "hi".to_string(),
            content_blocks: None,
            reasoning_content: None,
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
    let provider: Arc<dyn AgentProvider> = Arc::new(StaticProvider);
    let rt = build_runtime(
        &root,
        provider,
        PromptCacheOptions {
            enabled: true,
            backend: CacheBackend::Memory,
            native: PromptCacheNativeMode::Auto,
            layout: PromptCacheLayoutMode::Auto,
            enable_l2_response_cache: false,
            ttl_ms: 300000,
            max_entries: 1024,
            provider_id: "static".to_string(),
            model_id: String::new(),
            partition: "t".to_string(),
        },
    );

    let base = vec![Message {
        role: "system".to_string(),
        content: "SYS".to_string(),
        content_blocks: None,
        reasoning_content: None,
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
                    reasoning_content: None,
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
                    reasoning_content: None,
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
        c1.get("l1_hash").and_then(|v| v.as_str()),
        c2.get("l1_hash").and_then(|v| v.as_str())
    );
}

#[tokio::test]
async fn pc_03_tools_change_causes_l1_miss() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();

    let options = PromptCacheOptions {
        enabled: true,
        backend: CacheBackend::Memory,
        native: PromptCacheNativeMode::Auto,
        layout: PromptCacheLayoutMode::Auto,
        enable_l2_response_cache: false,
        ttl_ms: 300000,
        max_entries: 1024,
        provider_id: "static".to_string(),
        model_id: String::new(),
        partition: "t".to_string(),
    };

    let provider: Arc<dyn AgentProvider> = Arc::new(StaticProvider);
    let rt1 = build_runtime(&root, provider.clone(), options.clone());

    let mut state2 = deepagents::state::AgentState::default();
    let st = deepagents::skills::SkillToolSpec {
        name: "extra_tool".to_string(),
        description: "extra".to_string(),
        input_schema: serde_json::json!({}),
        steps: vec![],
        policy: deepagents::skills::SkillToolPolicy::default(),
        skill_name: "s".to_string(),
        skill_version: "0.0.0".to_string(),
        source: "x".to_string(),
        requires_isolation: false,
        subagent_type: None,
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
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        },
        Message {
            role: "user".to_string(),
            content: "hi".to_string(),
            content_blocks: None,
            reasoning_content: None,
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
        c1.get("l1_hash").and_then(|v| v.as_str()),
        c2.get("l1_hash").and_then(|v| v.as_str())
    );
}

#[tokio::test]
async fn pc_04_identical_skill_sources_have_stable_l1_hash() {
    let dir = tempfile::tempdir().unwrap();
    let root_a = dir.path().join("root-a");
    let root_b = dir.path().join("root-b");
    std::fs::create_dir_all(&root_a).unwrap();
    std::fs::create_dir_all(&root_b).unwrap();

    let skills_a = root_a.join("skills");
    let skills_b = root_b.join("skills");
    write_skill_package(&skills_a, "zeta-skill", "Zeta", "zeta-tool");
    write_skill_package(&skills_a, "alpha-skill", "Alpha", "alpha-tool");
    write_skill_package(&skills_b, "alpha-skill", "Alpha", "alpha-tool");
    write_skill_package(&skills_b, "zeta-skill", "Zeta", "zeta-tool");

    let options = PromptCacheOptions {
        enabled: true,
        backend: CacheBackend::Memory,
        native: PromptCacheNativeMode::Auto,
        layout: PromptCacheLayoutMode::Auto,
        enable_l2_response_cache: false,
        ttl_ms: 300000,
        max_entries: 1024,
        provider_id: "static".to_string(),
        model_id: String::new(),
        partition: "t".to_string(),
    };

    let provider: Arc<dyn AgentProvider> = Arc::new(StaticProvider);
    let runtime_a = build_runtime_with_skills(
        &root_a.to_string_lossy(),
        provider.clone(),
        options.clone(),
        skills_a.to_string_lossy().to_string(),
    );
    let runtime_b = build_runtime_with_skills(
        &root_b.to_string_lossy(),
        provider,
        options,
        skills_b.to_string_lossy().to_string(),
    );

    let messages = vec![Message {
        role: "user".to_string(),
        content: "hi".to_string(),
        content_blocks: None,
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        status: None,
    }];

    let out_a = runtime_a.run(messages.clone()).await;
    let out_b = runtime_b.run(messages).await;

    let l1_a = first_cache_event_of_level(&out_a, "L1");
    let l1_b = first_cache_event_of_level(&out_b, "L1");
    assert_eq!(
        l1_a.get("components")
            .and_then(|value| value.get("l1_hash"))
            .and_then(|value| value.as_str()),
        l1_b.get("components")
            .and_then(|value| value.get("l1_hash"))
            .and_then(|value| value.as_str())
    );
}

#[tokio::test]
async fn pk_03_system_change_causes_l1_miss() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let provider: Arc<dyn AgentProvider> = Arc::new(StaticProvider);
    let rt = build_runtime(
        &root,
        provider,
        PromptCacheOptions {
            enabled: true,
            backend: CacheBackend::Memory,
            native: PromptCacheNativeMode::Auto,
            layout: PromptCacheLayoutMode::Auto,
            enable_l2_response_cache: false,
            ttl_ms: 300000,
            max_entries: 1024,
            provider_id: "static".to_string(),
            model_id: String::new(),
            partition: "t".to_string(),
        },
    );

    let out1 = rt
        .run(vec![
            Message {
                role: "system".to_string(),
                content: "A".to_string(),
                content_blocks: None,
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                status: None,
            },
            Message {
                role: "user".to_string(),
                content: "hi".to_string(),
                content_blocks: None,
                reasoning_content: None,
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
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                status: None,
            },
            Message {
                role: "user".to_string(),
                content: "hi".to_string(),
                content_blocks: None,
                reasoning_content: None,
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
            .and_then(|c| c.get("l1_hash"))
            .and_then(|v| v.as_str()),
        e2.get("components")
            .and_then(|c| c.get("l1_hash"))
            .and_then(|v| v.as_str())
    );
}

#[tokio::test]
async fn pc_07_secret_never_appears_in_cache_events() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let provider: Arc<dyn AgentProvider> = Arc::new(StaticProvider);
    let rt = build_runtime(
        &root,
        provider,
        PromptCacheOptions {
            enabled: true,
            backend: CacheBackend::Memory,
            native: PromptCacheNativeMode::Auto,
            layout: PromptCacheLayoutMode::Auto,
            enable_l2_response_cache: true,
            ttl_ms: 300000,
            max_entries: 1024,
            provider_id: "static".to_string(),
            model_id: String::new(),
            partition: "t".to_string(),
        },
    );

    let out = rt
        .run(vec![
            Message {
                role: "system".to_string(),
                content: "SECRET_TOKEN_ABC123".to_string(),
                content_blocks: None,
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                status: None,
            },
            Message {
                role: "user".to_string(),
                content: "hi".to_string(),
                content_blocks: None,
                reasoning_content: None,
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

#[tokio::test]
async fn pc_09_provider_native_unsupported_emits_status() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let provider: Arc<dyn AgentProvider> = Arc::new(StaticProvider);
    let rt = build_runtime(
        &root,
        provider,
        PromptCacheOptions {
            enabled: true,
            backend: CacheBackend::Memory,
            native: PromptCacheNativeMode::Auto,
            layout: PromptCacheLayoutMode::Auto,
            enable_l2_response_cache: true,
            ttl_ms: 300000,
            max_entries: 1024,
            provider_id: "static".to_string(),
            model_id: String::new(),
            partition: "t".to_string(),
        },
    );

    let out = rt
        .run(vec![Message {
            role: "user".to_string(),
            content: "hi".to_string(),
            content_blocks: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }])
        .await;
    assert_eq!(out.status, RunStatus::Completed);
    let e2 = first_cache_event_of_level(&out, "L2");
    assert_eq!(
        e2.get("provider_cache_status").and_then(|v| v.as_str()),
        Some("unsupported")
    );
}

#[tokio::test]
async fn pc_08_l2_hit_in_streaming_runtime_skips_delta_replay() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let provider = Arc::new(StreamingSingleCallProvider::default());
    let provider_dyn: Arc<dyn AgentProvider> = provider.clone();
    let rt = build_runtime(
        &root,
        provider_dyn,
        PromptCacheOptions {
            enabled: true,
            backend: CacheBackend::Memory,
            native: PromptCacheNativeMode::Auto,
            layout: PromptCacheLayoutMode::Auto,
            enable_l2_response_cache: true,
            ttl_ms: 300000,
            max_entries: 1024,
            provider_id: "streaming-single".to_string(),
            model_id: String::new(),
            partition: "t".to_string(),
        },
    );

    let msgs = vec![Message {
        role: "user".to_string(),
        content: "hi".to_string(),
        content_blocks: None,
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        status: None,
    }];

    let mut sink1 = VecRunEventSink::new();
    let out1 = rt.run_with_events(msgs.clone(), &mut sink1).await;
    assert_eq!(out1.status, RunStatus::Completed);
    assert_eq!(provider.collector_calls.load(Ordering::SeqCst), 1);
    assert!(sink1
        .events()
        .iter()
        .any(|event| matches!(event, RunEvent::AssistantTextDelta { text, .. } if text == "OK")));
    assert!(sink1.events().iter().any(|event| matches!(
        event,
        RunEvent::UsageReported {
            input_tokens: Some(2),
            output_tokens: Some(1),
            total_tokens: Some(3),
            ..
        }
    )));

    let mut sink2 = VecRunEventSink::new();
    let out2 = rt.run_with_events(msgs, &mut sink2).await;
    assert_eq!(out2.status, RunStatus::Completed);
    assert_eq!(provider.collector_calls.load(Ordering::SeqCst), 1);
    assert!(!sink2
        .events()
        .iter()
        .any(|event| matches!(event, RunEvent::AssistantTextDelta { .. })));
    assert!(!sink2
        .events()
        .iter()
        .any(|event| matches!(event, RunEvent::UsageReported { .. })));
    assert!(sink2.events().iter().any(|event| matches!(
        event,
        RunEvent::ProviderStepReceived {
            step_type: ProviderStepKind::FinalText,
            ..
        }
    )));
    assert!(sink2
        .events()
        .iter()
        .any(|event| matches!(event, RunEvent::AssistantMessage { .. })));
    assert!(matches!(
        sink2.events().last(),
        Some(RunEvent::RunFinished {
            status: RunStatus::Completed,
            ..
        })
    ));
}

#[tokio::test]
async fn pc_10_native_l1_reuse_works_without_l2_response_cache() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let provider = Arc::new(NativeHintProvider::default());
    let provider_dyn: Arc<dyn AgentProvider> = provider.clone();
    let rt = build_runtime(
        &root,
        provider_dyn,
        PromptCacheOptions {
            enabled: true,
            backend: CacheBackend::Memory,
            native: PromptCacheNativeMode::Auto,
            layout: PromptCacheLayoutMode::Auto,
            enable_l2_response_cache: false,
            ttl_ms: 300000,
            max_entries: 1024,
            provider_id: "native-hint".to_string(),
            model_id: String::new(),
            partition: "t".to_string(),
        },
    );

    let out1 = rt.run(system_and_user_messages("a")).await;
    let out2 = rt.run(system_and_user_messages("b")).await;

    assert_eq!(out1.status, RunStatus::Completed);
    assert_eq!(out2.status, RunStatus::Completed);
    assert_eq!(provider.step_calls(), 2);
    assert_eq!(provider.applied_handle_presence(), vec![false, true]);
    assert_eq!(provider.observed_event_counts(), vec![1, 1]);

    let e2 = first_cache_event_of_level(&out2, "L1");
    assert_eq!(
        e2.get("provider_cache_status").and_then(|v| v.as_str()),
        Some("hit")
    );
    assert_eq!(
        e2.get("cache_source").and_then(|v| v.as_str()),
        Some("hybrid")
    );
    assert!(e2
        .get("provider_handle_hash")
        .and_then(|v| v.as_str())
        .is_some());
    let trace_str = serde_json::to_string(&out2.trace).unwrap();
    assert!(!trace_str.contains("native-prefix-1"));
}

#[tokio::test]
async fn pc_11_streaming_native_observation_updates_l1_without_l2() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let provider = Arc::new(NativeHintProvider::default());
    let provider_dyn: Arc<dyn AgentProvider> = provider.clone();
    let rt = build_runtime(
        &root,
        provider_dyn,
        PromptCacheOptions {
            enabled: true,
            backend: CacheBackend::Memory,
            native: PromptCacheNativeMode::Auto,
            layout: PromptCacheLayoutMode::Auto,
            enable_l2_response_cache: false,
            ttl_ms: 300000,
            max_entries: 1024,
            provider_id: "native-hint-stream".to_string(),
            model_id: String::new(),
            partition: "t".to_string(),
        },
    );

    let mut sink1 = VecRunEventSink::new();
    let out1 = rt
        .run_with_events(system_and_user_messages("a"), &mut sink1)
        .await;
    let mut sink2 = VecRunEventSink::new();
    let out2 = rt
        .run_with_events(system_and_user_messages("b"), &mut sink2)
        .await;

    assert_eq!(out1.status, RunStatus::Completed);
    assert_eq!(out2.status, RunStatus::Completed);
    assert_eq!(provider.applied_handle_presence(), vec![false, true]);
    assert_eq!(provider.observed_event_counts(), vec![1, 1]);
    assert!(sink1.events().iter().any(|event| matches!(
        event,
        RunEvent::UsageReported {
            input_tokens: Some(3),
            output_tokens: Some(1),
            total_tokens: Some(4),
            ..
        }
    )));
    let e2 = first_cache_event_of_level(&out2, "L1");
    assert_eq!(
        e2.get("provider_cache_status").and_then(|v| v.as_str()),
        Some("hit")
    );
}

#[tokio::test]
async fn pc_12_native_required_fails_when_provider_cannot_observe_native_cache() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let provider = Arc::new(RequiredUnsupportedProvider::new());
    let provider_dyn: Arc<dyn AgentProvider> = provider.clone();
    let rt = build_runtime(
        &root,
        provider_dyn,
        PromptCacheOptions {
            enabled: true,
            backend: CacheBackend::Memory,
            native: PromptCacheNativeMode::Required,
            layout: PromptCacheLayoutMode::Auto,
            enable_l2_response_cache: false,
            ttl_ms: 300000,
            max_entries: 1024,
            provider_id: "required-unsupported".to_string(),
            model_id: String::new(),
            partition: "t".to_string(),
        },
    );

    let out = rt.run(system_and_user_messages("hi")).await;

    assert_eq!(provider.calls(), 1);
    assert_eq!(out.status, RunStatus::Error);
    assert_eq!(
        out.error.as_ref().map(|error| error.code.as_str()),
        Some("prompt_cache_error")
    );
    assert!(out
        .error
        .as_ref()
        .map(|error| error.message.contains("prompt_cache_native_required"))
        .unwrap_or(false));
}
