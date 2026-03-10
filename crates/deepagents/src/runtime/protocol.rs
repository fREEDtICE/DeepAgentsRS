use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::approval::{ApprovalPolicy, ExecutionMode};
use crate::audit::AuditSink;
use crate::provider::ProviderToolCall;
use crate::state::AgentState;
use crate::types::Message;
use crate::DeepAgent;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub tool_name: String,
    pub arguments: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRecord {
    pub tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    pub output: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Completed,
    Interrupted,
    Error,
}

impl Default for RunStatus {
    fn default() -> Self {
        Self::Completed
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitlPolicy {
    pub allow_approve: bool,
    pub allow_reject: bool,
    pub allow_edit: bool,
}

impl Default for HitlPolicy {
    fn default() -> Self {
        Self {
            allow_approve: true,
            allow_reject: true,
            allow_edit: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitlHints {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub danger_level: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitlInterrupt {
    pub interrupt_id: String,
    pub tool_name: String,
    pub tool_call_id: String,
    pub proposed_args: serde_json::Value,
    #[serde(default)]
    pub policy: HitlPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hints: Option<HitlHints>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HitlDecision {
    Approve,
    Reject {
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Edit {
        args: serde_json::Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOutput {
    #[serde(default)]
    pub status: RunStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interrupts: Vec<HitlInterrupt>,
    pub final_text: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallRecord>,
    #[serde(default)]
    pub tool_results: Vec<ToolResultRecord>,
    #[serde(default)]
    pub state: AgentState,
    pub error: Option<RuntimeError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summarization_events: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default = "default_max_steps")]
    pub max_steps: usize,
    #[serde(default = "default_provider_timeout_ms")]
    pub provider_timeout_ms: u64,
}

fn default_max_steps() -> usize {
    8
}

fn default_provider_timeout_ms() -> u64 {
    1000
}

#[async_trait]
pub trait Runtime: Send + Sync {
    async fn run(&self, messages: Vec<Message>) -> RunOutput;
}

#[async_trait]
pub trait StreamingRuntime: Send + Sync {
    async fn run_with_events(
        &mut self,
        sink: &mut dyn crate::runtime::RunEventSink,
    ) -> RunOutput;
}

pub struct ToolCallContext<'a> {
    pub agent: &'a DeepAgent,
    pub tool_call: &'a ProviderToolCall,
    pub call_id: &'a str,
    pub messages: &'a mut Vec<Message>,
    pub state: &'a mut AgentState,
    pub root: &'a str,
    pub mode: ExecutionMode,
    pub approval: Option<&'a Arc<dyn ApprovalPolicy>>,
    pub audit: Option<&'a Arc<dyn AuditSink>>,
    pub runtime_middlewares: &'a [Arc<dyn RuntimeMiddleware>],
    pub task_depth: usize,
}

pub struct HandledToolCall {
    pub output: serde_json::Value,
    pub error: Option<String>,
}

#[async_trait]
pub trait RuntimeMiddleware: Send + Sync {
    async fn before_run(
        &self,
        messages: Vec<Message>,
        _state: &mut AgentState,
    ) -> anyhow::Result<Vec<Message>> {
        Ok(messages)
    }

    async fn before_provider_step(
        &self,
        messages: Vec<Message>,
        _state: &mut AgentState,
    ) -> anyhow::Result<Vec<Message>> {
        Ok(messages)
    }

    async fn patch_provider_step(
        &self,
        step: crate::provider::ProviderStep,
        _next_call_id: &mut u64,
    ) -> anyhow::Result<crate::provider::ProviderStep> {
        Ok(step)
    }

    async fn handle_tool_call(
        &self,
        _ctx: &mut ToolCallContext<'_>,
    ) -> anyhow::Result<Option<HandledToolCall>> {
        Ok(None)
    }
}
