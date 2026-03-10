use std::process::Command;

#[test]
fn e2e_run_writes_events_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = serde_json::json!({
      "steps": [
        { "type": "tool_calls", "calls": [
          { "tool_name": "write_file", "arguments": { "file_path": "a.txt", "content": "hello\n" }, "call_id": "w1" }
        ]},
        { "type": "final_text", "text": "done" }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();
    let events_path = root.join("events.jsonl");

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let out = Command::new(bin)
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "run",
            "--provider",
            "mock",
            "--mock-script",
            script_path.to_string_lossy().as_ref(),
            "--events-jsonl",
            events_path.to_string_lossy().as_ref(),
            "--input",
            "write file",
        ])
        .output()
        .unwrap();

    assert!(out.status.success());

    let run_output: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        run_output.get("final_text").and_then(|v| v.as_str()),
        Some("done")
    );

    let events_text = std::fs::read_to_string(&events_path).unwrap();
    let events: Vec<serde_json::Value> = events_text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    assert!(events
        .iter()
        .any(|event| { event.get("type").and_then(|v| v.as_str()) == Some("run_started") }));
    assert!(events.iter().any(|event| {
        event.get("type").and_then(|v| v.as_str()) == Some("tool_call_started")
            && event.get("tool_call_id").and_then(|v| v.as_str()) == Some("w1")
    }));
    assert!(matches!(
        events.last(),
        Some(event)
            if event.get("type").and_then(|v| v.as_str()) == Some("run_finished")
                && event.get("status").and_then(|v| v.as_str()) == Some("completed")
    ));
}

#[test]
fn e2e_run_stream_events_echoes_jsonl_to_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = serde_json::json!({
      "steps": [
        { "type": "final_text", "text": "done" }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let out = Command::new(bin)
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "run",
            "--provider",
            "mock",
            "--mock-script",
            script_path.to_string_lossy().as_ref(),
            "--stream-events",
            "--input",
            "go",
        ])
        .output()
        .unwrap();

    assert!(out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.lines().any(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
            .as_deref()
            == Some("run_started")
    }));
    assert!(stderr.lines().any(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
            .as_deref()
            == Some("run_finished")
    }));
}
