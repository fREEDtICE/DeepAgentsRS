use std::collections::HashMap;
use std::sync::Arc;

use crate::backends::{LocalSandbox, SandboxBackend};
use crate::middleware::filesystem::FilesystemMiddleware;
use crate::middleware::protocol::{MiddlewareContext, ToolExecution};
use crate::middleware::Middleware;
use crate::state::AgentState;
use crate::tools::{default_tools, Tool, ToolResult};
use crate::types::{AgentRequest, AgentResponse};

#[derive(Clone)]
pub struct DeepAgent {
    backend: Arc<dyn SandboxBackend>,
    tools: HashMap<&'static str, Arc<dyn Tool>>,
    middlewares: Vec<Arc<dyn Middleware>>,
}

impl DeepAgent {
    pub fn with_backend(backend: Arc<dyn SandboxBackend>) -> Self {
        let tools_vec = default_tools(backend.clone());
        let tools = tools_vec.into_iter().map(|t| (t.name(), t)).collect();
        let middlewares: Vec<Arc<dyn Middleware>> = vec![Arc::new(FilesystemMiddleware::new())];
        Self {
            backend,
            tools,
            middlewares,
        }
    }

    pub fn with_backend_and_tools(
        backend: Arc<dyn SandboxBackend>,
        tools: Vec<Arc<dyn Tool>>,
    ) -> Self {
        let tools = tools.into_iter().map(|t| (t.name(), t)).collect();
        let middlewares: Vec<Arc<dyn Middleware>> = vec![Arc::new(FilesystemMiddleware::new())];
        Self {
            backend,
            tools,
            middlewares,
        }
    }

    pub fn backend(&self) -> Arc<dyn SandboxBackend> {
        self.backend.clone()
    }

    pub async fn run(&self, _req: AgentRequest) -> anyhow::Result<AgentResponse> {
        Ok(AgentResponse {
            output_text: String::new(),
        })
    }

    pub async fn call_tool(
        &self,
        name: &str,
        input: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {name}"))?;
        Ok(tool.call(input).await?.output)
    }

    pub async fn call_tool_stateful(
        &self,
        name: &str,
        input: serde_json::Value,
        state: &mut AgentState,
    ) -> anyhow::Result<(ToolResult, Option<crate::state::FilesystemDelta>)> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {name}"))?;

        let exec = ToolExecution {
            tool_name: name.to_string(),
            input: input.clone(),
            output: None,
            error: None,
        };
        for mw in &self.middlewares {
            mw.before_tool(self.backend.as_ref(), state, &exec).await?;
        }

        let result = tool.call(input).await;
        let mut tool_result: Option<ToolResult> = None;
        let exec = match result {
            Ok(res) => {
                tool_result = Some(res.clone());
                ToolExecution {
                    tool_name: name.to_string(),
                    input: exec.input,
                    output: Some(res.output),
                    error: None,
                }
            }
            Err(e) => ToolExecution {
                tool_name: name.to_string(),
                input: exec.input,
                output: None,
                error: Some(e.to_string()),
            },
        };

        let filesystem_delta = {
            let mut ctx = MiddlewareContext {
                backend: self.backend.as_ref(),
                state,
                tool: &exec,
                filesystem_delta: None,
            };
            for mw in &self.middlewares {
                mw.after_tool(&mut ctx).await?;
            }
            ctx.filesystem_delta
        };

        match exec.error {
            Some(err) => Err(anyhow::anyhow!(err)),
            None => Ok((
                tool_result.unwrap_or(ToolResult {
                    output: serde_json::Value::Null,
                    content_blocks: None,
                }),
                filesystem_delta,
            )),
        }
    }
}

pub fn create_deep_agent(root: impl Into<std::path::PathBuf>) -> anyhow::Result<DeepAgent> {
    let backend: Arc<dyn SandboxBackend> = Arc::new(LocalSandbox::new(root)?);
    Ok(DeepAgent::with_backend(backend))
}

pub fn create_deep_agent_with_backend(backend: Arc<dyn SandboxBackend>) -> DeepAgent {
    DeepAgent::with_backend(backend)
}

pub fn create_local_sandbox_backend(
    root: impl Into<std::path::PathBuf>,
    shell_allow_list: Option<Vec<String>>,
) -> anyhow::Result<Arc<dyn SandboxBackend>> {
    Ok(Arc::new(
        LocalSandbox::new(root)?.with_shell_allow_list(shell_allow_list),
    ))
}
