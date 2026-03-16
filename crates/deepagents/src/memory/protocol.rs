//! memory 子系统的“协议层”（纯数据结构与 trait）。
//!
//! 这里刻意不包含具体存储实现，只定义：
//! - 配额/淘汰策略（MemoryPolicy / MemoryEvictionPolicy）
//! - 统一的条目模型（MemoryEntry）与查询语义（MemoryQuery）
//! - 诊断与淘汰统计（MemoryDiagnostics / MemoryEvictionReport）
//! - 统一错误模型（MemoryError / MemoryErrorCode）
//! - 存储后端需要实现的能力（MemoryStore）
//!
//! 设计要点：
//! - 类型全部可序列化/反序列化，用于跨组件传递与持久化。
//! - 错误码（MemoryErrorCode）用于上层做可预测的分支处理。
//! - 时间字段统一使用 RFC3339 字符串，便于与外部系统交互。

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Durable memory visibility scope.
///
/// The scope decides which runtime identity context must match before one
/// memory item is considered visible to the caller.
pub enum MemoryScope {
    /// Private user memory shared across that user's channels.
    User,
    /// Thread-local memory isolated to one conversation thread.
    Thread,
    /// Workspace-shared memory visible to matching workspace members.
    Workspace,
    /// Optional agent/system-owned memory.
    System,
}

impl Default for MemoryScope {
    fn default() -> Self {
        Self::User
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Durable memory classification used by CLI and retrieval filtering.
pub enum MemoryType {
    /// Stable factual knowledge.
    Semantic,
    /// Reusable instruction or preference.
    Procedural,
    /// One-off event or observation.
    Episodic,
    /// Explicitly pinned memory.
    Pinned,
    /// Profile-style identity or preference record.
    Profile,
}

impl Default for MemoryType {
    fn default() -> Self {
        Self::Semantic
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Lifecycle state of one memory record.
pub enum MemoryStatus {
    /// Visible durable memory.
    Active,
    /// Soft-deleted memory preserved for audit/query flows.
    Deleted,
    /// Non-deleted but inactive memory retained for lifecycle transitions.
    Inactive,
}

impl Default for MemoryStatus {
    fn default() -> Self {
        Self::Active
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// 存储策略：控制容量上限与超限时的淘汰方式。
///
/// 这是“运行时策略”，实现方可以在加载持久化文件后更新当前策略。
pub struct MemoryPolicy {
    /// 允许保留的最大条目数（超过则触发淘汰）。
    pub max_entries: usize,
    /// 允许保留的估算总字节数（超过则触发淘汰）。
    pub max_bytes_total: usize,
    /// 淘汰策略：决定当超限时移除哪些条目。
    pub eviction: MemoryEvictionPolicy,
}

impl Default for MemoryPolicy {
    fn default() -> Self {
        Self {
            max_entries: 200,
            max_bytes_total: 200_000,
            eviction: MemoryEvictionPolicy::Lru,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// 淘汰策略。
///
/// - Lru：按 last_accessed_at 最早的优先淘汰（最近最少使用）
/// - Fifo：按 created_at 最早的优先淘汰（先进先出）
/// - Ttl：按 updated_at 早于截止时间的优先淘汰（过期清理）
pub enum MemoryEvictionPolicy {
    Lru,
    Fifo,
    Ttl { ttl_secs: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// 一条“记忆”记录。
///
/// 约定：
/// - key：稳定标识，用于覆盖写入与检索
/// - value：正文内容（可为 Markdown/纯文本）
/// - tags：可选标签，用于筛选
/// - *_at：RFC3339 格式时间戳字符串
/// - access_count：访问计数，用于可观测性与潜在策略扩展
pub struct MemoryEntry {
    /// 唯一键（同 key 的 put 视为更新）。
    pub key: String,
    /// 记录内容。
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 可选标题，便于 explain/list/query 输出更可读。
    pub title: Option<String>,
    #[serde(default)]
    /// 可见性作用域。
    pub scope: MemoryScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 作用域标识，例如 thread_id。
    pub scope_id: Option<String>,
    #[serde(default)]
    /// 记忆类型。
    pub memory_type: MemoryType,
    #[serde(default)]
    /// 是否被显式 pin。
    pub pinned: bool,
    #[serde(default)]
    /// 生命周期状态。
    pub status: MemoryStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 置信度，范围约定为 0..=100。
    pub confidence: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 显著性，范围约定为 0..=100。
    pub salience: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 被当前条目 supersede 的旧 key。
    pub supersedes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// user scope 的拥有者。
    pub owner_user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// workspace scope 的拥有者。
    pub owner_workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 最近一次写入时关联的 channel account。
    pub owner_channel_account_id: Option<String>,
    #[serde(default)]
    /// 标签集合（可为空）。
    pub tags: Vec<String>,
    /// 创建时间（RFC3339）。
    pub created_at: String,
    /// 最近一次内容更新的时间（RFC3339）。
    pub updated_at: String,
    /// 最近一次读取/命中的时间（RFC3339）。
    pub last_accessed_at: String,
    /// 访问计数（读/命中会增加）。
    pub access_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
/// 查询条件。
///
/// 语义是“交集过滤”：prefix 与 tag 同时存在时必须同时满足。
pub struct MemoryQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    /// key 前缀匹配（starts_with）。
    pub prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 标签精确匹配（tags 包含该值）。
    pub tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 返回条目数上限（实现可给默认值）。
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 按 scope 过滤。
    pub scope: Option<MemoryScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 按 scope_id 过滤。
    pub scope_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 按记忆类型过滤。
    pub memory_type: Option<MemoryType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 按 pinned 过滤。
    pub pinned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 按状态过滤。
    pub status: Option<MemoryStatus>,
    #[serde(default)]
    /// 是否包含 inactive/deleted 记录。
    pub include_inactive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 当前调用用户。
    pub actor_user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 当前调用线程。
    pub actor_thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 当前调用工作区。
    pub actor_workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 当前调用渠道账号。
    pub actor_channel_account_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
/// 一次淘汰执行的统计报告（用于调试与可观测性）。
pub struct MemoryEvictionReport {
    pub before_entries: usize,
    pub after_entries: usize,
    pub evicted: usize,
    #[serde(default)]
    pub evicted_keys: Vec<String>,
    pub before_bytes_total: usize,
    pub after_bytes_total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
/// 内存系统的诊断信息（常用于“加载/注入”流程的汇总）。
pub struct MemoryDiagnostics {
    /// 成功加载的源数量（例如多个文件/片段）。
    pub loaded_sources: usize,
    /// 跳过的不存在源数量（例如可选文件缺失）。
    pub skipped_not_found: usize,
    #[serde(default)]
    /// 过程中遇到的错误（字符串化，便于汇报）。
    pub errors: Vec<String>,
    /// 是否发生了截断（例如超过最大注入字符数）。
    pub truncated: bool,
    /// 写入到 prompt/上下文中的字符数。
    pub injected_chars: usize,
    /// 合并后的总字符数（包含模板/头尾等）。
    pub combined_chars: usize,
}

#[derive(Debug, Error)]
#[error("{code}: {message}")]
/// memory 子系统统一错误。
///
/// - code：稳定错误码，供上层做逻辑分支/告警
/// - message：面向开发者的简短描述
/// - source：底层错误（可选），保留堆栈信息
/// - context：额外键值上下文（例如路径、key 等）
pub struct MemoryError {
    pub code: MemoryErrorCode,
    pub message: String,
    #[source]
    pub source: Option<anyhow::Error>,
    pub context: BTreeMap<String, String>,
}

impl MemoryError {
    pub fn new(code: MemoryErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source: None,
            context: BTreeMap::new(),
        }
    }

    pub fn with_source(mut self, source: anyhow::Error) -> Self {
        self.source = Some(source);
        self
    }

    pub fn with_context(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.context.insert(k.into(), v.into());
        self
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// 稳定错误码：用于跨组件/版本的错误分类。
pub enum MemoryErrorCode {
    NotFound,
    PermissionDenied,
    Corrupt,
    IoError,
    QuotaExceeded,
    InvalidRequest,
}

impl std::fmt::Display for MemoryErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            MemoryErrorCode::NotFound => "memory_not_found",
            MemoryErrorCode::PermissionDenied => "memory_permission_denied",
            MemoryErrorCode::Corrupt => "memory_corrupt",
            MemoryErrorCode::IoError => "memory_io_error",
            MemoryErrorCode::QuotaExceeded => "memory_quota_exceeded",
            MemoryErrorCode::InvalidRequest => "invalid_request",
        };
        write!(f, "{s}")
    }
}

#[async_trait]
/// memory 存储后端需要实现的能力集合。
///
/// 生命周期约定：
/// - load：把持久化状态加载到实现内部（允许惰性加载）
/// - flush：把当前状态持久化到存储介质
/// - put/get/query：基本读写与检索
/// - evict_if_needed：在策略限制下执行淘汰，并返回统计
pub trait MemoryStore: Send + Sync {
    fn name(&self) -> &str;
    fn policy(&self) -> MemoryPolicy;

    async fn load(&self) -> Result<(), MemoryError>;
    async fn flush(&self) -> Result<(), MemoryError>;

    async fn put(&self, entry: MemoryEntry) -> Result<(), MemoryError>;
    async fn get(&self, key: &str) -> Result<Option<MemoryEntry>, MemoryError>;
    async fn query(&self, q: MemoryQuery) -> Result<Vec<MemoryEntry>, MemoryError>;

    async fn evict_if_needed(&self) -> Result<MemoryEvictionReport, MemoryError>;
}
