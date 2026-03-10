use std::process::Command;

fn run_runtime(
    bin: &str,
    root: &str,
    shell_allow: &[&str],
    provider: &str,
    mock_script: &str,
    input: &str,
) -> (std::process::ExitStatus, serde_json::Value) {
    let mut cmd = Command::new(bin);
    cmd.args(["--root", root]);
    for a in shell_allow {
        cmd.args(["--shell-allow", a]);
    }
    cmd.args(["run", "--provider", provider]);
    cmd.args(["--mock-script", mock_script]);
    cmd.args(["--provider-timeout-ms", "1000"]);
    cmd.args(["--max-steps", "16"]);
    cmd.args(["--input", input]);
    let out = cmd.output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    (out.status, v)
}

fn write_json(path: &std::path::Path, v: &serde_json::Value) -> std::path::PathBuf {
    std::fs::write(path, serde_json::to_vec_pretty(v).unwrap()).unwrap();
    path.to_path_buf()
}

fn tool_result_content(v: &serde_json::Value, idx: usize) -> String {
    v.get("tool_results")
        .and_then(|x| x.as_array())
        .and_then(|a| a.get(idx))
        .and_then(|r| r.get("output"))
        .and_then(|o| o.get("content"))
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string()
}

fn tool_result_error(v: &serde_json::Value, idx: usize) -> String {
    v.get("tool_results")
        .and_then(|x| x.as_array())
        .and_then(|a| a.get(idx))
        .and_then(|r| r.get("error"))
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string()
}

#[test]
fn phase4_sa01_minimal_task_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "task", "arguments": { "description": "say hi", "subagent_type": "general-purpose" }, "call_id": "t1" }
        ]},
        { "type": "final_from_last_tool_first_line", "prefix": "" }
      ]
    });
    let script_path = write_json(&root.join("script.json"), &script);

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        "run",
    );
    assert!(st.success());
    assert_eq!(v.get("final_text").and_then(|s| s.as_str()).unwrap(), "HI");
    assert_eq!(v.get("tool_calls").unwrap().as_array().unwrap().len(), 1);
    assert_eq!(v.get("tool_results").unwrap().as_array().unwrap().len(), 1);
}

#[test]
fn phase4_sa02_isolation_child_messages_only_description() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "task", "arguments": { "description": "check isolation", "subagent_type": "echo-subagent" }, "call_id": "t1" }
        ]},
        { "type": "final_text", "text": "done" }
      ]
    });
    let script_path = write_json(&root.join("script.json"), &script);

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        "SECRET_IN_MAIN",
    );
    assert!(st.success());
    let payload: serde_json::Value = serde_json::from_str(&tool_result_content(&v, 0)).unwrap();
    assert_eq!(
        payload
            .get("messages_len")
            .and_then(|n| n.as_u64())
            .unwrap(),
        1
    );
    assert_eq!(
        payload
            .get("first_message")
            .and_then(|m| m.get("content"))
            .and_then(|s| s.as_str())
            .unwrap(),
        "check isolation"
    );
    assert!(!payload
        .get("saw_secret_in_messages")
        .and_then(|b| b.as_bool())
        .unwrap());
}

#[test]
fn phase4_sa03_isolation_excluded_keys_not_propagated_to_child() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "task", "arguments": { "description": "check keys", "subagent_type": "echo-subagent" }, "call_id": "t1" }
        ]},
        { "type": "final_text", "text": "done" }
      ]
    });
    let script_path = write_json(&root.join("script.json"), &script);

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        "input",
    );
    assert!(st.success());
    let payload: serde_json::Value = serde_json::from_str(&tool_result_content(&v, 0)).unwrap();
    let keys = payload
        .get("state_extra_keys")
        .and_then(|a| a.as_array())
        .unwrap()
        .iter()
        .filter_map(|x| x.as_str())
        .collect::<Vec<_>>();
    assert!(!keys.contains(&"todos"));
    assert!(!keys.contains(&"messages"));
    assert!(!keys.contains(&"structured_response"));
    assert!(!keys.contains(&"skills_metadata"));
    assert!(!keys.contains(&"memory_contents"));
}

#[test]
fn phase4_sa04_only_final_message_is_returned() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "task", "arguments": { "description": "multi", "subagent_type": "multi-message-subagent" }, "call_id": "t1" }
        ]},
        { "type": "final_from_last_tool_first_line", "prefix": "" }
      ]
    });
    let script_path = write_json(&root.join("script.json"), &script);

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        "input",
    );
    assert!(st.success());
    assert_eq!(
        v.get("final_text").and_then(|s| s.as_str()).unwrap(),
        "final"
    );
}

#[test]
fn phase4_sa05_state_update_filtered_and_merged() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "task", "arguments": { "description": "merge", "subagent_type": "state-extra-subagent" }, "call_id": "t1" }
        ]},
        { "type": "final_text", "text": "done" }
      ]
    });
    let script_path = write_json(&root.join("script.json"), &script);

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        "input",
    );
    assert!(st.success());
    let state = v.get("state").unwrap();
    let extra = state.get("extra").and_then(|e| e.as_object()).unwrap();
    assert!(extra.contains_key("allowed_key"));
    assert!(!extra.contains_key("todos"));
    assert!(!extra.contains_key("memory_contents"));
    assert!(!extra.contains_key("skills_metadata"));
    assert!(!extra.contains_key("structured_response"));
    assert!(!extra.contains_key("messages"));
}

#[test]
fn phase4_sa06_subagent_output_must_be_non_empty() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "task", "arguments": { "description": "broken", "subagent_type": "broken-subagent" }, "call_id": "t1" }
        ]},
        { "type": "final_text", "text": "done" }
      ]
    });
    let script_path = write_json(&root.join("script.json"), &script);

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        "input",
    );
    assert!(st.success());
    let err = tool_result_error(&v, 0);
    assert!(err.contains("subagent_invalid_output"));
}

#[test]
fn phase4_sa07_child_tool_side_effects_happen_without_polluting_parent_trace() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "task", "arguments": { "description": "write", "subagent_type": "write-file-subagent" }, "call_id": "t1" }
        ]},
        { "type": "final_from_last_tool_first_line", "prefix": "" }
      ]
    });
    let script_path = write_json(&root.join("script.json"), &script);

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        "input",
    );
    assert!(st.success());
    assert_eq!(
        v.get("final_text").and_then(|s| s.as_str()).unwrap(),
        "DONE"
    );
    assert!(root.join("child.txt").exists());
    assert_eq!(v.get("tool_calls").unwrap().as_array().unwrap().len(), 1);
    assert_eq!(
        v.get("tool_calls").unwrap().as_array().unwrap()[0]
            .get("tool_name")
            .and_then(|s| s.as_str())
            .unwrap(),
        "task"
    );
}

#[test]
fn phase4_sa08_nested_task_is_supported_with_depth_limit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "task", "arguments": { "description": "nested", "subagent_type": "nested-task-subagent" }, "call_id": "t1" }
        ]},
        { "type": "final_from_last_tool_first_line", "prefix": "" }
      ]
    });
    let script_path = write_json(&root.join("script.json"), &script);

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        "input",
    );
    assert!(st.success());
    assert_eq!(v.get("final_text").and_then(|s| s.as_str()).unwrap(), "HI");
}

#[test]
fn phase4_security_root_boundary_is_enforced_in_subagent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.parent().unwrap().join("secret.txt"), "SECRET\n").unwrap();

    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "task", "arguments": { "description": "escape", "subagent_type": "root-escape-subagent" }, "call_id": "t1" }
        ]},
        { "type": "final_from_last_tool_first_line", "prefix": "" }
      ]
    });
    let script_path = write_json(&root.join("script.json"), &script);

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        "input",
    );
    assert!(st.success());
    let text = v.get("final_text").and_then(|s| s.as_str()).unwrap();
    assert!(text.contains("permission_denied") || text.contains("invalid_path"));
}

#[test]
fn phase4_security_execute_policy_is_enforced_in_subagent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "task", "arguments": { "description": "exec", "subagent_type": "execute-deny-subagent" }, "call_id": "t1" }
        ]},
        { "type": "final_from_last_tool_first_line", "prefix": "" }
      ]
    });
    let script_path = write_json(&root.join("script.json"), &script);

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        "input",
    );
    assert!(st.success());
    let text = v.get("final_text").and_then(|s| s.as_str()).unwrap();
    assert!(text.contains("command_not_allowed"));
    assert!(
        text.contains("approval_required")
            || text.contains("not_in_allow_list")
            || text.contains("dangerous_pattern")
    );
}
