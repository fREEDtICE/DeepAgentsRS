use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use deepagents::approval::{DefaultApprovalPolicy, ExecutionMode};
use deepagents::llm::{
    final_text_step, AssistantMessageMetadata, ChatRequest, ChatResponse, LlmEvent, LlmEventStream,
    LlmProvider, LlmProviderCapabilities, MockLlmProvider, ToolChoice,
};
use deepagents::provider::mock::{MockProvider, MockScript, MockStep};
use deepagents::provider::{
    AgentProvider, AgentProviderRequest, AgentStep, AgentStepOutput, AgentToolCall,
    LlmProviderAdapter,
};
use deepagents::runtime::simple::{SimpleRuntime, SimpleRuntimeOptions};
use deepagents::runtime::{
    ProviderStepKind, ResumableRunner, ResumableRunnerOptions, RunEvent, RunStatus,
    StreamingRuntime, VecRunEventSink,
};
use deepagents::types::{ContentBlock, Message};

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
    let provider: Arc<dyn deepagents::provider::AgentProvider> =
        Arc::new(MockProvider::from_script(script));
    let backend = deepagents::create_local_sandbox_backend(root, None).unwrap();
    let agent = deepagents::create_deep_agent_with_backend(backend);

    ResumableRunner::new(
        agent,
        provider,
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
    provider: Arc<dyn deepagents::provider::AgentProvider>,
    interrupt_on_tools: &[&str],
) -> ResumableRunner {
    let backend = deepagents::create_local_sandbox_backend(root, None).unwrap();
    let agent = deepagents::create_deep_agent_with_backend(backend);

    ResumableRunner::new(
        agent,
        provider,
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
    let provider: Arc<dyn deepagents::provider::AgentProvider> =
        Arc::new(MockProvider::from_script(script));
    let backend = deepagents::create_local_sandbox_backend(root, None).unwrap();
    let agent = deepagents::create_deep_agent_with_backend(backend);

    SimpleRuntime::new(
        agent,
        provider,
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

#[derive(Default)]
struct CaptureToolChoiceProvider {
    seen: std::sync::Mutex<Vec<ToolChoice>>,
}

#[derive(Default)]
struct ReasoningRoundTripProvider {
    step_idx: AtomicUsize,
    requests: std::sync::Mutex<Vec<AgentProviderRequest>>,
}

impl ReasoningRoundTripProvider {
    fn requests(&self) -> Vec<AgentProviderRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct MultimodalRoundTripProvider {
    step_idx: AtomicUsize,
    requests: std::sync::Mutex<Vec<AgentProviderRequest>>,
}

impl MultimodalRoundTripProvider {
    fn requests(&self) -> Vec<AgentProviderRequest> {
        self.requests.lock().unwrap().clone()
    }
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
            ..Default::default()
        }
    }

    async fn chat(&self, _req: ChatRequest) -> anyhow::Result<ChatResponse> {
        self.chat_calls.fetch_add(1, Ordering::SeqCst);
        Ok(ChatResponse::new("chat-only"))
    }

    async fn stream_chat(&self, _req: ChatRequest) -> anyhow::Result<LlmEventStream> {
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        Err(anyhow::anyhow!("streaming unsupported"))
    }
}

#[async_trait]
impl AgentProvider for CountingProvider {
    async fn step(&self, _req: AgentProviderRequest) -> anyhow::Result<AgentStep> {
        self.step_calls.fetch_add(1, Ordering::SeqCst);
        Ok(AgentStep::FinalText {
            text: "DONE".to_string(),
        })
    }

    async fn step_with_collector(
        &self,
        _req: AgentProviderRequest,
        _collector: &mut dyn deepagents::provider::AgentProviderEventCollector,
    ) -> anyhow::Result<AgentStep> {
        self.collector_calls.fetch_add(1, Ordering::SeqCst);
        Err(anyhow::anyhow!(
            "step_with_collector should not be used by run()"
        ))
    }
}

#[async_trait]
impl AgentProvider for CaptureToolChoiceProvider {
    async fn step(&self, req: AgentProviderRequest) -> anyhow::Result<AgentStep> {
        self.seen.lock().unwrap().push(req.tool_choice);
        Ok(AgentStep::FinalText {
            text: "ok".to_string(),
        })
    }
}

#[async_trait]
impl AgentProvider for ReasoningRoundTripProvider {
    async fn step(&self, req: AgentProviderRequest) -> anyhow::Result<AgentStep> {
        Ok(self.step_output(req).await?.step)
    }

    async fn step_output(&self, req: AgentProviderRequest) -> anyhow::Result<AgentStepOutput> {
        self.requests.lock().unwrap().push(req);
        let output = match self.step_idx.fetch_add(1, Ordering::SeqCst) {
            0 => AgentStepOutput::from(AgentStep::ToolCalls {
                calls: vec![AgentToolCall {
                    tool_name: "write_file".to_string(),
                    arguments: serde_json::json!({
                        "file_path": "reasoning.txt",
                        "content": "persisted\n"
                    }),
                    call_id: Some("w1".to_string()),
                }],
            })
            .with_assistant_metadata(AssistantMessageMetadata {
                content_blocks: None,
                reasoning_content: Some("Need to preserve this reasoning.".to_string()),
            }),
            _ => AgentStepOutput::from(AgentStep::FinalText {
                text: "done".to_string(),
            }),
        };
        Ok(output)
    }
}

#[async_trait]
impl AgentProvider for MultimodalRoundTripProvider {
    async fn step(&self, req: AgentProviderRequest) -> anyhow::Result<AgentStep> {
        Ok(self.step_output(req).await?.step)
    }

    async fn step_output(&self, req: AgentProviderRequest) -> anyhow::Result<AgentStepOutput> {
        self.requests.lock().unwrap().push(req);
        let output = match self.step_idx.fetch_add(1, Ordering::SeqCst) {
            0 => AgentStepOutput::from(AgentStep::AssistantMessageWithToolCalls {
                text: "Reviewing the image.".to_string(),
                calls: vec![AgentToolCall {
                    tool_name: "write_file".to_string(),
                    arguments: serde_json::json!({
                        "file_path": "multimodal.txt",
                        "content": "persisted\n"
                    }),
                    call_id: Some("w1".to_string()),
                }],
            })
            .with_assistant_metadata(AssistantMessageMetadata {
                content_blocks: Some(vec![ContentBlock::image_base64("image/png", "AAEC")]),
                reasoning_content: None,
            }),
            _ => AgentStepOutput::from(AgentStep::FinalText {
                text: "done".to_string(),
            }),
        };
        Ok(output)
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
                    calls: vec![AgentToolCall {
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
                calls: vec![AgentToolCall {
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
                reasoning_content: None,
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
    let provider: Arc<dyn deepagents::provider::AgentProvider> = Arc::new(LlmProviderAdapter::new(
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
    let provider_obj: Arc<dyn AgentProvider> = provider.clone();
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
            reasoning_content: None,
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
    let provider: Arc<dyn deepagents::provider::AgentProvider> =
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

#[tokio::test]
async fn re10_llm_adapter_rejects_explicit_tool_binding_when_tool_calling_is_unsupported() {
    let provider = LlmProviderAdapter::new(Arc::new(ChatOnlyLlmProvider::default()));
    let req = AgentProviderRequest {
        messages: vec![Message {
            role: "user".to_string(),
            content: "go".to_string(),
            content_blocks: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }],
        tool_specs: vec![deepagents::runtime::ToolSpec {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string" }
                }
            }),
        }],
        tool_choice: deepagents::llm::ToolChoice::Required,
        state: deepagents::state::AgentState::default(),
        last_tool_results: Vec::new(),
        structured_output: None,
    };

    let err = provider.step(req).await.unwrap_err();
    assert_eq!(err.to_string(), "provider_unsupported_tool_calling");
}

#[tokio::test]
async fn re10a_llm_adapter_rejects_required_tool_choice_without_tools_even_if_tool_calling_is_supported(
) {
    let capabilities = LlmProviderCapabilities {
        supports_tool_calling: true,
        supports_streaming: false,
        ..Default::default()
    };

    let provider = LlmProviderAdapter::new(Arc::new(
        deepagents::llm::MockLlmProvider::new(vec![deepagents::llm::final_text_step("ok")])
            .with_capabilities(capabilities),
    ));
    let req = AgentProviderRequest {
        messages: vec![Message {
            role: "user".to_string(),
            content: "go".to_string(),
            content_blocks: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }],
        tool_specs: Vec::new(),
        tool_choice: deepagents::llm::ToolChoice::Required,
        state: deepagents::state::AgentState::default(),
        last_tool_results: Vec::new(),
        structured_output: None,
    };

    let err = provider.step(req).await.unwrap_err();
    assert_eq!(err.to_string(), "tool_choice_requires_tools");
}

#[tokio::test]
async fn re10b_llm_adapter_rejects_structured_output_when_provider_does_not_support_it() {
    let provider = LlmProviderAdapter::new(Arc::new(ChatOnlyLlmProvider::default()));
    let req = AgentProviderRequest {
        messages: vec![Message {
            role: "user".to_string(),
            content: "go".to_string(),
            content_blocks: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }],
        tool_specs: Vec::new(),
        tool_choice: deepagents::llm::ToolChoice::Auto,
        state: deepagents::state::AgentState::default(),
        last_tool_results: Vec::new(),
        structured_output: Some(deepagents::llm::StructuredOutputSpec {
            name: "final_answer".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "answer": { "type": "string" }
                },
                "required": ["answer"],
                "additionalProperties": false
            }),
            description: None,
            strict: true,
        }),
    };

    let err = provider.step(req).await.unwrap_err();
    assert_eq!(err.to_string(), "provider_unsupported_structured_output");
}

#[tokio::test]
async fn re11_runner_preserves_assistant_text_when_step_also_contains_tool_calls() {
    let dir = tempfile::tempdir().unwrap();
    let mut runner = build_runner(
        dir.path(),
        MockScript {
            steps: vec![
                MockStep::AssistantMessageWithToolCalls {
                    text: "Checking the file".to_string(),
                    calls: vec![AgentToolCall {
                        tool_name: "write_file".to_string(),
                        arguments: serde_json::json!({
                            "file_path": "note.txt",
                            "content": "hello"
                        }),
                        call_id: Some("w1".to_string()),
                    }],
                },
                MockStep::FinalText {
                    text: "done".to_string(),
                },
            ],
        },
        &[],
    );
    runner.push_user_input("go".to_string());

    let mut sink = VecRunEventSink::new();
    let out = runner.run_with_events(&mut sink).await;

    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(out.final_text, "done");
    assert!(sink.events().iter().any(|event| matches!(
        event,
        RunEvent::ProviderStepReceived {
            step_type: ProviderStepKind::AssistantMessageWithToolCalls,
            ..
        }
    )));
    assert!(sink.events().iter().any(|event| matches!(
        event,
        RunEvent::AssistantMessage { message, .. }
            if message.content == "Checking the file"
                && message.tool_calls.as_ref().map(|calls| calls.len()) == Some(1)
    )));
    assert!(out
        .tool_results
        .iter()
        .any(|result| result.call_id.as_deref() == Some("w1")));
}

#[tokio::test]
async fn re11a_runner_round_trips_reasoning_content_in_provider_history() {
    let dir = tempfile::tempdir().unwrap();
    let inner = Arc::new(ReasoningRoundTripProvider::default());
    let provider: Arc<dyn deepagents::provider::AgentProvider> = inner.clone();
    let mut runner = build_runner_from_provider(dir.path(), provider, &[]);
    runner.push_user_input("go".to_string());

    let mut sink = VecRunEventSink::new();
    let out = runner.run_with_events(&mut sink).await;

    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(out.final_text, "done");
    assert!(sink.events().iter().any(|event| matches!(
        event,
        RunEvent::AssistantMessage { message, .. }
            if message.reasoning_content.as_deref() == Some("Need to preserve this reasoning.")
                && message.tool_calls.as_ref().map(|calls| calls.len()) == Some(1)
    )));

    let requests = inner.requests();
    assert_eq!(requests.len(), 2);
    let assistant = requests[1]
        .messages
        .iter()
        .find(|message| message.role == "assistant" && message.tool_calls.is_some())
        .expect("assistant tool-call history message");
    assert_eq!(
        assistant.reasoning_content.as_deref(),
        Some("Need to preserve this reasoning.")
    );
    assert_eq!(
        assistant
            .tool_calls
            .as_ref()
            .and_then(|calls| calls.first())
            .map(|call| call.id.as_str()),
        Some("w1")
    );
}

#[tokio::test]
async fn re11aa_runner_round_trips_multimodal_content_blocks_in_provider_history() {
    let dir = tempfile::tempdir().unwrap();
    let inner = Arc::new(MultimodalRoundTripProvider::default());
    let provider: Arc<dyn deepagents::provider::AgentProvider> = inner.clone();
    let mut runner = build_runner_from_provider(dir.path(), provider, &[]);
    runner.push_user_input("go".to_string());

    let mut sink = VecRunEventSink::new();
    let out = runner.run_with_events(&mut sink).await;

    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(out.final_text, "done");
    assert!(sink.events().iter().any(|event| matches!(
        event,
        RunEvent::AssistantMessage { message, .. }
            if message.content == "Reviewing the image."
                && message.content_blocks.as_ref()
                    == Some(&vec![ContentBlock::image_base64("image/png", "AAEC")])
                && message.tool_calls.as_ref().map(|calls| calls.len()) == Some(1)
    )));

    let requests = inner.requests();
    assert_eq!(requests.len(), 2);
    let assistant = requests[1]
        .messages
        .iter()
        .find(|message| message.role == "assistant" && message.tool_calls.is_some())
        .expect("assistant tool-call history message");
    assert_eq!(
        assistant.content_blocks.as_ref(),
        Some(&vec![ContentBlock::image_base64("image/png", "AAEC")])
    );
    assert_eq!(
        assistant
            .tool_calls
            .as_ref()
            .and_then(|calls| calls.first())
            .map(|call| call.id.as_str()),
        Some("w1")
    );
}

#[tokio::test]
async fn re11b_runner_parses_structured_output_into_run_output() {
    let dir = tempfile::tempdir().unwrap();
    let provider: Arc<dyn deepagents::provider::AgentProvider> =
        Arc::new(MockProvider::from_script(MockScript {
            steps: vec![MockStep::FinalText {
                text: "{\"answer\":\"done\",\"confidence\":0.9}".to_string(),
            }],
        }));
    let mut runner = build_runner_from_provider(dir.path(), provider, &[]).with_structured_output(
        deepagents::llm::StructuredOutputSpec {
            name: "final_answer".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "answer": { "type": "string" },
                    "confidence": { "type": "number" }
                },
                "required": ["answer", "confidence"],
                "additionalProperties": false
            }),
            description: None,
            strict: true,
        },
    );
    runner.push_user_input("go".to_string());

    let out = runner.run().await;

    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(out.final_text, "{\"answer\":\"done\",\"confidence\":0.9}");
    assert_eq!(
        out.structured_output,
        Some(serde_json::json!({
            "answer": "done",
            "confidence": 0.9
        }))
    );
}

#[tokio::test]
async fn re11c_runner_returns_error_when_structured_output_is_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let provider: Arc<dyn deepagents::provider::AgentProvider> =
        Arc::new(MockProvider::from_script(MockScript {
            steps: vec![MockStep::FinalText {
                text: "not json".to_string(),
            }],
        }));
    let mut runner = build_runner_from_provider(dir.path(), provider, &[]).with_structured_output(
        deepagents::llm::StructuredOutputSpec {
            name: "final_answer".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "answer": { "type": "string" }
                },
                "required": ["answer"],
                "additionalProperties": false
            }),
            description: None,
            strict: true,
        },
    );
    runner.push_user_input("go".to_string());

    let out = runner.run().await;

    assert_eq!(out.status, RunStatus::Error);
    assert_eq!(out.final_text, "not json");
    assert_eq!(
        out.error.as_ref().map(|error| error.code.as_str()),
        Some("structured_output_invalid_response")
    );
    assert_eq!(out.structured_output, None);
}

#[tokio::test]
async fn re12_runner_passes_configured_tool_choice_into_provider_requests() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(CaptureToolChoiceProvider::default());
    let provider_dyn: Arc<dyn deepagents::provider::AgentProvider> = provider.clone();
    let mut runner = build_runner_from_provider(dir.path(), provider_dyn, &[])
        .with_tool_choice(ToolChoice::Required);
    runner.push_user_input("go".to_string());

    let out = runner.run().await;

    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(
        provider.seen.lock().unwrap().as_slice(),
        &[ToolChoice::Required]
    );
}

#[tokio::test]
async fn re13_non_interactive_execute_require_approval_is_reported_as_tool_error_not_interrupt() {
    let dir = tempfile::tempdir().unwrap();
    let provider: Arc<dyn deepagents::provider::AgentProvider> =
        Arc::new(MockProvider::from_script(MockScript {
            steps: vec![
                MockStep::ToolCalls {
                    calls: vec![AgentToolCall {
                        tool_name: "execute".to_string(),
                        arguments: serde_json::json!({
                            "command": "echo hi",
                            "timeout": 5
                        }),
                        call_id: Some("e1".to_string()),
                    }],
                },
                MockStep::FinalText {
                    text: "done".to_string(),
                },
            ],
        }));
    let backend = deepagents::create_local_sandbox_backend(dir.path(), None).unwrap();
    let agent = deepagents::create_deep_agent_with_backend(backend);
    let mut runner = ResumableRunner::new(
        agent,
        provider,
        ResumableRunnerOptions {
            config: deepagents::runtime::RuntimeConfig {
                max_steps: 8,
                provider_timeout_ms: 1000,
            },
            approval: Some(Arc::new(DefaultApprovalPolicy::new(Vec::<String>::new()))),
            audit: None,
            root: dir.path().to_string_lossy().to_string(),
            mode: ExecutionMode::NonInteractive,
            interrupt_on: BTreeMap::new(),
        },
    );
    runner.push_user_input("go".to_string());

    let out = runner.run().await;

    assert_eq!(out.status, RunStatus::Completed);
    assert!(out.interrupts.is_empty());
    assert_eq!(out.final_text, "done");
    let err = out
        .tool_results
        .iter()
        .find(|result| result.tool_name == "execute")
        .and_then(|result| result.error.as_deref())
        .unwrap_or("");
    assert!(err.contains("command_not_allowed"));
    assert!(err.contains("approval_required"));
}

#[tokio::test]
async fn re14_runner_returns_error_when_structured_output_violates_schema() {
    let dir = tempfile::tempdir().unwrap();
    let provider: Arc<dyn deepagents::provider::AgentProvider> =
        Arc::new(MockProvider::from_script(MockScript {
            steps: vec![MockStep::FinalText {
                text: "{\"confidence\":0.9}".to_string(),
            }],
        }));
    let mut runner = build_runner_from_provider(dir.path(), provider, &[]).with_structured_output(
        deepagents::llm::StructuredOutputSpec {
            name: "final_answer".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "answer": { "type": "string" },
                    "confidence": { "type": "number" }
                },
                "required": ["answer", "confidence"],
                "additionalProperties": false
            }),
            description: None,
            strict: true,
        },
    );
    runner.push_user_input("go".to_string());

    let out = runner.run().await;

    assert_eq!(out.status, RunStatus::Error);
    assert_eq!(out.final_text, "{\"confidence\":0.9}");
    assert_eq!(
        out.error.as_ref().map(|error| error.code.as_str()),
        Some("structured_output_invalid_response")
    );
    assert_eq!(out.structured_output, None);
}
