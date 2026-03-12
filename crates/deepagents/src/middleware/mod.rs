//! 中间件层：在工具（tool）执行的前后插入可组合的钩子。
//!
//! 设计目标：
//! - **可插拔**：通过 [`ToolExecutionMiddleware`](protocol::ToolExecutionMiddleware) trait 组合多个横切能力（如文件系统状态归约）。
//! - **低侵入**：不改动具体工具实现，只在调用边界拦截输入/输出。
//! - **面向状态**：在 after hook 里把工具的副作用归约进 [`AgentState`](crate::state::AgentState)。

pub mod filesystem;
pub mod protocol;

pub use protocol::ToolExecutionMiddleware;
