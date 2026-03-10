use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};

use crate::memory::protocol::{
    MemoryEntry, MemoryError, MemoryErrorCode, MemoryEvictionPolicy, MemoryEvictionReport,
    MemoryPolicy, MemoryQuery, MemoryStore,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryFileV1 {
    version: u32,
    policy: MemoryPolicy,
    #[serde(default)]
    entries: Vec<MemoryEntry>,
}

#[derive(Debug, Default)]
struct InMemory {
    loaded: bool,
    policy: MemoryPolicy,
    entries: Vec<MemoryEntry>,
}

#[derive(Clone)]
pub struct FileMemoryStore {
    name: String,
    path: PathBuf,
    agents_md_path: PathBuf,
    default_policy: MemoryPolicy,
    state: Arc<Mutex<InMemory>>,
}

impl FileMemoryStore {
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

    pub fn store_path(&self) -> &Path {
        &self.path
    }

    pub fn agents_md_path(&self) -> &Path {
        &self.agents_md_path
    }

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

    fn now_rfc3339() -> String {
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    fn entry_size_bytes(e: &MemoryEntry) -> usize {
        e.key.len()
            + e.value.len()
            + e.tags.iter().map(|t| t.len()).sum::<usize>()
            + e.created_at.len()
            + e.updated_at.len()
            + e.last_accessed_at.len()
            + 16
    }

    fn bytes_total(entries: &[MemoryEntry]) -> usize {
        entries.iter().map(Self::entry_size_bytes).sum()
    }

    fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    }

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
        true
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

    async fn load(&self) -> Result<(), MemoryError> {
        self.ensure_loaded().await
    }

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

fn render_agents_md_v1(entries: &[MemoryEntry]) -> String {
    let mut buf = String::new();
    buf.push_str("<auto_generated_memory_v1>\n");
    buf.push_str("This section is generated from memory_store.json.\n\n");
    for e in entries {
        buf.push_str("## ");
        buf.push_str(&e.key);
        buf.push('\n');
        buf.push_str(e.value.trim());
        buf.push_str("\n\n");
    }
    buf.push_str("</auto_generated_memory_v1>\n");
    buf
}

async fn read_file_bytes(path: &Path) -> Result<Option<Vec<u8>>, anyhow::Error> {
    match tokio::fs::read(path).await {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::new(e)),
    }
}

async fn write_atomic(path: &Path, content: &[u8]) -> Result<(), anyhow::Error> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    tokio::fs::create_dir_all(parent).await?;
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, content).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}
