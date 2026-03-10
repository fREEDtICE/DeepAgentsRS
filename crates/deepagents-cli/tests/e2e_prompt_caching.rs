use std::process::Command;

#[test]
fn e2e_prompt_cache_events_are_emitted_and_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let script = serde_json::json!({
      "steps": [
        { "type": "final_text", "text": "OK" }
      ]
    });
    let script_path = root.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();

    let bin = env!("CARGO_BIN_EXE_deepagents");
    let out = Command::new(bin)
        .args(["--root", root.to_string_lossy().as_ref()])
        .args(["run", "--provider", "mock"])
        .args(["--mock-script", script_path.to_string_lossy().as_ref()])
        .args(["--prompt-cache", "memory"])
        .args(["--input", "SECRET_SHOULD_NOT_LEAK"])
        .output()
        .unwrap();
    assert!(out.status.success());

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let trace = v.get("trace").and_then(|t| t.as_object()).unwrap();
    let events = trace
        .get("provider_cache_events")
        .and_then(|e| e.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(!events.is_empty());
    assert!(events
        .iter()
        .any(|e| e.get("cache_level").and_then(|v| v.as_str()) == Some("L1")));

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(!stdout.contains("SECRET_SHOULD_NOT_LEAK"));
}
