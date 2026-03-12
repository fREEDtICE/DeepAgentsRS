use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagents::approval::ExecutionMode;
use deepagents::llm::{
    ChatRequest, ChatResponse, ChatRole, LlmEventStream, LlmProvider, LlmProviderCapabilities,
    ToolCall, ToolChoice, ToolSpec, ToolsPayload,
};
use deepagents::provider::{
    AgentProvider, AgentProviderRequest, AgentStep, AgentToolCall, LlmProviderAdapter,
    VecAgentProviderEventCollector,
};
use deepagents::runtime::{
    ResumableRunner, ResumableRunnerOptions, RunEvent, RunStatus, RuntimeConfig, VecRunEventSink,
};

#[derive(Clone)]
struct PromptGuidedTestProvider {
    responses: Arc<Mutex<VecDeque<ChatResponse>>>,
    last_request: Arc<Mutex<Option<ChatRequest>>>,
    chat_calls: Arc<AtomicUsize>,
    stream_calls: Arc<AtomicUsize>,
    capabilities: LlmProviderCapabilities,
    instructions: String,
}

impl PromptGuidedTestProvider {
    fn new(responses: Vec<AgentStep>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(VecDeque::from(
                responses
                    .into_iter()
                    .map(response_from_step)
                    .collect::<Vec<_>>(),
            ))),
            last_request: Arc::new(Mutex::new(None)),
            chat_calls: Arc::new(AtomicUsize::new(0)),
            stream_calls: Arc::new(AtomicUsize::new(0)),
            capabilities: LlmProviderCapabilities {
                supports_streaming: false,
                supports_tool_calling: false,
                reports_usage: false,
                supports_structured_output: false,
                supports_reasoning_content: false,
                ..Default::default()
            },
            instructions: "Use the tagged tool payload when you need tools.".to_string(),
        }
    }

    fn with_capabilities(mut self, capabilities: LlmProviderCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    fn last_request(&self) -> ChatRequest {
        self.last_request
            .lock()
            .unwrap()
            .clone()
            .expect("captured request")
    }
}

#[async_trait]
impl LlmProvider for PromptGuidedTestProvider {
    fn capabilities(&self) -> LlmProviderCapabilities {
        self.capabilities
    }

    fn convert_tools(&self, tool_specs: &[ToolSpec]) -> anyhow::Result<ToolsPayload> {
        if tool_specs.is_empty() {
            return Ok(ToolsPayload::None);
        }
        Ok(ToolsPayload::PromptGuided {
            instructions: self.instructions.clone(),
        })
    }

    async fn chat(&self, req: ChatRequest) -> anyhow::Result<ChatResponse> {
        self.chat_calls.fetch_add(1, Ordering::SeqCst);
        *self.last_request.lock().unwrap() = Some(req);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("missing test response"))
    }

    async fn stream_chat(&self, _req: ChatRequest) -> anyhow::Result<LlmEventStream> {
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(tokio_stream::empty()))
    }
}

fn response_from_step(step: AgentStep) -> ChatResponse {
    match step {
        AgentStep::AssistantMessage { text } | AgentStep::FinalText { text } => {
            ChatResponse::new(text)
        }
        AgentStep::AssistantMessageWithToolCalls { text, calls } => ChatResponse::new(text)
            .with_tool_calls(calls.into_iter().map(convert_tool_call).collect()),
        AgentStep::ToolCalls { calls } => ChatResponse::new("")
            .with_tool_calls(calls.into_iter().map(convert_tool_call).collect()),
        AgentStep::SkillCall { .. } => panic!("skill calls are not valid llm responses"),
        AgentStep::Error { error } => {
            panic!("unexpected provider error in test fixture: {error:?}")
        }
    }
}

fn convert_tool_call(call: AgentToolCall) -> ToolCall {
    ToolCall {
        id: call.call_id.unwrap_or_default(),
        name: call.tool_name,
        arguments: call.arguments,
    }
}

fn sample_request(tool_choice: ToolChoice) -> AgentProviderRequest {
    AgentProviderRequest {
        messages: vec![
            deepagents::types::Message {
                role: "system".to_string(),
                content: "You are helpful.".to_string(),
                content_blocks: None,
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                status: None,
            },
            deepagents::types::Message {
                role: "user".to_string(),
                content: "Check README.".to_string(),
                content_blocks: None,
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                status: None,
            },
        ],
        tool_specs: vec![deepagents::runtime::ToolSpec {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string" }
                },
                "required": ["file_path"],
                "additionalProperties": false
            }),
        }],
        tool_choice,
        skills: Vec::new(),
        state: deepagents::state::AgentState::default(),
        last_tool_results: Vec::new(),
        structured_output: None,
    }
}

fn build_runner(root: &std::path::Path, provider: Arc<dyn AgentProvider>) -> ResumableRunner {
    let backend = deepagents::create_local_sandbox_backend(root, None).unwrap();
    let agent = deepagents::create_deep_agent_with_backend(backend);

    ResumableRunner::new(
        agent,
        provider,
        Vec::new(),
        ResumableRunnerOptions {
            config: RuntimeConfig {
                max_steps: 8,
                provider_timeout_ms: 1000,
            },
            approval: None,
            audit: None,
            root: root.to_string_lossy().to_string(),
            mode: ExecutionMode::NonInteractive,
            interrupt_on: Default::default(),
        },
    )
}

#[tokio::test]
async fn prompt_guided_adapter_injects_contract_and_parses_tool_calls() {
    let provider = PromptGuidedTestProvider::new(vec![AgentStep::FinalText {
        text: "<tool_call>\n{\"content\":\"Let me check.\",\"tool_calls\":[{\"name\":\"read_file\",\"arguments\":{\"file_path\":\"README.md\"},\"id\":\"call_9\"}]}\n</tool_call>".to_string(),
    }]);
    let adapter = LlmProviderAdapter::new(Arc::new(provider.clone()));

    let step = adapter
        .step(sample_request(ToolChoice::Auto))
        .await
        .unwrap();
    assert!(matches!(
        step,
        AgentStep::AssistantMessageWithToolCalls { text, calls }
            if text == "Let me check."
                && calls.len() == 1
                && calls[0].tool_name == "read_file"
                && calls[0].call_id.as_deref() == Some("call_9")
    ));

    let request = provider.last_request();
    assert_eq!(request.messages.len(), 3);
    assert_eq!(request.messages[1].role, ChatRole::System);
    assert!(request.messages[1]
        .content
        .contains("Tool calling fallback contract:"));
    assert!(request.messages[1].content.contains("<tool_call>"));
    assert!(request.messages[1].content.contains("read_file"));
}

#[tokio::test]
async fn prompt_guided_adapter_rejects_unknown_named_tool_choice_before_chat() {
    let provider = PromptGuidedTestProvider::new(vec![AgentStep::FinalText {
        text: "unused".to_string(),
    }]);
    let adapter = LlmProviderAdapter::new(Arc::new(provider.clone()));

    let err = adapter
        .step(sample_request(ToolChoice::Named {
            name: "missing_tool".to_string(),
        }))
        .await
        .unwrap_err();

    assert_eq!(
        err.to_string(),
        "prompt_guided_unknown_tool_choice: missing_tool"
    );
    assert_eq!(provider.chat_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn prompt_guided_adapter_required_choice_rejects_plain_text_response() {
    let provider = PromptGuidedTestProvider::new(vec![AgentStep::FinalText {
        text: "I can answer without tools.".to_string(),
    }]);
    let adapter = LlmProviderAdapter::new(Arc::new(provider));

    let err = adapter
        .step(sample_request(ToolChoice::Required))
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "prompt_guided_tool_call_required");
}

#[tokio::test]
async fn prompt_guided_collector_path_uses_chat_instead_of_streaming() {
    let provider = PromptGuidedTestProvider::new(vec![AgentStep::FinalText {
        text: "<tool_call>\n{\"tool_calls\":[{\"name\":\"read_file\",\"arguments\":{\"file_path\":\"README.md\"}}]}\n</tool_call>".to_string(),
    }])
    .with_capabilities(LlmProviderCapabilities {
        supports_streaming: true,
        supports_tool_calling: false,
        reports_usage: false,
        supports_structured_output: false,
        supports_reasoning_content: false,
        ..Default::default()
    });
    let adapter = LlmProviderAdapter::new(Arc::new(provider.clone()));
    let mut collector = VecAgentProviderEventCollector::new();

    let step = adapter
        .step_with_collector(sample_request(ToolChoice::Auto), &mut collector)
        .await
        .unwrap();

    assert!(matches!(step, AgentStep::ToolCalls { calls } if calls.len() == 1));
    assert!(collector.into_events().is_empty());
    assert_eq!(provider.chat_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.stream_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn prompt_guided_runner_executes_tool_calls_end_to_end() {
    let provider = PromptGuidedTestProvider::new(vec![
        AgentStep::FinalText {
            text: "<tool_call>\n{\"content\":\"Writing the file.\",\"tool_calls\":[{\"name\":\"write_file\",\"arguments\":{\"file_path\":\"note.txt\",\"content\":\"hello\\n\"},\"id\":\"call_1\"}]}\n</tool_call>".to_string(),
        },
        AgentStep::FinalText {
            text: "All done.".to_string(),
        },
    ])
    .with_capabilities(LlmProviderCapabilities {
        supports_streaming: true,
        supports_tool_calling: false,
        reports_usage: false,
        supports_structured_output: false,
        supports_reasoning_content: false,
        ..Default::default()
    });
    let provider: Arc<dyn AgentProvider> =
        Arc::new(LlmProviderAdapter::new(Arc::new(provider.clone())));
    let dir = tempfile::tempdir().unwrap();
    let mut runner = build_runner(dir.path(), provider);
    runner.push_user_input("create note.txt".to_string());

    let mut sink = VecRunEventSink::new();
    let out = runner.run_with_events(&mut sink).await;
    let events = sink.into_events();

    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(out.final_text, "All done.");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("note.txt")).unwrap(),
        "hello\n"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::AssistantMessage { message, .. } if message.content == "Writing the file."
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::ToolCallStarted { tool_name, tool_call_id, .. }
            if tool_name == "write_file" && tool_call_id == "call_1"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::RunFinished { reason, .. } if reason == "final_text"
    )));
}
