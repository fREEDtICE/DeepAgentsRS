use std::process::Command;

fn run_tool_stateful(
    bin: &str,
    root: &str,
    state_file: &str,
    tool: &str,
    input: &str,
    shell_allow: &[&str],
) -> (std::process::ExitStatus, serde_json::Value) {
    let mut cmd = Command::new(bin);
    cmd.args(["--root", root]);
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

#[test]
fn phase1_state_write_edit_delete_and_observability() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let state_file = root.join("state.json");
    let state_file_s = state_file.to_string_lossy().to_string();
    let bin = env!("CARGO_BIN_EXE_deepagents");

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "write_file",
        &format!(
            r#"{{"file_path":"{}","content":"hello\nworld\n"}}"#,
            root.join("a.txt").to_string_lossy()
        ),
        &[],
    );
    assert!(st.success());
    let path = v.get("output").and_then(|o| o.get("path")).and_then(|p| p.as_str()).unwrap();
    let rec = v
        .get("state")
        .and_then(|s| s.get("filesystem"))
        .and_then(|fs| fs.get("files"))
        .and_then(|m| m.get(path))
        .unwrap();
    assert_eq!(
        rec.get("content").unwrap().as_array().unwrap(),
        &vec![serde_json::Value::String("hello".into()), serde_json::Value::String("world".into())]
    );

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "edit_file",
        &format!(
            r#"{{"file_path":"{}","old_string":"world","new_string":"rust"}}"#,
            path
        ),
        &[],
    );
    assert!(st.success());
    let rec = v
        .get("state")
        .and_then(|s| s.get("filesystem"))
        .and_then(|fs| fs.get("files"))
        .and_then(|m| m.get(path))
        .unwrap();
    assert_eq!(
        rec.get("content").unwrap().as_array().unwrap(),
        &vec![serde_json::Value::String("hello".into()), serde_json::Value::String("rust".into())]
    );

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "delete_file",
        &format!(r#"{{"file_path":"{}"}}"#, path),
        &[],
    );
    assert!(st.success());
    let rec = v
        .get("state")
        .and_then(|s| s.get("filesystem"))
        .and_then(|fs| fs.get("files"))
        .and_then(|m| m.get(path))
        .unwrap();
    assert!(rec.get("deleted").unwrap().as_bool().unwrap());
}

#[test]
fn phase1_schema_validation_missing_wrong_unknown_fields() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let state_file = root.join("state.json");
    let state_file_s = state_file.to_string_lossy().to_string();
    let bin = env!("CARGO_BIN_EXE_deepagents");

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "write_file",
        r#"{"file_path":"a.txt"}"#,
        &[],
    );
    assert!(!st.success());
    assert!(v.get("error").and_then(|e| e.as_str()).unwrap().contains("missing field"));

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "read_file",
        r#"{"file_path":"a.txt","limit":"oops"}"#,
        &[],
    );
    assert!(!st.success());
    assert!(v.get("error").and_then(|e| e.as_str()).unwrap().contains("invalid type"));

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "ls",
        r#"{"path":".","extra":1}"#,
        &[],
    );
    assert!(!st.success());
    assert!(v.get("error").and_then(|e| e.as_str()).unwrap().contains("unknown field"));
}

#[test]
fn phase1_defaults_and_grep_modes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("README.md"), "needle\nx\nneedle\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "needle\n").unwrap();

    let state_file_s = root.join("state.json").to_string_lossy().to_string();
    let bin = env!("CARGO_BIN_EXE_deepagents");

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "read_file",
        &format!(r#"{{"file_path":"{}"}}"#, root.join("README.md").to_string_lossy()),
        &[],
    );
    assert!(st.success());
    assert!(v.get("output").unwrap().get("content").unwrap().as_str().unwrap().contains("1→needle"));

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "grep",
        &format!(
            r#"{{"pattern":"needle","path":"{}","glob":"**/*.*","output_mode":"content","head_limit":100}}"#,
            root.to_string_lossy()
        ),
        &[],
    );
    assert!(st.success());
    let content = v.get("output").unwrap().as_array().unwrap();
    assert!(content.len() >= 2);
    assert!(content.iter().all(|m| m.get("line").unwrap().as_u64().unwrap() >= 1));

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "grep",
        &format!(r#"{{"pattern":"needle","path":"{}","glob":"**/*.*"}}"#, root.to_string_lossy()),
        &[],
    );
    assert!(st.success());
    assert!(v.get("output").unwrap().as_array().unwrap().iter().all(|p| p.as_str().is_some()));

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "grep",
        &format!(
            r#"{{"pattern":"needle","path":"{}","glob":"**/*.*","output_mode":"count"}}"#,
            root.to_string_lossy()
        ),
        &[],
    );
    assert!(st.success());
    let counts = v.get("output").unwrap().as_array().unwrap();
    assert!(counts.iter().all(|e| e.is_array() && e.as_array().unwrap().len() == 2));
}

#[test]
fn phase1_glob_paths_reusable_for_read() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src/nested")).unwrap();
    std::fs::write(root.join("src/nested/deep.txt"), "hello\n").unwrap();

    let state_file_s = root.join("state.json").to_string_lossy().to_string();
    let bin = env!("CARGO_BIN_EXE_deepagents");

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "glob",
        r#"{"pattern":"**/*.txt"}"#,
        &[],
    );
    assert!(st.success());
    let p = v.get("output").unwrap().as_array().unwrap()[0].as_str().unwrap().to_string();

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "read_file",
        &format!(r#"{{"file_path":"{}","limit":1}}"#, p),
        &[],
    );
    assert!(st.success());
    assert!(v.get("output").unwrap().get("content").unwrap().as_str().unwrap().contains("1→hello"));
}

#[test]
fn phase1_truncation_and_pagination_for_read_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("large.txt"), "a\nb\nc\nd\ne\n").unwrap();
    let state_file_s = root.join("state.json").to_string_lossy().to_string();
    let bin = env!("CARGO_BIN_EXE_deepagents");

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "read_file",
        &format!(r#"{{"file_path":"{}","limit":2,"offset":0}}"#, root.join("large.txt").to_string_lossy()),
        &[],
    );
    assert!(st.success());
    assert!(v.get("output").unwrap().get("truncated").unwrap().as_bool().unwrap());
    let next = v.get("output").unwrap().get("next_offset").unwrap().as_u64().unwrap();
    assert_eq!(next, 2);

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "read_file",
        &format!(
            r#"{{"file_path":"{}","limit":2,"offset":{}}}"#,
            root.join("large.txt").to_string_lossy(),
            next
        ),
        &[],
    );
    assert!(st.success());
    let s = v.get("output").unwrap().get("content").unwrap().as_str().unwrap();
    assert!(s.contains("3→c"));
}

#[test]
fn phase1_security_outside_root_and_symlink_escape() {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let secret = outer.path().join("secret.txt");
    std::fs::write(&secret, "top-secret\n").unwrap();
    let link = root.join("link.txt");
    std::os::unix::fs::symlink(&secret, &link).unwrap();

    let state_file_s = root.join("state.json").to_string_lossy().to_string();
    let bin = env!("CARGO_BIN_EXE_deepagents");

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "read_file",
        r#"{"file_path":"../secret.txt","limit":1}"#,
        &[],
    );
    assert!(!st.success());
    assert!(v.get("error").and_then(|e| e.as_str()).unwrap().contains("permission_denied"));

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "read_file",
        &format!(r#"{{"file_path":"{}","limit":1}}"#, link.to_string_lossy()),
        &[],
    );
    assert!(!st.success());
    assert!(v.get("error").and_then(|e| e.as_str()).unwrap().contains("permission_denied"));
}

#[test]
fn phase1_error_codes_and_execute_contracts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("d")).unwrap();
    std::fs::write(root.join("d/x.txt"), "x\n").unwrap();

    let state_file_s = root.join("state.json").to_string_lossy().to_string();
    let bin = env!("CARGO_BIN_EXE_deepagents");

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "read_file",
        &format!(r#"{{"file_path":"{}","limit":1}}"#, root.join("nope.txt").to_string_lossy()),
        &[],
    );
    assert!(!st.success());
    assert!(v.get("error").and_then(|e| e.as_str()).unwrap().contains("file_not_found"));

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "read_file",
        &format!(r#"{{"file_path":"{}","limit":1}}"#, root.join("d").to_string_lossy()),
        &[],
    );
    assert!(!st.success());
    assert!(v.get("error").and_then(|e| e.as_str()).unwrap().contains("is_directory"));

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "write_file",
        &format!(
            r#"{{"file_path":"{}","content":"x"}}"#,
            root.join("missing_dir/a.txt").to_string_lossy()
        ),
        &[],
    );
    assert!(st.success());
    assert_eq!(
        v.get("output").unwrap().get("error").and_then(|e| e.as_str()),
        Some("parent_not_found")
    );

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "edit_file",
        &format!(
            r#"{{"file_path":"{}","old_string":"nope","new_string":"y"}}"#,
            root.join("d/x.txt").to_string_lossy()
        ),
        &[],
    );
    assert!(st.success());
    assert_eq!(v.get("output").unwrap().get("error").and_then(|e| e.as_str()), Some("no_match"));

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "execute",
        r#"{"command":"sleep 2","timeout":1}"#,
        &["sleep"],
    );
    assert!(!st.success());
    assert!(v.get("error").and_then(|e| e.as_str()).unwrap().contains("timeout"));

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "execute",
        r#"{"command":"ls","timeout":5}"#,
        &["echo"],
    );
    assert!(!st.success());
    assert!(v.get("error").and_then(|e| e.as_str()).unwrap().contains("command_not_allowed"));

    let (st, v) = run_tool_stateful(
        bin,
        root.to_string_lossy().as_ref(),
        &state_file_s,
        "execute",
        r#"{"command":"yes 012345678901234567890123456789 | head -n 12000","timeout":5}"#,
        &["yes", "head"],
    );
    assert!(st.success());
    let tr = v.get("output").unwrap().get("truncated").and_then(|t| t.as_bool()).unwrap_or(false);
    assert!(tr);
}
