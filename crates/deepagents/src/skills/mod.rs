pub mod declarative;
pub mod loader;
pub mod protocol;
pub mod validator;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use protocol::{SkillCall, SkillError, SkillPlugin, SkillSpec};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub path: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub steps: Vec<SkillToolStep>,
    pub policy: SkillToolPolicy,
    pub skill_name: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillToolStep {
    pub tool_name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillToolPolicy {
    pub allow_filesystem: bool,
    pub allow_execute: bool,
    pub allow_network: bool,
    pub max_steps: usize,
    pub timeout_ms: u64,
    pub max_output_chars: usize,
}

impl Default for SkillToolPolicy {
    fn default() -> Self {
        Self {
            allow_filesystem: false,
            allow_execute: false,
            allow_network: false,
            max_steps: 8,
            timeout_ms: 1000,
            max_output_chars: 12000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillsDiagnostics {
    #[serde(default)]
    pub sources: Vec<SkillSourceDiagnostics>,
    #[serde(default)]
    pub overrides: Vec<SkillOverrideRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSourceDiagnostics {
    pub source: String,
    pub loaded: usize,
    pub skipped: usize,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillOverrideRecord {
    pub name: String,
    pub overridden_source: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoadedSkills {
    #[serde(default)]
    pub metadata: Vec<SkillMetadata>,
    #[serde(default)]
    pub tools: Vec<SkillToolSpec>,
    #[serde(default)]
    pub diagnostics: SkillsDiagnostics,
}
