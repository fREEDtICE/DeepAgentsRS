use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::post;
use axum::{Json, Router};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn post_json(
    app: axum::Router,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    (status, v)
}

async fn post_stream(app: axum::Router, path: &str, body: serde_json::Value) -> (StatusCode, String) {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

fn parse_sse_events(body: &str) -> Vec<serde_json::Value> {
    let mut events = Vec::new();
    for chunk in body.split("\n\n") {
        let mut event_name = None;
        let mut data = None;
        for line in chunk.lines() {
            if let Some(rest) = line.strip_prefix("event: ") {
                event_name = Some(rest.to_string());
            }
            if let Some(rest) = line.strip_prefix("data: ") {
                data = Some(rest.to_string());
            }
        }
        if event_name.as_deref() == Some("run_event") {
            if let Some(data) = data {
                events.push(serde_json::from_str(&data).unwrap());
            }
        }
    }
    events
}

async fn get(app: axum::Router, path: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    (status, v)
}

#[tokio::test]
async fn phase3_session_tool_and_state_flow() {
    let app = deepagents_acp::server::router();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let audit = root.join("audit.jsonl");

    let (st, v) = post_json(
        app.clone(),
        "/new_session",
        serde_json::json!({
            "root": root.to_string_lossy(),
            "execution_mode": "non_interactive",
            "shell_allow_list": ["echo"],
            "audit_json": audit.to_string_lossy()
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.get("ok").and_then(|v| v.as_bool()), Some(true));
    let session_id = v
        .get("result")
        .unwrap()
        .get("session_id")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    let (st, v) = post_json(
        app.clone(),
        "/call_tool",
        serde_json::json!({
            "session_id": session_id,
            "tool_name": "write_file",
            "input": { "file_path": "a.txt", "content": "hello\n" }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.get("ok").and_then(|v| v.as_bool()), Some(true));
    assert!(
        v.get("result")
            .unwrap()
            .get("state_version")
            .and_then(|v| v.as_u64())
            .unwrap()
            >= 1
    );

    let (st, v) = get(app.clone(), &format!("/session_state/{}", session_id)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.get("ok").and_then(|v| v.as_bool()), Some(true));
    let state_version = v
        .get("result")
        .unwrap()
        .get("state_version")
        .and_then(|v| v.as_u64())
        .unwrap();
    assert!(state_version >= 1);
}

#[tokio::test]
async fn phase3_execute_deny_by_default_and_allow_list() {
    let app = deepagents_acp::server::router();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let (st, v) = post_json(
        app.clone(),
        "/new_session",
        serde_json::json!({
            "root": root.to_string_lossy(),
            "execution_mode": "non_interactive"
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let session_id = v
        .get("result")
        .unwrap()
        .get("session_id")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    let (st, v) = post_json(
        app.clone(),
        "/call_tool",
        serde_json::json!({
            "session_id": session_id,
            "tool_name": "execute",
            "input": { "command": "echo hi", "timeout": 5 }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.get("ok").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        v.get("result")
            .unwrap()
            .get("error")
            .unwrap()
            .get("code")
            .and_then(|v| v.as_str()),
        Some("approval_required")
    );

    let (st, v) = post_json(
        app.clone(),
        "/new_session",
        serde_json::json!({
            "root": root.to_string_lossy(),
            "execution_mode": "non_interactive",
            "shell_allow_list": ["echo"]
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let session_id = v
        .get("result")
        .unwrap()
        .get("session_id")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    let (st, v) = post_json(
        app.clone(),
        "/call_tool",
        serde_json::json!({
            "session_id": session_id,
            "tool_name": "execute",
            "input": { "command": "echo hi", "timeout": 5 }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.get("ok").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        v.get("result")
            .unwrap()
            .get("output")
            .unwrap()
            .get("exit_code")
            .and_then(|v| v.as_i64()),
        Some(0)
    );
}

#[tokio::test]
async fn phase3_end_session_is_idempotent_and_blocks_tool_calls() {
    let app = deepagents_acp::server::router();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let (st, v) = post_json(
        app.clone(),
        "/new_session",
        serde_json::json!({
            "root": root.to_string_lossy(),
            "execution_mode": "non_interactive"
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let session_id = v
        .get("result")
        .unwrap()
        .get("session_id")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    let (st, v) = post_json(
        app.clone(),
        "/end_session",
        serde_json::json!({ "session_id": session_id }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.get("ok").and_then(|v| v.as_bool()), Some(true));

    let (st, v) = post_json(
        app.clone(),
        "/end_session",
        serde_json::json!({ "session_id": session_id }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.get("ok").and_then(|v| v.as_bool()), Some(true));

    let (st, v) = post_json(
        app.clone(),
        "/call_tool",
        serde_json::json!({
            "session_id": session_id,
            "tool_name": "read_file",
            "input": { "file_path": "README.md", "limit": 1 }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::GONE);
    assert_eq!(v.get("ok").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(
        v.get("error").unwrap().get("code").and_then(|v| v.as_str()),
        Some("already_closed")
    );
}

#[tokio::test]
async fn phase3_run_stream_emits_sse_events() {
    let app = deepagents_acp::server::router();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("README.md"), "hello\n").unwrap();

    let (_, v) = post_json(
        app.clone(),
        "/new_session",
        serde_json::json!({
            "root": root.to_string_lossy(),
            "execution_mode": "non_interactive"
        }),
    )
    .await;
    let session_id = v["result"]["session_id"].as_str().unwrap().to_string();

    let (st, body) = post_stream(
        app.clone(),
        "/run_stream",
        serde_json::json!({
            "session_id": session_id,
            "provider": "mock",
            "mock_script": {
                "steps": [
                    { "type": "tool_calls", "calls": [
                        { "tool_name": "read_file", "arguments": { "file_path": "README.md", "limit": 1 }, "call_id": "r1" }
                    ]},
                    { "type": "final_text", "text": "done" }
                ]
            },
            "input": "read file"
        }),
    )
    .await;

    assert_eq!(st, StatusCode::OK);
    let events = parse_sse_events(&body);
    assert!(events.iter().any(|event| event["type"] == "run_started"));
    assert!(events.iter().any(|event| {
        event["type"] == "tool_call_started" && event["tool_call_id"] == "r1"
    }));
    assert!(matches!(
        events.last(),
        Some(event) if event["type"] == "run_finished" && event["status"] == "completed"
    ));
}

#[tokio::test]
async fn phase3_resume_stream_continues_after_interrupt() {
    let app = deepagents_acp::server::router();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let (_, v) = post_json(
        app.clone(),
        "/new_session",
        serde_json::json!({
            "root": root.to_string_lossy(),
            "execution_mode": "interactive"
        }),
    )
    .await;
    let session_id = v["result"]["session_id"].as_str().unwrap().to_string();

    let (st, body) = post_stream(
        app.clone(),
        "/run_stream",
        serde_json::json!({
            "session_id": session_id,
            "provider": "mock",
            "mock_script": {
                "steps": [
                    { "type": "tool_calls", "calls": [
                        { "tool_name": "write_file", "arguments": { "file_path": "a.txt", "content": "hello\n" }, "call_id": "w1" }
                    ]},
                    { "type": "final_text", "text": "done" }
                ]
            },
            "input": "write file"
        }),
    )
    .await;

    assert_eq!(st, StatusCode::OK);
    let run_events = parse_sse_events(&body);
    assert!(run_events.iter().any(|event| event["type"] == "interrupt"));
    assert!(matches!(
        run_events.last(),
        Some(event) if event["type"] == "run_finished" && event["status"] == "interrupted"
    ));

    let (st, body) = post_stream(
        app.clone(),
        "/resume_stream",
        serde_json::json!({
            "session_id": session_id,
            "interrupt_id": "w1",
            "decision": { "type": "approve" }
        }),
    )
    .await;

    assert_eq!(st, StatusCode::OK);
    let resume_events = parse_sse_events(&body);
    assert!(resume_events.iter().any(|event| {
        event["type"] == "tool_call_started" && event["tool_call_id"] == "w1"
    }));
    assert!(matches!(
        resume_events.last(),
        Some(event) if event["type"] == "run_finished" && event["status"] == "completed"
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn phase3_openai_compatible_run_works() {
    let provider_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider_addr = provider_listener.local_addr().unwrap();
    let provider_app = Router::new().route("/chat/completions", post(openai_stream_handler));
    tokio::spawn(async move {
        axum::serve(provider_listener, provider_app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let app = deepagents_acp::server::router();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let (_, v) = post_json(
        app.clone(),
        "/new_session",
        serde_json::json!({
            "root": root.to_string_lossy(),
            "execution_mode": "non_interactive"
        }),
    )
    .await;
    let session_id = v["result"]["session_id"].as_str().unwrap().to_string();

    let (st, v) = post_json(
        app,
        "/run",
        serde_json::json!({
            "session_id": session_id,
            "provider": "openai-compatible",
            "model": "gpt-4o-mini",
            "base_url": format!("http://{}", provider_addr),
            "input": "hello"
        }),
    )
    .await;

    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["ok"], true);
    assert_eq!(v["result"]["output"]["final_text"], "done");
  }

#[tokio::test(flavor = "multi_thread")]
async fn phase3_openai_compatible_run_stream_emits_deltas() {
    let provider_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider_addr = provider_listener.local_addr().unwrap();
    let provider_app = Router::new().route("/chat/completions", post(openai_stream_handler));
    tokio::spawn(async move {
        axum::serve(provider_listener, provider_app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let app = deepagents_acp::server::router();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let (_, v) = post_json(
        app.clone(),
        "/new_session",
        serde_json::json!({
            "root": root.to_string_lossy(),
            "execution_mode": "non_interactive"
        }),
    )
    .await;
    let session_id = v["result"]["session_id"].as_str().unwrap().to_string();

    let (st, body) = post_stream(
        app,
        "/run_stream",
        serde_json::json!({
            "session_id": session_id,
            "provider": "openai-compatible",
            "model": "gpt-4o-mini",
            "base_url": format!("http://{}", provider_addr),
            "input": "hello"
        }),
    )
    .await;

    assert_eq!(st, StatusCode::OK);
    let events = parse_sse_events(&body);
    assert!(events.iter().any(|event| event["type"] == "assistant_text_delta"));
    assert!(events.iter().any(|event| event["type"] == "usage_reported"));
    assert!(matches!(
        events.last(),
        Some(event) if event["type"] == "run_finished" && event["status"] == "completed"
    ));
}
async fn openai_stream_handler(
    Json(body): Json<serde_json::Value>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    assert_eq!(body["model"], "gpt-4o-mini");
    let stream = tokio_stream::iter([
        Ok(Event::default().data(serde_json::json!({
            "choices": [{
                "delta": { "content": "do" },
                "finish_reason": null
            }]
        }).to_string())),
        Ok(Event::default().data(serde_json::json!({
            "choices": [{
                "delta": { "content": "ne" },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 5,
                "completion_tokens": 2,
                "total_tokens": 7
            }
        }).to_string())),
        Ok(Event::default().data("[DONE]")),
    ]);
    Sse::new(stream).keep_alive(KeepAlive::default())
}
