use std::sync::Arc;

use deepagents::provider::mock::{MockProvider, MockScript, MockStep};
use deepagents::provider::protocol::ProviderToolCall;
use deepagents::runtime::{Runtime, RuntimeMiddleware};
use deepagents::state::{AgentState, TodoItem};
use deepagents::{create_deep_agent_with_backend, create_local_sandbox_backend};

fn runtime_with_script(
    root: &std::path::Path,
    script: MockScript,
    initial_state: AgentState,
) -> impl Runtime {
    let backend = create_local_sandbox_backend(root.to_string_lossy().to_string(), None).unwrap();
    let agent = create_deep_agent_with_backend(backend);
    let provider: Arc<dyn deepagents::provider::Provider> =
        Arc::new(MockProvider::from_script(script));
    let todo_mw: Arc<dyn RuntimeMiddleware> =
        Arc::new(deepagents::runtime::TodoListMiddleware::new());
    deepagents::runtime::simple::SimpleRuntime::new(
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
    .with_runtime_middlewares(vec![todo_mw])
    .with_initial_state(initial_state)
}

#[tokio::test]
async fn todo_tm01_merge_false_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = MockScript {
        steps: vec![
            MockStep::ToolCalls {
                calls: vec![ProviderToolCall {
                    tool_name: "write_todos".to_string(),
                    arguments: serde_json::json!({
                        "merge": false,
                        "todos": [
                            { "id": "c", "content": "C", "status": "pending", "priority": "high" }
                        ]
                    }),
                    call_id: Some("c1".to_string()),
                }],
            },
            MockStep::FinalText {
                text: "done".to_string(),
            },
        ],
    };

    let initial_state = AgentState {
        todos: vec![
            TodoItem {
                id: "a".to_string(),
                content: "A".to_string(),
                status: "pending".to_string(),
                priority: "low".to_string(),
                active_form: None,
            },
            TodoItem {
                id: "b".to_string(),
                content: "B".to_string(),
                status: "pending".to_string(),
                priority: "low".to_string(),
                active_form: None,
            },
        ],
        ..Default::default()
    };

    let out = runtime_with_script(root, script, initial_state)
        .run(vec![deepagents::types::Message {
            role: "user".to_string(),
            content: "x".to_string(),
            content_blocks: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }])
        .await;

    assert_eq!(out.state.todos.len(), 1);
    assert_eq!(out.state.todos[0].id, "c");
}

#[tokio::test]
async fn todo_tm02_merge_true_partial_update_preserves_fields() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = MockScript {
        steps: vec![
            MockStep::ToolCalls {
                calls: vec![ProviderToolCall {
                    tool_name: "write_todos".to_string(),
                    arguments: serde_json::json!({
                        "merge": true,
                        "todos": [
                            { "id": "a", "status": "completed" }
                        ]
                    }),
                    call_id: Some("c1".to_string()),
                }],
            },
            MockStep::FinalText {
                text: "done".to_string(),
            },
        ],
    };

    let initial_state = AgentState {
        todos: vec![TodoItem {
            id: "a".to_string(),
            content: "A".to_string(),
            status: "pending".to_string(),
            priority: "high".to_string(),
            active_form: None,
        }],
        ..Default::default()
    };

    let out = runtime_with_script(root, script, initial_state)
        .run(vec![deepagents::types::Message {
            role: "user".to_string(),
            content: "x".to_string(),
            content_blocks: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }])
        .await;

    assert_eq!(out.state.todos.len(), 1);
    assert_eq!(out.state.todos[0].id, "a");
    assert_eq!(out.state.todos[0].status, "completed");
    assert_eq!(out.state.todos[0].content, "A");
    assert_eq!(out.state.todos[0].priority, "high");
}

#[tokio::test]
async fn todo_tm04_duplicate_ids_is_error_and_does_not_mutate_state() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = MockScript {
        steps: vec![
            MockStep::ToolCalls {
                calls: vec![ProviderToolCall {
                    tool_name: "write_todos".to_string(),
                    arguments: serde_json::json!({
                        "merge": false,
                        "todos": [
                            { "id": "a", "content": "A", "status": "pending", "priority": "high" },
                            { "id": "a", "content": "A2", "status": "pending", "priority": "high" }
                        ]
                    }),
                    call_id: Some("c1".to_string()),
                }],
            },
            MockStep::FinalText {
                text: "done".to_string(),
            },
        ],
    };

    let initial_state = AgentState {
        todos: vec![TodoItem {
            id: "a".to_string(),
            content: "orig".to_string(),
            status: "pending".to_string(),
            priority: "high".to_string(),
            active_form: None,
        }],
        ..Default::default()
    };

    let out = runtime_with_script(root, script, initial_state)
        .run(vec![deepagents::types::Message {
            role: "user".to_string(),
            content: "x".to_string(),
            content_blocks: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }])
        .await;

    assert_eq!(out.state.todos.len(), 1);
    assert_eq!(out.state.todos[0].content, "orig");
    let err = out
        .tool_results
        .iter()
        .find(|r| r.tool_name == "write_todos")
        .unwrap()
        .error
        .clone()
        .unwrap();
    assert!(err.contains("duplicate todo id"));
}

#[tokio::test]
async fn todo_tm05_summary_gate_rejects_without_completion_transition() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = MockScript {
        steps: vec![
            MockStep::ToolCalls {
                calls: vec![ProviderToolCall {
                    tool_name: "write_todos".to_string(),
                    arguments: serde_json::json!({
                        "merge": true,
                        "todos": [
                            { "id": "a", "status": "pending" }
                        ],
                        "summary": "x"
                    }),
                    call_id: Some("c1".to_string()),
                }],
            },
            MockStep::FinalText {
                text: "done".to_string(),
            },
        ],
    };

    let initial_state = AgentState {
        todos: vec![TodoItem {
            id: "a".to_string(),
            content: "A".to_string(),
            status: "pending".to_string(),
            priority: "high".to_string(),
            active_form: None,
        }],
        ..Default::default()
    };

    let out = runtime_with_script(root, script, initial_state)
        .run(vec![deepagents::types::Message {
            role: "user".to_string(),
            content: "x".to_string(),
            content_blocks: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }])
        .await;

    assert_eq!(out.state.todos[0].status, "pending");
    let err = out
        .tool_results
        .iter()
        .find(|r| r.tool_name == "write_todos")
        .unwrap()
        .error
        .clone()
        .unwrap();
    assert!(err.contains("summary is only allowed"));
}

#[tokio::test]
async fn todo_tp03_parallel_guard_rejects_all_write_todos_but_allows_other_tools() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("README.md"), "hello\n").unwrap();

    let script = MockScript {
        steps: vec![
            MockStep::ToolCalls {
                calls: vec![
                    ProviderToolCall {
                        tool_name: "write_todos".to_string(),
                        arguments: serde_json::json!({
                            "merge": false,
                            "todos": [
                                { "id": "b", "content": "B", "status": "pending", "priority": "high" }
                            ]
                        }),
                        call_id: Some("t1".to_string()),
                    },
                    ProviderToolCall {
                        tool_name: "read_file".to_string(),
                        arguments: serde_json::json!({ "file_path": "README.md", "limit": 5 }),
                        call_id: Some("r1".to_string()),
                    },
                    ProviderToolCall {
                        tool_name: "write_todos".to_string(),
                        arguments: serde_json::json!({
                            "merge": true,
                            "todos": [
                                { "id": "a", "status": "completed" }
                            ]
                        }),
                        call_id: Some("t2".to_string()),
                    },
                ],
            },
            MockStep::FinalText {
                text: "done".to_string(),
            },
        ],
    };

    let initial_state = AgentState {
        todos: vec![TodoItem {
            id: "a".to_string(),
            content: "A".to_string(),
            status: "pending".to_string(),
            priority: "high".to_string(),
            active_form: None,
        }],
        ..Default::default()
    };

    let out = runtime_with_script(root, script, initial_state)
        .run(vec![deepagents::types::Message {
            role: "user".to_string(),
            content: "x".to_string(),
            content_blocks: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }])
        .await;

    assert_eq!(out.state.todos.len(), 1);
    assert_eq!(out.state.todos[0].id, "a");
    assert_eq!(out.state.todos[0].status, "pending");

    let rejected: Vec<_> = out
        .tool_results
        .iter()
        .filter(|r| r.tool_name == "write_todos")
        .collect();
    assert_eq!(rejected.len(), 2);
    for r in rejected {
        let err = r.error.as_deref().unwrap_or_default();
        assert!(err.contains("should never be called multiple times in parallel"));
    }

    let read = out
        .tool_results
        .iter()
        .find(|r| r.tool_name == "read_file")
        .unwrap();
    assert!(read.error.is_none());
}
