use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::approval::{ApprovalPolicy, ExecutionMode};
use crate::audit::AuditSink;
use crate::backends::{LocalSandbox, SandboxBackend};
use crate::llm::{StructuredOutputSpec, ToolChoice};
use crate::middleware::filesystem::FilesystemMiddleware;
use crate::middleware::protocol::{MiddlewareContext, ToolError, ToolExecution};
use crate::middleware::ToolExecutionMiddleware;
use crate::provider::AgentProvider;
use crate::runtime::simple::{SimpleRuntime, SimpleRuntimeOptions};
use crate::runtime::{RuntimeConfig, RuntimeMiddleware};
use crate::skills::SkillPlugin;
use crate::state::AgentState;
use crate::tools::{default_tools, Tool, ToolResult};
use crate::types::{AgentRequest, AgentResponse};

#[doc(hidden)]
pub struct NeedsRoot;

#[doc(hidden)]
pub struct Ready;

#[derive(Default)]
struct AgentRuntimeBuilderState {
    skills: Vec<Arc<dyn SkillPlugin>>,
    config: RuntimeConfig,
    approval: Option<Arc<dyn ApprovalPolicy>>,
    audit: Option<Arc<dyn AuditSink>>,
    root: Option<String>,
    mode: ExecutionMode,
    tool_choice: ToolChoice,
    structured_output: Option<StructuredOutputSpec>,
    runtime_middlewares: Vec<Arc<dyn RuntimeMiddleware>>,
    initial_state: AgentState,
    task_depth: usize,
}

pub struct AgentRuntimeBuilder<State> {
    agent: DeepAgent,
    provider: Arc<dyn AgentProvider>,
    state: AgentRuntimeBuilderState,
    marker: PhantomData<State>,
}

#[derive(Clone)]
pub struct DeepAgent {
    backend: Arc<dyn SandboxBackend>,
    tools: HashMap<&'static str, Arc<dyn Tool>>,
    middlewares: Vec<Arc<dyn ToolExecutionMiddleware>>,
}

impl DeepAgent {
    pub fn with_backend(backend: Arc<dyn SandboxBackend>) -> Self {
        let tools_vec = default_tools(backend.clone());
        let tools = tools_vec.into_iter().map(|t| (t.name(), t)).collect();
        let middlewares: Vec<Arc<dyn ToolExecutionMiddleware>> =
            vec![Arc::new(FilesystemMiddleware::new())];
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
        let middlewares: Vec<Arc<dyn ToolExecutionMiddleware>> =
            vec![Arc::new(FilesystemMiddleware::new())];
        Self {
            backend,
            tools,
            middlewares,
        }
    }

    pub fn backend(&self) -> Arc<dyn SandboxBackend> {
        self.backend.clone()
    }

    pub fn runtime(self, provider: Arc<dyn AgentProvider>) -> AgentRuntimeBuilder<NeedsRoot> {
        AgentRuntimeBuilder {
            agent: self,
            provider,
            state: AgentRuntimeBuilderState::default(),
            marker: PhantomData,
        }
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
        let mut exec = match result {
            Ok(res) => {
                tool_result = Some(res.clone());
                ToolExecution {
                    tool_name: name.to_string(),
                    input: exec.input,
                    output: Some(res.output),
                    error: None,
                }
            }
            Err(e) => {
                let (code, message) = classify_tool_anyhow_error(&e);
                ToolExecution {
                    tool_name: name.to_string(),
                    input: exec.input,
                    output: None,
                    error: Some(ToolError {
                        code,
                        message,
                        source: e,
                    }),
                }
            }
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

        match exec.error.take() {
            Some(err) => Err(err.source),
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

fn classify_tool_anyhow_error(e: &anyhow::Error) -> (String, String) {
    if let Some(be) = e.downcast_ref::<crate::backends::protocol::BackendError>() {
        return (be.code_str().to_string(), be.message.clone());
    }
    if let Some(me) = e.downcast_ref::<crate::memory::protocol::MemoryError>() {
        return (me.code.to_string(), me.message.clone());
    }
    ("unknown".to_string(), e.to_string())
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

impl<State> AgentRuntimeBuilder<State> {
    pub fn with_skills(mut self, skills: Vec<Arc<dyn SkillPlugin>>) -> Self {
        self.state.skills = skills;
        self
    }

    pub fn with_config(mut self, config: RuntimeConfig) -> Self {
        self.state.config = config;
        self
    }

    pub fn with_approval(mut self, approval: Arc<dyn ApprovalPolicy>) -> Self {
        self.state.approval = Some(approval);
        self
    }

    pub fn with_audit(mut self, audit: Arc<dyn AuditSink>) -> Self {
        self.state.audit = Some(audit);
        self
    }

    pub fn with_mode(mut self, mode: ExecutionMode) -> Self {
        self.state.mode = mode;
        self
    }

    pub fn with_tool_choice(mut self, tool_choice: ToolChoice) -> Self {
        self.state.tool_choice = tool_choice;
        self
    }

    pub fn with_structured_output(mut self, structured_output: StructuredOutputSpec) -> Self {
        self.state.structured_output = Some(structured_output);
        self
    }

    pub fn with_runtime_middlewares(
        mut self,
        runtime_middlewares: Vec<Arc<dyn RuntimeMiddleware>>,
    ) -> Self {
        self.state.runtime_middlewares = runtime_middlewares;
        self
    }

    pub fn with_initial_state(mut self, initial_state: AgentState) -> Self {
        self.state.initial_state = initial_state;
        self
    }

    pub fn with_task_depth(mut self, task_depth: usize) -> Self {
        self.state.task_depth = task_depth;
        self
    }
}

impl AgentRuntimeBuilder<NeedsRoot> {
    pub fn with_root(mut self, root: impl Into<String>) -> AgentRuntimeBuilder<Ready> {
        self.state.root = Some(root.into());
        AgentRuntimeBuilder {
            agent: self.agent,
            provider: self.provider,
            state: self.state,
            marker: PhantomData,
        }
    }
}

impl AgentRuntimeBuilder<Ready> {
    pub fn build(self) -> anyhow::Result<SimpleRuntime> {
        let AgentRuntimeBuilder {
            agent,
            provider,
            state,
            ..
        } = self;
        let root = state
            .root
            .filter(|root| !root.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("agent_runtime_builder_root_required"))?;
        let runtime = SimpleRuntime::new(
            agent,
            provider,
            state.skills,
            SimpleRuntimeOptions {
                config: state.config,
                approval: state.approval,
                audit: state.audit,
                root,
                mode: state.mode,
            },
        )
        .with_runtime_middlewares(state.runtime_middlewares)
        .with_initial_state(state.initial_state)
        .with_task_depth(state.task_depth)
        .with_tool_choice(state.tool_choice);

        Ok(if let Some(structured_output) = state.structured_output {
            runtime.with_structured_output(structured_output)
        } else {
            runtime
        })
    }
}
