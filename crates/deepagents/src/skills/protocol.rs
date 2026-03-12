use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::provider::AgentToolCall;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSpec {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCall {
    pub name: String,
    #[serde(default)]
    pub input: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillError {
    pub code: String,
    pub message: String,
}

#[async_trait]
pub trait SkillPlugin: Send + Sync {
    fn list_skills(&self) -> Vec<SkillSpec>;
    async fn call(&self, call: SkillCall) -> Result<Vec<AgentToolCall>, SkillError>;
}
