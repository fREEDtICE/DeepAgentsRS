use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct CacheInsertResult {
    pub inserted: bool,
    pub evicted: u64,
}

#[derive(Debug, Clone)]
pub struct CacheLookupResult<V> {
    pub value: Option<V>,
    pub expired: bool,
}

/// A small, thread-safe key-value cache with TTL and bounded capacity semantics.
pub trait CacheStore<V>: Send + Sync {
    fn get(&self, key: &str) -> CacheLookupResult<V>;
    fn insert(&self, key: String, value: V) -> CacheInsertResult;
}

struct MemoryEntry<V> {
    value: V,
    expires_at: Instant,
}

struct MemoryCacheInner<V> {
    entries: HashMap<String, MemoryEntry<V>>,
    lru: VecDeque<String>,
}

pub struct MemoryCacheStore<V> {
    inner: Mutex<MemoryCacheInner<V>>,
    ttl: Duration,
    max_entries: usize,
}

impl<V> MemoryCacheStore<V> {
    pub fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            inner: Mutex::new(MemoryCacheInner {
                entries: HashMap::new(),
                lru: VecDeque::new(),
            }),
            ttl,
            max_entries: max_entries.max(1),
        }
    }

    fn touch(inner: &mut MemoryCacheInner<V>, key: &str) {
        if inner.lru.is_empty() {
            inner.lru.push_back(key.to_string());
            return;
        }
        inner.lru.retain(|k| k != key);
        inner.lru.push_back(key.to_string());
    }

    fn purge_key(inner: &mut MemoryCacheInner<V>, key: &str) {
        inner.entries.remove(key);
        inner.lru.retain(|k| k != key);
    }
}

impl<V: Clone + Send> CacheStore<V> for MemoryCacheStore<V> {
    fn get(&self, key: &str) -> CacheLookupResult<V> {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap();
        let Some((expires_at, value)) = inner
            .entries
            .get(key)
            .map(|e| (e.expires_at, e.value.clone()))
        else {
            return CacheLookupResult {
                value: None,
                expired: false,
            };
        };
        if now >= expires_at {
            Self::purge_key(&mut inner, key);
            return CacheLookupResult {
                value: None,
                expired: true,
            };
        }
        Self::touch(&mut inner, key);
        CacheLookupResult {
            value: Some(value),
            expired: false,
        }
    }

    fn insert(&self, key: String, value: V) -> CacheInsertResult {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap();
        let expires_at = now + self.ttl;
        let inserted = !inner.entries.contains_key(&key);
        inner
            .entries
            .insert(key.clone(), MemoryEntry { value, expires_at });
        Self::touch(&mut inner, &key);

        let mut evicted = 0u64;
        while inner.entries.len() > self.max_entries {
            let Some(oldest) = inner.lru.pop_front() else {
                break;
            };
            if inner.entries.remove(&oldest).is_some() {
                evicted += 1;
            }
        }
        CacheInsertResult { inserted, evicted }
    }
}
