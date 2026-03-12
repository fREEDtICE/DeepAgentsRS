//! backends 模块：把“工具/沙箱能力”抽象为可插拔的后端实现。
//!
//! - [protocol] 定义后端能力边界（文件系统 + 命令执行）与统一错误类型。
//! - [local] 提供一个本地目录下的沙箱实现，用于开发/测试或本地运行。
//! - [composite] 提供按路径前缀路由到不同后端的组合后端（类似虚拟挂载）。

pub mod composite;
pub mod local;
pub mod protocol;

pub use composite::CompositeBackend;
pub use local::LocalSandbox;
pub use protocol::{Backend, BackendError, FilesystemBackend, SandboxBackend};
