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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScopeType {
    Thread,
    User,
    #[default]
    Workspace,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    Profile,
    Episodic,
    #[default]
    Semantic,
    Procedural,
    Pinned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemorySourceKind {
    ExplicitUserRequest,
    ExtractedFromMessage,
    Inferred,
    WorkspaceEvent,
    #[default]
    SystemImported,
    ConsolidatedSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct MemorySource {
    #[serde(default)]
    pub kind: MemorySourceKind,
    #[serde(default)]
    pub message_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryPrivacyLevel {
    #[default]
    Private,
    Workspace,
    System,
    Sensitive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    #[default]
    Active,
    Superseded,
    Inactive,
    Deleted,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRuntimeMode {
    #[default]
    Compatibility,
    Scoped,
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
    #[serde(default)]
    /// 稳定 memory ID。兼容旧存储时允许为空，落盘前会补齐。
    pub memory_id: String,
    #[serde(default)]
    /// 作用域类型。
    pub scope_type: MemoryScopeType,
    #[serde(default)]
    /// 作用域 ID。
    pub scope_id: String,
    #[serde(default)]
    /// 内存类型。
    pub memory_type: MemoryType,
    #[serde(default)]
    /// 面向用户/操作员的标题。
    pub title: String,
    #[serde(default)]
    /// 来源与溯源信息。
    pub source: MemorySource,
    #[serde(default = "default_memory_author")]
    /// 记录作者（user / agent / system 等）。
    pub author: String,
    #[serde(default = "default_memory_confidence")]
    /// 置信度。
    pub confidence: f32,
    #[serde(default = "default_memory_salience")]
    /// 显著性。
    pub salience: f32,
    #[serde(default)]
    /// 隐私级别。
    pub privacy_level: MemoryPrivacyLevel,
    #[serde(default)]
    /// 是否为 pinned 记忆。
    pub pinned: bool,
    #[serde(default)]
    /// 标签集合（可为空）。
    pub tags: Vec<String>,
    /// 创建时间（RFC3339）。
    pub created_at: String,
    /// 最近一次内容更新的时间（RFC3339）。
    pub updated_at: String,
    /// 最近一次读取/命中的时间（RFC3339）。
    pub last_accessed_at: String,
    #[serde(default)]
    /// 生效时间。
    pub valid_from: String,
    #[serde(default)]
    /// 失效时间（为空表示仍然有效）。
    pub valid_to: Option<String>,
    #[serde(default)]
    /// 被哪条新记忆取代。
    pub supersedes: Option<String>,
    #[serde(default)]
    /// 向量/嵌入引用（如果有）。
    pub embedding_ref: Option<String>,
    /// 访问计数（读/命中会增加）。
    pub access_count: u64,
    #[serde(default)]
    /// 生命周期状态。
    pub status: MemoryStatus,
}

impl MemoryEntry {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            memory_id: String::new(),
            scope_type: MemoryScopeType::default(),
            scope_id: String::new(),
            memory_type: MemoryType::default(),
            title: String::new(),
            source: MemorySource::default(),
            author: default_memory_author(),
            confidence: default_memory_confidence(),
            salience: default_memory_salience(),
            privacy_level: MemoryPrivacyLevel::default(),
            pinned: false,
            tags: Vec::new(),
            created_at: String::new(),
            updated_at: String::new(),
            last_accessed_at: String::new(),
            valid_from: String::new(),
            valid_to: None,
            supersedes: None,
            embedding_ref: None,
            access_count: 0,
            status: MemoryStatus::default(),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self.status, MemoryStatus::Active)
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 作用域类型过滤。
    pub scope_type: Option<MemoryScopeType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 作用域 ID 过滤。
    pub scope_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 内存类型过滤。
    pub memory_type: Option<MemoryType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// pinned 过滤。
    pub pinned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 生命周期状态过滤。
    pub status: Option<MemoryStatus>,
    #[serde(default)]
    /// 是否包含非 active 的条目。
    pub include_inactive: bool,
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
/// - put/get/query/delete：基本读写与检索
/// - evict_if_needed：在策略限制下执行淘汰，并返回统计
pub trait MemoryStore: Send + Sync {
    fn name(&self) -> &str;
    fn policy(&self) -> MemoryPolicy;

    async fn load(&self) -> Result<(), MemoryError>;
    async fn flush(&self) -> Result<(), MemoryError>;
    async fn set_policy(&self, policy: MemoryPolicy) -> Result<(), MemoryError>;

    async fn put(&self, entry: MemoryEntry) -> Result<(), MemoryError>;
    async fn put_with_report(
        &self,
        entry: MemoryEntry,
    ) -> Result<MemoryEvictionReport, MemoryError>;
    async fn get(&self, key: &str) -> Result<Option<MemoryEntry>, MemoryError>;
    async fn inspect(&self, key: &str) -> Result<Option<MemoryEntry>, MemoryError>;
    async fn query(&self, q: MemoryQuery) -> Result<Vec<MemoryEntry>, MemoryError>;
    async fn delete(&self, key: &str) -> Result<bool, MemoryError>;

    async fn evict_if_needed(&self) -> Result<MemoryEvictionReport, MemoryError>;
}

fn default_memory_author() -> String {
    "system".to_string()
}

fn default_memory_confidence() -> f32 {
    1.0
}

fn default_memory_salience() -> f32 {
    0.5
}
