use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::runtime::{HandledToolCall, RuntimeMiddleware, ToolCallContext};
use crate::state::{AgentState, TodoItem};
use crate::types::Message;
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct TodoListMiddleware;

impl TodoListMiddleware {
    pub fn new() -> Self {
        Self
    }
}

fn bump(state: &mut AgentState, key: &str) {
    let next = state
        .extra
        .get(key)
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        .saturating_add(1);
    state.extra.insert(key.to_string(), Value::from(next));
}

#[derive(Debug, Clone, serde::Deserialize)]
struct WriteTodosInput {
    #[serde(default)]
    pub todos: Vec<WriteTodoPatch>,
    #[serde(default)]
    pub merge: bool,
    #[serde(default)]
    pub summary: Option<String>,
}

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
        bump(state, "_mw_todolist_before_run");
        Ok(messages)
    }

    async fn before_provider_step(
        &self,
        messages: Vec<Message>,
        state: &mut AgentState,
    ) -> Result<Vec<Message>> {
        bump(state, "_mw_todolist_before_provider_step");
        Ok(messages)
    }

    async fn handle_tool_call(
        &self,
        ctx: &mut ToolCallContext<'_>,
    ) -> Result<Option<HandledToolCall>> {
        if ctx.tool_call.tool_name != "write_todos" {
            return Ok(None);
        }
        let input: WriteTodosInput = match serde_json::from_value(ctx.tool_call.arguments.clone()) {
            Ok(v) => v,
            Err(e) => {
                return Ok(Some(HandledToolCall {
                    output: Value::Null,
                    error: Some(format!("invalid_tool_call: {e}")),
                }))
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
