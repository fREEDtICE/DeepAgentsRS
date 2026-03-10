use std::process::Command;

use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::post;
use axum::{Json, Router};

#[tokio::test(flavor = "multi_thread")]
async fn e2e_run_with_openai_compatible_provider() {
    let app = Router::new().route("/chat/completions", post(chat_json_handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let bin = env!("CARGO_BIN_EXE_deepagents");
    let out = Command::new(bin)
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "run",
            "--provider",
            "openai-compatible",
            "--model",
            "gpt-4o-mini",
            "--base-url",
            &format!("http://{}", addr),
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
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v.get("final_text").and_then(|value| value.as_str()),
        Some("done")
    );
    assert!(v.get("error").is_some_and(|value| value.is_null()));
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_stream_events_print_provider_capabilities() {
    let app = Router::new().route("/chat/completions", post(chat_stream_handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let bin = env!("CARGO_BIN_EXE_deepagents");
    let out = Command::new(bin)
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "run",
            "--provider",
            "openai-compatible",
            "--model",
            "gpt-4o-mini",
            "--base-url",
            &format!("http://{}", addr),
            "--stream-events",
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
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("\"provider_id\":\"openai-compatible\""));
    assert!(stderr.contains("\"supports_streaming\":true"));
    assert!(stderr.contains("\"supports_structured_output\":false"));
}

async fn chat_json_handler(Json(body): Json<serde_json::Value>) -> Json<serde_json::Value> {
    assert_eq!(body["model"], "gpt-4o-mini");
    let messages = body["messages"].as_array().cloned().unwrap_or_default();
    assert!(messages.iter().any(|message| message["role"] == "user"));
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

#[allow(dead_code)]
async fn chat_stream_handler(
    Json(body): Json<serde_json::Value>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    assert_eq!(body["model"], "gpt-4o-mini");
    let messages = body["messages"].as_array().cloned().unwrap_or_default();
    assert!(messages.iter().any(|message| message["role"] == "user"));
    let stream = tokio_stream::iter([
        Ok(Event::default().data(
            serde_json::json!({
                "choices": [{
                    "delta": {
                        "content": "done"
                    },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 5,
                    "completion_tokens": 1,
                    "total_tokens": 6
                }
            })
            .to_string(),
        )),
        Ok(Event::default().data("[DONE]")),
    ]);
    Sse::new(stream).keep_alive(KeepAlive::default())
}
