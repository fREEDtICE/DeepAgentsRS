use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentState {
    #[serde(default)]
    pub filesystem: FilesystemState,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FilesystemState {
    #[serde(default)]
    pub files: BTreeMap<String, FileRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    #[serde(default)]
    pub content: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<String>,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FilesystemDelta {
    #[serde(default)]
    pub files: BTreeMap<String, FileDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upsert: Option<FileRecord>,
    #[serde(default)]
    pub delete: bool,
}

pub trait StateReducer<S, D>: Send + Sync {
    fn reduce(&self, state: &mut S, delta: D);
}

#[derive(Debug, Clone, Default)]
pub struct DefaultFilesystemReducer;

impl StateReducer<FilesystemState, FilesystemDelta> for DefaultFilesystemReducer {
    fn reduce(&self, state: &mut FilesystemState, delta: FilesystemDelta) {
        for (path, d) in delta.files {
            if d.delete {
                let rec = state.files.entry(path).or_insert_with(|| FileRecord {
                    content: Vec::new(),
                    created_at: None,
                    modified_at: None,
                    deleted: true,
                    truncated: false,
                });
                if let Some(upsert) = d.upsert {
                    *rec = upsert;
                } else {
                    rec.content.clear();
                    rec.deleted = true;
                    rec.truncated = false;
                }
                continue;
            }
            if let Some(upsert) = d.upsert {
                state.files.insert(path, upsert);
            }
        }
    }
}
