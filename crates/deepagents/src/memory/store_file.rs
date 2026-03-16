//! 基于本地文件的 memory 存储实现。
//!
//! 这个实现把所有条目缓存在进程内（InMemory），并将其持久化为一个 JSON 文件（MemoryFileV1）。
//! 关键设计点：
//! - 惰性加载：首次读写前 ensure_loaded() 从磁盘加载一次，避免启动时 I/O。
//! - 原子写入：flush()/render_agents_md() 使用 write_atomic()，先写 tmp 再 rename。
//! - 配额与淘汰：put() 后调用 evict_if_needed()，按策略把超限条目移除。
//! - 额外输出：render_agents_md() 会把条目渲染到 AGENTS.md，供外部工具消费。
//!
//! 注意：这里用 Mutex 保护内部状态，定位是“简单可靠”的本地存储，不追求极致并发吞吐。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};

use crate::memory::protocol::{
    MemoryEntry, MemoryError, MemoryErrorCode, MemoryEvictionPolicy, MemoryEvictionReport,
    MemoryPolicy, MemoryQuery, MemoryScope, MemoryStatus, MemoryStore,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
/// 持久化文件的 v1 版本格式。
///
/// - version：用于向前/向后兼容（目前仅支持 1）
/// - policy：与文件绑定的策略快照
/// - entries：条目列表
struct MemoryFileV1 {
    version: u32,
    policy: MemoryPolicy,
    #[serde(default)]
    entries: Vec<MemoryEntry>,
}

#[derive(Debug, Default)]
/// 进程内缓存状态。
///
/// loaded 用于保证“只加载一次”，避免并发场景重复 I/O 与反序列化。
struct InMemory {
    loaded: bool,
    policy: MemoryPolicy,
    entries: Vec<MemoryEntry>,
}

#[derive(Clone)]
/// 文件后端的 MemoryStore 实现。
///
/// - path：memory_store.json（或同类文件）位置
/// - agents_md_path：额外导出的 AGENTS.md 路径
/// - default_policy：文件不存在时的初始策略
/// - state：共享状态（允许克隆 store 在多处使用）
pub struct FileMemoryStore {
    name: String,
    path: PathBuf,
    agents_md_path: PathBuf,
    default_policy: MemoryPolicy,
    state: Arc<Mutex<InMemory>>,
}

impl FileMemoryStore {
    /// 创建一个文件后端的 memory store。
    ///
    /// 默认会把 agents_md_path 设为与存储文件同目录下的 AGENTS.md。
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let agents_md_path = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("AGENTS.md");
        Self {
            name: "file".to_string(),
            path,
            agents_md_path,
            default_policy: MemoryPolicy::default(),
            state: Arc::new(Mutex::new(InMemory::default())),
        }
    }

    pub fn with_policy(mut self, policy: MemoryPolicy) -> Self {
        self.default_policy = policy;
        self
    }

    pub fn with_agents_md_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.agents_md_path = path.into();
        self
    }

    /// 持久化文件路径（通常是 memory_store.json）。
    pub fn store_path(&self) -> &Path {
        &self.path
    }

    /// AGENTS.md 输出路径。
    pub fn agents_md_path(&self) -> &Path {
        &self.agents_md_path
    }

    /// 把当前条目渲染为 AGENTS.md（会触发惰性加载）。
    pub async fn render_agents_md(&self) -> Result<(), MemoryError> {
        self.ensure_loaded().await?;
        let entries = {
            let guard = self.state.lock().unwrap();
            guard.entries.clone()
        };
        let md = render_agents_md_v1(&entries);
        write_atomic(&self.agents_md_path, md.as_bytes())
            .await
            .map_err(|e| {
                MemoryError::new(MemoryErrorCode::IoError, "failed to write AGENTS.md")
                    .with_source(e)
            })?;
        Ok(())
    }

    async fn ensure_loaded(&self) -> Result<(), MemoryError> {
        let already = { self.state.lock().unwrap().loaded };
        if already {
            return Ok(());
        }

        let (policy, entries) = match read_file_bytes(&self.path).await {
            Ok(Some(bytes)) => {
                let f: MemoryFileV1 = serde_json::from_slice(&bytes).map_err(|e| {
                    MemoryError::new(
                        MemoryErrorCode::Corrupt,
                        "failed to parse memory store file",
                    )
                    .with_source(anyhow::Error::new(e))
                })?;
                if f.version != 1 {
                    return Err(MemoryError::new(
                        MemoryErrorCode::Corrupt,
                        format!("unsupported_version: {}", f.version),
                    ));
                }
                (f.policy, f.entries)
            }
            Ok(None) => (self.default_policy.clone(), Vec::new()),
            Err(e) => {
                return Err(MemoryError::new(
                    MemoryErrorCode::IoError,
                    "failed to read memory store file",
                )
                .with_source(e))
            }
        };

        let mut guard = self.state.lock().unwrap();
        if !guard.loaded {
            guard.loaded = true;
            guard.policy = policy;
            guard.entries = entries;
        }
        Ok(())
    }

    /// 当前时间（RFC3339，秒级精度，带 Z）。
    fn now_rfc3339() -> String {
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    /// 粗略估算一个条目的“占用字节数”，用于配额判断。
    fn entry_size_bytes(e: &MemoryEntry) -> usize {
        e.key.len()
            + e.value.len()
            + e.title.as_deref().map(str::len).unwrap_or(0)
            + e.scope_id.as_deref().map(str::len).unwrap_or(0)
            + e.supersedes.as_deref().map(str::len).unwrap_or(0)
            + e.owner_user_id.as_deref().map(str::len).unwrap_or(0)
            + e.owner_workspace_id.as_deref().map(str::len).unwrap_or(0)
            + e.owner_channel_account_id.as_deref().map(str::len).unwrap_or(0)
            + e.tags.iter().map(|t| t.len()).sum::<usize>()
            + e.created_at.len()
            + e.updated_at.len()
            + e.last_accessed_at.len()
            + 16
    }

    fn bytes_total(entries: &[MemoryEntry]) -> usize {
        entries.iter().map(Self::entry_size_bytes).sum()
    }

    /// 解析 RFC3339 时间戳；解析失败返回 None（上层可用兜底策略）。
    fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    }

    /// 查询匹配：所有已设置条件都必须同时满足。
    fn matches_query(e: &MemoryEntry, q: &MemoryQuery) -> bool {
        if let Some(prefix) = &q.prefix {
            if !e.key.starts_with(prefix) {
                return false;
            }
        }
        if let Some(tag) = &q.tag {
            if !e.tags.iter().any(|t| t == tag) {
                return false;
            }
        }
        if let Some(scope) = q.scope {
            if e.scope != scope {
                return false;
            }
        }
        if let Some(scope_id) = &q.scope_id {
            if e.scope_id.as_deref() != Some(scope_id.as_str()) {
                return false;
            }
        }
        if let Some(memory_type) = q.memory_type {
            if e.memory_type != memory_type {
                return false;
            }
        }
        if let Some(pinned) = q.pinned {
            if e.pinned != pinned {
                return false;
            }
        }
        if let Some(status) = q.status {
            if e.status != status {
                return false;
            }
        } else if !q.include_inactive && e.status != MemoryStatus::Active {
            return false;
        }
        Self::is_visible_to_actor(e, q)
    }

    /// Scope-aware access control for query/list operations.
    ///
    /// The file store keeps this logic local so every CLI command sees one
    /// consistent visibility policy.
    fn is_visible_to_actor(e: &MemoryEntry, q: &MemoryQuery) -> bool {
        match e.scope {
            MemoryScope::User => match e.owner_user_id.as_deref() {
                Some(owner) => {
                    if let Some(actor) = q.actor_user_id.as_deref() {
                        owner == actor
                    } else if let Some(actor) = q.actor_channel_account_id.as_deref() {
                        e.owner_channel_account_id.as_deref() == Some(actor)
                    } else {
                        false
                    }
                }
                None => true,
            },
            MemoryScope::Thread => match e.scope_id.as_deref() {
                Some(thread_id) => {
                    let actor_thread = q
                        .actor_thread_id
                        .as_deref()
                        .or(q.scope_id.as_deref());
                    actor_thread == Some(thread_id)
                }
                None => true,
            },
            MemoryScope::Workspace => match e.owner_workspace_id.as_deref() {
                Some(workspace_id) => q.actor_workspace_id.as_deref() == Some(workspace_id),
                None => true,
            },
            MemoryScope::System => true,
        }
    }
}

#[async_trait::async_trait]
impl MemoryStore for FileMemoryStore {
    fn name(&self) -> &str {
        &self.name
    }

    fn policy(&self) -> MemoryPolicy {
        self.default_policy.clone()
    }

    /// 显式触发加载（通常也会在首次读写时自动触发）。
    async fn load(&self) -> Result<(), MemoryError> {
        self.ensure_loaded().await
    }

    /// 将当前状态刷盘为 JSON。
    async fn flush(&self) -> Result<(), MemoryError> {
        self.ensure_loaded().await?;
        let (policy, entries) = {
            let guard = self.state.lock().unwrap();
            (guard.policy.clone(), guard.entries.clone())
        };

        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                MemoryError::new(MemoryErrorCode::IoError, "failed to create store dir")
                    .with_source(anyhow::Error::new(e))
            })?;
        }

        let f = MemoryFileV1 {
            version: 1,
            policy,
            entries,
        };
        let bytes = serde_json::to_vec_pretty(&f).map_err(|e| {
            MemoryError::new(MemoryErrorCode::Corrupt, "failed to serialize store file")
                .with_source(anyhow::Error::new(e))
        })?;
        write_atomic(&self.path, &bytes).await.map_err(|e| {
            MemoryError::new(MemoryErrorCode::IoError, "failed to write store file").with_source(e)
        })?;
        Ok(())
    }

    /// 写入/更新条目。
    ///
    /// - 对同 key 的条目执行覆盖更新，并更新时间/访问统计字段
    /// - 写入后会按策略执行淘汰，确保不超过配额
    async fn put(&self, mut entry: MemoryEntry) -> Result<(), MemoryError> {
        self.ensure_loaded().await?;
        {
            let mut guard = self.state.lock().unwrap();
            let now = Self::now_rfc3339();
            if entry.created_at.is_empty() {
                entry.created_at = now.clone();
            }
            if entry.updated_at.is_empty() {
                entry.updated_at = now.clone();
            }
            if entry.last_accessed_at.is_empty() {
                entry.last_accessed_at = now.clone();
            }
            if entry.access_count == 0 {
                entry.access_count = 1;
            }

            if let Some(existing) = guard.entries.iter_mut().find(|e| e.key == entry.key) {
                existing.value = entry.value;
                existing.title = entry.title;
                existing.scope = entry.scope;
                existing.scope_id = entry.scope_id;
                existing.memory_type = entry.memory_type;
                existing.pinned = entry.pinned;
                existing.status = entry.status;
                existing.confidence = entry.confidence;
                existing.salience = entry.salience;
                existing.supersedes = entry.supersedes;
                existing.owner_user_id = entry.owner_user_id;
                existing.owner_workspace_id = entry.owner_workspace_id;
                existing.owner_channel_account_id = entry.owner_channel_account_id;
                existing.tags = entry.tags;
                existing.updated_at = now.clone();
                existing.last_accessed_at = now;
                existing.access_count = existing.access_count.saturating_add(1);
            } else {
                guard.entries.push(entry);
            }
        }
        let _ = self.evict_if_needed().await?;
        Ok(())
    }

    /// 获取条目（命中则更新 last_accessed_at 与 access_count）。
    async fn get(&self, key: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        self.ensure_loaded().await?;
        let mut guard = self.state.lock().unwrap();
        let now = Self::now_rfc3339();
        if let Some(e) = guard.entries.iter_mut().find(|e| e.key == key) {
            e.last_accessed_at = now;
            e.access_count = e.access_count.saturating_add(1);
            return Ok(Some(e.clone()));
        }
        Ok(None)
    }

    /// 查询条目：
    /// - 先过滤，再按 last_accessed_at 倒序（更“新”的优先返回）
    /// - 对返回集合中的条目同步更新访问统计
    async fn query(&self, q: MemoryQuery) -> Result<Vec<MemoryEntry>, MemoryError> {
        self.ensure_loaded().await?;
        let mut guard = self.state.lock().unwrap();
        let limit = q.limit.unwrap_or(50);
        let mut out = guard
            .entries
            .iter()
            .filter(|e| Self::matches_query(e, &q))
            .cloned()
            .collect::<Vec<_>>();

        out.sort_by(|a, b| {
            let at = Self::parse_ts(&a.last_accessed_at);
            let bt = Self::parse_ts(&b.last_accessed_at);
            bt.cmp(&at)
        });
        out.truncate(limit);

        let now = Self::now_rfc3339();
        for e in guard.entries.iter_mut() {
            if out.iter().any(|x| x.key == e.key) {
                e.last_accessed_at = now.clone();
                e.access_count = e.access_count.saturating_add(1);
            }
        }
        Ok(out)
    }

    /// 按策略淘汰直到满足 max_entries 与 max_bytes_total。
    ///
    /// 这里的实现刻意选择“简单循环”：每次移除 1 条最应该淘汰的记录，
    /// 直到配额满足或条目为空。对小规模条目（默认 200）可读性更重要。
    async fn evict_if_needed(&self) -> Result<MemoryEvictionReport, MemoryError> {
        self.ensure_loaded().await?;
        let mut guard = self.state.lock().unwrap();

        let before_entries = guard.entries.len();
        let before_bytes_total = Self::bytes_total(&guard.entries);
        let policy = guard.policy.clone();
        let mut evicted_keys: Vec<String> = Vec::new();

        loop {
            let bytes_total = Self::bytes_total(&guard.entries);
            let over_entries = guard.entries.len() > policy.max_entries;
            let over_bytes = bytes_total > policy.max_bytes_total;
            if !over_entries && !over_bytes {
                break;
            }
            if guard.entries.is_empty() {
                break;
            }

            let idx = match policy.eviction {
                MemoryEvictionPolicy::Fifo => guard
                    .entries
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, e)| Self::parse_ts(&e.created_at).unwrap_or_else(Utc::now))
                    .map(|(i, _)| i)
                    .unwrap_or(0),
                MemoryEvictionPolicy::Lru => guard
                    .entries
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, e)| {
                        Self::parse_ts(&e.last_accessed_at).unwrap_or_else(Utc::now)
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(0),
                MemoryEvictionPolicy::Ttl { ttl_secs } => {
                    let cutoff = Utc::now() - chrono::Duration::seconds(ttl_secs as i64);
                    let mut ttl_candidates: Vec<(usize, DateTime<Utc>)> = guard
                        .entries
                        .iter()
                        .enumerate()
                        .filter_map(|(i, e)| Self::parse_ts(&e.updated_at).map(|t| (i, t)))
                        .filter(|(_, t)| *t < cutoff)
                        .collect();
                    ttl_candidates.sort_by_key(|(_, t)| *t);
                    ttl_candidates.first().map(|(i, _)| *i).unwrap_or(0)
                }
            };
            let removed = guard.entries.remove(idx);
            evicted_keys.push(removed.key);
        }

        let after_entries = guard.entries.len();
        let after_bytes_total = Self::bytes_total(&guard.entries);
        Ok(MemoryEvictionReport {
            before_entries,
            after_entries,
            evicted: before_entries.saturating_sub(after_entries),
            evicted_keys,
            before_bytes_total,
            after_bytes_total,
        })
    }
}

/// 把 memory entries 渲染为 AGENTS.md 的 v1 片段。
fn render_agents_md_v1(entries: &[MemoryEntry]) -> String {
    let mut buf = String::new();
    buf.push_str("<auto_generated_memory_v1>\n");
    buf.push_str("This section is generated from memory_store.json.\n\n");
    for e in entries {
        if e.status != MemoryStatus::Active {
            continue;
        }
        buf.push_str("## ");
        buf.push_str(&e.key);
        buf.push('\n');
        buf.push_str(e.value.trim());
        buf.push_str("\n\n");
    }
    buf.push_str("</auto_generated_memory_v1>\n");
    buf
}

/// 读取文件为字节；如果文件不存在则返回 Ok(None)。
async fn read_file_bytes(path: &Path) -> Result<Option<Vec<u8>>, anyhow::Error> {
    match tokio::fs::read(path).await {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::new(e)),
    }
}

/// 原子写入：写入临时文件后 rename 覆盖目标文件。
async fn write_atomic(path: &Path, content: &[u8]) -> Result<(), anyhow::Error> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    tokio::fs::create_dir_all(parent).await?;
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, content).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}
