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
use deepagents::config::{
    ConfigKey, ConfigManager, ConfigOverrides, ConfigScope, ConfigValue, EffectiveConfig,
    PromptCacheBackendKind,
};
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
    tool_choice: Option<deepagents::llm::ToolChoice>,
    #[serde(default)]
    structured_output: Option<deepagents::llm::StructuredOutputSpec>,
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigListRequest {
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigGetRequest {
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigSetRequest {
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    key: String,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigUnsetRequest {
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigDoctorRequest {
    #[serde(default)]
    root: Option<String>,
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
        .route("/config/schema", get(config_schema))
        .route("/config/list", post(config_list))
        .route("/config/get", post(config_get))
        .route("/config/set", post(config_set))
        .route("/config/unset", post(config_unset))
        .route("/config/doctor", post(config_doctor))
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

async fn config_schema() -> (StatusCode, Json<Value>) {
    let manager = match ConfigManager::new(".") {
        Ok(manager) => manager,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                resp_err(err.code, &err.message, None),
            )
        }
    };
    match serde_json::to_value(manager.schema()) {
        Ok(value) => (StatusCode::OK, resp_ok(value)),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            resp_err("internal_error", &err.to_string(), None),
        ),
    }
}

async fn config_list(Json(req): Json<ConfigListRequest>) -> (StatusCode, Json<Value>) {
    let manager = match config_manager_from_root(req.root.as_deref()) {
        Ok(manager) => manager,
        Err(err) => return err,
    };
    let scope = match parse_config_scope(req.scope.as_deref(), ConfigScope::Effective) {
        Ok(scope) => scope,
        Err(err) => return err,
    };
    match manager.list(scope, &ConfigOverrides::new()) {
        Ok(entries) => (
            StatusCode::OK,
            resp_ok(json!({ "scope": scope, "entries": entries })),
        ),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            resp_err(err.code, &err.message, None),
        ),
    }
}

async fn config_get(Json(req): Json<ConfigGetRequest>) -> (StatusCode, Json<Value>) {
    let manager = match config_manager_from_root(req.root.as_deref()) {
        Ok(manager) => manager,
        Err(err) => return err,
    };
    let scope = match parse_config_scope(req.scope.as_deref(), ConfigScope::Effective) {
        Ok(scope) => scope,
        Err(err) => return err,
    };
    let key = match ConfigKey::parse(req.key) {
        Ok(key) => key,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                resp_err(err.code, &err.message, None),
            )
        }
    };
    match manager.get(scope, &key, &ConfigOverrides::new()) {
        Ok(value) => match serde_json::to_value(value) {
            Ok(value) => (StatusCode::OK, resp_ok(value)),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                resp_err("internal_error", &err.to_string(), None),
            ),
        },
        Err(err) => (
            StatusCode::BAD_REQUEST,
            resp_err(err.code, &err.message, None),
        ),
    }
}

async fn config_set(Json(req): Json<ConfigSetRequest>) -> (StatusCode, Json<Value>) {
    let manager = match config_manager_from_root(req.root.as_deref()) {
        Ok(manager) => manager,
        Err(err) => return err,
    };
    let scope = match parse_config_scope(req.scope.as_deref(), ConfigScope::Workspace) {
        Ok(scope) => scope,
        Err(err) => return err,
    };
    let key = match ConfigKey::parse(req.key) {
        Ok(key) => key,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                resp_err(err.code, &err.message, None),
            )
        }
    };
    let value = match manager.parse_json_value(&key, req.value) {
        Ok(value) => value,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                resp_err(err.code, &err.message, None),
            )
        }
    };
    match manager.set(scope, &key, value) {
        Ok(()) => (
            StatusCode::OK,
            resp_ok(json!({ "status": "ok", "scope": scope, "key": key })),
        ),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            resp_err(err.code, &err.message, None),
        ),
    }
}

async fn config_unset(Json(req): Json<ConfigUnsetRequest>) -> (StatusCode, Json<Value>) {
    let manager = match config_manager_from_root(req.root.as_deref()) {
        Ok(manager) => manager,
        Err(err) => return err,
    };
    let scope = match parse_config_scope(req.scope.as_deref(), ConfigScope::Workspace) {
        Ok(scope) => scope,
        Err(err) => return err,
    };
    let key = match ConfigKey::parse(req.key) {
        Ok(key) => key,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                resp_err(err.code, &err.message, None),
            )
        }
    };
    match manager.unset(scope, &key) {
        Ok(()) => (
            StatusCode::OK,
            resp_ok(json!({ "status": "ok", "scope": scope, "key": key })),
        ),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            resp_err(err.code, &err.message, None),
        ),
    }
}

async fn config_doctor(Json(req): Json<ConfigDoctorRequest>) -> (StatusCode, Json<Value>) {
    let manager = match config_manager_from_root(req.root.as_deref()) {
        Ok(manager) => manager,
        Err(err) => return err,
    };
    match manager.doctor(&ConfigOverrides::new()) {
        Ok(report) => match serde_json::to_value(report) {
            Ok(value) => (StatusCode::OK, resp_ok(value)),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                resp_err("internal_error", &err.to_string(), None),
            ),
        },
        Err(err) => (
            StatusCode::BAD_REQUEST,
            resp_err(err.code, &err.message, None),
        ),
    }
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

    let config_manager = match ConfigManager::new(req.root.clone()) {
        Ok(manager) => manager,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                resp_err(err.code, &err.message, None),
            )
        }
    };
    let overrides = match build_session_overrides(&req) {
        Ok(overrides) => overrides,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                resp_err(err.code, &err.message, None),
            )
        }
    };
    let effective = match config_manager.resolve_effective(&overrides) {
        Ok(effective) => effective,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                resp_err(err.code, &err.message, None),
            )
        }
    };
    let mode = effective.security.execution_mode;
    let allow_list = effective.security.shell_allow_list.clone();

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
    let audit: Option<Arc<dyn AuditSink>> =
        build_audit_sink(&config_manager, effective.audit.jsonl_path.as_deref());

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
            let (code, msg) = classify_anyhow_tool_error(&e);
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

fn parse_config_scope(
    scope: Option<&str>,
    default: ConfigScope,
) -> Result<ConfigScope, (StatusCode, Json<Value>)> {
    match scope {
        Some(scope) => ConfigScope::parse(scope).map_err(|err| {
            (
                StatusCode::BAD_REQUEST,
                resp_err(err.code, &err.message, None),
            )
        }),
        None => Ok(default),
    }
}

fn config_manager_from_root(
    root: Option<&str>,
) -> Result<ConfigManager, (StatusCode, Json<Value>)> {
    ConfigManager::new(root.unwrap_or(".")).map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            resp_err(err.code, &err.message, None),
        )
    })
}

fn build_session_overrides(
    req: &NewSessionRequest,
) -> Result<ConfigOverrides, deepagents::config::ConfigError> {
    let mut overrides = ConfigOverrides::new();
    if let Some(execution_mode) = req.execution_mode.as_deref() {
        let normalized = match execution_mode {
            "non-interactive" => "non_interactive",
            other => other,
        };
        insert_override(
            &mut overrides,
            "security.execution_mode",
            ConfigValue::String(normalized.to_string()),
        )?;
    }
    if let Some(shell_allow_list) = req.shell_allow_list.as_ref() {
        insert_override(
            &mut overrides,
            "security.shell_allow_list",
            ConfigValue::StringList(shell_allow_list.clone()),
        )?;
    }
    if let Some(audit_json) = req.audit_json.as_ref() {
        insert_override(
            &mut overrides,
            "audit.jsonl_path",
            ConfigValue::String(audit_json.clone()),
        )?;
    }
    Ok(overrides)
}

fn build_run_overrides(
    req: &RunRequest,
) -> Result<ConfigOverrides, deepagents::config::ConfigError> {
    let mut overrides = ConfigOverrides::new();
    let provider_id = canonical_provider_id(&req.provider);
    if provider_id != "mock" && provider_id != "mock2" {
        insert_override(
            &mut overrides,
            &format!("providers.{provider_id}.enabled"),
            ConfigValue::Boolean(true),
        )?;
    }
    if let Some(model) = req.model.as_ref() {
        insert_override(
            &mut overrides,
            &format!("providers.{provider_id}.model"),
            ConfigValue::String(model.clone()),
        )?;
    }
    if let Some(base_url) = req.base_url.as_ref() {
        insert_override(
            &mut overrides,
            &format!("providers.{provider_id}.base_url"),
            ConfigValue::String(base_url.clone()),
        )?;
    }
    if let Some(api_key_env) = req.api_key_env.as_ref() {
        insert_override(
            &mut overrides,
            &format!("providers.{provider_id}.api_key_env"),
            ConfigValue::String(api_key_env.clone()),
        )?;
    }
    if let Some(max_steps) = req.max_steps {
        insert_override(
            &mut overrides,
            "runtime.max_steps",
            ConfigValue::Integer(max_steps as i64),
        )?;
    }
    if let Some(timeout_ms) = req.provider_timeout_ms {
        insert_override(
            &mut overrides,
            "runtime.provider_timeout_ms",
            ConfigValue::Integer(timeout_ms as i64),
        )?;
    }
    if let Some(memory_disable) = req.memory_disable {
        insert_override(
            &mut overrides,
            "memory.file.enabled",
            ConfigValue::Boolean(!memory_disable),
        )?;
    }
    if let Some(summarization_disable) = req.summarization_disable {
        insert_override(
            &mut overrides,
            "runtime.summarization.enabled",
            ConfigValue::Boolean(!summarization_disable),
        )?;
    }
    Ok(overrides)
}

fn insert_override(
    overrides: &mut ConfigOverrides,
    key: &str,
    value: ConfigValue,
) -> Result<(), deepagents::config::ConfigError> {
    let key = ConfigKey::parse(key.to_string())?;
    overrides.set(key, value);
    Ok(())
}

fn build_audit_sink(
    config_manager: &ConfigManager,
    audit_path: Option<&str>,
) -> Option<Arc<dyn AuditSink>> {
    audit_path.map(|path| {
        let path = config_manager.resolve_path(path);
        let path_string = path.to_string_lossy().into_owned();
        Arc::new(JsonlFileAuditSink::new(&path_string)) as Arc<dyn AuditSink>
    })
}

fn canonical_provider_id(provider_id: &str) -> &str {
    match provider_id {
        "openai_compatible" => "openai-compatible",
        other => other,
    }
}

fn resolve_provider_api_key(
    direct_api_key: Option<String>,
    explicit_env_var: Option<&str>,
    configured_env_var: Option<&str>,
) -> Result<Option<String>, (StatusCode, Json<Value>)> {
    if direct_api_key.is_some() {
        return Ok(direct_api_key);
    }
    if let Some(env_var) = explicit_env_var {
        return match std::env::var(env_var) {
            Ok(value) => Ok(Some(value)),
            Err(_) => Err((
                StatusCode::BAD_REQUEST,
                resp_err(
                    "invalid_request",
                    "missing env var for api_key_env",
                    Some(json!({ "env": env_var })),
                ),
            )),
        };
    }
    let Some(env_var) = configured_env_var else {
        return Ok(None);
    };
    Ok(std::env::var(env_var).ok())
}

fn classify_anyhow_tool_error(e: &anyhow::Error) -> (String, String) {
    if let Some(be) = e.downcast_ref::<deepagents::backends::protocol::BackendError>() {
        return (be.code_str().to_string(), be.message.clone());
    }
    if let Some(me) = e.downcast_ref::<deepagents::memory::protocol::MemoryError>() {
        return (me.code.to_string(), me.message.clone());
    }
    let s = e.to_string();
    if s.chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
    {
        return (s.clone(), s);
    }
    if let Some(rest) = s.strip_prefix("command_not_allowed: ") {
        if let Some((code, _)) = rest.split_once(':') {
            return (code.trim().to_string(), s);
        }
        return ("command_not_allowed".to_string(), s);
    }
    if let Some((code, _)) = s.split_once(':') {
        if code
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
        {
            return (code.trim().to_string(), s);
        }
    }
    ("unknown".to_string(), s)
}

#[allow(clippy::too_many_arguments)]
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

    let config_manager = config_manager_from_root(Some(&session.root))?;
    let overrides = build_run_overrides(req).map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            resp_err(err.code, &err.message, None),
        )
    })?;
    let effective = config_manager
        .resolve_effective(&overrides)
        .map_err(|err| {
            (
                StatusCode::BAD_REQUEST,
                resp_err(err.code, &err.message, None),
            )
        })?;
    let provider_bundle = build_provider_bundle(req, &effective)?;
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

    if effective.memory.enabled {
        let sources = effective.memory.sources.clone();
        let options = deepagents::runtime::MemoryLoadOptions {
            allow_host_paths: effective.memory.allow_host_paths,
            max_injected_chars: effective.memory.max_injected_chars,
            ..Default::default()
        };
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

    if effective.runtime.summarization.enabled {
        let options = deepagents::runtime::SummarizationOptions {
            policy: deepagents::runtime::SummarizationPolicyKind::Budget,
            max_char_budget: effective.runtime.summarization.max_char_budget,
            max_turns_visible: effective.runtime.summarization.max_turns_visible,
            min_recent_messages: effective.runtime.summarization.min_recent_messages,
            redact_tool_args: effective.runtime.summarization.redact_tool_args,
            max_tool_arg_chars: effective.runtime.summarization.max_tool_arg_chars,
            truncate_tool_args_keep_last: effective.runtime.summarization.truncate_keep_last,
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

    let model_id = effective
        .provider(canonical_provider_id(&req.provider))
        .and_then(|p| p.model.clone())
        .unwrap_or_default();
    asm.push(
        deepagents::runtime::RuntimeMiddlewareSlot::PromptCaching,
        "prompt_caching",
        Arc::new(deepagents::runtime::PromptCachingMiddleware::new(
            deepagents::runtime::PromptCacheOptions {
                enabled: matches!(
                    effective.runtime.prompt_cache.backend,
                    PromptCacheBackendKind::Memory
                ),
                backend: deepagents::runtime::CacheBackend::Memory,
                native: match effective.runtime.prompt_cache.native {
                    deepagents::config::PromptCacheNativeMode::Auto => {
                        deepagents::runtime::PromptCacheNativeMode::Auto
                    }
                    deepagents::config::PromptCacheNativeMode::Off => {
                        deepagents::runtime::PromptCacheNativeMode::Off
                    }
                    deepagents::config::PromptCacheNativeMode::Required => {
                        deepagents::runtime::PromptCacheNativeMode::Required
                    }
                },
                layout: match effective.runtime.prompt_cache.layout {
                    deepagents::config::PromptCacheLayoutMode::Auto => {
                        deepagents::runtime::PromptCacheLayoutMode::Auto
                    }
                    deepagents::config::PromptCacheLayoutMode::SingleSystem => {
                        deepagents::runtime::PromptCacheLayoutMode::SingleSystem
                    }
                    deepagents::config::PromptCacheLayoutMode::PreservePrefixSegments => {
                        deepagents::runtime::PromptCacheLayoutMode::PreservePrefixSegments
                    }
                },
                enable_l2_response_cache: effective.runtime.prompt_cache.l2,
                ttl_ms: effective.runtime.prompt_cache.ttl_ms,
                max_entries: effective.runtime.prompt_cache.max_entries,
                provider_id: canonical_provider_id(&req.provider).to_string(),
                model_id,
                partition: session.root.clone(),
            },
        )),
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
        deepagents::runtime::ResumableRunnerOptions {
            config: deepagents::runtime::RuntimeConfig {
                max_steps: effective.runtime.max_steps,
                provider_timeout_ms: effective.runtime.provider_timeout_ms,
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
    effective: &EffectiveConfig,
) -> Result<deepagents::provider::ProviderInitBundle, (StatusCode, Json<Value>)> {
    let provider_id = canonical_provider_id(&req.provider);
    match provider_id {
        "mock" => {
            let script = parse_mock_script(req.mock_script.clone())?;
            Ok(deepagents::provider::build_provider_bundle(
                provider_id.to_string(),
                deepagents::provider::ProviderInitSpec::Mock {
                    script,
                    omit_call_ids: false,
                },
            ))
        }
        "mock2" => {
            let script = parse_mock_script(req.mock_script.clone())?;
            Ok(deepagents::provider::build_provider_bundle(
                provider_id.to_string(),
                deepagents::provider::ProviderInitSpec::Mock {
                    script,
                    omit_call_ids: true,
                },
            ))
        }
        "openai-compatible" | "openai_compatible" => {
            let provider_cfg = effective.provider("openai-compatible").ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    resp_err(
                        "invalid_request",
                        "missing config for openai-compatible provider",
                        None,
                    ),
                )
            })?;
            let model = provider_cfg.model.clone().ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    resp_err(
                        "invalid_request",
                        "missing model for openai-compatible provider",
                        None,
                    ),
                )
            })?;
            let mut config = deepagents::llm::OpenAiCompatibleConfig::new(model);
            if let Some(base_url) = provider_cfg.base_url.clone() {
                config = config.with_base_url(base_url);
            }
            let api_key = resolve_provider_api_key(
                req.api_key.clone(),
                req.api_key_env.as_deref(),
                provider_cfg
                    .api_key_env
                    .as_ref()
                    .map(|value| value.as_str()),
            )?;
            if let Some(api_key) = api_key {
                config = config.with_api_key(api_key);
            }
            Ok(deepagents::provider::build_provider_bundle(
                provider_id.to_string(),
                deepagents::provider::ProviderInitSpec::OpenAiCompatible { config },
            ))
        }
        "openrouter" => {
            let provider_cfg = effective.provider("openrouter").ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    resp_err(
                        "invalid_request",
                        "missing config for openrouter provider",
                        None,
                    ),
                )
            })?;
            let model = provider_cfg.model.clone().ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    resp_err(
                        "invalid_request",
                        "missing model for openrouter provider",
                        None,
                    ),
                )
            })?;
            let mut config = deepagents::llm::OpenRouterConfig::new(model);
            if let Some(base_url) = provider_cfg.base_url.clone() {
                config = config.with_base_url(base_url);
            }
            let api_key = resolve_provider_api_key(
                req.api_key.clone(),
                req.api_key_env.as_deref(),
                provider_cfg
                    .api_key_env
                    .as_ref()
                    .map(|value| value.as_str()),
            )?;
            if let Some(api_key) = api_key {
                config = config.with_api_key(api_key);
            }
            Ok(deepagents::provider::build_provider_bundle(
                provider_id.to_string(),
                deepagents::provider::ProviderInitSpec::OpenRouter { config },
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
    tool_choice: &deepagents::llm::ToolChoice,
    structured_output: Option<&deepagents::llm::StructuredOutputSpec>,
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
        deepagents::llm::ToolChoice::Required | deepagents::llm::ToolChoice::Named { .. }
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
