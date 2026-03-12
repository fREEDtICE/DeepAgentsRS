use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn e2e_channels_doctor_reports_cli_channel() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("channels.toml");
    std::fs::write(&config_path, "[channels_config]\ncli = true\n").unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let out = Command::new(bin)
        .args([
            "channels",
            "doctor",
            "--config",
            config_path.to_string_lossy().as_ref(),
        ])
        .output()
        .unwrap();

    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let channels = v.get("channels").and_then(|v| v.as_array()).unwrap();
    assert!(channels.iter().any(|entry| {
        entry.get("channel").and_then(|v| v.as_str()) == Some("cli")
            && entry.get("status").and_then(|v| v.as_str()) == Some("healthy")
    }));
}

#[test]
fn e2e_channels_serve_processes_cli_messages() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let script_path = root.join("mock.json");
    std::fs::write(
        &script_path,
        serde_json::json!({
            "steps": [
                { "type": "final_text", "text": "hello from channel" }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let config_path = root.join("channels.toml");
    std::fs::write(
        &config_path,
        format!(
            "root = {:?}\n[provider]\nid = \"mock\"\nmock_script = {:?}\n\n[channels_config]\ncli = true\n",
            root.to_string_lossy().to_string(),
            script_path.to_string_lossy().to_string(),
        ),
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let mut child = Command::new(bin)
        .args([
            "channels",
            "serve",
            "--config",
            config_path.to_string_lossy().as_ref(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    {
        let stdin = child.stdin.as_mut().unwrap();
        stdin.write_all(b"ping\n").unwrap();
        stdin.flush().unwrap();
        std::thread::sleep(Duration::from_secs(1));
        stdin.write_all(b"/exit\n").unwrap();
        stdin.flush().unwrap();
    }

    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stdout.contains("hello from channel"),
        "stdout={stdout:?}\nstderr={stderr:?}"
    );
}
