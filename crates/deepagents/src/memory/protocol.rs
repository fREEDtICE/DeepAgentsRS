use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryPolicy {
    pub max_entries: usize,
    pub max_bytes_total: usize,
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
pub enum MemoryEvictionPolicy {
    Lru,
    Fifo,
    Ttl { ttl_secs: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryEntry {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_accessed_at: String,
    pub access_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct MemoryQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
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
pub struct MemoryDiagnostics {
    pub loaded_sources: usize,
    pub skipped_not_found: usize,
    #[serde(default)]
    pub errors: Vec<String>,
    pub truncated: bool,
    pub injected_chars: usize,
    pub combined_chars: usize,
}

#[derive(Debug, Error)]
#[error("{code}: {message}")]
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
