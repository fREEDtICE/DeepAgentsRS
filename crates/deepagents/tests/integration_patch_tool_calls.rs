use std::sync::Arc;

use deepagents::approval::ExecutionMode;
use deepagents::provider::mock::{MockProvider, MockScript, MockStep};
use deepagents::provider::ProviderToolCall;
use deepagents::runtime::patch_tool_calls::patch_dangling_tool_calls;
use deepagents::runtime::patch_tool_calls::PatchToolCallsMiddleware;
use deepagents::runtime::simple::SimpleRuntime;
use deepagents::runtime::Runtime;
use deepagents::runtime::RuntimeConfig;
use deepagents::types::{Message, ToolCall};

fn msg(role: &str, content: &str) -> Message {
    Message {
        role: role.to_string(),
        content: content.to_string(),
        content_blocks: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        status: None,
    }
}

fn assistant_with_tool_calls(calls: Vec<ToolCall>) -> Message {
    Message {
        role: "assistant".to_string(),
        content: String::new(),
        content_blocks: None,
        tool_calls: Some(calls),
        tool_call_id: None,
        name: None,
        status: None,
    }
}

fn tool_msg(call_id: &str, name: &str, status: &str, content: &str) -> Message {
    Message {
        role: "tool".to_string(),
        content: content.to_string(),
        content_blocks: None,
        tool_calls: None,
        tool_call_id: Some(call_id.to_string()),
        name: Some(name.to_string()),
        status: Some(status.to_string()),
    }
}

#[test]
fn pt01_single_dangling_tool_call_is_patched() {
    let calls = vec![ToolCall {
        id: "x".to_string(),
        name: "write_file".to_string(),
        arguments: serde_json::json!({"file_path":"a.txt","content":"hi"}),
    }];
    let messages = vec![assistant_with_tool_calls(calls)];
    let patched = patch_dangling_tool_calls(messages);
    assert_eq!(patched.len(), 2);
    assert_eq!(patched[1].role, "tool");
    assert_eq!(patched[1].tool_call_id.as_deref().unwrap(), "x");
    assert_eq!(patched[1].status.as_deref().unwrap(), "patched");
    let v = serde_json::from_str::<serde_json::Value>(&patched[1].content).unwrap();
    assert_eq!(v.get("status").and_then(|v| v.as_str()).unwrap(), "patched");
    assert_eq!(
        v.get("error").and_then(|v| v.as_str()).unwrap(),
        "tool_call_cancelled: missing tool result"
    );
    let c = v.get("content").and_then(|v| v.as_str()).unwrap();
    assert!(c.starts_with("PATCHED_TOOL_CALL:"));
}

#[test]
fn pt02_multiple_dangling_tool_calls_are_patched_in_order() {
    let calls = vec![
        ToolCall {
            id: "a".to_string(),
            name: "write_file".to_string(),
            arguments: serde_json::json!({}),
        },
        ToolCall {
            id: "b".to_string(),
            name: "edit_file".to_string(),
            arguments: serde_json::json!({}),
        },
    ];
    let messages = vec![assistant_with_tool_calls(calls)];
    let patched = patch_dangling_tool_calls(messages);
    assert_eq!(patched.len(), 3);
    assert_eq!(patched[1].tool_call_id.as_deref().unwrap(), "a");
    assert_eq!(patched[2].tool_call_id.as_deref().unwrap(), "b");
}

#[test]
fn pt03_history_consistent_is_not_modified() {
    let calls = vec![ToolCall {
        id: "x".to_string(),
        name: "write_file".to_string(),
        arguments: serde_json::json!({}),
    }];
    let messages = vec![
        assistant_with_tool_calls(calls),
        tool_msg("x", "write_file", "success", "ok"),
    ];
    let patched = patch_dangling_tool_calls(messages.clone());
    assert_eq!(patched.len(), messages.len());
    assert_eq!(patched[0].tool_calls.as_ref().unwrap().len(), 1);
    assert_eq!(patched[1].tool_call_id.as_deref().unwrap(), "x");
}

#[test]
fn pt04_only_truly_dangling_tool_calls_are_patched() {
    let calls = vec![
        ToolCall {
            id: "a".to_string(),
            name: "write_file".to_string(),
            arguments: serde_json::json!({}),
        },
        ToolCall {
            id: "b".to_string(),
            name: "grep".to_string(),
            arguments: serde_json::json!({}),
        },
    ];
    let messages = vec![
        assistant_with_tool_calls(calls),
        msg("user", "later"),
        tool_msg("a", "write_file", "success", "done"),
    ];
    let patched = patch_dangling_tool_calls(messages);
    assert_eq!(patched.len(), 4);
    assert_eq!(patched[1].tool_call_id.as_deref().unwrap(), "b");
    assert_eq!(patched[1].status.as_deref().unwrap(), "patched");
}

#[test]
fn pt05_patch_is_idempotent() {
    let calls = vec![ToolCall {
        id: "x".to_string(),
        name: "write_file".to_string(),
        arguments: serde_json::json!({}),
    }];
    let messages = vec![assistant_with_tool_calls(calls)];
    let once = patch_dangling_tool_calls(messages);
    let twice = patch_dangling_tool_calls(once.clone());
    assert_eq!(once.len(), twice.len());
    assert_eq!(once[1].tool_call_id, twice[1].tool_call_id);
}

#[tokio::test]
async fn normalize_accepts_string_json_arguments_for_tool_calls() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("README.md"), "Project: DeepAgents\nhello\n").unwrap();

    let script = MockScript {
        steps: vec![
            MockStep::ToolCalls {
                calls: vec![ProviderToolCall {
                    tool_name: "read_file".to_string(),
                    arguments: serde_json::Value::String(
                        serde_json::json!({"file_path":"README.md","limit":20}).to_string(),
                    ),
                    call_id: Some("c1".to_string()),
                }],
            },
            MockStep::FinalFromLastToolFirstLine {
                prefix: Some("first: ".to_string()),
            },
        ],
    };
    let provider: Arc<dyn deepagents::provider::Provider> =
        Arc::new(MockProvider::from_script(script));

    let backend = deepagents::create_local_sandbox_backend(root, None).unwrap();
    let agent = deepagents::create_deep_agent_with_backend(backend);

    let patch_mw: Arc<dyn deepagents::runtime::RuntimeMiddleware> =
        Arc::new(PatchToolCallsMiddleware::new());
    let runtime = SimpleRuntime::new(
        agent,
        provider,
        Vec::new(),
        deepagents::runtime::simple::SimpleRuntimeOptions {
            config: RuntimeConfig {
                max_steps: 8,
                provider_timeout_ms: 1000,
            },
            approval: None,
            audit: None,
            root: root.to_string_lossy().to_string(),
            mode: ExecutionMode::NonInteractive,
        },
    )
    .with_runtime_middlewares(vec![patch_mw]);

    let out = runtime.run(vec![msg("user", "read readme")]).await;
    assert!(out.error.is_none());
    assert_eq!(out.final_text, "first: Project: DeepAgents");
}
