//! memory 子系统入口模块。
//!
//! - protocol：定义对外可用的数据结构与 trait（不含具体实现）
//! - store_file：一个基于本地 JSON 文件的简单实现（FileMemoryStore）
//!
//! 对外主要通过 re-export 暴露稳定 API，减少上层对内部文件布局的耦合。

pub mod protocol;
pub mod store_file;

pub use protocol::{
    MemoryDiagnostics, MemoryEntry, MemoryError, MemoryErrorCode, MemoryEvictionPolicy,
    MemoryEvictionReport, MemoryPolicy, MemoryQuery, MemoryStore,
};
pub use store_file::FileMemoryStore;
