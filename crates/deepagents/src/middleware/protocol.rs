use async_trait::async_trait;

use crate::backends::SandboxBackend;
use crate::state::{AgentState, FilesystemDelta};

#[derive(Debug)]
/// 工具执行失败时的结构化错误信息。
///
/// 该类型用于把“工具层错误”与“运行时/框架层错误”（`anyhow::Error`）区分开来：
/// - `code`/`message` 面向上层策略与模型可读性
/// - `source` 保留底层错误链用于诊断
pub struct ToolError {
    pub code: String,
    pub message: String,
    pub source: anyhow::Error,
}

#[derive(Debug)]
/// 一次工具调用的可观测记录（输入/输出/错误）。
///
/// 中间件通常只依赖该结构提供的稳定字段，而不依赖某个具体工具的内部实现。
pub struct ToolExecution {
    pub tool_name: String,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub error: Option<ToolError>,
}

/// 中间件在 `after_tool` 阶段可访问的上下文。
///
/// - `backend`：用于读取沙盒文件等副作用查询（注意：不要在这里执行会改变世界状态的操作，除非中间件语义明确）。
/// - `state`：可写的 agent 运行态，用于把工具副作用归约进状态。
/// - `tool`：本次工具执行的输入/输出/错误。
/// - `filesystem_delta`：由中间件计算出的增量（当前主要用于文件系统相关能力）。
pub struct MiddlewareContext<'a> {
    pub backend: &'a dyn SandboxBackend,
    pub state: &'a mut AgentState,
    pub tool: &'a ToolExecution,
    pub filesystem_delta: Option<FilesystemDelta>,
}

#[async_trait]
/// 工具调用的中间件协议。
///
/// 生命周期：
/// - `before_tool`：在工具执行前运行，适合做校验、注入上下文或准备数据。
/// - `after_tool`：在工具执行后运行，适合做副作用归约（例如：把文件写入/删除同步到 `AgentState`）。
///
/// 默认实现是 no-op，便于按需覆盖。
pub trait ToolExecutionMiddleware: Send + Sync {
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
