use std::sync::Arc;

use deepagents::provider::mock::{MockProvider, MockScript, MockStep};
use deepagents::runtime::{RunStatus, Runtime};
use deepagents::types::{AgentRequest, Message};

fn user_message(content: &str) -> Message {
    Message {
        role: "user".to_string(),
        content: content.to_string(),
        content_blocks: None,
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        status: None,
    }
}

#[tokio::test]
async fn deep_agent_runtime_builder_creates_runnable_runtime() {
    let root = tempfile::tempdir().unwrap();
    let agent = deepagents::create_deep_agent(root.path()).unwrap();
    let provider = Arc::new(MockProvider::from_script(MockScript {
        steps: vec![MockStep::FinalText {
            text: "done".to_string(),
        }],
    }));

    let runtime = agent
        .runtime(provider)
        .with_root(root.path().display().to_string())
        .build()
        .unwrap();

    let out = runtime.run(vec![user_message("hello")]).await;

    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(out.final_text, "done");
}

#[tokio::test]
async fn deep_agent_run_keeps_backward_compatible_default_response() {
    let root = tempfile::tempdir().unwrap();
    let agent = deepagents::create_deep_agent(root.path()).unwrap();

    let out = agent
        .run(AgentRequest {
            messages: vec![user_message("hello")],
        })
        .await
        .unwrap();

    assert_eq!(out.output_text, "");
}
