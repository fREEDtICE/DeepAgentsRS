use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use deepagents::approval::{redact_command, ApprovalDecision, ApprovalPolicy, ApprovalRequest, DefaultApprovalPolicy, ExecutionMode};
use deepagents::audit::{AuditEvent, AuditSink};
use deepagents::state::AgentState;
use deepagents::{create_deep_agent_with_backend, create_local_sandbox_backend, DeepAgent};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const PROTOCOL_VERSION: &str = "v1";

#[derive(Clone)]
pub struct ServerConfig {
    pub bind: String,
}

#[derive(Clone)]
pub struct AppState {
    sessions: Arc<tokio::sync::RwLock<HashMap<String, Arc<tokio::sync::Mutex<Session>>>>>,
    next_id: Arc<AtomicU64>,
}

#[derive(Clone)]
struct Session {
    session_id: String,
    root: String,
    agent: DeepAgent,
    state: AgentState,
    state_version: u64,
    mode: ExecutionMode,
    approval: Arc<dyn ApprovalPolicy>,
    audit: Option<Arc<dyn AuditSink>>,
    closed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NewSessionRequest {
    #[serde(default)]
    protocol_version: Option<String>,
    root: String,
    #[serde(default)]
    execution_mode: Option<String>,
    #[serde(default)]
    shell_allow_list: Option<Vec<String>>,
    #[serde(default)]
    audit_json: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallToolRequest {
    #[serde(default)]
    protocol_version: Option<String>,
    session_id: String,
    tool_name: String,
    input: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EndSessionRequest {
    #[serde(default)]
    protocol_version: Option<String>,
    session_id: String,
}

#[derive(Debug, Serialize)]
struct ErrorObj {
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

fn resp_ok(result: Value) -> Json<Value> {
    Json(json!({
        "protocol_version": PROTOCOL_VERSION,
        "ok": true,
        "result": result
    }))
}

fn resp_err(code: &str, message: &str, details: Option<Value>) -> Json<Value> {
    Json(json!({
        "protocol_version": PROTOCOL_VERSION,
        "ok": false,
        "error": ErrorObj {
            code: code.to_string(),
            message: message.to_string(),
            details,
        }
    }))
}

pub fn router() -> Router {
    let state = AppState {
        sessions: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        next_id: Arc::new(AtomicU64::new(1)),
    };

    Router::new()
        .route("/initialize", post(initialize))
        .route("/new_session", post(new_session))
        .route("/call_tool", post(call_tool))
        .route("/session_state/:session_id", get(get_session_state))
        .route("/end_session", post(end_session))
        .with_state(state)
}

async fn initialize() -> (StatusCode, Json<Value>) {
    let v = resp_ok(json!({
        "name": "deepagents-acp",
        "protocol_version": PROTOCOL_VERSION,
        "capabilities": {
            "supports_state": true
        }
    }));
    (StatusCode::OK, v)
}

async fn new_session(
    State(st): State<AppState>,
    Json(req): Json<NewSessionRequest>,
) -> (StatusCode, Json<Value>) {
    if let Some(v) = req.protocol_version.as_deref() {
        if v != PROTOCOL_VERSION {
            return (
                StatusCode::BAD_REQUEST,
                resp_err("invalid_request", "unsupported protocol_version", Some(json!({ "got": v }))),
            );
        }
    }

    let mode = parse_execution_mode(req.execution_mode.as_deref()).unwrap_or(ExecutionMode::NonInteractive);
    let allow_list = req.shell_allow_list.unwrap_or_default();

    let backend_shell_allow = match mode {
        ExecutionMode::NonInteractive => Some(allow_list.clone()),
        ExecutionMode::Interactive => {
            if allow_list.is_empty() {
                None
            } else {
                Some(allow_list.clone())
            }
        }
    };

    let backend = match create_local_sandbox_backend(req.root.clone(), backend_shell_allow) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                resp_err("invalid_request", "invalid root", Some(json!({ "error": e.to_string() }))),
            )
        }
    };
    let agent = create_deep_agent_with_backend(backend);

    let approval: Arc<dyn ApprovalPolicy> = Arc::new(DefaultApprovalPolicy::new(allow_list));
    let audit: Option<Arc<dyn AuditSink>> = req
        .audit_json
        .as_deref()
        .map(|p| Arc::new(JsonlFileAuditSink::new(p)) as Arc<dyn AuditSink>);

    let n = st.next_id.fetch_add(1, Ordering::Relaxed);
    let session_id = format!("s-{}-{}", now_ms(), n);

    let session = Session {
        session_id: session_id.clone(),
        root: req.root,
        agent,
        state: AgentState::default(),
        state_version: 0,
        mode,
        approval,
        audit,
        closed: false,
    };

    st.sessions
        .write()
        .await
        .insert(session_id.clone(), Arc::new(tokio::sync::Mutex::new(session)));

    (StatusCode::OK, resp_ok(json!({ "session_id": session_id })))
}

async fn call_tool(
    State(st): State<AppState>,
    Json(req): Json<CallToolRequest>,
) -> (StatusCode, Json<Value>) {
    if let Some(v) = req.protocol_version.as_deref() {
        if v != PROTOCOL_VERSION {
            return (
                StatusCode::BAD_REQUEST,
                resp_err("invalid_request", "unsupported protocol_version", Some(json!({ "got": v }))),
            );
        }
    }

    let sessions = st.sessions.read().await;
    let Some(s) = sessions.get(&req.session_id).cloned() else {
        return (
            StatusCode::NOT_FOUND,
            resp_err("session_not_found", "session not found", Some(json!({ "session_id": req.session_id }))),
        );
    };
    drop(sessions);

    let mut session = s.lock().await;
    if session.closed {
        return (
            StatusCode::GONE,
            resp_err("already_closed", "session already closed", Some(json!({ "session_id": req.session_id }))),
        );
    }

    session.state_version = session.state_version.saturating_add(1);

    if req.tool_name == "execute" {
        let cmd = req.input.get("command").and_then(|v| v.as_str()).map(|s| s.to_string());
        if let Some(cmd) = cmd.as_deref() {
            let decision = session.approval.decide(&ApprovalRequest {
                command: cmd.to_string(),
                root: session.root.clone(),
                mode: session.mode,
            });

            match decision {
                ApprovalDecision::Allow { .. } => {}
                ApprovalDecision::Deny { code, reason } => {
                    record_audit(&session, cmd, "deny", &code, &reason, None, None, None);
                    return (
                        StatusCode::OK,
                        resp_ok(tool_result_error(
                            &code,
                            &reason,
                            session.state_version,
                            Some(&session.state),
                            None,
                        )),
                    );
                }
                ApprovalDecision::RequireApproval { code, reason } => {
                    record_audit(&session, cmd, "require_approval", &code, &reason, None, None, None);
                    if matches!(session.mode, ExecutionMode::NonInteractive) {
                        return (
                            StatusCode::OK,
                            resp_ok(tool_result_error(
                                &code,
                                &reason,
                                session.state_version,
                                Some(&session.state),
                                None,
                            )),
                        );
                    }
                    return (
                        StatusCode::OK,
                        resp_ok(tool_result_error(
                            &code,
                            &reason,
                            session.state_version,
                            Some(&session.state),
                            None,
                        )),
                    );
                }
            }
        }
    }

    let started = std::time::Instant::now();
    let agent = session.agent.clone();
    let tool_name = req.tool_name.clone();
    let input = req.input.clone();
    let result = agent.call_tool_stateful(&tool_name, input, &mut session.state).await;
    let duration_ms = started.elapsed().as_millis() as u64;

    match result {
        Ok((out, delta)) => {
            if req.tool_name == "execute" {
                let cmd = req
                    .input
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                record_audit(
                    &session,
                    cmd,
                    "allow",
                    "allow",
                    "allowed",
                    out.get("exit_code").and_then(|v| v.as_i64()).map(|v| v as i32),
                    out.get("truncated").and_then(|v| v.as_bool()),
                    Some(duration_ms),
                );
            }
            (
                StatusCode::OK,
                resp_ok(json!({
                    "output": out,
                    "error": Value::Null,
                    "state": session.state,
                    "delta": delta,
                    "state_version": session.state_version
                })),
            )
        }
        Err(e) => {
            let (code, msg) = classify_tool_error(e.to_string().as_str());
            if req.tool_name == "execute" {
                let cmd = req
                    .input
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                record_audit(&session, cmd, "allow", "allow", "allowed but failed", None, None, Some(duration_ms));
            }
            (
                StatusCode::OK,
                resp_ok(tool_result_error(
                    &code,
                    &msg,
                    session.state_version,
                    Some(&session.state),
                    None,
                )),
            )
        }
    }
}

async fn get_session_state(
    State(st): State<AppState>,
    Path(session_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    let sessions = st.sessions.read().await;
    let Some(s) = sessions.get(&session_id).cloned() else {
        return (
            StatusCode::NOT_FOUND,
            resp_err("session_not_found", "session not found", Some(json!({ "session_id": session_id }))),
        );
    };
    drop(sessions);

    let session = s.lock().await;
    if session.closed {
        return (
            StatusCode::GONE,
            resp_err("already_closed", "session already closed", Some(json!({ "session_id": session_id }))),
        );
    }
    (
        StatusCode::OK,
        resp_ok(json!({
            "state": session.state,
            "state_version": session.state_version
        })),
    )
}

async fn end_session(
    State(st): State<AppState>,
    Json(req): Json<EndSessionRequest>,
) -> (StatusCode, Json<Value>) {
    if let Some(v) = req.protocol_version.as_deref() {
        if v != PROTOCOL_VERSION {
            return (
                StatusCode::BAD_REQUEST,
                resp_err("invalid_request", "unsupported protocol_version", Some(json!({ "got": v }))),
            );
        }
    }

    let sessions = st.sessions.read().await;
    let Some(s) = sessions.get(&req.session_id).cloned() else {
        return (
            StatusCode::NOT_FOUND,
            resp_err("session_not_found", "session not found", Some(json!({ "session_id": req.session_id }))),
        );
    };
    drop(sessions);

    let mut session = s.lock().await;
    let already_closed = session.closed;
    session.closed = true;
    (StatusCode::OK, resp_ok(json!({ "already_closed": already_closed })))
}

fn tool_result_error(
    code: &str,
    message: &str,
    state_version: u64,
    state: Option<&AgentState>,
    delta: Option<Value>,
) -> Value {
    let st = state
        .map(|s| serde_json::to_value(s).unwrap_or(Value::Null))
        .unwrap_or(Value::Null);
    let dl = delta.unwrap_or(Value::Null);
    json!({
        "output": Value::Null,
        "error": { "code": code, "message": message },
        "state": st,
        "delta": dl,
        "state_version": state_version
    })
}

fn parse_execution_mode(s: Option<&str>) -> Option<ExecutionMode> {
    match s? {
        "non_interactive" | "non-interactive" => Some(ExecutionMode::NonInteractive),
        "interactive" => Some(ExecutionMode::Interactive),
        _ => None,
    }
}

fn classify_tool_error(s: &str) -> (String, String) {
    if let Some(rest) = s.strip_prefix("command_not_allowed: ") {
        if let Some((code, _)) = rest.split_once(':') {
            return (code.trim().to_string(), s.to_string());
        }
        return ("command_not_allowed".to_string(), s.to_string());
    }
    for code in [
        "file_not_found",
        "is_directory",
        "permission_denied",
        "no_match",
        "timeout",
        "invalid_input",
        "schema_validation_failed",
    ] {
        if s.contains(code) {
            return (code.to_string(), s.to_string());
        }
    }
    ("unknown".to_string(), s.to_string())
}

fn record_audit(
    session: &Session,
    command: &str,
    decision: &str,
    decision_code: &str,
    decision_reason: &str,
    exit_code: Option<i32>,
    truncated: Option<bool>,
    duration_ms: Option<u64>,
) {
    let Some(sink) = &session.audit else {
        return;
    };
    let _ = sink.record(AuditEvent {
        timestamp_ms: now_ms(),
        root: session.root.clone(),
        mode: mode_str(session.mode),
        command_redacted: redact_command(command),
        decision: decision.to_string(),
        decision_code: decision_code.to_string(),
        decision_reason: decision_reason.to_string(),
        exit_code,
        truncated,
        duration_ms,
    });
}

fn mode_str(mode: ExecutionMode) -> String {
    match mode {
        ExecutionMode::NonInteractive => "non_interactive".to_string(),
        ExecutionMode::Interactive => "interactive".to_string(),
    }
}

fn now_ms() -> i64 {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    dur.as_millis() as i64
}

#[derive(Clone)]
struct JsonlFileAuditSink {
    path: std::path::PathBuf,
}

impl JsonlFileAuditSink {
    fn new(path: &str) -> Self {
        Self {
            path: std::path::PathBuf::from(path),
        }
    }
}

impl AuditSink for JsonlFileAuditSink {
    fn record(&self, event: AuditEvent) -> anyhow::Result<()> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let line = serde_json::to_string(&event)?;
        writeln!(f, "{}", line)?;
        Ok(())
    }
}
