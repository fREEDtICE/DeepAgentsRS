use deepagents::provider::ProviderToolCall;
use deepagents::runtime::patch_tool_calls::{patch_dangling_tool_calls, sanitize_tool_call_id};
use deepagents::runtime::tool_compat::{
    normalize_messages, normalize_tool_call_for_execution, tool_results_from_messages,
    NormalizedToolCall,
};
use deepagents::types::{Message, ToolCall};

fn assistant_with_tool_calls(calls: Vec<ToolCall>) -> Message {
    Message {
        role: "assistant".to_string(),
        content: String::new(),
        content_blocks: None,
        reasoning_content: None,
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
        reasoning_content: None,
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
        Message {
            role: "user".to_string(),
            content: "later".to_string(),
            content_blocks: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        },
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

#[test]
fn normalize_messages_extracts_assistant_tool_calls_from_json_content() {
    let content = serde_json::json!({
        "content": "hi",
        "tool_calls": [
            { "id": "t1", "name": "read_file", "arguments": { "file_path": "README.md" } }
        ]
    })
    .to_string();
    let msg = Message {
        role: "assistant".to_string(),
        content,
        content_blocks: None,
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        status: None,
    };
    let out = normalize_messages(vec![msg]);
    assert_eq!(out[0].content, "hi");
    assert_eq!(out[0].tool_calls.as_ref().unwrap().len(), 1);
    assert_eq!(out[0].tool_calls.as_ref().unwrap()[0].id, "t1");
}

#[test]
fn normalize_messages_extracts_tool_call_id_from_tool_json_content() {
    let msg = Message {
        role: "tool".to_string(),
        content: r#"{"tool_call_id":"x","content":"ok"}"#.to_string(),
        content_blocks: None,
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        status: None,
    };
    let out = normalize_messages(vec![msg]);
    assert_eq!(out[0].tool_call_id.as_deref().unwrap(), "x");
    assert_eq!(out[0].status.as_deref().unwrap(), "success");
}

#[test]
fn provider_tool_call_deserializes_field_aliases() {
    let v = serde_json::json!({
        "name": "read_file",
        "input": { "file_path": "README.md" },
        "id": "c1"
    });
    let c: ProviderToolCall = serde_json::from_value(v).unwrap();
    assert_eq!(c.tool_name, "read_file");
    assert_eq!(c.call_id.as_deref().unwrap(), "c1");
    assert!(c.arguments.is_object());
}

#[test]
fn normalize_tool_call_for_execution_rejects_invalid_json_string_arguments() {
    let mut next = 1u64;
    let call = ProviderToolCall {
        tool_name: "read_file".to_string(),
        arguments: serde_json::Value::String("not json".to_string()),
        call_id: None,
    };
    let out = normalize_tool_call_for_execution(call, &mut next);
    match out {
        NormalizedToolCall::Invalid { call, error } => {
            assert_eq!(call.call_id.as_deref().unwrap(), "call-1");
            assert!(error.starts_with("invalid_tool_call:"));
        }
        NormalizedToolCall::Valid(_) => panic!("expected invalid"),
    }
}

#[test]
fn tool_results_from_messages_parses_runtime_tool_json_shape() {
    let msg = Message {
        role: "tool".to_string(),
        content: serde_json::json!({
            "tool_call_id": "x",
            "tool_name": "read_file",
            "status": "success",
            "output": { "content": "hello" },
            "content": "hello"
        })
        .to_string(),
        content_blocks: None,
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: Some("x".to_string()),
        name: Some("read_file".to_string()),
        status: Some("success".to_string()),
    };
    let out = tool_results_from_messages(&[msg]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].call_id.as_deref().unwrap(), "x");
    assert_eq!(out[0].tool_name, "read_file");
    assert_eq!(out[0].status.as_deref().unwrap(), "success");
    assert_eq!(
        out[0]
            .output
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap(),
        "hello"
    );
}

#[test]
fn sanitize_tool_call_id_blocks_path_segments() {
    assert_eq!(sanitize_tool_call_id("../a/b\\c"), ".__a_b_c");
    assert_eq!(sanitize_tool_call_id(""), "_");
}
