use std::process::Command;

use axum::routing::post;
use axum::{Json, Router};

#[test]
fn e2e_config_set_get_and_doctor() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let config_home = dir.path().join("global-config");
    let bin = env!("CARGO_BIN_EXE_deepagents");
    let env_name = "DEEPAGENTS_TEST_CLI_KEY";

    let out = Command::new(bin)
        .env("DEEPAGENTS_CONFIG_HOME", &config_home)
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "config",
            "set",
            "providers.openai-compatible.api_key_env",
            env_name,
            "--scope",
            "workspace",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = Command::new(bin)
        .env("DEEPAGENTS_CONFIG_HOME", &config_home)
        .env(env_name, "sk-secret-should-not-print")
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "config",
            "get",
            "providers.openai-compatible.api_key_env",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["key"], "providers.openai-compatible.api_key_env");
    assert!(value["value"].is_null());
    assert_eq!(value["secret_status"], "set");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains(env_name));
    assert!(!stdout.contains("sk-secret-should-not-print"));

    let out = Command::new(bin)
        .env("DEEPAGENTS_CONFIG_HOME", &config_home)
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "config",
            "doctor",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_run_uses_workspace_config_defaults() {
    let app = Router::new().route("/chat/completions", post(chat_json_handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let config_home = dir.path().join("global-config");
    let bin = env!("CARGO_BIN_EXE_deepagents");
    let env_name = "DEEPAGENTS_TEST_CLI_RUN_KEY";

    for (key, value) in [
        ("providers.openai-compatible.enabled", "true"),
        ("providers.openai-compatible.model", "gpt-4o-mini"),
        (
            "providers.openai-compatible.base_url",
            &format!("http://{}", addr),
        ),
        ("providers.openai-compatible.api_key_env", env_name),
    ] {
        let out = Command::new(bin)
            .env("DEEPAGENTS_CONFIG_HOME", &config_home)
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
            "stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let out = Command::new(bin)
        .env("DEEPAGENTS_CONFIG_HOME", &config_home)
        .env(env_name, "dummy-key")
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "run",
            "--provider",
            "openai-compatible",
            "--input",
            "hello",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["final_text"], "done");
}

#[test]
fn e2e_memory_uses_configured_store_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let config_home = dir.path().join("global-config");
    let custom_store = root.join("custom").join("memory.json");
    let bin = env!("CARGO_BIN_EXE_deepagents");

    let out = Command::new(bin)
        .env("DEEPAGENTS_CONFIG_HOME", &config_home)
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "config",
            "set",
            "memory.file.store_path",
            custom_store.to_string_lossy().as_ref(),
            "--scope",
            "workspace",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = Command::new(bin)
        .env("DEEPAGENTS_CONFIG_HOME", &config_home)
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "memory",
            "put",
            "--key",
            "favorite",
            "--value",
            "rust",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(custom_store.exists());
}

async fn chat_json_handler(Json(body): Json<serde_json::Value>) -> Json<serde_json::Value> {
    assert_eq!(body["model"], "gpt-4o-mini");
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
