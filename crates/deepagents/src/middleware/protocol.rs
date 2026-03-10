use async_trait::async_trait;

use crate::backends::SandboxBackend;
use crate::state::{AgentState, FilesystemDelta};

#[derive(Debug, Clone)]
pub struct ToolExecution {
    pub tool_name: String,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
}

pub struct MiddlewareContext<'a> {
    pub backend: &'a dyn SandboxBackend,
    pub state: &'a mut AgentState,
    pub tool: &'a ToolExecution,
    pub filesystem_delta: Option<FilesystemDelta>,
}

#[async_trait]
pub trait Middleware: Send + Sync {
    async fn before_tool(
        &self,
        _backend: &dyn SandboxBackend,
        _state: &mut AgentState,
        _tool: &ToolExecution,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn after_tool(&self, _ctx: &mut MiddlewareContext<'_>) -> anyhow::Result<()> {
        Ok(())
    }
}
