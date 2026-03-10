use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use deepagents::approval::ExecutionMode;
use deepagents::provider::mock::{MockProvider, MockScript, MockStep};
use deepagents::provider::{
    final_text_step, LlmEvent, LlmEventStream, LlmProvider, LlmProviderAdapter,
    LlmProviderCapabilities, MockLlmProvider,
};
use deepagents::provider::{Provider, ProviderRequest, ProviderStep, ProviderToolCall};
use deepagents::runtime::simple::{SimpleRuntime, SimpleRuntimeOptions};
use deepagents::runtime::{
    ProviderStepKind, ResumableRunner, ResumableRunnerOptions, RunEvent, RunStatus,
    StreamingRuntime, VecRunEventSink,
};
use deepagents::types::Message;

fn interrupt_on(keys: &[&str]) -> BTreeMap<String, bool> {
    let mut m = BTreeMap::new();
    for key in keys {
        m.insert((*key).to_string(), true);
    }
    m
}

fn build_runner(
    root: &std::path::Path,
    script: MockScript,
    interrupt_on_tools: &[&str],
) -> ResumableRunner {
    let provider: Arc<dyn deepagents::provider::Provider> =
        Arc::new(MockProvider::from_script(script));
    let backend = deepagents::create_local_sandbox_backend(root, None).unwrap();
    let agent = deepagents::create_deep_agent_with_backend(backend);

    ResumableRunner::new(
        agent,
        provider,
        Vec::new(),
        ResumableRunnerOptions {
            config: deepagents::runtime::RuntimeConfig {
                max_steps: 8,
                provider_timeout_ms: 1000,
            },
            approval: None,
            audit: None,
            root: root.to_string_lossy().to_string(),
            mode: ExecutionMode::NonInteractive,
            interrupt_on: interrupt_on(interrupt_on_tools),
        },
    )
}

fn build_runner_from_provider(
    root: &std::path::Path,
    provider: Arc<dyn deepagents::provider::Provider>,
    interrupt_on_tools: &[&str],
) -> ResumableRunner {
    let backend = deepagents::create_local_sandbox_backend(root, None).unwrap();
    let agent = deepagents::create_deep_agent_with_backend(backend);

    ResumableRunner::new(
        agent,
        provider,
        Vec::new(),
        ResumableRunnerOptions {
            config: deepagents::runtime::RuntimeConfig {
                max_steps: 8,
                provider_timeout_ms: 1000,
            },
            approval: None,
            audit: None,
            root: root.to_string_lossy().to_string(),
            mode: ExecutionMode::NonInteractive,
            interrupt_on: interrupt_on(interrupt_on_tools),
        },
    )
}

fn build_simple_runtime(root: &std::path::Path, script: MockScript) -> SimpleRuntime {
    let provider: Arc<dyn deepagents::provider::Provider> =
        Arc::new(MockProvider::from_script(script));
    let backend = deepagents::create_local_sandbox_backend(root, None).unwrap();
    let agent = deepagents::create_deep_agent_with_backend(backend);

    SimpleRuntime::new(
        agent,
        provider,
        Vec::new(),
        SimpleRuntimeOptions {
            config: deepagents::runtime::RuntimeConfig {
                max_steps: 8,
                provider_timeout_ms: 1000,
            },
            approval: None,
            audit: None,
            root: root.to_string_lossy().to_string(),
            mode: ExecutionMode::NonInteractive,
        },
    )
}

#[derive(Default)]
struct CountingProvider {
    step_calls: AtomicUsize,
    collector_calls: AtomicUsize,
}

#[derive(Default)]
struct ChatOnlyLlmProvider {
    chat_calls: AtomicUsize,
    stream_calls: AtomicUsize,
}

#[async_trait]
impl LlmProvider for ChatOnlyLlmProvider {
    fn capabilities(&self) -> LlmProviderCapabilities {
        LlmProviderCapabilities {
            supports_streaming: false,
            supports_tool_calling: false,
            reports_usage: false,
            supports_structured_output: false,
            supports_reasoning_content: false,
        }
    }

    async fn chat(&self, _req: ProviderRequest) -> anyhow::Result<ProviderStep> {
        self.chat_calls.fetch_add(1, Ordering::SeqCst);
        Ok(ProviderStep::FinalText {
            text: "chat-only".to_string(),
        })
    }

    async fn stream_chat(&self, _req: ProviderRequest) -> anyhow::Result<LlmEventStream> {
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        Err(anyhow::anyhow!("streaming unsupported"))
    }
}

#[async_trait]
impl Provider for CountingProvider {
    async fn step(&self, _req: ProviderRequest) -> anyhow::Result<ProviderStep> {
        self.step_calls.fetch_add(1, Ordering::SeqCst);
        Ok(ProviderStep::FinalText {
            text: "DONE".to_string(),
        })
    }

    async fn step_with_collector(
        &self,
        _req: ProviderRequest,
        _collector: &mut dyn deepagents::provider::ProviderEventCollector,
    ) -> anyhow::Result<ProviderStep> {
        self.collector_calls.fetch_add(1, Ordering::SeqCst);
        Err(anyhow::anyhow!(
            "step_with_collector should not be used by run()"
        ))
    }
}

#[tokio::test]
async fn re01_final_text_run_emits_basic_events() {
    let dir = tempfile::tempdir().unwrap();
    let mut runner = build_runner(
        dir.path(),
        MockScript {
            steps: vec![MockStep::FinalText {
                text: "DONE".to_string(),
            }],
        },
        &[],
    );
    runner.push_user_input("go".to_string());

    let mut sink = VecRunEventSink::new();
    let out = runner.run_with_events(&mut sink).await;
    let events = sink.into_events();

    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(events.len(), 5);
    assert!(matches!(
        events[0],
        RunEvent::RunStarted {
            resumed_from_interrupt: false
        }
    ));
    assert!(matches!(
        events[1],
        RunEvent::ModelRequestBuilt {
            step_index: 0,
            message_count: 1,
            ..
        }
    ));
    assert!(matches!(
        events[2],
        RunEvent::ProviderStepReceived {
            step_index: 0,
            step_type: ProviderStepKind::FinalText,
        }
    ));
    assert!(matches!(
        &events[3],
        RunEvent::AssistantMessage { step_index: 0, message }
            if message.role == "assistant" && message.content == "DONE"
    ));
    assert!(matches!(
        events[4],
        RunEvent::RunFinished {
            status: RunStatus::Completed,
            ref reason,
            step_count: 1,
            ..
        } if reason == "final_text"
    ));
}

#[tokio::test]
async fn re02_tool_run_emits_tool_events_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut runner = build_runner(
        dir.path(),
        MockScript {
            steps: vec![
                MockStep::ToolCalls {
                    calls: vec![ProviderToolCall {
                        tool_name: "write_file".to_string(),
                        arguments: serde_json::json!({
                            "file_path": "a.txt",
                            "content": "hello\n"
                        }),
                        call_id: Some("w1".to_string()),
                    }],
                },
                MockStep::FinalText {
                    text: "DONE".to_string(),
                },
            ],
        },
        &[],
    );
    runner.push_user_input("go".to_string());

    let mut sink = VecRunEventSink::new();
    let out = runner.run_with_events(&mut sink).await;
    let events = sink.into_events();

    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "hello\n"
    );

    let started_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                RunEvent::ToolCallStarted {
                    step_index: 0,
                    tool_call_id,
                    ..
                } if tool_call_id == "w1"
            )
        })
        .unwrap();
    let finished_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                RunEvent::ToolCallFinished {
                    step_index: 0,
                    tool_call_id,
                    status: Some(status),
                    ..
                } if tool_call_id == "w1" && status == "success"
            )
        })
        .unwrap();
    let appended_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                RunEvent::ToolMessageAppended {
                    step_index: 0,
                    tool_call_id,
                    status: Some(status),
                    ..
                } if tool_call_id == "w1" && status == "success"
            )
        })
        .unwrap();
    let state_idx = events
        .iter()
        .position(|event| matches!(
            event,
            RunEvent::StateUpdated { step_index: 0, updated_keys } if updated_keys.iter().any(|k| k == "filesystem")
        ))
        .unwrap();

    assert!(started_idx < finished_idx);
    assert!(finished_idx < appended_idx);
    assert!(appended_idx < state_idx);
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::ProviderStepReceived {
            step_index: 1,
            step_type: ProviderStepKind::FinalText,
        }
    )));
}

#[tokio::test]
async fn re03_interrupt_emits_interrupt_without_tool_started() {
    let dir = tempfile::tempdir().unwrap();
    let mut runner = build_runner(
        dir.path(),
        MockScript {
            steps: vec![MockStep::ToolCalls {
                calls: vec![ProviderToolCall {
                    tool_name: "write_file".to_string(),
                    arguments: serde_json::json!({
                        "file_path": "blocked.txt",
                        "content": "hello\n"
                    }),
                    call_id: Some("w1".to_string()),
                }],
            }],
        },
        &["write_file"],
    );
    runner.push_user_input("go".to_string());

    let mut sink = VecRunEventSink::new();
    let out = runner.run_with_events(&mut sink).await;
    let events = sink.into_events();

    assert_eq!(out.status, RunStatus::Interrupted);
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::Interrupt {
            step_index: 0,
            interrupt,
        } if interrupt.tool_call_id == "w1"
    )));
    assert!(!events
        .iter()
        .any(|event| matches!(event, RunEvent::ToolCallStarted { .. })));
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            status: RunStatus::Interrupted,
            ..
        })
    ));
}

#[tokio::test]
async fn re04_simple_runtime_run_with_events_delegates_to_runner() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = build_simple_runtime(
        dir.path(),
        MockScript {
            steps: vec![MockStep::FinalText {
                text: "DONE".to_string(),
            }],
        },
    );

    let mut sink = VecRunEventSink::new();
    let out = runtime
        .run_with_events(
            vec![Message {
                role: "user".to_string(),
                content: "go".to_string(),
                content_blocks: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                status: None,
            }],
            &mut sink,
        )
        .await;

    assert_eq!(out.status, RunStatus::Completed);
    assert!(sink.events().iter().any(|event| matches!(
        event,
        RunEvent::ProviderStepReceived {
            step_type: ProviderStepKind::FinalText,
            ..
        }
    )));
}

#[tokio::test]
async fn re05_provider_error_still_emits_run_finished() {
    let dir = tempfile::tempdir().unwrap();
    let mut runner = build_runner(
        dir.path(),
        MockScript {
            steps: vec![MockStep::Error {
                code: "boom".to_string(),
                message: "fail".to_string(),
            }],
        },
        &[],
    );
    runner.push_user_input("go".to_string());

    let mut sink = VecRunEventSink::new();
    let out = runner.run_with_events(&mut sink).await;
    let events = sink.into_events();

    assert_eq!(out.status, RunStatus::Error);
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            status: RunStatus::Error,
            ref reason,
            ..
        }) if reason == "provider_step_error"
    ));
}

#[tokio::test]
async fn re06_llm_adapter_streams_delta_events_before_provider_step() {
    let dir = tempfile::tempdir().unwrap();
    let provider: Arc<dyn deepagents::provider::Provider> = Arc::new(LlmProviderAdapter::new(
        Arc::new(MockLlmProvider::new(vec![
            LlmEvent::AssistantTextDelta {
                text: "Hel".to_string(),
            },
            LlmEvent::AssistantTextDelta {
                text: "lo".to_string(),
            },
            LlmEvent::ToolCallArgsDelta {
                tool_call_id: "t1".to_string(),
                delta: "{\"file_path\":\"README.md\"}".to_string(),
            },
            LlmEvent::Usage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                total_tokens: Some(15),
            },
            final_text_step("Hello"),
        ])),
    ));
    let mut runner = build_runner_from_provider(dir.path(), provider, &[]);
    runner.push_user_input("go".to_string());

    let mut sink = VecRunEventSink::new();
    let out = runner.run_with_events(&mut sink).await;
    let events = sink.into_events();

    assert_eq!(out.status, RunStatus::Completed);
    let provider_step_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                RunEvent::ProviderStepReceived {
                    step_type: ProviderStepKind::FinalText,
                    ..
                }
            )
        })
        .unwrap();
    let text_delta_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                RunEvent::AssistantTextDelta { text, .. } if text == "Hel"
            )
        })
        .unwrap();
    let args_delta_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                RunEvent::ToolCallArgsDelta { tool_call_id, .. } if tool_call_id == "t1"
            )
        })
        .unwrap();
    let usage_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                RunEvent::UsageReported {
                    input_tokens: Some(10),
                    output_tokens: Some(5),
                    total_tokens: Some(15),
                    ..
                }
            )
        })
        .unwrap();
    let assistant_msg_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                RunEvent::AssistantMessage { message, .. } if message.content == "Hello"
            )
        })
        .unwrap();

    assert!(text_delta_idx < args_delta_idx);
    assert!(args_delta_idx < usage_idx);
    assert!(usage_idx < provider_step_idx);
    assert!(provider_step_idx < assistant_msg_idx);
}

#[tokio::test]
async fn re07_run_uses_non_stream_provider_path() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(CountingProvider::default());
    let provider_obj: Arc<dyn Provider> = provider.clone();
    let mut runner = build_runner_from_provider(dir.path(), provider_obj, &[]);
    runner.push_user_input("go".to_string());

    let out = runner.run().await;

    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(provider.step_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.collector_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn re08_simple_runtime_implements_streaming_runtime_trait() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = build_simple_runtime(
        dir.path(),
        MockScript {
            steps: vec![MockStep::FinalText {
                text: "DONE".to_string(),
            }],
        },
    );

    let mut sink = VecRunEventSink::new();
    let out = StreamingRuntime::run_with_events(
        &runtime,
        vec![Message {
            role: "user".to_string(),
            content: "go".to_string(),
            content_blocks: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }],
        &mut sink,
    )
    .await;

    assert_eq!(out.status, RunStatus::Completed);
    assert!(sink.events().iter().any(|event| matches!(
        event,
        RunEvent::ProviderStepReceived {
            step_type: ProviderStepKind::FinalText,
            ..
        }
    )));
}

#[tokio::test]
async fn re09_llm_adapter_falls_back_to_chat_when_streaming_not_supported() {
    let dir = tempfile::tempdir().unwrap();
    let inner = Arc::new(ChatOnlyLlmProvider::default());
    let provider: Arc<dyn deepagents::provider::Provider> =
        Arc::new(LlmProviderAdapter::new(inner.clone()));
    let mut runner = build_runner_from_provider(dir.path(), provider, &[]);
    runner.push_user_input("go".to_string());

    let mut sink = VecRunEventSink::new();
    let out = runner.run_with_events(&mut sink).await;

    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(out.final_text, "chat-only");
    assert_eq!(inner.chat_calls.load(Ordering::SeqCst), 1);
    assert_eq!(inner.stream_calls.load(Ordering::SeqCst), 0);
    assert!(!sink
        .events()
        .iter()
        .any(|event| matches!(event, RunEvent::AssistantTextDelta { .. })));
}
