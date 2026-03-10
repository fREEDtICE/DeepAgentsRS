use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::approval::{ApprovalPolicy, ExecutionMode};
use crate::audit::AuditSink;
use crate::provider::Provider;
use crate::runtime::events::RunEventSink;
use crate::runtime::protocol::{
    RunOutput, Runtime, RuntimeConfig, RuntimeMiddleware,
};
use crate::runtime::{ResumableRunner, ResumableRunnerOptions};
use crate::skills::SkillPlugin;
use crate::state::AgentState;
use crate::types::Message;
use crate::DeepAgent;

pub struct SimpleRuntime {
    agent: DeepAgent,
    provider: Arc<dyn Provider>,
    skills: Vec<Arc<dyn SkillPlugin>>,
    config: RuntimeConfig,
    approval: Option<Arc<dyn ApprovalPolicy>>,
    audit: Option<Arc<dyn AuditSink>>,
    root: String,
    mode: ExecutionMode,
    runtime_middlewares: Vec<Arc<dyn RuntimeMiddleware>>,
    initial_state: AgentState,
    task_depth: usize,
}

pub struct SimpleRuntimeOptions {
    pub config: RuntimeConfig,
    pub approval: Option<Arc<dyn ApprovalPolicy>>,
    pub audit: Option<Arc<dyn AuditSink>>,
    pub root: String,
    pub mode: ExecutionMode,
}

impl SimpleRuntime {
    pub fn new(
        agent: DeepAgent,
        provider: Arc<dyn Provider>,
        skills: Vec<Arc<dyn SkillPlugin>>,
        options: SimpleRuntimeOptions,
    ) -> Self {
        let SimpleRuntimeOptions {
            config,
            approval,
            audit,
            root,
            mode,
        } = options;
        Self {
            agent,
            provider,
            skills,
            config,
            approval,
            audit,
            root,
            mode,
            runtime_middlewares: Vec::new(),
            initial_state: AgentState::default(),
            task_depth: 0,
        }
    }

    pub fn with_runtime_middlewares(
        mut self,
        middlewares: Vec<Arc<dyn RuntimeMiddleware>>,
    ) -> Self {
        self.runtime_middlewares = middlewares;
        self
    }

    pub fn with_initial_state(mut self, state: AgentState) -> Self {
        self.initial_state = state;
        self
    }

    pub fn with_task_depth(mut self, depth: usize) -> Self {
        self.task_depth = depth;
        self
    }

    pub async fn run_with_events(
        &self,
        messages: Vec<Message>,
        sink: &mut dyn RunEventSink,
    ) -> RunOutput {
        let mut runner = self.build_runner(messages);
        runner.run_with_events(sink).await
    }

    fn build_runner(&self, messages: Vec<Message>) -> ResumableRunner {
        ResumableRunner::new(
            self.agent.clone(),
            self.provider.clone(),
            self.skills.clone(),
            ResumableRunnerOptions {
                config: self.config.clone(),
                approval: self.approval.clone(),
                audit: self.audit.clone(),
                root: self.root.clone(),
                mode: self.mode,
                interrupt_on: BTreeMap::new(),
            },
        )
        .with_runtime_middlewares(self.runtime_middlewares.clone())
        .with_initial_state(self.initial_state.clone())
        .with_initial_messages(messages)
        .with_task_depth(self.task_depth)
    }
}

#[async_trait]
impl Runtime for SimpleRuntime {
    async fn run(&self, messages: Vec<Message>) -> RunOutput {
        let mut runner = self.build_runner(messages);
        runner.run().await
    }
}
