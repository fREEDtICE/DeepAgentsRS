use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn post_json(app: axum::Router, path: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
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

    let (st, v) = post_json(app.clone(), "/new_session", serde_json::json!({
        "root": root.to_string_lossy(),
        "execution_mode": "non_interactive",
        "shell_allow_list": ["echo"],
        "audit_json": audit.to_string_lossy()
    }))
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.get("ok").and_then(|v| v.as_bool()), Some(true));
    let session_id = v.get("result").unwrap().get("session_id").unwrap().as_str().unwrap().to_string();

    let (st, v) = post_json(app.clone(), "/call_tool", serde_json::json!({
        "session_id": session_id,
        "tool_name": "write_file",
        "input": { "file_path": "a.txt", "content": "hello\n" }
    }))
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.get("ok").and_then(|v| v.as_bool()), Some(true));
    assert!(v.get("result").unwrap().get("state_version").and_then(|v| v.as_u64()).unwrap() >= 1);

    let (st, v) = get(app.clone(), &format!("/session_state/{}", session_id)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.get("ok").and_then(|v| v.as_bool()), Some(true));
    let state_version = v.get("result").unwrap().get("state_version").and_then(|v| v.as_u64()).unwrap();
    assert!(state_version >= 1);
}

#[tokio::test]
async fn phase3_execute_deny_by_default_and_allow_list() {
    let app = deepagents_acp::server::router();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let (st, v) = post_json(app.clone(), "/new_session", serde_json::json!({
        "root": root.to_string_lossy(),
        "execution_mode": "non_interactive"
    }))
    .await;
    assert_eq!(st, StatusCode::OK);
    let session_id = v.get("result").unwrap().get("session_id").unwrap().as_str().unwrap().to_string();

    let (st, v) = post_json(app.clone(), "/call_tool", serde_json::json!({
        "session_id": session_id,
        "tool_name": "execute",
        "input": { "command": "echo hi", "timeout": 5 }
    }))
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.get("ok").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        v.get("result").unwrap().get("error").unwrap().get("code").and_then(|v| v.as_str()),
        Some("approval_required")
    );

    let (st, v) = post_json(app.clone(), "/new_session", serde_json::json!({
        "root": root.to_string_lossy(),
        "execution_mode": "non_interactive",
        "shell_allow_list": ["echo"]
    }))
    .await;
    assert_eq!(st, StatusCode::OK);
    let session_id = v.get("result").unwrap().get("session_id").unwrap().as_str().unwrap().to_string();

    let (st, v) = post_json(app.clone(), "/call_tool", serde_json::json!({
        "session_id": session_id,
        "tool_name": "execute",
        "input": { "command": "echo hi", "timeout": 5 }
    }))
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.get("ok").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        v.get("result").unwrap().get("output").unwrap().get("exit_code").and_then(|v| v.as_i64()),
        Some(0)
    );
}

#[tokio::test]
async fn phase3_end_session_is_idempotent_and_blocks_tool_calls() {
    let app = deepagents_acp::server::router();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let (st, v) = post_json(app.clone(), "/new_session", serde_json::json!({
        "root": root.to_string_lossy(),
        "execution_mode": "non_interactive"
    }))
    .await;
    assert_eq!(st, StatusCode::OK);
    let session_id = v.get("result").unwrap().get("session_id").unwrap().as_str().unwrap().to_string();

    let (st, v) = post_json(app.clone(), "/end_session", serde_json::json!({ "session_id": session_id })).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.get("ok").and_then(|v| v.as_bool()), Some(true));

    let (st, v) = post_json(app.clone(), "/end_session", serde_json::json!({ "session_id": session_id })).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.get("ok").and_then(|v| v.as_bool()), Some(true));

    let (st, v) = post_json(app.clone(), "/call_tool", serde_json::json!({
        "session_id": session_id,
        "tool_name": "read_file",
        "input": { "file_path": "README.md", "limit": 1 }
    }))
    .await;
    assert_eq!(st, StatusCode::GONE);
    assert_eq!(v.get("ok").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(v.get("error").unwrap().get("code").and_then(|v| v.as_str()), Some("already_closed"));
}

