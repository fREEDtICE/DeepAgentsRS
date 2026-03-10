use std::process::Command;

fn run_runtime(
    bin: &str,
    root: &str,
    shell_allow: &[&str],
    provider: &str,
    mock_script: &str,
    plugins: &[&str],
    input: &str,
    provider_timeout_ms: u64,
    max_steps: Option<usize>,
) -> (std::process::ExitStatus, serde_json::Value) {
    let mut cmd = Command::new(bin);
    cmd.args(["--root", root]);
    for a in shell_allow {
        cmd.args(["--shell-allow", a]);
    }
    cmd.args(["run", "--provider", provider]);
    cmd.args(["--mock-script", mock_script]);
    for p in plugins {
        cmd.args(["--plugin", p]);
    }
    if let Some(ms) = max_steps {
        cmd.args(["--max-steps", &ms.to_string()]);
    }
    cmd.args(["--provider-timeout-ms", &provider_timeout_ms.to_string()]);
    cmd.args(["--input", input]);
    let out = cmd.output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    (out.status, v)
}

#[test]
fn phase1_5_minimal_loop_read_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("README.md"), "Project: DeepAgents\nhello\n").unwrap();

    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "read_file", "arguments": { "file_path": "README.md", "limit": 20 }, "call_id": "c1" }
        ]},
        { "type": "final_from_last_tool_first_line", "prefix": "first: " }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        &[],
        "read readme",
        1000,
        None,
    );
    assert!(st.success());
    assert_eq!(
        v.get("final_text").and_then(|s| s.as_str()).unwrap(),
        "first: Project: DeepAgents"
    );
    assert_eq!(v.get("tool_calls").unwrap().as_array().unwrap().len(), 1);
    assert_eq!(v.get("tool_results").unwrap().as_array().unwrap().len(), 1);
    assert!(v.get("error").unwrap().is_null());
}

#[test]
fn phase1_5_provider_replacement_mock2() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("README.md"), "Project: DeepAgents\n").unwrap();

    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "read_file", "arguments": { "file_path": "README.md", "limit": 20 }, "call_id": "c1" }
        ]},
        { "type": "final_from_last_tool_first_line", "prefix": "" }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock2",
        script_path.to_string_lossy().as_ref(),
        &[],
        "read readme",
        1000,
        None,
    );
    assert!(st.success());
    assert_eq!(
        v.get("final_text").and_then(|s| s.as_str()).unwrap(),
        "Project: DeepAgents"
    );
    assert_eq!(
        v.get("tool_calls").unwrap().as_array().unwrap()[0]
            .get("call_id")
            .and_then(|c| c.as_str())
            .unwrap(),
        "call-1"
    );
}

#[test]
fn phase1_5_tool_call_parsing_rejects_non_object_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("README.md"), "Project: DeepAgents\n").unwrap();

    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "read_file", "arguments": "oops" }
        ]},
        { "type": "final_text", "text": "done" }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        &[],
        "bad tool call",
        1000,
        None,
    );
    assert!(st.success());
    assert_eq!(
        v.get("final_text").and_then(|s| s.as_str()).unwrap(),
        "done"
    );
    let err = v.get("tool_results").unwrap().as_array().unwrap()[0]
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap();
    assert!(err.contains("invalid_tool_call"));
}

#[test]
fn phase1_5_provider_timeout_is_classified() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("README.md"), "Project: DeepAgents\n").unwrap();

    let script = serde_json::json!({
      "steps": [
        { "type": "delay_ms", "ms": 200 }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        &[],
        "timeout",
        50,
        None,
    );
    assert!(!st.success());
    assert_eq!(
        v.get("error")
            .unwrap()
            .get("code")
            .and_then(|c| c.as_str())
            .unwrap(),
        "provider_timeout"
    );
}

#[test]
fn phase1_5_tool_error_is_recorded_and_run_can_still_finalize() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "read_file", "arguments": { "file_path": "NOPE.md", "limit": 1 } }
        ]},
        { "type": "final_text", "text": "ok" }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        &[],
        "missing file",
        1000,
        None,
    );
    assert!(st.success());
    assert_eq!(v.get("final_text").and_then(|s| s.as_str()).unwrap(), "ok");
    let err = v.get("tool_results").unwrap().as_array().unwrap()[0]
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap();
    assert!(err.contains("file_not_found"));
}

#[test]
fn phase1_5_declarative_skill_plugin_can_trigger_tool() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("README.md"), "Project: DeepAgents\n").unwrap();

    let manifest = serde_json::json!({
      "skills": [
        {
          "name": "read_readme",
          "description": "Read README",
          "tool_calls": [
            { "tool_name": "read_file", "arguments": { "file_path": "README.md", "limit": 20 } }
          ]
        }
      ]
    });
    let plugin_path = root.join("skills.json");
    std::fs::write(&plugin_path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();

    let script = serde_json::json!({
      "steps": [
        { "type": "skill_call", "name": "read_readme", "input": {} },
        { "type": "final_from_last_tool_first_line", "prefix": "" }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        &[plugin_path.to_string_lossy().as_ref()],
        "use skill",
        1000,
        None,
    );
    assert!(st.success());
    assert_eq!(
        v.get("final_text").and_then(|s| s.as_str()).unwrap(),
        "Project: DeepAgents"
    );
}

#[test]
fn phase1_5_multi_round_tool_calls() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("README.md"), "Project: DeepAgents\nhello\n").unwrap();
    let mut large = String::new();
    for i in 1..=250 {
        large.push_str(&format!("line{i}\n"));
    }
    std::fs::write(root.join("large.txt"), large).unwrap();

    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "read_file", "arguments": { "file_path": "README.md", "limit": 20 } }
        ]},
        { "type": "tool_calls", "calls": [
          { "tool_name": "read_file", "arguments": { "file_path": "large.txt", "offset": 200, "limit": 1 } }
        ]},
        { "type": "final_from_last_tool_first_line", "prefix": "line: " }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        &[],
        "multi",
        1000,
        None,
    );
    assert!(st.success());
    assert_eq!(v.get("tool_calls").unwrap().as_array().unwrap().len(), 2);
    assert_eq!(v.get("tool_results").unwrap().as_array().unwrap().len(), 2);
    assert_eq!(
        v.get("final_text").and_then(|s| s.as_str()).unwrap(),
        "line: line201"
    );
    assert!(v.get("tool_results").unwrap().as_array().unwrap()[0]
        .get("output")
        .and_then(|o| o.get("content"))
        .and_then(|c| c.as_str())
        .unwrap()
        .contains("Project: DeepAgents"));
}

#[test]
fn phase1_5_no_tool_direct_answer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = serde_json::json!({
      "steps": [
        { "type": "final_text", "text": "hello" }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        &[],
        "no tool",
        1000,
        None,
    );
    assert!(st.success());
    assert_eq!(
        v.get("final_text").and_then(|s| s.as_str()).unwrap(),
        "hello"
    );
    assert_eq!(v.get("tool_calls").unwrap().as_array().unwrap().len(), 0);
    assert_eq!(v.get("tool_results").unwrap().as_array().unwrap().len(), 0);
    assert_eq!(
        v.get("trace")
            .unwrap()
            .get("reason")
            .and_then(|r| r.as_str())
            .unwrap(),
        "final_text"
    );
}

#[test]
fn phase1_5_unknown_tool_is_recorded_and_run_can_still_finalize() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "no_such_tool", "arguments": {} }
        ]},
        { "type": "final_text", "text": "ok" }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        &[],
        "unknown tool",
        1000,
        None,
    );
    assert!(st.success());
    assert_eq!(v.get("final_text").and_then(|s| s.as_str()).unwrap(), "ok");
    let err = v.get("tool_results").unwrap().as_array().unwrap()[0]
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap();
    assert!(err.contains("unknown tool: no_such_tool"));
}

#[test]
fn phase1_5_schema_validation_missing_and_wrong_types_are_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("README.md"), "Project: DeepAgents\n").unwrap();

    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "read_file", "arguments": { "limit": 1 } }
        ]},
        { "type": "tool_calls", "calls": [
          { "tool_name": "read_file", "arguments": { "file_path": "README.md", "limit": "oops" } }
        ]},
        { "type": "final_text", "text": "done" }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        &[],
        "schema",
        1000,
        None,
    );
    assert!(st.success());
    assert_eq!(
        v.get("final_text").and_then(|s| s.as_str()).unwrap(),
        "done"
    );
    let r = v.get("tool_results").unwrap().as_array().unwrap();
    assert!(r[0]
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap()
        .contains("missing field"));
    assert!(r[1]
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap()
        .contains("invalid type"));
}

#[test]
fn phase1_5_path_escape_and_symlink_escape_are_denied() {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let secret = outer.path().join("secret.txt");
    std::fs::write(&secret, "top-secret\n").unwrap();
    let link = root.join("link.txt");
    std::os::unix::fs::symlink(&secret, &link).unwrap();

    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "read_file", "arguments": { "file_path": "../secret.txt", "limit": 1 } }
        ]},
        { "type": "tool_calls", "calls": [
          { "tool_name": "read_file", "arguments": { "file_path": link.to_string_lossy(), "limit": 1 } }
        ]},
        { "type": "final_text", "text": "ok" }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        &[],
        "escape",
        1000,
        None,
    );
    assert!(st.success());
    let r = v.get("tool_results").unwrap().as_array().unwrap();
    assert!(r[0]
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap()
        .contains("permission_denied"));
    assert!(r[1]
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap()
        .contains("permission_denied"));
    assert_eq!(v.get("final_text").and_then(|s| s.as_str()).unwrap(), "ok");
}

#[test]
fn phase1_5_is_directory_error_is_recorded_and_run_can_still_finalize() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("d")).unwrap();

    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "read_file", "arguments": { "file_path": "d", "limit": 1 } }
        ]},
        { "type": "final_text", "text": "ok" }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        &[],
        "dir",
        1000,
        None,
    );
    assert!(st.success());
    assert_eq!(v.get("final_text").and_then(|s| s.as_str()).unwrap(), "ok");
    let err = v.get("tool_results").unwrap().as_array().unwrap()[0]
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap();
    assert!(err.contains("is_directory"));
}

#[test]
fn phase1_5_execute_allow_list_rejects_disallowed_commands() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "execute", "arguments": { "command": "touch banned.txt", "timeout": 5 } }
        ]},
        { "type": "final_text", "text": "ok" }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &["echo"],
        "mock",
        script_path.to_string_lossy().as_ref(),
        &[],
        "execute",
        1000,
        None,
    );
    assert!(st.success());
    assert_eq!(v.get("final_text").and_then(|s| s.as_str()).unwrap(), "ok");
    let err = v.get("tool_results").unwrap().as_array().unwrap()[0]
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap();
    assert!(err.contains("command_not_allowed"));
    assert!(!root.join("banned.txt").exists());
}

#[test]
fn phase1_5_max_steps_exceeded_is_classified() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("README.md"), "Project: DeepAgents\n").unwrap();

    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "read_file", "arguments": { "file_path": "README.md", "limit": 1 } }
        ]}
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        &[],
        "max steps",
        1000,
        Some(1),
    );
    assert!(!st.success());
    assert_eq!(
        v.get("error")
            .unwrap()
            .get("code")
            .and_then(|c| c.as_str())
            .unwrap(),
        "max_steps_exceeded"
    );
    assert_eq!(
        v.get("trace")
            .unwrap()
            .get("reason")
            .and_then(|r| r.as_str())
            .unwrap(),
        "max_steps_exceeded"
    );
}

#[test]
fn phase1_5_skill_not_found_is_classified() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = serde_json::json!({
      "steps": [
        { "type": "skill_call", "name": "nope", "input": {} }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        &[],
        "skill",
        1000,
        None,
    );
    assert!(!st.success());
    assert_eq!(
        v.get("error")
            .unwrap()
            .get("code")
            .and_then(|c| c.as_str())
            .unwrap(),
        "skill_not_found"
    );
    assert_eq!(
        v.get("trace")
            .unwrap()
            .get("reason")
            .and_then(|r| r.as_str())
            .unwrap(),
        "skill_error"
    );
}

#[test]
fn phase1_5_declarative_skill_merge_args_overrides_base() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("README.md"), "Project: DeepAgents\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "needle\n").unwrap();

    let manifest = serde_json::json!({
      "skills": [
        {
          "name": "read_any",
          "description": "Read any file",
          "tool_calls": [
            { "tool_name": "read_file", "arguments": { "file_path": "README.md", "limit": 20 } }
          ]
        }
      ]
    });
    let plugin_path = root.join("skills.json");
    std::fs::write(&plugin_path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();

    let script = serde_json::json!({
      "steps": [
        { "type": "skill_call", "name": "read_any", "input": { "file_path": "src/lib.rs", "limit": 1 } },
        { "type": "final_from_last_tool_first_line", "prefix": "" }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        &[plugin_path.to_string_lossy().as_ref()],
        "merge args",
        1000,
        None,
    );
    assert!(st.success());
    assert_eq!(
        v.get("final_text").and_then(|s| s.as_str()).unwrap(),
        "needle"
    );
}

#[test]
fn phase1_5_state_write_edit_delete_is_observable_in_run_output() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "write_file", "arguments": { "file_path": "a.txt", "content": "hello\nworld\n" } }
        ]},
        { "type": "tool_calls", "calls": [
          { "tool_name": "edit_file", "arguments": { "file_path": "a.txt", "old_string": "world", "new_string": "rust" } }
        ]},
        { "type": "tool_calls", "calls": [
          { "tool_name": "delete_file", "arguments": { "file_path": "a.txt" } }
        ]},
        { "type": "final_text", "text": "ok" }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        &[],
        "mock",
        script_path.to_string_lossy().as_ref(),
        &[],
        "state",
        1000,
        None,
    );
    assert!(st.success());
    assert_eq!(v.get("final_text").and_then(|s| s.as_str()).unwrap(), "ok");

    let results = v.get("tool_results").unwrap().as_array().unwrap();
    assert_eq!(results.len(), 3);

    let write_path = results[0]
        .get("output")
        .and_then(|o| o.get("path"))
        .and_then(|p| p.as_str())
        .unwrap()
        .to_string();
    assert!(results[0].get("error").is_none());

    assert_eq!(
        results[1]
            .get("output")
            .and_then(|o| o.get("occurrences"))
            .and_then(|n| n.as_u64())
            .unwrap(),
        1
    );
    let edit_path = results[1]
        .get("output")
        .and_then(|o| o.get("path"))
        .and_then(|p| p.as_str())
        .unwrap();
    assert_eq!(edit_path, write_path);

    let delete_path = results[2]
        .get("output")
        .and_then(|o| o.get("path"))
        .and_then(|p| p.as_str())
        .unwrap();
    assert_eq!(delete_path, write_path);

    let rec = v
        .get("state")
        .and_then(|s| s.get("filesystem"))
        .and_then(|fs| fs.get("files"))
        .and_then(|m| m.get(&write_path))
        .unwrap();
    assert!(rec.get("deleted").and_then(|d| d.as_bool()).unwrap());

    assert!(!root.join("a.txt").exists());
}
