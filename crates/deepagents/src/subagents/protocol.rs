use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::approval::{ApprovalPolicy, ExecutionMode};
use crate::audit::AuditSink;
use crate::runtime::RuntimeMiddleware;
use crate::state::AgentState;
use crate::types::Message;
use crate::DeepAgent;

pub const EXCLUDED_STATE_KEYS: [&str; 5] = [
    "messages",
    "todos",
    "structured_response",
    "skills_metadata",
    "memory_contents",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskInput {
    pub description: String,
    pub subagent_type: String,
}

#[derive(Clone)]
pub struct SubAgentRunRequest {
    pub description: String,
    pub messages: Vec<Message>,
    pub state: AgentState,
    pub agent: DeepAgent,
    pub root: String,
    pub mode: ExecutionMode,
    pub approval: Option<Arc<dyn ApprovalPolicy>>,
    pub audit: Option<Arc<dyn AuditSink>>,
    pub runtime_middlewares: Vec<Arc<dyn RuntimeMiddleware>>,
    pub task_depth: usize,
}

#[derive(Clone)]
pub struct SubAgentRunOutput {
    pub final_text: String,
    pub state: AgentState,
}

#[async_trait]
pub trait CompiledSubAgent: Send + Sync {
    fn subagent_type(&self) -> &str;
    fn description(&self) -> &str;
    async fn run(&self, req: SubAgentRunRequest) -> anyhow::Result<SubAgentRunOutput>;
}

#[derive(Debug, Clone)]
pub struct SubAgentInfo {
    pub subagent_type: String,
    pub description: String,
}

pub trait SubAgentRegistry: Send + Sync {
    fn register(&self, agent: Arc<dyn CompiledSubAgent>) -> anyhow::Result<()>;
    fn resolve(&self, subagent_type: &str) -> Option<Arc<dyn CompiledSubAgent>>;
    fn list(&self) -> Vec<SubAgentInfo>;
}

pub fn filter_state_for_child(parent: &AgentState) -> AgentState {
    let mut child = parent.clone();
    for k in EXCLUDED_STATE_KEYS {
        child.extra.remove(k);
    }
    child.private = Default::default();
    child
}

pub fn merge_child_state(parent: &mut AgentState, child: &AgentState) {
    parent.filesystem = child.filesystem.clone();
    for (k, v) in child.extra.iter() {
        if EXCLUDED_STATE_KEYS.iter().any(|x| x == k) {
            continue;
        }
        parent.extra.insert(k.clone(), v.clone());
    }
}

pub fn state_extra_keys(state: &AgentState) -> Vec<String> {
    state.extra.keys().cloned().collect()
}

pub fn state_extra_from_pairs(pairs: Vec<(&str, Value)>) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for (k, v) in pairs {
        out.insert(k.to_string(), v);
    }
    out
}
