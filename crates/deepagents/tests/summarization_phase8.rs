use deepagents::runtime::{
    RuntimeMiddleware, SummarizationMiddleware, SummarizationOptions, SummarizationPolicyKind,
};
use deepagents::state::AgentState;
use deepagents::types::{Message, ToolCall};

fn build_message(role: &str, content: &str) -> Message {
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

fn build_tool_message(content: &str, name: &str, args: serde_json::Value) -> Message {
    Message {
        role: "assistant".to_string(),
        content: content.to_string(),
        content_blocks: None,
        tool_calls: Some(vec![ToolCall {
            id: "call_1".to_string(),
            name: name.to_string(),
            arguments: args,
        }]),
        tool_call_id: None,
        name: None,
        status: None,
    }
}

#[tokio::test]
async fn summarization_event_is_emitted_and_history_written() {
    let temp = tempfile::tempdir().unwrap();
    let options = SummarizationOptions {
        policy: SummarizationPolicyKind::Budget,
        max_char_budget: 120,
        min_recent_messages: 2,
        ..Default::default()
    };
    let mw = SummarizationMiddleware::new(temp.path().to_string_lossy().to_string(), options);

    let mut state = AgentState::default();
    let messages = vec![
        build_message("user", "hello world message one"),
        build_message("assistant", "response one with extra text"),
        build_message("user", "message two that is quite long"),
        build_message("assistant", "response two with extra text"),
        build_message("user", "message three that is quite long"),
    ];

    let effective = mw
        .before_provider_step(messages.clone(), &mut state)
        .await
        .unwrap();
    let event = state.extra.get("_summarization_event").cloned().unwrap();
    let event: deepagents::runtime::SummarizationEvent = serde_json::from_value(event).unwrap();
    assert!(event.cutoff_index > 0);
    assert!(effective.first().unwrap().name.as_deref() == Some("summarization"));
    let thread_id = state
        .extra
        .get("thread_id")
        .and_then(|v| v.as_str())
        .unwrap();
    let history_path = temp
        .path()
        .join("conversation_history")
        .join(format!("{thread_id}.md"));
    assert!(history_path.exists());
}

#[tokio::test]
async fn truncates_old_tool_args_only() {
    let temp = tempfile::tempdir().unwrap();
    let options = SummarizationOptions {
        redact_tool_args: true,
        max_tool_arg_chars: 20,
        truncate_tool_args_keep_last: 1,
        ..Default::default()
    };
    let mw = SummarizationMiddleware::new(temp.path().to_string_lossy().to_string(), options);

    let mut state = AgentState::default();
    let messages = vec![
        build_tool_message(
            "",
            "write_file",
            serde_json::json!({"path": "/a.txt", "content": "abcdefghijklmnopqrstuvwxyz0123456789"}),
        ),
        build_tool_message(
            "",
            "write_file",
            serde_json::json!({"path": "/b.txt", "content": "abcdefghijklmnopqrstuvwxyz0123456789"}),
        ),
    ];

    let effective = mw.before_provider_step(messages, &mut state).await.unwrap();
    let first = effective[0].tool_calls.as_ref().unwrap()[0]
        .arguments
        .to_string();
    let second = effective[1].tool_calls.as_ref().unwrap()[0]
        .arguments
        .to_string();
    assert!(first.contains("truncated"));
    assert!(second.contains("abcdefghijklmnopqrstuvwxyz0123456789"));
}
