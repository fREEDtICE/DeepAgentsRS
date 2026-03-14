use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::routing::post;
use axum::{Json, Router};
use serde_json::Value;

fn run_json_command(args: &[String]) -> (Output, Value) {
    let bin = env!("CARGO_BIN_EXE_deepagents");
    let out = Command::new(bin).args(args).output().unwrap();
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    (out, value)
}

fn run_command(args: &[String]) -> Output {
    let bin = env!("CARGO_BIN_EXE_deepagents");
    Command::new(bin).args(args).output().unwrap()
}

fn run_command_with_config_home(config_home: &Path, args: &[String]) -> Output {
    let bin = env!("CARGO_BIN_EXE_deepagents");
    Command::new(bin)
        .env("DEEPAGENTS_CONFIG_HOME", config_home)
        .args(args)
        .output()
        .unwrap()
}

fn run_json_command_with_config_home(config_home: &Path, args: &[String]) -> (Output, Value) {
    let out = run_command_with_config_home(config_home, args);
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    (out, value)
}

fn read_json_file(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn set_workspace_config(root: &Path, config_home: &Path, key: &str, value: &str) {
    let bin = env!("CARGO_BIN_EXE_deepagents");
    let out = Command::new(bin)
        .env("DEEPAGENTS_CONFIG_HOME", config_home)
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "config",
            "set",
            key,
            value,
            "--scope",
            "workspace",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "key={key} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn message_text(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn memory_message(requests: &Arc<Mutex<Vec<Value>>>) -> String {
    let guard = requests.lock().unwrap();
    let request = guard
        .first()
        .expect("expected at least one provider request");
    let messages = request["messages"]
        .as_array()
        .expect("expected provider messages array");
    messages
        .iter()
        .find(|message| {
            message.get("role").and_then(Value::as_str) == Some("system")
                && message_text(message).contains("DEEPAGENTS_MEMORY_INJECTED_V")
        })
        .map(message_text)
        .expect("expected injected memory system message")
}

fn write_mock_script(path: &std::path::Path) -> std::path::PathBuf {
    let script = serde_json::json!({
        "steps": [
            { "type": "final_text", "text": "ok" }
        ]
    });
    let script_path = path.join("script.json");
    std::fs::write(&script_path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();
    script_path
}

async fn spawn_recording_server() -> (String, Arc<Mutex<Vec<Value>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let app = Router::new().route(
        "/chat/completions",
        post(move |Json(body): Json<Value>| {
            let captured = captured.clone();
            async move {
                captured.lock().unwrap().push(body);
                Json(serde_json::json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "done"
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 5,
                        "completion_tokens": 1,
                        "total_tokens": 6
                    }
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    (format!("http://{}", addr), requests)
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_memory_injection_includes_prompt_block_and_skip_diagnostics() {
    let (base_url, requests) = spawn_recording_server().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".deepagents")).unwrap();
    std::fs::write(
        root.join(".deepagents").join("AGENTS.md"),
        "# Memory\nfavorite = rust\n",
    )
    .unwrap();

    let args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "run".to_string(),
        "--provider".to_string(),
        "openai-compatible".to_string(),
        "--model".to_string(),
        "gpt-4o-mini".to_string(),
        "--base-url".to_string(),
        base_url,
        "--input".to_string(),
        "hello".to_string(),
    ];
    let (out, value) = run_json_command(&args);

    assert!(
        out.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(value["final_text"], "done");
    assert_eq!(
        value["state"]["extra"]["memory_diagnostics"]["loaded_sources"],
        1
    );
    assert_eq!(
        value["state"]["extra"]["memory_diagnostics"]["skipped_not_found"],
        1
    );
    assert_eq!(
        value["state"]["extra"]["memory_diagnostics"]["truncated"],
        false
    );

    let memory = memory_message(&requests);
    assert!(memory.contains("DEEPAGENTS_MEMORY_INJECTED_V1"));
    assert!(memory.contains("<agent_memory>"));
    assert!(memory.contains(".deepagents/AGENTS.md"));
    assert!(memory.contains("favorite = rust"));
    assert!(memory.contains("<memory_diagnostics>"));
    assert!(memory.contains("loaded_sources=1; skipped_not_found=1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_memory_host_path_opt_in_loads_absolute_source() {
    let (base_url, requests) = spawn_recording_server().await;
    let root = tempfile::tempdir().unwrap();
    let host = tempfile::tempdir().unwrap();
    let host_agents = host.path().join("AGENTS.md");
    std::fs::write(&host_agents, "# Host Memory\nshared = yes\n").unwrap();

    let args = vec![
        "--root".to_string(),
        root.path().to_string_lossy().into_owned(),
        "run".to_string(),
        "--provider".to_string(),
        "openai-compatible".to_string(),
        "--model".to_string(),
        "gpt-4o-mini".to_string(),
        "--base-url".to_string(),
        base_url,
        "--memory-source".to_string(),
        host_agents.to_string_lossy().into_owned(),
        "--memory-allow-host-paths".to_string(),
        "--input".to_string(),
        "hello".to_string(),
    ];
    let (out, value) = run_json_command(&args);

    assert!(
        out.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        value["state"]["extra"]["memory_diagnostics"]["loaded_sources"],
        1
    );
    assert_eq!(
        value["state"]["extra"]["memory_diagnostics"]["skipped_not_found"],
        0
    );

    let memory = memory_message(&requests);
    assert!(memory.contains("Host Memory"));
    assert!(memory.contains("shared = yes"));
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_memory_scoped_runtime_injects_ranked_memory_pack() {
    let (base_url, requests) = spawn_recording_server().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".deepagents")).unwrap();
    std::fs::write(
        root.join(".deepagents").join("AGENTS.md"),
        "# Legacy Memory\nlegacy = true\n",
    )
    .unwrap();
    let store = root.join(".deepagents").join("memory_store.json");

    let remember_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "remember".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "reply_style".to_string(),
        "--value".to_string(),
        "Reply in concise Chinese.".to_string(),
        "--title".to_string(),
        "Preferred reply style".to_string(),
        "--scope".to_string(),
        "user".to_string(),
        "--scope-id".to_string(),
        "user_123".to_string(),
        "--type".to_string(),
        "procedural".to_string(),
        "--tag".to_string(),
        "preference".to_string(),
        "--actor-user-id".to_string(),
        "user_123".to_string(),
    ];
    let (remember_out, _) = run_json_command(&remember_args);
    assert!(
        remember_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&remember_out.stderr)
    );

    let workspace_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "put".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "release_day".to_string(),
        "--value".to_string(),
        "The team ships every Friday.".to_string(),
        "--title".to_string(),
        "Release cadence".to_string(),
        "--scope".to_string(),
        "workspace".to_string(),
        "--scope-id".to_string(),
        "ws_team".to_string(),
        "--type".to_string(),
        "semantic".to_string(),
        "--tag".to_string(),
        "release".to_string(),
        "--actor-user-id".to_string(),
        "user_123".to_string(),
        "--actor-workspace-id".to_string(),
        "ws_team".to_string(),
    ];
    let (workspace_out, _) = run_json_command(&workspace_args);
    assert!(
        workspace_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&workspace_out.stderr)
    );

    let thread_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "remember".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "current_topic".to_string(),
        "--value".to_string(),
        "This thread is about the release update.".to_string(),
        "--title".to_string(),
        "Current topic".to_string(),
        "--scope".to_string(),
        "thread".to_string(),
        "--scope-id".to_string(),
        "thread_abc".to_string(),
        "--type".to_string(),
        "episodic".to_string(),
        "--actor-user-id".to_string(),
        "user_123".to_string(),
        "--actor-thread-id".to_string(),
        "thread_abc".to_string(),
    ];
    let (thread_out, _) = run_json_command(&thread_args);
    assert!(
        thread_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&thread_out.stderr)
    );

    let args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "run".to_string(),
        "--provider".to_string(),
        "openai-compatible".to_string(),
        "--model".to_string(),
        "gpt-4o-mini".to_string(),
        "--base-url".to_string(),
        base_url,
        "--memory-runtime-mode".to_string(),
        "scoped".to_string(),
        "--actor-user-id".to_string(),
        "user_123".to_string(),
        "--actor-thread-id".to_string(),
        "thread_abc".to_string(),
        "--actor-workspace-id".to_string(),
        "ws_team".to_string(),
        "--input".to_string(),
        "Please prepare a concise Chinese release update for the team.".to_string(),
    ];
    let (out, value) = run_json_command(&args);

    assert!(
        out.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(value["final_text"], "done");
    assert_eq!(
        value["state"]["extra"]["memory_retrieval"]["mode"],
        "scoped"
    );
    assert!(
        value["state"]["extra"]["memory_retrieval"]["selected"]
            .as_array()
            .unwrap()
            .len()
            >= 2
    );

    let memory = memory_message(&requests);
    assert!(memory.contains("DEEPAGENTS_MEMORY_INJECTED_V2"));
    assert!(memory.contains("<memory_pack>"));
    assert!(memory.contains("<thread_memory>"));
    assert!(memory.contains("<pinned_memory>"));
    assert!(memory.contains("<workspace_context>"));
    assert!(memory.contains("Reply in concise Chinese."));
    assert!(memory.contains("The team ships every Friday."));
    assert!(memory.contains("<memory_diagnostics>"));
    assert!(!memory.contains("legacy = true"));
    assert!(!memory.contains(".deepagents/AGENTS.md"));
}

#[test]
fn e2e_memory_outside_root_is_denied_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("AGENTS.md"), "# Outside\n").unwrap();
    let script_path = write_mock_script(&root);

    let args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "run".to_string(),
        "--provider".to_string(),
        "mock".to_string(),
        "--mock-script".to_string(),
        script_path.to_string_lossy().into_owned(),
        "--memory-source".to_string(),
        "../outside/AGENTS.md".to_string(),
        "--input".to_string(),
        "hello".to_string(),
    ];
    let (out, value) = run_json_command(&args);

    assert!(!out.status.success());
    assert_eq!(value["status"], "error");
    assert_eq!(value["error"]["code"], "middleware_error");
    assert!(value["error"]["message"]
        .as_str()
        .unwrap_or("")
        .contains("permission_denied: outside root"));
}

#[cfg(unix)]
#[test]
fn e2e_memory_symlink_source_is_denied() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(root.join(".deepagents")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("AGENTS.md"), "# Outside\n").unwrap();
    std::os::unix::fs::symlink(
        outside.join("AGENTS.md"),
        root.join(".deepagents/AGENTS.md"),
    )
    .unwrap();
    let script_path = write_mock_script(&root);

    let args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "run".to_string(),
        "--provider".to_string(),
        "mock".to_string(),
        "--mock-script".to_string(),
        script_path.to_string_lossy().into_owned(),
        "--input".to_string(),
        "hello".to_string(),
    ];
    let (out, value) = run_json_command(&args);

    assert!(!out.status.success());
    assert_eq!(value["status"], "error");
    assert!(value["error"]["message"]
        .as_str()
        .unwrap_or("")
        .contains("permission_denied: symlink not allowed"));
}

#[test]
fn e2e_memory_oversize_source_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".deepagents")).unwrap();
    std::fs::write(
        root.join(".deepagents").join("AGENTS.md"),
        format!("# Memory\n{}\n", "x".repeat(64)),
    )
    .unwrap();
    let script_path = write_mock_script(root);

    let args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "run".to_string(),
        "--provider".to_string(),
        "mock".to_string(),
        "--mock-script".to_string(),
        script_path.to_string_lossy().into_owned(),
        "--memory-max-source-bytes".to_string(),
        "8".to_string(),
        "--input".to_string(),
        "hello".to_string(),
    ];
    let (out, value) = run_json_command(&args);

    assert!(!out.status.success());
    assert_eq!(value["status"], "error");
    assert!(value["error"]["message"]
        .as_str()
        .unwrap_or("")
        .contains("memory_quota_exceeded: source too large"));
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_memory_truncation_is_reported() {
    let (base_url, requests) = spawn_recording_server().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".deepagents")).unwrap();
    std::fs::write(
        root.join(".deepagents").join("AGENTS.md"),
        format!("# Memory\n{}\n", "abc123".repeat(40)),
    )
    .unwrap();

    let args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "run".to_string(),
        "--provider".to_string(),
        "openai-compatible".to_string(),
        "--model".to_string(),
        "gpt-4o-mini".to_string(),
        "--base-url".to_string(),
        base_url,
        "--memory-max-injected-chars".to_string(),
        "80".to_string(),
        "--input".to_string(),
        "hello".to_string(),
    ];
    let (out, value) = run_json_command(&args);

    assert!(
        out.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        value["state"]["extra"]["memory_diagnostics"]["truncated"],
        true
    );
    let memory = memory_message(&requests);
    assert!(memory.contains("...(memory truncated)..."));
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_memory_strict_false_records_errors_and_continues() {
    let (base_url, requests) = spawn_recording_server().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".deepagents")).unwrap();
    std::fs::write(
        root.join(".deepagents").join("AGENTS.md"),
        "# Memory\nstrict = false\n",
    )
    .unwrap();
    std::fs::write(root.join("README.md"), "not a valid memory source\n").unwrap();

    let args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "run".to_string(),
        "--provider".to_string(),
        "openai-compatible".to_string(),
        "--model".to_string(),
        "gpt-4o-mini".to_string(),
        "--base-url".to_string(),
        base_url,
        "--memory-source".to_string(),
        ".deepagents/AGENTS.md".to_string(),
        "--memory-source".to_string(),
        "README.md".to_string(),
        "--memory-strict=false".to_string(),
        "--input".to_string(),
        "hello".to_string(),
    ];
    let (out, value) = run_json_command(&args);

    assert!(
        out.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        value["state"]["extra"]["memory_diagnostics"]["loaded_sources"],
        1
    );
    assert_eq!(
        value["state"]["extra"]["memory_diagnostics"]["skipped_not_found"],
        0
    );
    let errors = value["state"]["extra"]["memory_diagnostics"]["errors"]
        .as_array()
        .unwrap();
    assert_eq!(errors.len(), 1);
    assert!(errors[0]
        .as_str()
        .unwrap_or("")
        .contains("invalid_request: memory source must be AGENTS.md"));

    let memory = memory_message(&requests);
    assert!(memory.contains("strict = false"));
}

#[test]
fn e2e_memory_get_returns_hit_and_miss() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = root.join("memory_store.json");
    let bin = env!("CARGO_BIN_EXE_deepagents");

    let put = Command::new(bin)
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "memory",
            "put",
            "--store",
            store.to_string_lossy().as_ref(),
            "--key",
            "favorite",
            "--value",
            "rust",
        ])
        .output()
        .unwrap();
    assert!(
        put.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&put.stderr)
    );

    let hit_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "get".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "favorite".to_string(),
    ];
    let (hit_out, hit) = run_json_command(&hit_args);
    assert!(
        hit_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&hit_out.stderr)
    );
    assert_eq!(hit["entry"]["key"], "favorite");
    assert_eq!(hit["entry"]["value"], "rust");

    let miss_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "get".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "missing".to_string(),
    ];
    let (miss_out, miss) = run_json_command(&miss_args);
    assert!(
        miss_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&miss_out.stderr)
    );
    assert!(miss["entry"].is_null());
}

#[test]
fn e2e_memory_corrupt_store_fails_command() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = root.join("memory_store.json");
    std::fs::write(&store, "{bad json\n").unwrap();
    let bin = env!("CARGO_BIN_EXE_deepagents");

    let out = Command::new(bin)
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "memory",
            "get",
            "--store",
            store.to_string_lossy().as_ref(),
            "--key",
            "favorite",
        ])
        .output()
        .unwrap();

    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("memory_corrupt"));
}

#[test]
fn e2e_memory_delete_removes_entry_and_updates_agents_md() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = root.join("memory_store.json");

    let put_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "put".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "favorite".to_string(),
        "--value".to_string(),
        "rust".to_string(),
    ];
    let put_out = run_command(&put_args);
    assert!(
        put_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&put_out.stderr)
    );
    let agents_md = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
    assert!(agents_md.contains("favorite"));
    assert!(agents_md.contains("rust"));

    let delete_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "delete".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "favorite".to_string(),
    ];
    let (delete_out, delete_value) = run_json_command(&delete_args);
    assert!(
        delete_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&delete_out.stderr)
    );
    assert_eq!(delete_value["deleted"], true);
    assert_eq!(delete_value["entry"]["status"], "deleted");

    let get_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "get".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "favorite".to_string(),
    ];
    let (get_out, get_value) = run_json_command(&get_args);
    assert!(
        get_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&get_out.stderr)
    );
    assert!(get_value["entry"].is_null());

    let agents_md = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
    assert!(!agents_md.contains("favorite"));
    assert!(!agents_md.contains("rust"));

    let (delete_miss_out, delete_miss) = run_json_command(&delete_args);
    assert!(
        delete_miss_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&delete_miss_out.stderr)
    );
    assert_eq!(delete_miss["deleted"], false);
}

#[test]
fn e2e_memory_lifecycle_commands_support_scoped_records() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = root.join("memory_store.json");

    let remember_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "remember".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "reply_style".to_string(),
        "--value".to_string(),
        "Reply in concise Chinese.".to_string(),
        "--title".to_string(),
        "Preferred reply style".to_string(),
        "--scope".to_string(),
        "user".to_string(),
        "--scope-id".to_string(),
        "user_123".to_string(),
        "--type".to_string(),
        "procedural".to_string(),
        "--tag".to_string(),
        "preference".to_string(),
        "--actor-user-id".to_string(),
        "user_123".to_string(),
    ];
    let (remember_out, remember_value) = run_json_command(&remember_args);
    assert!(
        remember_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&remember_out.stderr)
    );
    assert_eq!(remember_value["remembered"], true);
    assert_eq!(remember_value["entry"]["scope_type"], "user");
    assert_eq!(remember_value["entry"]["scope_id"], "user_123");
    assert_eq!(remember_value["entry"]["memory_type"], "procedural");
    assert_eq!(remember_value["entry"]["pinned"], true);

    let edit_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "edit".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "reply_style".to_string(),
        "--value".to_string(),
        "Reply in concise Chinese bullet points.".to_string(),
        "--title".to_string(),
        "Preferred concise reply style".to_string(),
        "--confidence".to_string(),
        "0.9".to_string(),
        "--salience".to_string(),
        "0.8".to_string(),
        "--clear-tags".to_string(),
        "--tag".to_string(),
        "preference".to_string(),
        "--tag".to_string(),
        "language".to_string(),
        "--actor-user-id".to_string(),
        "user_123".to_string(),
    ];
    let (edit_out, edit_value) = run_json_command(&edit_args);
    assert!(
        edit_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&edit_out.stderr)
    );
    assert_eq!(edit_value["updated"], true);
    assert_eq!(
        edit_value["entry"]["title"],
        "Preferred concise reply style"
    );
    let confidence = edit_value["entry"]["confidence"].as_f64().unwrap();
    let salience = edit_value["entry"]["salience"].as_f64().unwrap();
    assert!((confidence - 0.9).abs() < 1e-6);
    assert!((salience - 0.8).abs() < 1e-6);

    let unpin_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "unpin".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "reply_style".to_string(),
        "--actor-user-id".to_string(),
        "user_123".to_string(),
    ];
    let (unpin_out, unpin_value) = run_json_command(&unpin_args);
    assert!(
        unpin_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&unpin_out.stderr)
    );
    assert_eq!(unpin_value["updated"], true);
    assert_eq!(unpin_value["entry"]["pinned"], false);

    let pin_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "pin".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "reply_style".to_string(),
        "--actor-user-id".to_string(),
        "user_123".to_string(),
    ];
    let (pin_out, pin_value) = run_json_command(&pin_args);
    assert!(
        pin_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&pin_out.stderr)
    );
    assert_eq!(pin_value["updated"], true);
    assert_eq!(pin_value["entry"]["pinned"], true);

    let explain_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "explain".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "reply_style".to_string(),
        "--actor-user-id".to_string(),
        "user_123".to_string(),
    ];
    let (explain_out, explain_value) = run_json_command(&explain_args);
    assert!(
        explain_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&explain_out.stderr)
    );
    assert_eq!(
        explain_value["entry"]["source"]["kind"],
        "explicit_user_request"
    );
    assert_eq!(explain_value["visible_to_get"], true);
    assert_eq!(explain_value["rendered_in_agents_md"], true);

    let delete_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "delete".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "reply_style".to_string(),
        "--actor-user-id".to_string(),
        "user_123".to_string(),
    ];
    let (delete_out, delete_value) = run_json_command(&delete_args);
    assert!(
        delete_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&delete_out.stderr)
    );
    assert_eq!(delete_value["deleted"], true);
    assert_eq!(delete_value["entry"]["status"], "deleted");

    let (explain_deleted_out, explain_deleted_value) = run_json_command(&explain_args);
    assert!(
        explain_deleted_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&explain_deleted_out.stderr)
    );
    assert_eq!(explain_deleted_value["entry"]["status"], "deleted");
    assert_eq!(explain_deleted_value["visible_to_get"], false);
    assert_eq!(explain_deleted_value["rendered_in_agents_md"], false);

    let query_deleted_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "query".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--scope".to_string(),
        "user".to_string(),
        "--scope-id".to_string(),
        "user_123".to_string(),
        "--status".to_string(),
        "deleted".to_string(),
        "--include-inactive".to_string(),
        "--actor-user-id".to_string(),
        "user_123".to_string(),
    ];
    let (query_deleted_out, query_deleted_value) = run_json_command(&query_deleted_args);
    assert!(
        query_deleted_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&query_deleted_out.stderr)
    );
    let entries = query_deleted_value["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["key"], "reply_style");
    assert_eq!(entries[0]["status"], "deleted");
}

#[test]
fn e2e_memory_cross_user_reads_are_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = root.join("memory_store.json");

    let remember_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "remember".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "diet".to_string(),
        "--value".to_string(),
        "Vegetarian".to_string(),
        "--scope".to_string(),
        "user".to_string(),
        "--actor-user-id".to_string(),
        "user_a".to_string(),
    ];
    let remember_out = run_command(&remember_args);
    assert!(
        remember_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&remember_out.stderr)
    );

    let get_owner_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "get".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "diet".to_string(),
        "--actor-user-id".to_string(),
        "user_a".to_string(),
    ];
    let (_, owner_value) = run_json_command(&get_owner_args);
    assert_eq!(owner_value["entry"]["value"], "Vegetarian");

    let get_other_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "get".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "diet".to_string(),
        "--actor-user-id".to_string(),
        "user_b".to_string(),
    ];
    let (_, other_value) = run_json_command(&get_other_args);
    assert!(other_value["entry"].is_null());
}

#[test]
fn e2e_memory_workspace_visibility_requires_membership() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = root.join("memory_store.json");

    let remember_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "remember".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "release_day".to_string(),
        "--value".to_string(),
        "Release is Thursday 3pm".to_string(),
        "--scope".to_string(),
        "workspace".to_string(),
        "--actor-user-id".to_string(),
        "user_a".to_string(),
        "--actor-workspace-id".to_string(),
        "ws_shared".to_string(),
    ];
    let remember_out = run_command(&remember_args);
    assert!(
        remember_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&remember_out.stderr)
    );

    let blocked_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "get".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "release_day".to_string(),
        "--actor-user-id".to_string(),
        "user_b".to_string(),
    ];
    let (_, blocked_value) = run_json_command(&blocked_args);
    assert!(blocked_value["entry"].is_null());

    let allowed_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "get".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "release_day".to_string(),
        "--actor-user-id".to_string(),
        "user_b".to_string(),
        "--actor-workspace-id".to_string(),
        "ws_shared".to_string(),
    ];
    let (_, allowed_value) = run_json_command(&allowed_args);
    assert_eq!(allowed_value["entry"]["value"], "Release is Thursday 3pm");
}

#[test]
fn e2e_memory_thread_scope_is_thread_local() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = root.join("memory_store.json");

    let remember_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "remember".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "reply_length".to_string(),
        "--value".to_string(),
        "Keep replies short in this thread".to_string(),
        "--scope".to_string(),
        "thread".to_string(),
        "--actor-user-id".to_string(),
        "user_a".to_string(),
        "--actor-thread-id".to_string(),
        "thread_a".to_string(),
    ];
    let remember_out = run_command(&remember_args);
    assert!(
        remember_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&remember_out.stderr)
    );

    let allowed_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "get".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "reply_length".to_string(),
        "--actor-user-id".to_string(),
        "user_a".to_string(),
        "--actor-thread-id".to_string(),
        "thread_a".to_string(),
    ];
    let (_, allowed_value) = run_json_command(&allowed_args);
    assert_eq!(
        allowed_value["entry"]["value"],
        "Keep replies short in this thread"
    );

    let blocked_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "get".to_string(),
        "--store".to_string(),
        store.to_string_lossy().into_owned(),
        "--key".to_string(),
        "reply_length".to_string(),
        "--actor-user-id".to_string(),
        "user_a".to_string(),
        "--actor-thread-id".to_string(),
        "thread_b".to_string(),
    ];
    let (_, blocked_value) = run_json_command(&blocked_args);
    assert!(blocked_value["entry"].is_null());
}

#[test]
fn e2e_memory_put_fails_when_agents_projection_cannot_be_written() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".deepagents").join("AGENTS.md")).unwrap();

    let args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "put".to_string(),
        "--key".to_string(),
        "favorite".to_string(),
        "--value".to_string(),
        "rust".to_string(),
    ];
    let out = run_command(&args);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("failed to write AGENTS.md"));
}

#[test]
fn e2e_memory_lru_policy_uses_persisted_read_metadata_across_cli_runs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let config_home = root.join("global-config");
    let store = root.join("memory_store.json");
    let store_value = store.to_string_lossy().into_owned();

    set_workspace_config(root, &config_home, "memory.file.store_path", &store_value);
    set_workspace_config(root, &config_home, "memory.file.max_entries", "2");
    set_workspace_config(root, &config_home, "memory.file.eviction", "lru");

    for (key, value) in [("a", "one"), ("b", "two")] {
        let args = vec![
            "--root".to_string(),
            root.to_string_lossy().into_owned(),
            "memory".to_string(),
            "put".to_string(),
            "--key".to_string(),
            key.to_string(),
            "--value".to_string(),
            value.to_string(),
        ];
        let out = run_command_with_config_home(&config_home, &args);
        assert!(
            out.status.success(),
            "key={key} stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    std::thread::sleep(Duration::from_millis(1100));

    let get_a_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "get".to_string(),
        "--key".to_string(),
        "a".to_string(),
    ];
    let (get_a_out, get_a) = run_json_command_with_config_home(&config_home, &get_a_args);
    assert!(
        get_a_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&get_a_out.stderr)
    );
    assert_eq!(get_a["entry"]["value"], "one");

    let put_c_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "put".to_string(),
        "--key".to_string(),
        "c".to_string(),
        "--value".to_string(),
        "three".to_string(),
    ];
    let put_c_out = run_command_with_config_home(&config_home, &put_c_args);
    assert!(
        put_c_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&put_c_out.stderr)
    );

    for (key, expected_present) in [("a", true), ("b", false), ("c", true)] {
        let args = vec![
            "--root".to_string(),
            root.to_string_lossy().into_owned(),
            "memory".to_string(),
            "get".to_string(),
            "--key".to_string(),
            key.to_string(),
        ];
        let (out, value) = run_json_command_with_config_home(&config_home, &args);
        assert!(
            out.status.success(),
            "key={key} stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(value["entry"].is_null(), !expected_present, "key={key}");
    }

    let store_json = read_json_file(&store);
    let keys: Vec<_> = store_json["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys.len(), 2);
    assert!(keys.contains(&"a"));
    assert!(keys.contains(&"c"));
}

#[test]
fn e2e_memory_fifo_policy_evicts_oldest_entry() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let config_home = root.join("global-config");
    let store = root.join("memory_store.json");
    let store_value = store.to_string_lossy().into_owned();

    set_workspace_config(root, &config_home, "memory.file.store_path", &store_value);
    set_workspace_config(root, &config_home, "memory.file.max_entries", "2");
    set_workspace_config(root, &config_home, "memory.file.eviction", "fifo");

    for (key, value) in [("a", "one"), ("b", "two"), ("c", "three")] {
        let args = vec![
            "--root".to_string(),
            root.to_string_lossy().into_owned(),
            "memory".to_string(),
            "put".to_string(),
            "--key".to_string(),
            key.to_string(),
            "--value".to_string(),
            value.to_string(),
        ];
        let out = run_command_with_config_home(&config_home, &args);
        assert!(
            out.status.success(),
            "key={key} stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    for (key, expected_present) in [("a", false), ("b", true), ("c", true)] {
        let args = vec![
            "--root".to_string(),
            root.to_string_lossy().into_owned(),
            "memory".to_string(),
            "get".to_string(),
            "--key".to_string(),
            key.to_string(),
        ];
        let (out, value) = run_json_command_with_config_home(&config_home, &args);
        assert!(
            out.status.success(),
            "key={key} stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(value["entry"].is_null(), !expected_present, "key={key}");
    }
}

#[test]
fn e2e_memory_ttl_policy_evicts_expired_entries() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let config_home = root.join("global-config");
    let store = root.join("memory_store.json");
    let store_value = store.to_string_lossy().into_owned();

    set_workspace_config(root, &config_home, "memory.file.store_path", &store_value);
    set_workspace_config(root, &config_home, "memory.file.max_entries", "10");
    set_workspace_config(root, &config_home, "memory.file.eviction", "ttl");
    set_workspace_config(root, &config_home, "memory.file.ttl_secs", "1");

    std::fs::write(
        &store,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 1,
            "policy": {
                "max_entries": 50,
                "max_bytes_total": 999999,
                "eviction": "Lru"
            },
            "entries": [
                {
                    "key": "old",
                    "value": "stale",
                    "tags": [],
                    "created_at": "2000-01-01T00:00:00Z",
                    "updated_at": "2000-01-01T00:00:00Z",
                    "last_accessed_at": "2000-01-01T00:00:00Z",
                    "access_count": 1
                },
                {
                    "key": "fresh",
                    "value": "current",
                    "tags": [],
                    "created_at": "2099-01-01T00:00:00Z",
                    "updated_at": "2099-01-01T00:00:00Z",
                    "last_accessed_at": "2099-01-01T00:00:00Z",
                    "access_count": 1
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let compact_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "compact".to_string(),
    ];
    let (compact_out, compact_value) =
        run_json_command_with_config_home(&config_home, &compact_args);
    assert!(
        compact_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&compact_out.stderr)
    );
    assert_eq!(compact_value["eviction"]["evicted"], 1);

    for (key, expected_present) in [("old", false), ("fresh", true)] {
        let args = vec![
            "--root".to_string(),
            root.to_string_lossy().into_owned(),
            "memory".to_string(),
            "get".to_string(),
            "--key".to_string(),
            key.to_string(),
        ];
        let (out, value) = run_json_command_with_config_home(&config_home, &args);
        assert!(
            out.status.success(),
            "key={key} stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(value["entry"].is_null(), !expected_present, "key={key}");
    }
}

#[test]
fn e2e_memory_config_policy_overrides_persisted_store_policy() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let config_home = root.join("global-config");
    let store = root.join("memory_store.json");
    let store_value = store.to_string_lossy().into_owned();

    set_workspace_config(root, &config_home, "memory.file.store_path", &store_value);
    set_workspace_config(root, &config_home, "memory.file.max_entries", "1");
    set_workspace_config(root, &config_home, "memory.file.eviction", "fifo");

    std::fs::write(
        &store,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 1,
            "policy": {
                "max_entries": 9,
                "max_bytes_total": 999999,
                "eviction": "Lru"
            },
            "entries": [
                {
                    "key": "a",
                    "value": "one",
                    "tags": [],
                    "created_at": "2000-01-01T00:00:00Z",
                    "updated_at": "2000-01-01T00:00:00Z",
                    "last_accessed_at": "2000-01-01T00:00:00Z",
                    "access_count": 1
                },
                {
                    "key": "b",
                    "value": "two",
                    "tags": [],
                    "created_at": "2001-01-01T00:00:00Z",
                    "updated_at": "2001-01-01T00:00:00Z",
                    "last_accessed_at": "2001-01-01T00:00:00Z",
                    "access_count": 1
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let compact_args = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "memory".to_string(),
        "compact".to_string(),
    ];
    let compact_out = run_command_with_config_home(&config_home, &compact_args);
    assert!(
        compact_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&compact_out.stderr)
    );

    let store_json = read_json_file(&store);
    assert_eq!(store_json["policy"]["max_entries"], 1);
    assert_eq!(store_json["policy"]["eviction"], "Fifo");
    let entries = store_json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["key"], "b");
}
