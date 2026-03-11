use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use deepagents::approval::{
    redact_command, ApprovalDecision, ApprovalPolicy, ApprovalRequest, DefaultApprovalPolicy,
    ExecutionMode,
};
use deepagents::audit::{AuditEvent, AuditSink};
use deepagents::provider::mock::MockScript;
use deepagents::state::AgentState;
use deepagents::{create_deep_agent_with_backend, create_local_sandbox_backend, DeepAgent};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

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

struct Session {
    root: String,
    agent: DeepAgent,
    state: AgentState,
    state_version: u64,
    mode: ExecutionMode,
    approval: Arc<dyn ApprovalPolicy>,
    audit: Option<Arc<dyn AuditSink>>,
    runner: Option<deepagents::runtime::ResumableRunner>,
    provider_info: Option<deepagents::provider::ProviderDiagnostics>,
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
struct RunRequest {
    #[serde(default)]
    protocol_version: Option<String>,
    session_id: String,
    provider: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    api_key_env: Option<String>,
    #[serde(default)]
    tool_choice: Option<deepagents::provider::ToolChoice>,
    #[serde(default)]
    structured_output: Option<deepagents::provider::StructuredOutputSpec>,
    #[serde(default)]
    mock_script: Option<Value>,
    input: String,
    #[serde(default)]
    max_steps: Option<usize>,
    #[serde(default)]
    provider_timeout_ms: Option<u64>,
    #[serde(default)]
    memory_disable: Option<bool>,
    #[serde(default)]
    summarization_disable: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResumeRequest {
    #[serde(default)]
    protocol_version: Option<String>,
    session_id: String,
    interrupt_id: String,
    decision: deepagents::runtime::HitlDecision,
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
        .route("/run", post(run_session))
        .route("/run_stream", post(run_session_stream))
        .route("/resume", post(resume_session))
        .route("/resume_stream", post(resume_session_stream))
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
                resp_err(
                    "invalid_request",
                    "unsupported protocol_version",
                    Some(json!({ "got": v })),
                ),
            );
        }
    }

    let mode = parse_execution_mode(req.execution_mode.as_deref())
        .unwrap_or(ExecutionMode::NonInteractive);
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
                resp_err(
                    "invalid_request",
                    "invalid root",
                    Some(json!({ "error": e.to_string() })),
                ),
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
        root: req.root,
        agent,
        state: AgentState::default(),
        state_version: 0,
        mode,
        approval,
        audit,
        runner: None,
        provider_info: None,
        closed: false,
    };

    st.sessions.write().await.insert(
        session_id.clone(),
        Arc::new(tokio::sync::Mutex::new(session)),
    );

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
                resp_err(
                    "invalid_request",
                    "unsupported protocol_version",
                    Some(json!({ "got": v })),
                ),
            );
        }
    }

    let sessions = st.sessions.read().await;
    let Some(s) = sessions.get(&req.session_id).cloned() else {
        return (
            StatusCode::NOT_FOUND,
            resp_err(
                "session_not_found",
                "session not found",
                Some(json!({ "session_id": req.session_id })),
            ),
        );
    };
    drop(sessions);

    let mut session = s.lock().await;
    if session.closed {
        return (
            StatusCode::GONE,
            resp_err(
                "already_closed",
                "session already closed",
                Some(json!({ "session_id": req.session_id })),
            ),
        );
    }

    session.state_version = session.state_version.saturating_add(1);

    if req.tool_name == "execute" {
        let cmd = req
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
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
                    record_audit(
                        &session,
                        cmd,
                        "require_approval",
                        &code,
                        &reason,
                        None,
                        None,
                        None,
                    );
                    let msg = format!(
                        "{reason} (interactive approval required; not supported by deepagents-acp yet)"
                    );
                    if matches!(session.mode, ExecutionMode::NonInteractive) {
                        return (
                            StatusCode::OK,
                            resp_ok(tool_result_error(
                                &code,
                                &msg,
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
                            &msg,
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
    let result = agent
        .call_tool_stateful(&tool_name, input, &mut session.state)
        .await;
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
                    out.output
                        .get("exit_code")
                        .and_then(|v| v.as_i64())
                        .map(|v| v as i32),
                    out.output.get("truncated").and_then(|v| v.as_bool()),
                    Some(duration_ms),
                );
            }
            (
                StatusCode::OK,
                resp_ok(json!({
                    "output": out.output,
                    "content_blocks": out.content_blocks,
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
                record_audit(
                    &session,
                    cmd,
                    "allow",
                    "allow",
                    "allowed but failed",
                    None,
                    None,
                    Some(duration_ms),
                );
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

async fn run_session(
    State(st): State<AppState>,
    Json(req): Json<RunRequest>,
) -> (StatusCode, Json<Value>) {
    let session = match prepare_run_session(&st, &req).await {
        Ok(session) => session,
        Err(err) => return err,
    };
    let (state_version, out) = execute_run_once(&session).await;
    let provider_info = session.lock().await.provider_info.clone();
    (
        StatusCode::OK,
        resp_ok(
            json!({ "output": out, "state_version": state_version, "provider_info": provider_info }),
        ),
    )
}

async fn run_session_stream(
    State(st): State<AppState>,
    Json(req): Json<RunRequest>,
) -> Result<
    Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>,
    (StatusCode, Json<Value>),
> {
    let session = prepare_run_session(&st, &req).await?;

    let (tx, rx) = tokio::sync::mpsc::channel(64);
    if let Some(provider_info) = session.lock().await.provider_info.clone() {
        let _ = tx.send(SsePayload::ProviderInfo(provider_info)).await;
    }
    tokio::spawn(async move {
        let mut sink = ChannelRunEventSink { tx };
        let _ = execute_run_stream(session, &mut sink).await;
    });

    Ok(sse_from_receiver(rx))
}

async fn resume_session(
    State(st): State<AppState>,
    Json(req): Json<ResumeRequest>,
) -> (StatusCode, Json<Value>) {
    let session = match prepare_resume_session(&st, &req).await {
        Ok(session) => session,
        Err(err) => return err,
    };
    let (state_version, out) = execute_resume_once(&session, &req).await;
    let provider_info = session.lock().await.provider_info.clone();
    (
        StatusCode::OK,
        resp_ok(
            json!({ "output": out, "state_version": state_version, "provider_info": provider_info }),
        ),
    )
}

async fn resume_session_stream(
    State(st): State<AppState>,
    Json(req): Json<ResumeRequest>,
) -> Result<
    Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>,
    (StatusCode, Json<Value>),
> {
    let session = prepare_resume_session(&st, &req).await?;

    let (tx, rx) = tokio::sync::mpsc::channel(64);
    if let Some(provider_info) = session.lock().await.provider_info.clone() {
        let _ = tx.send(SsePayload::ProviderInfo(provider_info)).await;
    }
    tokio::spawn(async move {
        let mut sink = ChannelRunEventSink { tx };
        let _ = execute_resume_stream(session, req, &mut sink).await;
    });

    Ok(sse_from_receiver(rx))
}

async fn get_session_state(
    State(st): State<AppState>,
    Path(session_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    let sessions = st.sessions.read().await;
    let Some(s) = sessions.get(&session_id).cloned() else {
        return (
            StatusCode::NOT_FOUND,
            resp_err(
                "session_not_found",
                "session not found",
                Some(json!({ "session_id": session_id })),
            ),
        );
    };
    drop(sessions);

    let session = s.lock().await;
    if session.closed {
        return (
            StatusCode::GONE,
            resp_err(
                "already_closed",
                "session already closed",
                Some(json!({ "session_id": session_id })),
            ),
        );
    }
    let pending_interrupt = session
        .runner
        .as_ref()
        .and_then(|r| r.pending_interrupt().cloned());
    (
        StatusCode::OK,
        resp_ok(json!({
            "state": session.state,
            "state_version": session.state_version,
            "pending_interrupt": pending_interrupt,
            "provider_info": session.provider_info
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
                resp_err(
                    "invalid_request",
                    "unsupported protocol_version",
                    Some(json!({ "got": v })),
                ),
            );
        }
    }

    let sessions = st.sessions.read().await;
    let Some(s) = sessions.get(&req.session_id).cloned() else {
        return (
            StatusCode::NOT_FOUND,
            resp_err(
                "session_not_found",
                "session not found",
                Some(json!({ "session_id": req.session_id })),
            ),
        );
    };
    drop(sessions);

    let mut session = s.lock().await;
    let already_closed = session.closed;
    session.closed = true;
    (
        StatusCode::OK,
        resp_ok(json!({ "already_closed": already_closed })),
    )
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

fn validate_protocol(version: Option<&str>) -> Result<(), (StatusCode, Json<Value>)> {
    if let Some(v) = version {
        if v != PROTOCOL_VERSION {
            return Err((
                StatusCode::BAD_REQUEST,
                resp_err(
                    "invalid_request",
                    "unsupported protocol_version",
                    Some(json!({ "got": v })),
                ),
            ));
        }
    }
    Ok(())
}

async fn prepare_run_session(
    st: &AppState,
    req: &RunRequest,
) -> Result<Arc<tokio::sync::Mutex<Session>>, (StatusCode, Json<Value>)> {
    validate_protocol(req.protocol_version.as_deref())?;
    let session = get_open_session(st, &req.session_id).await?;
    let mut session_guard = session.lock().await;
    ensure_runner_initialized(&mut session_guard, req)?;
    session_guard.state_version = session_guard.state_version.saturating_add(1);
    let runner = session_guard.runner.as_mut().unwrap();
    if runner.pending_interrupt().is_none() {
        runner.push_user_input(req.input.clone());
    }
    drop(session_guard);
    Ok(session)
}

async fn prepare_resume_session(
    st: &AppState,
    req: &ResumeRequest,
) -> Result<Arc<tokio::sync::Mutex<Session>>, (StatusCode, Json<Value>)> {
    validate_protocol(req.protocol_version.as_deref())?;
    let session = get_open_session(st, &req.session_id).await?;
    let mut session_guard = session.lock().await;
    session_guard.state_version = session_guard.state_version.saturating_add(1);
    if session_guard.runner.is_none() {
        let out = interrupt_not_found_output(&session_guard, "runner not initialized");
        return Err((
            StatusCode::OK,
            resp_ok(json!({
                "output": out,
                "state_version": session_guard.state_version
            })),
        ));
    }
    drop(session_guard);
    Ok(session)
}

async fn execute_run_once(
    session: &Arc<tokio::sync::Mutex<Session>>,
) -> (u64, deepagents::runtime::RunOutput) {
    let mut session_guard = session.lock().await;
    let out = session_guard.runner.as_mut().unwrap().run().await;
    session_guard.state = out.state.clone();
    (session_guard.state_version, out)
}

async fn execute_run_stream(
    session: Arc<tokio::sync::Mutex<Session>>,
    sink: &mut dyn deepagents::runtime::RunEventSink,
) -> (u64, deepagents::runtime::RunOutput) {
    let mut session_guard = session.lock().await;
    let out = session_guard
        .runner
        .as_mut()
        .unwrap()
        .run_with_events(sink)
        .await;
    session_guard.state = out.state.clone();
    (session_guard.state_version, out)
}

async fn execute_resume_once(
    session: &Arc<tokio::sync::Mutex<Session>>,
    req: &ResumeRequest,
) -> (u64, deepagents::runtime::RunOutput) {
    let mut session_guard = session.lock().await;
    let out = session_guard
        .runner
        .as_mut()
        .unwrap()
        .resume(&req.interrupt_id, req.decision.clone())
        .await;
    session_guard.state = out.state.clone();
    (session_guard.state_version, out)
}

async fn execute_resume_stream(
    session: Arc<tokio::sync::Mutex<Session>>,
    req: ResumeRequest,
    sink: &mut dyn deepagents::runtime::RunEventSink,
) -> (u64, deepagents::runtime::RunOutput) {
    let mut session_guard = session.lock().await;
    let out = session_guard
        .runner
        .as_mut()
        .unwrap()
        .resume_with_events(&req.interrupt_id, req.decision, sink)
        .await;
    session_guard.state = out.state.clone();
    (session_guard.state_version, out)
}

fn interrupt_not_found_output(session: &Session, message: &str) -> deepagents::runtime::RunOutput {
    deepagents::runtime::RunOutput {
        status: deepagents::runtime::RunStatus::Error,
        interrupts: Vec::new(),
        final_text: String::new(),
        tool_calls: Vec::new(),
        tool_results: Vec::new(),
        state: session.state.clone(),
        error: Some(deepagents::runtime::RuntimeError {
            code: "interrupt_not_found".to_string(),
            message: message.to_string(),
        }),
        structured_output: None,
        summarization_events: None,
        trace: None,
    }
}

async fn get_open_session(
    st: &AppState,
    session_id: &str,
) -> Result<Arc<tokio::sync::Mutex<Session>>, (StatusCode, Json<Value>)> {
    let sessions = st.sessions.read().await;
    let Some(session) = sessions.get(session_id).cloned() else {
        return Err((
            StatusCode::NOT_FOUND,
            resp_err(
                "session_not_found",
                "session not found",
                Some(json!({ "session_id": session_id })),
            ),
        ));
    };
    drop(sessions);

    if session.lock().await.closed {
        return Err((
            StatusCode::GONE,
            resp_err(
                "already_closed",
                "session already closed",
                Some(json!({ "session_id": session_id })),
            ),
        ));
    }

    Ok(session)
}

fn ensure_runner_initialized(
    session: &mut Session,
    req: &RunRequest,
) -> Result<(), (StatusCode, Json<Value>)> {
    if session.runner.is_some() {
        return Ok(());
    }

    let provider_bundle = build_provider_bundle(req)?;
    if let Some(spec) = req.structured_output.as_ref() {
        if let Err(error) = spec.validate() {
            return Err((
                StatusCode::BAD_REQUEST,
                resp_err(
                    "invalid_request",
                    "invalid structured_output",
                    Some(json!({ "error": error.to_string() })),
                ),
            ));
        }
    }
    let tool_choice = req.tool_choice.clone().unwrap_or_default();
    ensure_provider_request_supported(
        &provider_bundle.diagnostics,
        &tool_choice,
        req.structured_output.as_ref(),
    )?;
    let provider = provider_bundle.provider;
    session.provider_info = Some(provider_bundle.diagnostics);

    let subagent_registry = match deepagents::subagents::builtins::default_registry() {
        Ok(v) => v,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                resp_err(
                    "internal_error",
                    "failed to initialize subagent registry",
                    Some(json!({ "error": e.to_string() })),
                ),
            ))
        }
    };

    let subagent_mw: Arc<dyn deepagents::runtime::RuntimeMiddleware> = Arc::new(
        deepagents::subagents::SubAgentMiddleware::new(subagent_registry),
    );
    let patch_mw: Arc<dyn deepagents::runtime::RuntimeMiddleware> =
        Arc::new(deepagents::runtime::patch_tool_calls::PatchToolCallsMiddleware::new());

    let mut asm = deepagents::runtime::RuntimeMiddlewareAssembler::new();
    asm.push(
        deepagents::runtime::RuntimeMiddlewareSlot::TodoList,
        "todolist",
        Arc::new(deepagents::runtime::TodoListMiddleware::new()),
    );

    if !req.memory_disable.unwrap_or(false) {
        let sources = vec![".deepagents/AGENTS.md".to_string(), "AGENTS.md".to_string()];
        let options = deepagents::runtime::MemoryLoadOptions::default();
        let memory_mw: Arc<dyn deepagents::runtime::RuntimeMiddleware> = Arc::new(
            deepagents::runtime::MemoryMiddleware::new(session.root.clone(), sources, options),
        );
        asm.push(
            deepagents::runtime::RuntimeMiddlewareSlot::Memory,
            "memory",
            memory_mw,
        );
    }

    asm.push(
        deepagents::runtime::RuntimeMiddlewareSlot::FilesystemRuntime,
        "filesystem_runtime",
        Arc::new(deepagents::runtime::FilesystemRuntimeMiddleware::new(
            deepagents::runtime::FilesystemRuntimeOptions::default(),
        )),
    );
    asm.push(
        deepagents::runtime::RuntimeMiddlewareSlot::Subagents,
        "subagents",
        subagent_mw,
    );

    if !req.summarization_disable.unwrap_or(false) {
        let options = deepagents::runtime::SummarizationOptions {
            policy: deepagents::runtime::SummarizationPolicyKind::Budget,
            ..Default::default()
        };
        let summarization_mw: Arc<dyn deepagents::runtime::RuntimeMiddleware> = Arc::new(
            deepagents::runtime::SummarizationMiddleware::new(session.root.clone(), options),
        );
        asm.push(
            deepagents::runtime::RuntimeMiddlewareSlot::Summarization,
            "summarization",
            summarization_mw,
        );
    }

    asm.push(
        deepagents::runtime::RuntimeMiddlewareSlot::PromptCaching,
        "prompt_caching",
        Arc::new(deepagents::runtime::PromptCachingMiddleware::disabled()),
    );
    asm.push(
        deepagents::runtime::RuntimeMiddlewareSlot::PatchToolCalls,
        "patch_tool_calls",
        patch_mw,
    );

    let runtime_middlewares = match asm.build() {
        Ok(v) => v,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                resp_err(
                    "internal_error",
                    "failed to assemble runtime_middlewares",
                    Some(json!({ "error": e.to_string() })),
                ),
            ))
        }
    };

    let mut interrupt_on = std::collections::BTreeMap::new();
    for k in ["write_file", "edit_file", "delete_file", "execute"] {
        interrupt_on.insert(k.to_string(), true);
    }

    let mut runner = deepagents::runtime::ResumableRunner::new(
        session.agent.clone(),
        provider,
        vec![],
        deepagents::runtime::ResumableRunnerOptions {
            config: deepagents::runtime::RuntimeConfig {
                max_steps: req.max_steps.unwrap_or(8),
                provider_timeout_ms: req.provider_timeout_ms.unwrap_or(1000),
            },
            approval: Some(session.approval.clone()),
            audit: session.audit.clone(),
            root: session.root.clone(),
            mode: session.mode,
            interrupt_on,
        },
    )
    .with_runtime_middlewares(runtime_middlewares)
    .with_initial_state(session.state.clone())
    .with_tool_choice(tool_choice);
    if let Some(structured_output) = req.structured_output.clone() {
        runner = runner.with_structured_output(structured_output);
    }

    session.runner = Some(runner);

    Ok(())
}

fn parse_mock_script(mock_script: Option<Value>) -> Result<MockScript, (StatusCode, Json<Value>)> {
    let Some(mock_script) = mock_script else {
        return Err((
            StatusCode::BAD_REQUEST,
            resp_err(
                "invalid_request",
                "missing mock_script for mock provider",
                None,
            ),
        ));
    };
    serde_json::from_value(mock_script).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            resp_err(
                "invalid_request",
                "invalid mock_script",
                Some(json!({ "error": e.to_string() })),
            ),
        )
    })
}

fn build_provider_bundle(
    req: &RunRequest,
) -> Result<deepagents::provider::ProviderInitBundle, (StatusCode, Json<Value>)> {
    match req.provider.as_str() {
        "mock" => {
            let script = parse_mock_script(req.mock_script.clone())?;
            Ok(deepagents::provider::build_provider_bundle(
                req.provider.clone(),
                deepagents::provider::ProviderInitSpec::Mock {
                    script,
                    omit_call_ids: false,
                },
            ))
        }
        "mock2" => {
            let script = parse_mock_script(req.mock_script.clone())?;
            Ok(deepagents::provider::build_provider_bundle(
                req.provider.clone(),
                deepagents::provider::ProviderInitSpec::Mock {
                    script,
                    omit_call_ids: true,
                },
            ))
        }
        "openai-compatible" | "openai_compatible" => {
            let model = req.model.clone().ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    resp_err(
                        "invalid_request",
                        "missing model for openai-compatible provider",
                        None,
                    ),
                )
            })?;
            let mut config = deepagents::provider::OpenAiCompatibleConfig::new(model);
            if let Some(base_url) = &req.base_url {
                config = config.with_base_url(base_url.clone());
            }
            let api_key = match (&req.api_key, &req.api_key_env) {
                (Some(api_key), _) => Some(api_key.clone()),
                (None, Some(env_name)) => Some(std::env::var(env_name).map_err(|_| {
                    (
                        StatusCode::BAD_REQUEST,
                        resp_err(
                            "invalid_request",
                            "missing env var for api_key_env",
                            Some(json!({ "env": env_name })),
                        ),
                    )
                })?),
                (None, None) => std::env::var("OPENAI_API_KEY").ok(),
            };
            if let Some(api_key) = api_key {
                config = config.with_api_key(api_key);
            }
            Ok(deepagents::provider::build_provider_bundle(
                req.provider.clone(),
                deepagents::provider::ProviderInitSpec::OpenAiCompatible { config },
            ))
        }
        other => Err((
            StatusCode::BAD_REQUEST,
            resp_err(
                "invalid_request",
                "unknown provider",
                Some(json!({ "provider": other })),
            ),
        )),
    }
}

fn ensure_provider_request_supported(
    diagnostics: &deepagents::provider::ProviderDiagnostics,
    tool_choice: &deepagents::provider::ToolChoice,
    structured_output: Option<&deepagents::provider::StructuredOutputSpec>,
) -> Result<(), (StatusCode, Json<Value>)> {
    if structured_output.is_some() && !diagnostics.supports_structured_output() {
        return Err((
            StatusCode::BAD_REQUEST,
            resp_err(
                "invalid_request",
                "provider does not support structured_output",
                Some(json!({ "provider": diagnostics.provider_id.as_str() })),
            ),
        ));
    }

    if matches!(
        tool_choice,
        deepagents::provider::ToolChoice::Required | deepagents::provider::ToolChoice::Named { .. }
    ) && !diagnostics.supports_tool_choice()
    {
        return Err((
            StatusCode::BAD_REQUEST,
            resp_err(
                "invalid_request",
                "provider does not support tool_choice",
                Some(json!({ "provider": diagnostics.provider_id.as_str() })),
            ),
        ));
    }

    Ok(())
}

fn sse_from_receiver(
    rx: tokio::sync::mpsc::Receiver<SsePayload>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let stream = ReceiverStream::new(rx).map(|payload| {
        let (event_name, json) = match payload {
            SsePayload::ProviderInfo(provider_info) => {
                ("provider_info", serde_json::to_string(&provider_info))
            }
            SsePayload::RunEvent(event) => ("run_event", serde_json::to_string(&event)),
        };
        let data = json.unwrap_or_else(|_| "{\"type\":\"serialization_error\"}".to_string());
        Ok(Event::default().event(event_name).data(data))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

enum SsePayload {
    ProviderInfo(deepagents::provider::ProviderDiagnostics),
    RunEvent(deepagents::runtime::RunEvent),
}

struct ChannelRunEventSink {
    tx: tokio::sync::mpsc::Sender<SsePayload>,
}

#[async_trait::async_trait]
impl deepagents::runtime::RunEventSink for ChannelRunEventSink {
    async fn emit(&mut self, event: deepagents::runtime::RunEvent) -> anyhow::Result<()> {
        self.tx
            .send(SsePayload::RunEvent(event))
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
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
