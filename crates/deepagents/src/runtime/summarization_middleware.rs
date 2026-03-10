use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::runtime::{HandledToolCall, RuntimeMiddleware, ToolCallContext};
use crate::state::AgentState;
use crate::types::Message;

const EVENT_KEY: &str = "_summarization_event";
const EVENTS_KEY: &str = "_summarization_events";
const FORCE_KEY: &str = "_summarization_force";
const SUMMARY_NAME: &str = "summarization";
const SUMMARY_MARKER: &str = "SUMMARY_MESSAGE_V1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummarizationEvent {
    pub cutoff_index: usize,
    pub summary_message: Message,
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SummarizationPolicyKind {
    Budget,
    Turns,
    Importance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummarizationOptions {
    pub enabled: bool,
    pub policy: SummarizationPolicyKind,
    pub max_char_budget: usize,
    pub max_turns_visible: usize,
    pub min_recent_messages: usize,
    pub history_path_prefix: String,
    pub redact_tool_args: bool,
    pub max_tool_arg_chars: usize,
    pub truncate_tool_args_keep_last: usize,
    pub truncation_text: String,
    pub compact_min_ratio: f32,
    pub max_summary_chars: usize,
}

impl Default for SummarizationOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            policy: SummarizationPolicyKind::Budget,
            max_char_budget: 12_000,
            max_turns_visible: 12,
            min_recent_messages: 3,
            history_path_prefix: "/conversation_history".to_string(),
            redact_tool_args: true,
            max_tool_arg_chars: 2_000,
            truncate_tool_args_keep_last: 6,
            truncation_text: "...(truncated)...".to_string(),
            compact_min_ratio: 0.5,
            max_summary_chars: 1_200,
        }
    }
}

#[async_trait::async_trait]
pub trait SummarizationStore: Send + Sync {
    async fn persist(&self, thread_id: &str, content: &str) -> Result<Option<String>>;
}

#[derive(Debug, Clone)]
pub struct FilesystemSummarizationStore {
    root: PathBuf,
    prefix: String,
}

impl FilesystemSummarizationStore {
    pub fn new(root: impl Into<PathBuf>, prefix: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            prefix: prefix.into(),
        }
    }
}

#[async_trait::async_trait]
impl SummarizationStore for FilesystemSummarizationStore {
    async fn persist(&self, thread_id: &str, content: &str) -> Result<Option<String>> {
        let virtual_path = format!("{}/{}.md", self.prefix.trim_end_matches('/'), thread_id);
        let rel = virtual_path.trim_start_matches('/');
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return Ok(None);
            }
        }
        let mut file = match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => f,
            Err(_) => return Ok(None),
        };
        if std::io::Write::write_all(&mut file, content.as_bytes()).is_err() {
            return Ok(None);
        }
        Ok(Some(virtual_path))
    }
}

#[derive(Clone)]
pub struct SummarizationMiddleware {
    options: SummarizationOptions,
    store: Arc<dyn SummarizationStore>,
}

impl SummarizationMiddleware {
    pub fn new(root: impl Into<PathBuf>, options: SummarizationOptions) -> Self {
        let prefix = options.history_path_prefix.clone();
        let store = Arc::new(FilesystemSummarizationStore::new(root, prefix));
        Self { options, store }
    }

    pub fn with_store(mut self, store: Arc<dyn SummarizationStore>) -> Self {
        self.store = store;
        self
    }
}

#[async_trait::async_trait]
impl RuntimeMiddleware for SummarizationMiddleware {
    async fn before_provider_step(&self, messages: Vec<Message>, state: &mut AgentState) -> Result<Vec<Message>> {
        let (mut effective_messages, prior_event) = apply_prior_event(&messages, state);
        if self.options.redact_tool_args {
            effective_messages = truncate_tool_args(
                effective_messages,
                self.options.truncate_tool_args_keep_last,
                self.options.max_tool_arg_chars,
                &self.options.truncation_text,
            );
        }

        let force = state
            .extra
            .get(FORCE_KEY)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !self.options.enabled && !force {
            return Ok(effective_messages);
        }

        if !force && !should_summarize(&effective_messages, &self.options) {
            return Ok(effective_messages);
        }

        let prior_cutoff = prior_event.as_ref().map(|e| e.cutoff_index).unwrap_or(0);
        let effective_cutoff = compute_cutoff(&effective_messages, &self.options);
        let state_cutoff = if prior_event.is_some() {
            if effective_cutoff == 0 {
                0
            } else {
                prior_cutoff + effective_cutoff.saturating_sub(1)
            }
        } else {
            effective_cutoff
        };

        if state_cutoff == 0 || state_cutoff >= messages.len() {
            return Ok(effective_messages);
        }

        let pruned_messages = messages
            .iter()
            .skip(prior_cutoff)
            .take(state_cutoff.saturating_sub(prior_cutoff))
            .filter(|&m| !is_summary_message(m))
            .cloned()
            .collect::<Vec<_>>();
        let summary_message = build_summary_message(&pruned_messages, self.options.max_summary_chars);
        let thread_id = ensure_thread_id(state);
        let section = build_history_section(&pruned_messages, &summary_message, state_cutoff, &self.options);
        let file_path = self.store.persist(&thread_id, &section).await?;
        let event = SummarizationEvent {
            cutoff_index: state_cutoff,
            summary_message: summary_message.clone(),
            file_path,
        };
        store_event(state, &event)?;

        let mut next_effective = Vec::with_capacity(1 + messages.len().saturating_sub(state_cutoff));
        next_effective.push(summary_message);
        next_effective.extend(messages.iter().skip(state_cutoff).cloned());
        if self.options.redact_tool_args {
            next_effective = truncate_tool_args(
                next_effective,
                self.options.truncate_tool_args_keep_last,
                self.options.max_tool_arg_chars,
                &self.options.truncation_text,
            );
        }
        Ok(next_effective)
    }

    async fn handle_tool_call(&self, ctx: &mut ToolCallContext<'_>) -> Result<Option<HandledToolCall>> {
        if ctx.tool_call.tool_name != "compact_conversation" {
            return Ok(None);
        }
        let (mut effective_messages, prior_event) = apply_prior_event(ctx.messages, ctx.state);
        if self.options.redact_tool_args {
            effective_messages = truncate_tool_args(
                effective_messages,
                self.options.truncate_tool_args_keep_last,
                self.options.max_tool_arg_chars,
                &self.options.truncation_text,
            );
        }
        let total_chars = total_chars(&effective_messages);
        let min_chars = (self.options.max_char_budget as f32 * self.options.compact_min_ratio) as usize;
        if total_chars < min_chars {
            return Ok(Some(HandledToolCall {
                output: serde_json::json!({
                    "content": "conversation not large enough to compact",
                    "skipped": true
                }),
                error: None,
            }));
        }

        let prior_cutoff = prior_event.as_ref().map(|e| e.cutoff_index).unwrap_or(0);
        let effective_cutoff = compute_cutoff(&effective_messages, &self.options);
        let state_cutoff = if prior_event.is_some() {
            if effective_cutoff == 0 {
                0
            } else {
                prior_cutoff + effective_cutoff.saturating_sub(1)
            }
        } else {
            effective_cutoff
        };
        if state_cutoff == 0 || state_cutoff >= ctx.messages.len() {
            return Ok(Some(HandledToolCall {
                output: serde_json::json!({
                    "content": "conversation not large enough to compact",
                    "skipped": true
                }),
                error: None,
            }));
        }

        let pruned_messages = ctx
            .messages
            .iter()
            .skip(prior_cutoff)
            .take(state_cutoff.saturating_sub(prior_cutoff))
            .filter(|&m| !is_summary_message(m))
            .cloned()
            .collect::<Vec<_>>();
        let summary_message = build_summary_message(&pruned_messages, self.options.max_summary_chars);
        let thread_id = ensure_thread_id(ctx.state);
        let section = build_history_section(&pruned_messages, &summary_message, state_cutoff, &self.options);
        let file_path = self.store.persist(&thread_id, &section).await?;
        let event = SummarizationEvent {
            cutoff_index: state_cutoff,
            summary_message: summary_message.clone(),
            file_path,
        };
        store_event(ctx.state, &event)?;

        Ok(Some(HandledToolCall {
            output: serde_json::json!({
                "content": "conversation compacted",
                "cutoff_index": state_cutoff,
                "file_path": event.file_path,
                "summary_message": event.summary_message,
            }),
            error: None,
        }))
    }
}

fn apply_prior_event(messages: &[Message], state: &AgentState) -> (Vec<Message>, Option<SummarizationEvent>) {
    if let Some(event) = load_event(state) {
        let mut effective = Vec::with_capacity(1 + messages.len().saturating_sub(event.cutoff_index));
        effective.push(event.summary_message.clone());
        effective.extend(messages.iter().skip(event.cutoff_index).cloned());
        return (effective, Some(event));
    }
    (messages.to_vec(), None)
}

fn load_event(state: &AgentState) -> Option<SummarizationEvent> {
    let value = state.extra.get(EVENT_KEY)?;
    serde_json::from_value::<SummarizationEvent>(value.clone()).ok()
}

fn store_event(state: &mut AgentState, event: &SummarizationEvent) -> Result<()> {
    state
        .extra
        .insert(EVENT_KEY.to_string(), serde_json::to_value(event)?);
    let mut events = match state.extra.get(EVENTS_KEY) {
        Some(v) => serde_json::from_value::<Vec<SummarizationEvent>>(v.clone()).unwrap_or_default(),
        None => Vec::new(),
    };
    events.push(event.clone());
    state.extra.insert(EVENTS_KEY.to_string(), serde_json::to_value(events)?);
    Ok(())
}

fn ensure_thread_id(state: &mut AgentState) -> String {
    if let Some(v) = state.extra.get("thread_id").and_then(|v| v.as_str()) {
        return v.to_string();
    }
    let id = format!("session_{}", Utc::now().timestamp_millis());
    state
        .extra
        .insert("thread_id".to_string(), serde_json::Value::String(id.clone()));
    id
}

fn is_summary_message(msg: &Message) -> bool {
    msg.name.as_deref() == Some(SUMMARY_NAME) || msg.content.contains(SUMMARY_MARKER)
}

fn build_summary_message(messages: &[Message], max_chars: usize) -> Message {
    let mut lines = Vec::new();
    lines.push(SUMMARY_MARKER.to_string());
    lines.push(format!("summary_messages={}", messages.len()));
    for m in messages.iter().take(4) {
        lines.push(format!("{}: {}", m.role, preview(&m.content, 80)));
    }
    if messages.len() > 6 {
        lines.push("...".to_string());
    }
    for m in messages.iter().rev().take(2).collect::<Vec<_>>().into_iter().rev() {
        lines.push(format!("{}: {}", m.role, preview(&m.content, 80)));
    }
    let mut content = lines.join("\n");
    if content.chars().count() > max_chars {
        content = truncate_chars(&content, max_chars, "...(summary truncated)...");
    }
    Message {
        role: "user".to_string(),
        content,
        tool_calls: None,
        tool_call_id: None,
        name: Some(SUMMARY_NAME.to_string()),
        status: None,
    }
}

fn build_history_section(
    messages: &[Message],
    summary_message: &Message,
    cutoff_index: usize,
    options: &SummarizationOptions,
) -> String {
    let mut buf = String::new();
    buf.push_str("\n\n## summarization\n");
    buf.push_str(&format!("time: {}\n", Utc::now().to_rfc3339()));
    buf.push_str(&format!("cutoff_index: {}\n", cutoff_index));
    buf.push_str(&format!("policy: {:?}\n", options.policy));
    buf.push_str(&format!("max_char_budget: {}\n", options.max_char_budget));
    buf.push_str("summary:\n");
    buf.push_str(&summary_message.content);
    buf.push_str("\nmessages:\n");
    for (idx, m) in messages.iter().enumerate() {
        buf.push_str(&format!("- idx: {}\n  role: {}\n  content: {}\n", idx, m.role, compact_ws(&m.content)));
        if let Some(calls) = &m.tool_calls {
            let _ = serde_json::to_string(calls).map(|s| {
                buf.push_str("  tool_calls: ");
                buf.push_str(&s);
                buf.push('\n');
            });
        }
    }
    buf
}

fn should_summarize(messages: &[Message], options: &SummarizationOptions) -> bool {
    match options.policy {
        SummarizationPolicyKind::Budget => total_chars(messages) > options.max_char_budget,
        SummarizationPolicyKind::Turns => count_turns(messages) > options.max_turns_visible,
        SummarizationPolicyKind::Importance => total_chars(messages) > options.max_char_budget,
    }
}

fn compute_cutoff(messages: &[Message], options: &SummarizationOptions) -> usize {
    match options.policy {
        SummarizationPolicyKind::Turns => cutoff_for_turns(messages, options.max_turns_visible, options.min_recent_messages),
        _ => cutoff_for_budget(messages, options.max_char_budget, options.min_recent_messages),
    }
}

fn cutoff_for_budget(messages: &[Message], max_chars: usize, min_recent: usize) -> usize {
    let mut cutoff = 0;
    let mut remaining = total_chars(messages);
    if remaining <= max_chars {
        return 0;
    }
    let keep = min_recent.min(messages.len());
    while remaining > max_chars && cutoff + keep < messages.len() {
        remaining = total_chars(&messages[cutoff + 1..]);
        cutoff += 1;
    }
    cutoff
}

fn cutoff_for_turns(messages: &[Message], max_turns: usize, min_recent: usize) -> usize {
    if count_turns(messages) <= max_turns {
        return 0;
    }
    let mut turns = 0;
    let mut last_role = "";
    let mut idx = messages.len();
    for (i, m) in messages.iter().enumerate().rev() {
        if m.role != last_role {
            turns += 1;
            last_role = &m.role;
        }
        idx = i;
        if turns >= max_turns {
            break;
        }
    }
    let keep_floor = messages.len().saturating_sub(min_recent);
    idx.min(keep_floor)
}

fn truncate_tool_args(
    messages: Vec<Message>,
    keep_last: usize,
    max_chars: usize,
    truncation_text: &str,
) -> Vec<Message> {
    let mut out = messages;
    let keep_from = out.len().saturating_sub(keep_last);
    for (idx, msg) in out.iter_mut().enumerate() {
        if idx >= keep_from {
            continue;
        }
        let calls = match msg.tool_calls.as_mut() {
            Some(c) => c,
            None => continue,
        };
        for call in calls.iter_mut() {
            if !should_truncate_tool(&call.name) {
                continue;
            }
            call.arguments = truncate_value(call.arguments.clone(), max_chars, truncation_text);
        }
    }
    out
}

fn should_truncate_tool(name: &str) -> bool {
    matches!(name, "write_file" | "edit_file" | "execute")
}

fn truncate_value(value: serde_json::Value, max_chars: usize, truncation_text: &str) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => {
            if s.chars().count() > max_chars {
                serde_json::Value::String(truncate_chars(&s, max_chars, truncation_text))
            } else {
                serde_json::Value::String(s)
            }
        }
        serde_json::Value::Array(items) => {
            let out = items.into_iter().map(|v| truncate_value(v, max_chars, truncation_text)).collect();
            serde_json::Value::Array(out)
        }
        serde_json::Value::Object(map) => {
            let out = map
                .into_iter()
                .map(|(k, v)| (k, truncate_value(v, max_chars, truncation_text)))
                .collect();
            serde_json::Value::Object(out)
        }
        other => other,
    }
}

fn truncate_chars(s: &str, max_chars: usize, truncation_text: &str) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let head = max_chars / 2;
    let tail = max_chars - head;
    let head_str: String = s.chars().take(head).collect();
    let tail_str: String = s.chars().rev().take(tail).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{head_str}{truncation_text}{tail_str}")
}

fn total_chars(messages: &[Message]) -> usize {
    messages.iter().map(message_chars).sum()
}

fn message_chars(msg: &Message) -> usize {
    let mut count = msg.content.chars().count();
    if let Some(calls) = &msg.tool_calls {
        if let Ok(s) = serde_json::to_string(calls) {
            count += s.chars().count();
        }
    }
    count
}

fn count_turns(messages: &[Message]) -> usize {
    let mut turns = 0;
    let mut last_role = "";
    for m in messages {
        if m.role != last_role {
            turns += 1;
            last_role = &m.role;
        }
    }
    turns
}

fn preview(s: &str, max: usize) -> String {
    let out: String = s.chars().take(max).collect();
    compact_ws(&out)
}

fn compact_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
