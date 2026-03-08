use std::process::Command;

#[test]
fn e2e_cli_ls_and_read() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let readme = root.join("README.md");
    std::fs::write(&readme, "hello\nworld\n").unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");

    let out = Command::new(bin)
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "tool",
            "ls",
            "--input",
            &format!(r#"{{"path":"{}"}}"#, root.to_string_lossy()),
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v.as_array().unwrap().iter().any(|e| {
        e.get("path")
            .and_then(|p| p.as_str())
            .is_some_and(|p| p.ends_with("README.md"))
    }));

    let out = Command::new(bin)
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "tool",
            "read_file",
            "--input",
            &format!(
                r#"{{"file_path":"{}","limit":1}}"#,
                readme.to_string_lossy()
            ),
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let s = v.get("content").and_then(|c| c.as_str()).unwrap();
    assert!(s.contains("1→hello"));
}

#[test]
fn e2e_cli_write_and_edit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let p = root.join("a.txt");

    let out = Command::new(bin)
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "tool",
            "write_file",
            "--input",
            &format!(
                r#"{{"file_path":"{}","content":"hello world\n"}}"#,
                p.to_string_lossy()
            ),
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v.get("error").map(|e| e.is_null()).unwrap_or(true));

    let out = Command::new(bin)
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "tool",
            "edit_file",
            "--input",
            &format!(
                r#"{{"file_path":"{}","old_string":"world","new_string":"rust"}}"#,
                p.to_string_lossy()
            ),
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v.get("occurrences").unwrap().as_u64(), Some(1));
}
