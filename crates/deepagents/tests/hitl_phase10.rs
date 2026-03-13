use std::collections::BTreeMap;
use std::sync::Arc;

use deepagents::approval::{DefaultApprovalPolicy, ExecutionMode};
use deepagents::provider::mock::{MockProvider, MockScript, MockStep};
use deepagents::provider::AgentToolCall;
use deepagents::runtime::{HitlDecision, ResumableRunner, ResumableRunnerOptions, RunStatus};

fn interrupt_on(keys: &[&str]) -> BTreeMap<String, bool> {
    let mut m = BTreeMap::new();
    for k in keys {
        m.insert(k.to_string(), true);
    }
    m
}

#[tokio::test]
async fn h01_approve_executes_tool() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("a.txt"), "1").unwrap();

    let script = MockScript {
        steps: vec![
            MockStep::ToolCalls {
                calls: vec![AgentToolCall {
                    tool_name: "edit_file".to_string(),
                    arguments: serde_json::json!({
                        "file_path": "a.txt",
                        "old_string": "1",
                        "new_string": "2"
                    }),
                    call_id: Some("c1".to_string()),
                }],
            },
            MockStep::FinalText {
                text: "DONE".to_string(),
            },
        ],
    };
    let provider: Arc<dyn deepagents::provider::AgentProvider> =
        Arc::new(MockProvider::from_script(script));
    let backend = deepagents::create_local_sandbox_backend(root, None).unwrap();
    let agent = deepagents::create_deep_agent_with_backend(backend);

    let mut runner = ResumableRunner::new(
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
            interrupt_on: interrupt_on(&["edit_file"]),
        },
    );

    runner.push_user_input("go".to_string());
    let out1 = runner.run().await;
    assert_eq!(out1.status, RunStatus::Interrupted);
    assert_eq!(out1.interrupts.len(), 1);
    assert_eq!(out1.interrupts[0].tool_name, "edit_file");
    assert_eq!(out1.interrupts[0].tool_call_id, "c1");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "1");

    let out2 = runner.resume("c1", HitlDecision::Approve).await;
    assert_eq!(out2.status, RunStatus::Completed);
    assert!(out2.error.is_none());
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "2");
    assert!(out2
        .tool_results
        .iter()
        .any(|r| r.call_id.as_deref() == Some("c1") && r.status.as_deref() == Some("success")));
}

#[tokio::test]
async fn h02_reject_cancels_without_side_effect() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = MockScript {
        steps: vec![
            MockStep::ToolCalls {
                calls: vec![AgentToolCall {
                    tool_name: "write_file".to_string(),
                    arguments: serde_json::json!({
                        "file_path": "deny.txt",
                        "content": "x\n"
                    }),
                    call_id: Some("w1".to_string()),
                }],
            },
            MockStep::FinalText {
                text: "DONE".to_string(),
            },
        ],
    };
    let provider: Arc<dyn deepagents::provider::AgentProvider> =
        Arc::new(MockProvider::from_script(script));
    let backend = deepagents::create_local_sandbox_backend(root, None).unwrap();
    let agent = deepagents::create_deep_agent_with_backend(backend);

    let mut runner = ResumableRunner::new(
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
            interrupt_on: interrupt_on(&["write_file"]),
        },
    );

    runner.push_user_input("go".to_string());
    let out1 = runner.run().await;
    assert_eq!(out1.status, RunStatus::Interrupted);
    assert!(!root.join("deny.txt").exists());

    let out2 = runner
        .resume(
            "w1",
            HitlDecision::Reject {
                reason: Some("no".to_string()),
            },
        )
        .await;
    assert_eq!(out2.status, RunStatus::Completed);
    assert!(!root.join("deny.txt").exists());
    assert!(out2
        .tool_results
        .iter()
        .any(|r| r.call_id.as_deref() == Some("w1") && r.status.as_deref() == Some("rejected")));
}

#[tokio::test]
async fn h03_edit_changes_args_and_keeps_call_id() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = MockScript {
        steps: vec![
            MockStep::ToolCalls {
                calls: vec![AgentToolCall {
                    tool_name: "write_file".to_string(),
                    arguments: serde_json::json!({
                        "file_path": "a.txt",
                        "content": "1\n"
                    }),
                    call_id: None,
                }],
            },
            MockStep::FinalText {
                text: "DONE".to_string(),
            },
        ],
    };
    let provider: Arc<dyn deepagents::provider::AgentProvider> =
        Arc::new(MockProvider::from_script_without_call_ids(script));
    let backend = deepagents::create_local_sandbox_backend(root, None).unwrap();
    let agent = deepagents::create_deep_agent_with_backend(backend);

    let mut runner = ResumableRunner::new(
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
            interrupt_on: interrupt_on(&["write_file"]),
        },
    );

    runner.push_user_input("go".to_string());
    let out1 = runner.run().await;
    assert_eq!(out1.status, RunStatus::Interrupted);
    assert_eq!(out1.interrupts.len(), 1);
    let interrupt_id = out1.interrupts[0].interrupt_id.clone();
    assert_eq!(out1.interrupts[0].tool_call_id, interrupt_id);

    let out2 = runner
        .resume(
            &interrupt_id,
            HitlDecision::Edit {
                args: serde_json::json!({
                    "file_path": "b.txt",
                    "content": "2\n"
                }),
            },
        )
        .await;
    assert_eq!(out2.status, RunStatus::Completed);
    assert!(!root.join("a.txt").exists());
    assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "2\n");
    assert!(out2.tool_results.iter().any(|r| {
        r.call_id.as_deref() == Some(interrupt_id.as_str()) && r.status.as_deref() == Some("edited")
    }));
}

#[tokio::test]
async fn h04_multiple_interrupts_in_one_batch() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("b.txt"), "1").unwrap();

    let script = MockScript {
        steps: vec![
            MockStep::ToolCalls {
                calls: vec![
                    AgentToolCall {
                        tool_name: "write_file".to_string(),
                        arguments: serde_json::json!({
                            "file_path": "a.txt",
                            "content": "x\n"
                        }),
                        call_id: Some("a1".to_string()),
                    },
                    AgentToolCall {
                        tool_name: "edit_file".to_string(),
                        arguments: serde_json::json!({
                            "file_path": "b.txt",
                            "old_string": "1",
                            "new_string": "2"
                        }),
                        call_id: Some("b1".to_string()),
                    },
                ],
            },
            MockStep::FinalText {
                text: "DONE".to_string(),
            },
        ],
    };
    let provider: Arc<dyn deepagents::provider::AgentProvider> =
        Arc::new(MockProvider::from_script(script));
    let backend = deepagents::create_local_sandbox_backend(root, None).unwrap();
    let agent = deepagents::create_deep_agent_with_backend(backend);

    let mut runner = ResumableRunner::new(
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
            interrupt_on: interrupt_on(&["write_file", "edit_file"]),
        },
    );

    runner.push_user_input("go".to_string());
    let out1 = runner.run().await;
    assert_eq!(out1.status, RunStatus::Interrupted);
    assert_eq!(out1.interrupts[0].tool_call_id, "a1");
    assert!(!root.join("a.txt").exists());
    assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "1");

    let out2 = runner.resume("a1", HitlDecision::Approve).await;
    assert_eq!(out2.status, RunStatus::Interrupted);
    assert_eq!(out2.interrupts[0].tool_call_id, "b1");
    assert!(root.join("a.txt").exists());
    assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "1");

    let out3 = runner.resume("b1", HitlDecision::Approve).await;
    assert_eq!(out3.status, RunStatus::Completed);
    assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "2");
}

#[tokio::test]
async fn h05_invalid_resume_keeps_pending_and_allows_retry() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = MockScript {
        steps: vec![
            MockStep::ToolCalls {
                calls: vec![AgentToolCall {
                    tool_name: "write_file".to_string(),
                    arguments: serde_json::json!({
                        "file_path": "a.txt",
                        "content": "1\n"
                    }),
                    call_id: Some("w1".to_string()),
                }],
            },
            MockStep::FinalText {
                text: "DONE".to_string(),
            },
        ],
    };
    let provider: Arc<dyn deepagents::provider::AgentProvider> =
        Arc::new(MockProvider::from_script(script));
    let backend = deepagents::create_local_sandbox_backend(root, None).unwrap();
    let agent = deepagents::create_deep_agent_with_backend(backend);

    let mut runner = ResumableRunner::new(
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
            interrupt_on: interrupt_on(&["write_file"]),
        },
    );

    runner.push_user_input("go".to_string());
    let out1 = runner.run().await;
    assert_eq!(out1.status, RunStatus::Interrupted);
    assert!(!root.join("a.txt").exists());

    let out2 = runner
        .resume(
            "w1",
            HitlDecision::Edit {
                args: serde_json::json!({ "content": "x\n" }),
            },
        )
        .await;
    assert_eq!(out2.status, RunStatus::Error);
    assert!(out2.error.is_some());
    assert!(!root.join("a.txt").exists());
    assert!(runner.pending_interrupt().is_some());

    let out3 = runner
        .resume(
            "w1",
            HitlDecision::Edit {
                args: serde_json::json!({
                    "file_path": "b.txt",
                    "content": "2\n"
                }),
            },
        )
        .await;
    assert_eq!(out3.status, RunStatus::Completed);
    assert!(!root.join("a.txt").exists());
    assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "2\n");
}

#[tokio::test]
async fn h06_interactive_execute_resume_approve_runs_when_policy_requires_approval() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = MockScript {
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
                text: "DONE".to_string(),
            },
        ],
    };
    let provider: Arc<dyn deepagents::provider::AgentProvider> =
        Arc::new(MockProvider::from_script(script));
    let backend = deepagents::create_local_sandbox_backend(root, None).unwrap();
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
            root: root.to_string_lossy().to_string(),
            mode: ExecutionMode::Interactive,
            interrupt_on: BTreeMap::new(),
        },
    );

    runner.push_user_input("go".to_string());
    let out1 = runner.run().await;
    assert_eq!(out1.status, RunStatus::Interrupted);
    assert_eq!(out1.interrupts.len(), 1);
    assert_eq!(out1.interrupts[0].tool_name, "execute");
    assert_eq!(out1.interrupts[0].tool_call_id, "e1");

    let out2 = runner.resume("e1", HitlDecision::Approve).await;
    assert_eq!(out2.status, RunStatus::Completed);
    assert_eq!(out2.final_text, "DONE");
    let execute_record = out2
        .tool_results
        .iter()
        .find(|record| record.tool_name == "execute")
        .unwrap();
    assert_eq!(execute_record.status.as_deref(), Some("success"));
    assert_eq!(
        execute_record
            .output
            .get("exit_code")
            .and_then(|value| value.as_i64()),
        Some(0)
    );
}

#[tokio::test]
async fn h07_interactive_execute_resume_edit_runs_when_policy_requires_approval() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = MockScript {
        steps: vec![
            MockStep::ToolCalls {
                calls: vec![AgentToolCall {
                    tool_name: "execute".to_string(),
                    arguments: serde_json::json!({
                        "command": "echo blocked",
                        "timeout": 5
                    }),
                    call_id: Some("e2".to_string()),
                }],
            },
            MockStep::FinalText {
                text: "DONE".to_string(),
            },
        ],
    };
    let provider: Arc<dyn deepagents::provider::AgentProvider> =
        Arc::new(MockProvider::from_script(script));
    let backend = deepagents::create_local_sandbox_backend(root, None).unwrap();
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
            root: root.to_string_lossy().to_string(),
            mode: ExecutionMode::Interactive,
            interrupt_on: BTreeMap::new(),
        },
    );

    runner.push_user_input("go".to_string());
    let out1 = runner.run().await;
    assert_eq!(out1.status, RunStatus::Interrupted);
    assert_eq!(out1.interrupts.len(), 1);
    assert_eq!(out1.interrupts[0].tool_name, "execute");
    assert!(out1
        .tool_results
        .iter()
        .all(|record| record.tool_name != "execute"));

    let out2 = runner
        .resume(
            "e2",
            HitlDecision::Edit {
                args: serde_json::json!({
                    "command": "echo edited",
                    "timeout": 5
                }),
            },
        )
        .await;
    assert_eq!(out2.status, RunStatus::Completed);
    assert_eq!(out2.final_text, "DONE");
    let execute_record = out2
        .tool_results
        .iter()
        .find(|record| record.tool_name == "execute")
        .unwrap();
    assert_eq!(execute_record.status.as_deref(), Some("edited"));
    assert_eq!(
        execute_record
            .output
            .get("exit_code")
            .and_then(|value| value.as_i64()),
        Some(0)
    );
}
