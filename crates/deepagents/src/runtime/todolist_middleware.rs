use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::runtime::{HandledToolCall, RuntimeMiddleware, ToolCallContext};
use crate::state::{AgentState, TodoItem};
use crate::types::Message;
use serde_json::Value;

/// TodoList 相关的运行时中间件。
///
/// 作用：
/// - 作为运行时对 `write_todos` 工具调用的实现方：解析入参、校验、更新 `AgentState.todos`
/// - 维护一些轻量的调用计数（写入 `AgentState.extra`），用于诊断/观测中间件是否被触发
///
/// 数据模型：
/// - `AgentState.todos` 是当前 todo 列表的唯一来源
/// - `merge=false`：整表替换（要求每条必须提供 content/status/priority）
/// - `merge=true`：按 id 增量更新或新增
/// - `summary`：只允许在“至少一个 todo 状态从非 completed 变为 completed”时写入
#[derive(Debug, Clone, Default)]
pub struct TodoListMiddleware;

impl TodoListMiddleware {
    /// 创建中间件实例（无配置）。
    pub fn new() -> Self {
        Self
    }
}

/// 在 `state.extra[key]` 上做一个饱和自增计数，用于简单的运行时观测/调试。
fn bump(state: &mut AgentState, key: &str) {
    let next = state
        .extra
        .get(key)
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        .saturating_add(1);
    state.extra.insert(key.to_string(), Value::from(next));
}

/// `write_todos` 工具调用的输入结构。
///
/// - `todos`：todo patch 列表
/// - `merge`：是否以 merge 模式更新
/// - `summary`：仅当发生 completed 迁移时允许设置
#[derive(Debug, Clone, serde::Deserialize)]
struct WriteTodosInput {
    #[serde(default)]
    pub todos: Vec<WriteTodoPatch>,
    #[serde(default)]
    pub merge: bool,
    #[serde(default)]
    pub summary: Option<String>,
}

/// 单条 todo 的 patch 结构（用于 merge 或 replace）。
///
/// 说明：
/// - `merge=true` 时，content/status/priority 可以为 None，表示“不更新该字段”
/// - `merge=false` 时，content/status/priority 必须提供（由 `require_fields_for_replace` 保证）
#[derive(Debug, Clone, serde::Deserialize)]
struct WriteTodoPatch {
    pub id: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(rename = "activeForm", default)]
    pub active_form: Option<String>,
}

/// 校验请求中不存在重复的 todo id，并要求 id 非空。
fn validate_no_duplicate_ids(items: &[WriteTodoPatch]) -> anyhow::Result<()> {
    let mut seen: HashSet<&str> = HashSet::new();
    for t in items {
        let id = t.id.trim();
        if id.is_empty() {
            anyhow::bail!("invalid_request: todo id is required");
        }
        if !seen.insert(id) {
            anyhow::bail!("invalid_request: duplicate todo id: {id}");
        }
    }
    Ok(())
}

/// merge=false（整表替换）时的强校验：要求每条 todo 都提供必填字段。
///
/// 返回值为“可直接写入 state.todos 的完整列表”。
fn require_fields_for_replace(items: &[WriteTodoPatch]) -> anyhow::Result<Vec<TodoItem>> {
    let mut out = Vec::with_capacity(items.len());
    for t in items {
        let content = t
            .content
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("invalid_request: todo content is required for merge=false")
            })?;
        let status = t
            .status
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("invalid_request: todo status is required for merge=false")
            })?;
        let priority = t
            .priority
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("invalid_request: todo priority is required for merge=false")
            })?;
        out.push(TodoItem {
            id: t.id.trim().to_string(),
            content: content.to_string(),
            status: status.to_string(),
            priority: priority.to_string(),
            active_form: t.active_form.clone(),
        });
    }
    Ok(out)
}

/// merge=true 的应用逻辑：按 id 更新已有项或新增项。
///
/// 返回值表示：此次更新中是否发生了“状态迁移到 completed”（用于决定 summary 是否允许）。
fn apply_merge(state: &mut AgentState, items: &[WriteTodoPatch]) -> anyhow::Result<bool> {
    let mut idx: HashMap<String, usize> = HashMap::new();
    for (i, t) in state.todos.iter().enumerate() {
        idx.insert(t.id.clone(), i);
    }

    let mut completion_transition = false;
    for p in items {
        let id = p.id.trim();
        if let Some(&i) = idx.get(id) {
            let before_completed = state.todos[i].status == "completed";
            if let Some(content) = p
                .content
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                state.todos[i].content = content.to_string();
            }
            if let Some(status) = p.status.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                state.todos[i].status = status.to_string();
            }
            if let Some(priority) = p
                .priority
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                state.todos[i].priority = priority.to_string();
            }
            if let Some(active_form) = p
                .active_form
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                state.todos[i].active_form = Some(active_form.to_string());
            }
            let after_completed = state.todos[i].status == "completed";
            if !before_completed && after_completed {
                completion_transition = true;
            }
        } else {
            let content = p.content.as_deref().map(str::trim).unwrap_or_default();
            let status = p.status.as_deref().map(str::trim).unwrap_or_default();
            let priority = p.priority.as_deref().map(str::trim).unwrap_or_default();
            let after_completed = status == "completed";
            state.todos.push(TodoItem {
                id: id.to_string(),
                content: content.to_string(),
                status: status.to_string(),
                priority: priority.to_string(),
                active_form: p.active_form.clone(),
            });
            if after_completed {
                completion_transition = true;
            }
        }
    }
    Ok(completion_transition)
}

/// 执行一次 `write_todos` 请求，并返回标准化输出（包含更新后的 todos）。
///
/// 关键约束：
/// - `summary` 只能在“至少一条 todo 从非 completed -> completed”时出现
/// - 若违反该约束，会回滚 todos 修改，保证工具调用是原子的
fn apply_write_todos(state: &mut AgentState, input: WriteTodosInput) -> anyhow::Result<Value> {
    validate_no_duplicate_ids(&input.todos)?;

    let before = state.todos.clone();
    let completion_transition = if input.merge {
        apply_merge(state, &input.todos)?
    } else {
        let next = require_fields_for_replace(&input.todos)?;
        let mut completion_transition = false;
        let mut prev: HashMap<&str, &str> = HashMap::new();
        for t in before.iter() {
            prev.insert(t.id.as_str(), t.status.as_str());
        }
        for t in next.iter() {
            if prev.get(t.id.as_str()).copied().unwrap_or("pending") != "completed"
                && t.status == "completed"
            {
                completion_transition = true;
            }
        }
        state.todos = next;
        completion_transition
    };

    if let Some(summary) = input
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !completion_transition {
            // 不允许在未完成任务迁移的情况下提交 summary：回滚 todos，并报错。
            state.todos = before;
            anyhow::bail!(
                "invalid_request: summary is only allowed when a todo transitions to completed"
            );
        }
        state.extra.insert(
            "_todo_summary".to_string(),
            Value::String(summary.to_string()),
        );
    }

    Ok(serde_json::json!({ "todos": state.todos }))
}

#[async_trait::async_trait]
impl RuntimeMiddleware for TodoListMiddleware {
    async fn before_run(
        &self,
        messages: Vec<Message>,
        state: &mut AgentState,
    ) -> Result<Vec<Message>> {
        // 观测计数：中间件在一次 run 开始前是否触发。
        bump(state, "_mw_todolist_before_run");
        Ok(messages)
    }

    async fn before_provider_step(
        &self,
        messages: Vec<Message>,
        state: &mut AgentState,
    ) -> Result<Vec<Message>> {
        // 观测计数：中间件在 provider step 前是否触发。
        bump(state, "_mw_todolist_before_provider_step");
        Ok(messages)
    }

    async fn handle_tool_call(
        &self,
        ctx: &mut ToolCallContext<'_>,
    ) -> Result<Option<HandledToolCall>> {
        // 仅处理 write_todos 工具调用，其他工具交给后续中间件或默认实现。
        if ctx.tool_call.tool_name != "write_todos" {
            return Ok(None);
        }
        let input: WriteTodosInput = match serde_json::from_value(ctx.tool_call.arguments.clone()) {
            Ok(v) => v,
            Err(e) => {
                // 入参不是预期 JSON 结构：返回可读错误，并保持工具输出为 Null。
                return Ok(Some(HandledToolCall {
                    output: Value::Null,
                    error: Some(format!("invalid_tool_call: {e}")),
                }));
            }
        };
        match apply_write_todos(ctx.state, input) {
            Ok(out) => Ok(Some(HandledToolCall {
                output: out,
                error: None,
            })),
            Err(e) => Ok(Some(HandledToolCall {
                output: Value::Null,
                error: Some(e.to_string()),
            })),
        }
    }
}
