use std::process::Command;

fn run_tool_stateful(
    bin: &str,
    root: &str,
    state_file: &str,
    audit_json: &str,
    tool: &str,
    input: &str,
    shell_allow: &[&str],
) -> (std::process::ExitStatus, serde_json::Value) {
    let mut cmd = Command::new(bin);
    cmd.args(["--root", root]);
    cmd.args(["--execution-mode", "non-interactive"]);
    cmd.args(["--audit-json", audit_json]);
    for a in shell_allow {
        cmd.args(["--shell-allow", a]);
    }
    let out = cmd
        .args(["tool", tool, "--input", input, "--state-file", state_file])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    (out.status, v)
}

fn read_audit_lines(path: &std::path::Path) -> Vec<serde_json::Value> {
    let s = std::fs::read_to_string(path).unwrap();
    s.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .collect()
}

fn run_runtime(
    bin: &str,
    root: &str,
    audit_json: &str,
    provider: &str,
    mock_script: &str,
    shell_allow: &[&str],
    input: &str,
) -> (std::process::ExitStatus, serde_json::Value) {
    let mut cmd = Command::new(bin);
    cmd.args(["--root", root]);
    cmd.args(["--execution-mode", "non-interactive"]);
    cmd.args(["--audit-json", audit_json]);
    for a in shell_allow {
        cmd.args(["--shell-allow", a]);
    }
    cmd.args(["run", "--provider", provider]);
    cmd.args(["--mock-script", mock_script]);
    cmd.args(["--input", input]);
    let out = cmd.output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    (out.status, v)
}

#[test]
fn phase2_non_interactive_deny_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let state_file = root.join("state.json");
    let audit_file = root.join("audit.jsonl");
    let bin = env!("CARGO_BIN_EXE_deepagents");

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        state_file.to_string_lossy().as_ref(),
        audit_file.to_string_lossy().as_ref(),
        "execute",
        r#"{"command":"echo hi","timeout":5}"#,
        &[],
    );
    assert!(!st.success());
    assert!(v.get("error").and_then(|e| e.as_str()).unwrap().contains("command_not_allowed"));

    let lines = read_audit_lines(&audit_file);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].get("decision").and_then(|v| v.as_str()), Some("require_approval"));
    assert_eq!(
        lines[0].get("decision_code").and_then(|v| v.as_str()),
        Some("approval_required")
    );
}

#[test]
fn phase2_allow_list_allows_and_audits() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let state_file = root.join("state.json");
    let audit_file = root.join("audit.jsonl");
    let bin = env!("CARGO_BIN_EXE_deepagents");

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        state_file.to_string_lossy().as_ref(),
        audit_file.to_string_lossy().as_ref(),
        "execute",
        r#"{"command":"echo hi","timeout":5}"#,
        &["echo"],
    );
    assert!(st.success());
    assert_eq!(v.get("output").unwrap().get("exit_code").and_then(|v| v.as_i64()), Some(0));

    let lines = read_audit_lines(&audit_file);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].get("decision").and_then(|v| v.as_str()), Some("allow"));
    assert_eq!(lines[0].get("decision_code").and_then(|v| v.as_str()), Some("allow"));
    assert!(lines[0].get("duration_ms").and_then(|v| v.as_u64()).is_some());
}

#[test]
fn phase2_dangerous_pattern_denied_even_if_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let state_file = root.join("state.json");
    let audit_file = root.join("audit.jsonl");
    let bin = env!("CARGO_BIN_EXE_deepagents");

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        state_file.to_string_lossy().as_ref(),
        audit_file.to_string_lossy().as_ref(),
        "execute",
        r#"{"command":"echo $HOME","timeout":5}"#,
        &["echo"],
    );
    assert!(!st.success());
    assert!(v.get("error").and_then(|e| e.as_str()).unwrap().contains("dangerous_pattern"));

    let lines = read_audit_lines(&audit_file);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].get("decision").and_then(|v| v.as_str()), Some("deny"));
    assert_eq!(lines[0].get("decision_code").and_then(|v| v.as_str()), Some("dangerous_pattern"));
}

#[test]
fn phase2_audit_redacts_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let state_file = root.join("state.json");
    let audit_file = root.join("audit.jsonl");
    let bin = env!("CARGO_BIN_EXE_deepagents");

    let secret = "abc123";
    let (st, _v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        state_file.to_string_lossy().as_ref(),
        audit_file.to_string_lossy().as_ref(),
        "execute",
        &format!(r#"{{"command":"echo --token {}","timeout":5}}"#, secret),
        &["echo"],
    );
    assert!(st.success());

    let raw = std::fs::read_to_string(&audit_file).unwrap();
    assert!(!raw.contains(secret));
    assert!(raw.contains("--token ***"));
}

#[test]
fn phase2_run_path_does_not_bypass_policy_allow() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let audit_file = root.join("audit.jsonl");
    let bin = env!("CARGO_BIN_EXE_deepagents");

    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "execute", "arguments": { "command": "echo hi", "timeout": 5 }, "call_id": "c1" }
        ]},
        { "type": "final_text", "text": "ok" }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        audit_file.to_string_lossy().as_ref(),
        "mock",
        script_path.to_string_lossy().as_ref(),
        &["echo"],
        "do it",
    );
    assert!(st.success());
    assert_eq!(v.get("final_text").and_then(|s| s.as_str()), Some("ok"));
    assert_eq!(
        v.get("tool_results").unwrap().as_array().unwrap()[0]
            .get("output").and_then(|o| o.get("exit_code")).and_then(|v| v.as_i64()),
        Some(0)
    );

    let lines = read_audit_lines(&audit_file);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].get("decision").and_then(|v| v.as_str()), Some("allow"));
}

#[test]
fn phase2_run_path_does_not_bypass_policy_deny() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let audit_file = root.join("audit.jsonl");
    let bin = env!("CARGO_BIN_EXE_deepagents");

    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "execute", "arguments": { "command": "echo hi", "timeout": 5 }, "call_id": "c1" }
        ]},
        { "type": "final_text", "text": "ok" }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let (st, v) = run_runtime(
        bin,
        root.to_string_lossy().as_ref(),
        audit_file.to_string_lossy().as_ref(),
        "mock",
        script_path.to_string_lossy().as_ref(),
        &[],
        "do it",
    );
    assert!(st.success());
    let err = v.get("tool_results").unwrap().as_array().unwrap()[0]
        .get("error").and_then(|e| e.as_str()).unwrap();
    assert!(err.contains("approval_required"));

    let lines = read_audit_lines(&audit_file);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].get("decision").and_then(|v| v.as_str()), Some("require_approval"));
}
