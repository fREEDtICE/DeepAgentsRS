use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagents::provider::mock::{MockProvider, MockScript, MockStep};
use deepagents::provider::protocol::ProviderToolCall;
use deepagents::runtime::{
    HandledToolCall, Runtime, RuntimeMiddleware, RuntimeMiddlewareAssembler, RuntimeMiddlewareSlot,
    ToolCallContext,
};
use deepagents::types::Message;
use deepagents::{create_deep_agent_with_backend, create_local_sandbox_backend};

#[derive(Clone)]
struct RecordingMiddleware {
    label: &'static str,
    log: Arc<Mutex<Vec<String>>>,
    handle_dummy_tool: bool,
}

impl RecordingMiddleware {
    fn new(label: &'static str, log: Arc<Mutex<Vec<String>>>, handle_dummy_tool: bool) -> Self {
        Self {
            label,
            log,
            handle_dummy_tool,
        }
    }

    fn push(&self, what: &str) {
        self.log
            .lock()
            .unwrap()
            .push(format!("{what}:{}", self.label));
    }
}

#[async_trait]
impl RuntimeMiddleware for RecordingMiddleware {
    async fn before_run(
        &self,
        messages: Vec<Message>,
        _state: &mut deepagents::state::AgentState,
    ) -> anyhow::Result<Vec<Message>> {
        self.push("before_run");
        Ok(messages)
    }

    async fn before_provider_step(
        &self,
        messages: Vec<Message>,
        _state: &mut deepagents::state::AgentState,
    ) -> anyhow::Result<Vec<Message>> {
        self.push("before_provider_step");
        Ok(messages)
    }

    async fn patch_provider_step(
        &self,
        step: deepagents::provider::ProviderStep,
        _next_call_id: &mut u64,
    ) -> anyhow::Result<deepagents::provider::ProviderStep> {
        self.push("patch_provider_step");
        Ok(step)
    }

    async fn handle_tool_call(
        &self,
        ctx: &mut ToolCallContext<'_>,
    ) -> anyhow::Result<Option<HandledToolCall>> {
        self.push("handle_tool_call");
        if self.handle_dummy_tool && ctx.tool_call.tool_name == "dummy" {
            return Ok(Some(HandledToolCall {
                output: serde_json::json!({ "content": "handled" }),
                error: None,
            }));
        }
        Ok(None)
    }
}

#[tokio::test]
async fn phase8_5_hook_order_follows_slot_order() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let backend = create_local_sandbox_backend(root.to_string_lossy().to_string(), None).unwrap();
    let agent = create_deep_agent_with_backend(backend);

    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let a =
        Arc::new(RecordingMiddleware::new("A", log.clone(), false)) as Arc<dyn RuntimeMiddleware>;
    let b =
        Arc::new(RecordingMiddleware::new("B", log.clone(), false)) as Arc<dyn RuntimeMiddleware>;
    let c =
        Arc::new(RecordingMiddleware::new("C", log.clone(), true)) as Arc<dyn RuntimeMiddleware>;

    let mut asm = RuntimeMiddlewareAssembler::new();
    asm.push(RuntimeMiddlewareSlot::PatchToolCalls, "c_patch", c);
    asm.push(RuntimeMiddlewareSlot::TodoList, "a_todo", a);
    asm.push(RuntimeMiddlewareSlot::Summarization, "b_sum", b);
    let runtime_middlewares = asm.build().unwrap();

    let script = MockScript {
        steps: vec![
            MockStep::ToolCalls {
                calls: vec![ProviderToolCall {
                    tool_name: "dummy".to_string(),
                    arguments: serde_json::json!({}),
                    call_id: Some("c1".to_string()),
                }],
            },
            MockStep::FinalText {
                text: "done".to_string(),
            },
        ],
    };
    let provider: Arc<dyn deepagents::provider::Provider> =
        Arc::new(MockProvider::from_script(script));

    let runtime = deepagents::runtime::simple::SimpleRuntime::new(
        agent,
        provider,
        vec![],
        deepagents::runtime::simple::SimpleRuntimeOptions {
            config: deepagents::runtime::RuntimeConfig {
                max_steps: 8,
                provider_timeout_ms: 1000,
            },
            approval: None,
            audit: None,
            root: root.to_string_lossy().to_string(),
            mode: deepagents::approval::ExecutionMode::NonInteractive,
        },
    )
    .with_runtime_middlewares(runtime_middlewares);

    let _ = runtime
        .run(vec![Message {
            role: "user".to_string(),
            content: "hi".to_string(),
            content_blocks: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }])
        .await;

    let items = log.lock().unwrap().clone();

    let before_run: Vec<String> = items
        .iter()
        .filter(|s| s.starts_with("before_run:"))
        .cloned()
        .collect();
    assert_eq!(
        before_run,
        vec![
            "before_run:A".to_string(),
            "before_run:B".to_string(),
            "before_run:C".to_string()
        ]
    );

    let before_provider_step: Vec<String> = items
        .iter()
        .filter(|s| s.starts_with("before_provider_step:"))
        .cloned()
        .collect();
    assert_eq!(before_provider_step.len(), 6);
    assert_eq!(
        &before_provider_step[0..3],
        &[
            "before_provider_step:A",
            "before_provider_step:B",
            "before_provider_step:C"
        ]
    );
    assert_eq!(
        &before_provider_step[3..6],
        &[
            "before_provider_step:A",
            "before_provider_step:B",
            "before_provider_step:C"
        ]
    );

    let patch_provider_step: Vec<String> = items
        .iter()
        .filter(|s| s.starts_with("patch_provider_step:"))
        .cloned()
        .collect();
    assert_eq!(patch_provider_step.len(), 6);
    assert_eq!(
        &patch_provider_step[0..3],
        &[
            "patch_provider_step:A",
            "patch_provider_step:B",
            "patch_provider_step:C"
        ]
    );
    assert_eq!(
        &patch_provider_step[3..6],
        &[
            "patch_provider_step:A",
            "patch_provider_step:B",
            "patch_provider_step:C"
        ]
    );

    let handle_tool_call: Vec<String> = items
        .iter()
        .filter(|s| s.starts_with("handle_tool_call:"))
        .cloned()
        .collect();
    assert_eq!(
        handle_tool_call,
        vec![
            "handle_tool_call:A".to_string(),
            "handle_tool_call:B".to_string(),
            "handle_tool_call:C".to_string()
        ]
    );
}

#[test]
fn phase8_5_duplicate_non_user_slot_is_rejected() {
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let a =
        Arc::new(RecordingMiddleware::new("A", log.clone(), false)) as Arc<dyn RuntimeMiddleware>;
    let b =
        Arc::new(RecordingMiddleware::new("B", log.clone(), false)) as Arc<dyn RuntimeMiddleware>;

    let mut asm = RuntimeMiddlewareAssembler::new();
    asm.push(RuntimeMiddlewareSlot::TodoList, "a", a);
    asm.push(RuntimeMiddlewareSlot::TodoList, "b", b);
    assert!(asm.build().is_err());
}
