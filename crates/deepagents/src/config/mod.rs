use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::approval::ExecutionMode;
use crate::provider::catalog::ProviderSurfaceKind;

const CONFIG_SCHEMA_VERSION: u32 = 1;
const CONFIG_DOCUMENT_VERSION: u32 = 1;
const BUILTIN_SCHEMA: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/schema/config-schema.toml"
));

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigScope {
    Global,
    Workspace,
    Effective,
}

impl ConfigScope {
    pub fn parse(input: &str) -> Result<Self, ConfigError> {
        match input {
            "global" => Ok(Self::Global),
            "workspace" => Ok(Self::Workspace),
            "effective" => Ok(Self::Effective),
            other => Err(ConfigError::invalid_request(format!(
                "unknown config scope: {other}"
            ))),
        }
    }

    fn is_storage_scope(self) -> bool {
        matches!(self, Self::Global | Self::Workspace)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ConfigKey(String);

impl ConfigKey {
    pub fn parse(input: impl Into<String>) -> Result<Self, ConfigError> {
        let input = input.into();
        let mut segments = input.split('.');
        let first = segments
            .next()
            .filter(|segment| !segment.is_empty())
            .ok_or_else(|| ConfigError::invalid_key("config key cannot be empty"))?;
        validate_key_segment(first)?;
        for segment in segments {
            if segment.is_empty() {
                return Err(ConfigError::invalid_key(
                    "config key contains empty segment",
                ));
            }
            validate_key_segment(segment)?;
        }
        Ok(Self(input))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('.')
    }
}

impl std::fmt::Display for ConfigKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EnvVarName(String);

impl EnvVarName {
    pub fn parse(input: impl Into<String>) -> Result<Self, ConfigError> {
        let input = input.into();
        let mut chars = input.chars();
        let Some(first) = chars.next() else {
            return Err(ConfigError::invalid_value("env var name cannot be empty"));
        };
        if !(first.is_ascii_alphabetic() || first == '_') {
            return Err(ConfigError::invalid_value(
                "env var name must start with an ASCII letter or underscore",
            ));
        }
        if !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
            return Err(ConfigError::invalid_value(
                "env var name must contain only ASCII letters, digits, or underscore",
            ));
        }
        Ok(Self(input))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for EnvVarName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConfigValue {
    String(String),
    Boolean(bool),
    Integer(i64),
    StringList(Vec<String>),
}

impl ConfigValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_string_list(&self) -> Option<&[String]> {
        match self {
            Self::StringList(values) => Some(values),
            _ => None,
        }
    }

    fn to_toml(&self) -> toml::Value {
        match self {
            Self::String(value) => toml::Value::String(value.clone()),
            Self::Boolean(value) => toml::Value::Boolean(*value),
            Self::Integer(value) => toml::Value::Integer(*value),
            Self::StringList(values) => toml::Value::Array(
                values
                    .iter()
                    .map(|value| toml::Value::String(value.clone()))
                    .collect(),
            ),
        }
    }

    fn from_toml(value: &toml::Value) -> Result<Self, ConfigError> {
        match value {
            toml::Value::String(value) => Ok(Self::String(value.clone())),
            toml::Value::Boolean(value) => Ok(Self::Boolean(*value)),
            toml::Value::Integer(value) => Ok(Self::Integer(*value)),
            toml::Value::Array(values) => {
                let mut out = Vec::with_capacity(values.len());
                for value in values {
                    let Some(value) = value.as_str() else {
                        return Err(ConfigError::invalid_value(
                            "only arrays of strings are supported",
                        ));
                    };
                    out.push(value.to_string());
                }
                Ok(Self::StringList(out))
            }
            _ => Err(ConfigError::invalid_value(
                "unsupported config value type in TOML document",
            )),
        }
    }

    pub fn from_json(value: serde_json::Value) -> Result<Self, ConfigError> {
        match value {
            serde_json::Value::String(value) => Ok(Self::String(value)),
            serde_json::Value::Bool(value) => Ok(Self::Boolean(value)),
            serde_json::Value::Number(value) => value
                .as_i64()
                .map(Self::Integer)
                .ok_or_else(|| ConfigError::invalid_value("config integer must fit in i64")),
            serde_json::Value::Array(values) => {
                let mut out = Vec::with_capacity(values.len());
                for value in values {
                    let Some(value) = value.as_str() else {
                        return Err(ConfigError::invalid_value(
                            "config arrays must contain strings only",
                        ));
                    };
                    out.push(value.to_string());
                }
                Ok(Self::StringList(out))
            }
            _ => Err(ConfigError::invalid_value(
                "unsupported JSON config value type",
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigDocument {
    pub version: u32,
    #[serde(default)]
    pub values: toml::map::Map<String, toml::Value>,
}

impl Default for ConfigDocument {
    fn default() -> Self {
        Self {
            version: CONFIG_DOCUMENT_VERSION,
            values: toml::map::Map::new(),
        }
    }
}

impl ConfigDocument {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => {
                return Err(ConfigError::io_error(format!(
                    "failed to read config document: {}",
                    path.display()
                ))
                .with_source(err))
            }
        };
        let raw: RawConfigDocument =
            toml::from_str(std::str::from_utf8(&bytes).map_err(|err| {
                ConfigError::schema_invalid(format!(
                    "config document is not valid UTF-8: {}",
                    path.display()
                ))
                .with_source(err)
            })?)
            .map_err(|err| {
                ConfigError::schema_invalid(format!(
                    "failed to parse config document: {}",
                    path.display()
                ))
                .with_source(err)
            })?;
        if let Some(version) = raw.version {
            if version != CONFIG_DOCUMENT_VERSION {
                return Err(ConfigError::unsupported_version(version));
            }
        }
        Ok(Self {
            version: raw.version.unwrap_or(CONFIG_DOCUMENT_VERSION),
            values: raw.values,
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let raw = RawConfigDocument {
            version: Some(self.version),
            values: self.values.clone(),
        };
        let content = toml::to_string_pretty(&raw).map_err(|err| {
            ConfigError::schema_invalid("failed to serialize config document").with_source(err)
        })?;
        write_atomic(path, content.as_bytes())
    }

    pub fn get(&self, key: &ConfigKey) -> Result<Option<ConfigValue>, ConfigError> {
        let mut current = &self.values;
        let mut segments = key.segments().peekable();
        while let Some(segment) = segments.next() {
            let Some(value) = current.get(segment) else {
                return Ok(None);
            };
            if segments.peek().is_none() {
                return ConfigValue::from_toml(value).map(Some);
            }
            let Some(table) = value.as_table() else {
                return Err(ConfigError::invalid_value(format!(
                    "config path is not a table at {segment}"
                )));
            };
            current = table;
        }
        Ok(None)
    }

    pub fn set(&mut self, key: &ConfigKey, value: ConfigValue) -> Result<(), ConfigError> {
        set_nested_value(
            &mut self.values,
            &key.segments().collect::<Vec<_>>(),
            value.to_toml(),
        );
        Ok(())
    }

    pub fn unset(&mut self, key: &ConfigKey) {
        unset_nested_value(&mut self.values, &key.segments().collect::<Vec<_>>());
    }

    pub fn flatten_keys(&self) -> Vec<String> {
        let mut out = Vec::new();
        flatten_table_keys("", &self.values, &mut out);
        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSchema {
    pub version: u32,
    pub fields: Vec<SchemaField>,
}

impl ConfigSchema {
    pub fn builtin() -> Result<Self, ConfigError> {
        let raw: RawConfigSchema = toml::from_str(BUILTIN_SCHEMA).map_err(|err| {
            ConfigError::schema_invalid("failed to parse built-in config schema").with_source(err)
        })?;
        if raw.version != CONFIG_SCHEMA_VERSION {
            return Err(ConfigError::schema_invalid(format!(
                "unsupported config schema version: {}",
                raw.version
            )));
        }
        let mut seen = BTreeSet::new();
        let mut fields = Vec::with_capacity(raw.fields.len());
        for raw_field in raw.fields {
            let field = SchemaField::from_raw(raw_field)?;
            if !seen.insert(field.key.clone()) {
                return Err(ConfigError::schema_invalid(format!(
                    "duplicate config schema key: {}",
                    field.key
                )));
            }
            fields.push(field);
        }
        Ok(Self {
            version: raw.version,
            fields,
        })
    }

    pub fn field(&self, key: &ConfigKey) -> Option<&SchemaField> {
        self.fields.iter().find(|field| field.key == *key)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaField {
    pub key: ConfigKey,
    pub kind: SchemaValueKind,
    #[serde(default)]
    pub scopes: Vec<ConfigScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<ConfigValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<i64>,
}

impl SchemaField {
    fn from_raw(raw: RawSchemaField) -> Result<Self, ConfigError> {
        let key = ConfigKey::parse(raw.key)?;
        let kind = SchemaValueKind::parse(&raw.kind)?;
        let mut scopes = Vec::with_capacity(raw.scopes.len());
        for scope in raw.scopes {
            let scope = ConfigScope::parse(&scope)?;
            if !scope.is_storage_scope() {
                return Err(ConfigError::schema_invalid(format!(
                    "schema field {} cannot declare effective as storage scope",
                    key
                )));
            }
            scopes.push(scope);
        }
        if scopes.is_empty() {
            return Err(ConfigError::schema_invalid(format!(
                "schema field {} must declare at least one storage scope",
                key
            )));
        }
        let default = match raw.default {
            Some(value) => Some(ConfigValue::from_toml(&value)?),
            None => None,
        };
        let field = Self {
            key,
            kind,
            scopes,
            default,
            enum_values: raw.enum_values.unwrap_or_default(),
            min: raw.min,
            max: raw.max,
        };
        if let Some(value) = field.default.as_ref() {
            field.validate_value(value)?;
        }
        Ok(field)
    }

    pub fn validate_value(&self, value: &ConfigValue) -> Result<(), ConfigError> {
        match self.kind {
            SchemaValueKind::String | SchemaValueKind::Path => {
                let Some(value) = value.as_str() else {
                    return Err(ConfigError::invalid_value(format!(
                        "{} expects a string value",
                        self.key
                    )));
                };
                if value.is_empty() {
                    return Err(ConfigError::invalid_value(format!(
                        "{} cannot be empty",
                        self.key
                    )));
                }
            }
            SchemaValueKind::Boolean => {
                if value.as_bool().is_none() {
                    return Err(ConfigError::invalid_value(format!(
                        "{} expects a boolean value",
                        self.key
                    )));
                }
            }
            SchemaValueKind::Integer => {
                let Some(value) = value.as_i64() else {
                    return Err(ConfigError::invalid_value(format!(
                        "{} expects an integer value",
                        self.key
                    )));
                };
                if let Some(min) = self.min {
                    if value < min {
                        return Err(ConfigError::invalid_value(format!(
                            "{} must be >= {}",
                            self.key, min
                        )));
                    }
                }
                if let Some(max) = self.max {
                    if value > max {
                        return Err(ConfigError::invalid_value(format!(
                            "{} must be <= {}",
                            self.key, max
                        )));
                    }
                }
            }
            SchemaValueKind::StringList => {
                if value.as_string_list().is_none() {
                    return Err(ConfigError::invalid_value(format!(
                        "{} expects an array of strings",
                        self.key
                    )));
                }
            }
            SchemaValueKind::Enum => {
                let Some(value) = value.as_str() else {
                    return Err(ConfigError::invalid_value(format!(
                        "{} expects a string enum value",
                        self.key
                    )));
                };
                if !self.enum_values.iter().any(|candidate| candidate == value) {
                    return Err(ConfigError::invalid_value(format!(
                        "{} must be one of {:?}",
                        self.key, self.enum_values
                    )));
                }
            }
            SchemaValueKind::EnvVar => {
                let Some(value) = value.as_str() else {
                    return Err(ConfigError::invalid_value(format!(
                        "{} expects an env var name",
                        self.key
                    )));
                };
                let _ = EnvVarName::parse(value.to_string())?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaValueKind {
    String,
    Boolean,
    Integer,
    StringList,
    Enum,
    Path,
    EnvVar,
}

impl SchemaValueKind {
    fn parse(input: &str) -> Result<Self, ConfigError> {
        match input {
            "string" => Ok(Self::String),
            "boolean" => Ok(Self::Boolean),
            "integer" => Ok(Self::Integer),
            "string_list" => Ok(Self::StringList),
            "enum" => Ok(Self::Enum),
            "path" => Ok(Self::Path),
            "env_var" => Ok(Self::EnvVar),
            other => Err(ConfigError::schema_invalid(format!(
                "unknown config schema kind: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigOrigin {
    Default,
    Global,
    Workspace,
    Override,
    Unset,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedField {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<ConfigValue>,
    pub origin: ConfigOrigin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_status: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigOverrides {
    #[serde(default)]
    values: BTreeMap<ConfigKey, ConfigValue>,
}

impl ConfigOverrides {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, key: ConfigKey, value: ConfigValue) {
        self.values.insert(key, value);
    }

    pub fn extend(&mut self, other: Self) {
        for (key, value) in other.values {
            self.values.insert(key, value);
        }
    }

    fn get(&self, key: &ConfigKey) -> Option<&ConfigValue> {
        self.values.get(key)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveConfig {
    pub providers: BTreeMap<String, ProviderConfig>,
    pub memory: MemoryConfig,
    pub runtime: RuntimeConfig,
    pub security: SecurityConfig,
    pub audit: AuditConfig,
}

impl EffectiveConfig {
    pub fn provider(&self, provider_id: &str) -> Option<&ProviderConfig> {
        self.providers.get(provider_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_id: String,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<ProviderSurfaceKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<EnvVarName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryBackendKind {
    File,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub enabled: bool,
    pub backend: MemoryBackendKind,
    pub store_path: String,
    pub sources: Vec<String>,
    pub allow_host_paths: bool,
    pub max_injected_chars: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCacheBackendKind {
    Off,
    Memory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCacheNativeMode {
    Auto,
    Off,
    Required,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCacheLayoutMode {
    Auto,
    SingleSystem,
    PreservePrefixSegments,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptCacheConfig {
    pub backend: PromptCacheBackendKind,
    pub native: PromptCacheNativeMode,
    pub layout: PromptCacheLayoutMode,
    pub l2: bool,
    pub ttl_ms: u64,
    pub max_entries: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummarizationConfig {
    pub enabled: bool,
    pub max_char_budget: usize,
    pub max_turns_visible: usize,
    pub min_recent_messages: usize,
    pub redact_tool_args: bool,
    pub max_tool_arg_chars: usize,
    pub truncate_keep_last: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    pub max_steps: usize,
    pub provider_timeout_ms: u64,
    pub prompt_cache: PromptCacheConfig,
    pub summarization: SummarizationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub execution_mode: ExecutionMode,
    pub shell_allow_list: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jsonl_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigDoctorReport {
    pub issues: Vec<ConfigIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigIssue {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub message: String,
}

#[derive(Debug, Error)]
#[error("{code}: {message}")]
pub struct ConfigError {
    pub code: &'static str,
    pub message: String,
    #[source]
    pub source: Option<anyhow::Error>,
}

impl ConfigError {
    fn invalid_key(message: impl Into<String>) -> Self {
        Self {
            code: "config_invalid_key",
            message: message.into(),
            source: None,
        }
    }

    fn invalid_value(message: impl Into<String>) -> Self {
        Self {
            code: "config_invalid_value",
            message: message.into(),
            source: None,
        }
    }

    fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_request",
            message: message.into(),
            source: None,
        }
    }

    fn schema_invalid(message: impl Into<String>) -> Self {
        Self {
            code: "config_schema_invalid",
            message: message.into(),
            source: None,
        }
    }

    fn unsupported_version(version: u32) -> Self {
        Self {
            code: "config_version_unsupported",
            message: format!("unsupported config document version: {version}"),
            source: None,
        }
    }

    fn io_error(message: impl Into<String>) -> Self {
        Self {
            code: "config_io_error",
            message: message.into(),
            source: None,
        }
    }

    fn with_source<E>(mut self, error: E) -> Self
    where
        E: Into<anyhow::Error>,
    {
        self.source = Some(error.into());
        self
    }
}

#[derive(Debug, Clone)]
pub struct ConfigManager {
    root: PathBuf,
    schema: ConfigSchema,
}

impl ConfigManager {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        Ok(Self {
            root: root.into(),
            schema: ConfigSchema::builtin()?,
        })
    }

    pub fn schema(&self) -> &ConfigSchema {
        &self.schema
    }

    pub fn global_config_path() -> Result<PathBuf, ConfigError> {
        if let Some(base) = std::env::var_os("DEEPAGENTS_CONFIG_HOME") {
            return Ok(PathBuf::from(base).join("config.toml"));
        }
        if let Some(base) = std::env::var_os("XDG_CONFIG_HOME") {
            return Ok(PathBuf::from(base).join("deepagents").join("config.toml"));
        }
        let Some(home) = std::env::var_os("HOME") else {
            return Err(ConfigError::io_error(
                "cannot determine global config path: HOME is not set",
            ));
        };
        Ok(PathBuf::from(home)
            .join(".config")
            .join("deepagents")
            .join("config.toml"))
    }

    pub fn workspace_config_path(&self) -> PathBuf {
        self.root.join(".deepagents").join("config.toml")
    }

    pub fn list(
        &self,
        scope: ConfigScope,
        overrides: &ConfigOverrides,
    ) -> Result<Vec<ResolvedField>, ConfigError> {
        let global_doc = self.load_scope_document(ConfigScope::Global)?;
        let workspace_doc = self.load_scope_document(ConfigScope::Workspace)?;
        let mut entries = Vec::with_capacity(self.schema.fields.len());
        for field in &self.schema.fields {
            entries.push(self.resolve_field(
                field,
                scope,
                &global_doc,
                &workspace_doc,
                overrides,
            )?);
        }
        Ok(entries)
    }

    pub fn get(
        &self,
        scope: ConfigScope,
        key: &ConfigKey,
        overrides: &ConfigOverrides,
    ) -> Result<ResolvedField, ConfigError> {
        let field = self
            .schema
            .field(key)
            .ok_or_else(|| ConfigError::invalid_request(format!("unknown config key: {key}")))?;
        let global_doc = self.load_scope_document(ConfigScope::Global)?;
        let workspace_doc = self.load_scope_document(ConfigScope::Workspace)?;
        self.resolve_field(field, scope, &global_doc, &workspace_doc, overrides)
    }

    pub fn parse_cli_value(&self, key: &ConfigKey, raw: &str) -> Result<ConfigValue, ConfigError> {
        let field = self
            .schema
            .field(key)
            .ok_or_else(|| ConfigError::invalid_request(format!("unknown config key: {key}")))?;
        let value = match field.kind {
            SchemaValueKind::String | SchemaValueKind::Path | SchemaValueKind::Enum => {
                ConfigValue::String(raw.to_string())
            }
            SchemaValueKind::EnvVar => ConfigValue::String(raw.to_string()),
            SchemaValueKind::Boolean => ConfigValue::Boolean(
                raw.parse::<bool>()
                    .map_err(|_| ConfigError::invalid_value("expected true or false"))?,
            ),
            SchemaValueKind::Integer => ConfigValue::Integer(
                raw.parse::<i64>()
                    .map_err(|_| ConfigError::invalid_value("expected integer value"))?,
            ),
            SchemaValueKind::StringList => {
                if raw.trim_start().starts_with('[') {
                    ConfigValue::from_json(serde_json::from_str(raw).map_err(|err| {
                        ConfigError::invalid_value(format!("invalid JSON array: {err}"))
                    })?)?
                } else if raw.trim().is_empty() {
                    ConfigValue::StringList(Vec::new())
                } else {
                    ConfigValue::StringList(
                        raw.split(',')
                            .map(|value| value.trim().to_string())
                            .filter(|value| !value.is_empty())
                            .collect(),
                    )
                }
            }
        };
        field.validate_value(&value)?;
        Ok(value)
    }

    pub fn parse_json_value(
        &self,
        key: &ConfigKey,
        raw: serde_json::Value,
    ) -> Result<ConfigValue, ConfigError> {
        let field = self
            .schema
            .field(key)
            .ok_or_else(|| ConfigError::invalid_request(format!("unknown config key: {key}")))?;
        let value = ConfigValue::from_json(raw)?;
        field.validate_value(&value)?;
        Ok(value)
    }

    pub fn set(
        &self,
        scope: ConfigScope,
        key: &ConfigKey,
        value: ConfigValue,
    ) -> Result<(), ConfigError> {
        if !scope.is_storage_scope() {
            return Err(ConfigError::invalid_request(
                "config set only supports global or workspace scope",
            ));
        }
        let field = self
            .schema
            .field(key)
            .ok_or_else(|| ConfigError::invalid_request(format!("unknown config key: {key}")))?;
        if !field.scopes.contains(&scope) {
            return Err(ConfigError::invalid_request(format!(
                "config key {} is not allowed in {:?} scope",
                key, scope
            )));
        }
        field.validate_value(&value)?;
        let mut doc = self.load_scope_document(scope)?;
        doc.set(key, value)?;
        let path = self.scope_path(scope)?;
        doc.save(&path)
    }

    pub fn unset(&self, scope: ConfigScope, key: &ConfigKey) -> Result<(), ConfigError> {
        if !scope.is_storage_scope() {
            return Err(ConfigError::invalid_request(
                "config unset only supports global or workspace scope",
            ));
        }
        let mut doc = self.load_scope_document(scope)?;
        doc.unset(key);
        let path = self.scope_path(scope)?;
        doc.save(&path)
    }

    pub fn resolve_effective(
        &self,
        overrides: &ConfigOverrides,
    ) -> Result<EffectiveConfig, ConfigError> {
        let global_doc = self.load_scope_document(ConfigScope::Global)?;
        let workspace_doc = self.load_scope_document(ConfigScope::Workspace)?;
        let values = self.collect_effective_values(&global_doc, &workspace_doc, overrides)?;

        let mut providers = BTreeMap::new();
        for provider_id in self.provider_ids() {
            let enabled = values
                .get(&format!("providers.{provider_id}.enabled"))
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let surface = values
                .get(&format!("providers.{provider_id}.surface"))
                .and_then(|value| value.as_str())
                .map(parse_provider_surface)
                .transpose()?;
            let base_url = values
                .get(&format!("providers.{provider_id}.base_url"))
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let api_key_env = values
                .get(&format!("providers.{provider_id}.api_key_env"))
                .and_then(|value| value.as_str())
                .map(|value| EnvVarName::parse(value.to_string()))
                .transpose()?;
            let model = values
                .get(&format!("providers.{provider_id}.model"))
                .and_then(|value| value.as_str())
                .map(str::to_string);
            providers.insert(
                provider_id.clone(),
                ProviderConfig {
                    provider_id,
                    enabled,
                    surface,
                    base_url,
                    api_key_env,
                    model,
                },
            );
        }

        Ok(EffectiveConfig {
            providers,
            memory: MemoryConfig {
                enabled: values
                    .get("memory.file.enabled")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(true),
                backend: parse_memory_backend(
                    values
                        .get("memory.backend")
                        .and_then(|value| value.as_str())
                        .unwrap_or("file"),
                )?,
                store_path: values
                    .get("memory.file.store_path")
                    .and_then(|value| value.as_str())
                    .unwrap_or(".deepagents/memory_store.json")
                    .to_string(),
                sources: values
                    .get("memory.file.sources")
                    .and_then(|value| value.as_string_list())
                    .unwrap_or(&[])
                    .to_vec(),
                allow_host_paths: values
                    .get("memory.file.allow_host_paths")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false),
                max_injected_chars: value_to_usize(
                    values
                        .get("memory.file.max_injected_chars")
                        .and_then(|value| value.as_i64())
                        .unwrap_or(30_000),
                )?,
            },
            runtime: RuntimeConfig {
                max_steps: value_to_usize(
                    values
                        .get("runtime.max_steps")
                        .and_then(|value| value.as_i64())
                        .unwrap_or(8),
                )?,
                provider_timeout_ms: value_to_u64(
                    values
                        .get("runtime.provider_timeout_ms")
                        .and_then(|value| value.as_i64())
                        .unwrap_or(1000),
                )?,
                prompt_cache: PromptCacheConfig {
                    backend: parse_prompt_cache_backend(
                        values
                            .get("runtime.prompt_cache.backend")
                            .and_then(|value| value.as_str())
                            .unwrap_or("off"),
                    )?,
                    native: parse_prompt_cache_native(
                        values
                            .get("runtime.prompt_cache.native")
                            .and_then(|value| value.as_str())
                            .unwrap_or("auto"),
                    )?,
                    layout: parse_prompt_cache_layout(
                        values
                            .get("runtime.prompt_cache.layout")
                            .and_then(|value| value.as_str())
                            .unwrap_or("auto"),
                    )?,
                    l2: values
                        .get("runtime.prompt_cache.l2")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false),
                    ttl_ms: value_to_u64(
                        values
                            .get("runtime.prompt_cache.ttl_ms")
                            .and_then(|value| value.as_i64())
                            .unwrap_or(300_000),
                    )?,
                    max_entries: value_to_usize(
                        values
                            .get("runtime.prompt_cache.max_entries")
                            .and_then(|value| value.as_i64())
                            .unwrap_or(1024),
                    )?,
                },
                summarization: SummarizationConfig {
                    enabled: values
                        .get("runtime.summarization.enabled")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(true),
                    max_char_budget: value_to_usize(
                        values
                            .get("runtime.summarization.max_char_budget")
                            .and_then(|value| value.as_i64())
                            .unwrap_or(12_000),
                    )?,
                    max_turns_visible: value_to_usize(
                        values
                            .get("runtime.summarization.max_turns_visible")
                            .and_then(|value| value.as_i64())
                            .unwrap_or(12),
                    )?,
                    min_recent_messages: value_to_usize(
                        values
                            .get("runtime.summarization.min_recent_messages")
                            .and_then(|value| value.as_i64())
                            .unwrap_or(3),
                    )?,
                    redact_tool_args: values
                        .get("runtime.summarization.redact_tool_args")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(true),
                    max_tool_arg_chars: value_to_usize(
                        values
                            .get("runtime.summarization.max_tool_arg_chars")
                            .and_then(|value| value.as_i64())
                            .unwrap_or(2000),
                    )?,
                    truncate_keep_last: value_to_usize(
                        values
                            .get("runtime.summarization.truncate_keep_last")
                            .and_then(|value| value.as_i64())
                            .unwrap_or(6),
                    )?,
                },
            },
            security: SecurityConfig {
                execution_mode: parse_execution_mode(
                    values
                        .get("security.execution_mode")
                        .and_then(|value| value.as_str())
                        .unwrap_or("non_interactive"),
                )?,
                shell_allow_list: values
                    .get("security.shell_allow_list")
                    .and_then(|value| value.as_string_list())
                    .unwrap_or(&[])
                    .to_vec(),
            },
            audit: AuditConfig {
                jsonl_path: values
                    .get("audit.jsonl_path")
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
            },
        })
    }

    pub fn doctor(&self, overrides: &ConfigOverrides) -> Result<ConfigDoctorReport, ConfigError> {
        let mut issues = Vec::new();
        let global_doc = match self.load_scope_document(ConfigScope::Global) {
            Ok(doc) => Some(doc),
            Err(err) => {
                issues.push(ConfigIssue {
                    code: err.code.to_string(),
                    key: None,
                    message: err.message.clone(),
                });
                None
            }
        };
        let workspace_doc = match self.load_scope_document(ConfigScope::Workspace) {
            Ok(doc) => Some(doc),
            Err(err) => {
                issues.push(ConfigIssue {
                    code: err.code.to_string(),
                    key: None,
                    message: err.message.clone(),
                });
                None
            }
        };

        if let Some(doc) = global_doc.as_ref() {
            issues.extend(self.validate_document(doc));
        }
        if let Some(doc) = workspace_doc.as_ref() {
            issues.extend(self.validate_document(doc));
        }
        if let Ok(effective) = self.resolve_effective(overrides) {
            for provider in effective.providers.values() {
                if !provider.enabled {
                    continue;
                }
                let Some(env_var) = provider.api_key_env.as_ref() else {
                    continue;
                };
                if std::env::var(env_var.as_str()).is_err() {
                    issues.push(ConfigIssue {
                        code: "config_env_missing".to_string(),
                        key: Some(format!("providers.{}.api_key_env", provider.provider_id)),
                        message: format!("environment variable {} is not set", env_var.as_str()),
                    });
                }
            }
        }
        Ok(ConfigDoctorReport { issues })
    }

    pub fn resolve_path(&self, raw: &str) -> PathBuf {
        let path = PathBuf::from(raw);
        if path.is_absolute() {
            return path;
        }
        self.root.join(path)
    }

    fn load_scope_document(&self, scope: ConfigScope) -> Result<ConfigDocument, ConfigError> {
        let path = self.scope_path(scope)?;
        ConfigDocument::load(&path)
    }

    fn scope_path(&self, scope: ConfigScope) -> Result<PathBuf, ConfigError> {
        match scope {
            ConfigScope::Global => Self::global_config_path(),
            ConfigScope::Workspace => Ok(self.workspace_config_path()),
            ConfigScope::Effective => Err(ConfigError::invalid_request(
                "effective scope does not have a backing document",
            )),
        }
    }

    fn resolve_field(
        &self,
        field: &SchemaField,
        scope: ConfigScope,
        global_doc: &ConfigDocument,
        workspace_doc: &ConfigDocument,
        overrides: &ConfigOverrides,
    ) -> Result<ResolvedField, ConfigError> {
        let (value, origin) = match scope {
            ConfigScope::Global => (
                global_doc.get(&field.key)?,
                if global_doc.get(&field.key)?.is_some() {
                    ConfigOrigin::Global
                } else {
                    ConfigOrigin::Unset
                },
            ),
            ConfigScope::Workspace => (
                workspace_doc.get(&field.key)?,
                if workspace_doc.get(&field.key)?.is_some() {
                    ConfigOrigin::Workspace
                } else {
                    ConfigOrigin::Unset
                },
            ),
            ConfigScope::Effective => {
                if let Some(value) = overrides.get(&field.key).cloned() {
                    (Some(value), ConfigOrigin::Override)
                } else if let Some(value) = workspace_doc.get(&field.key)? {
                    (Some(value), ConfigOrigin::Workspace)
                } else if let Some(value) = global_doc.get(&field.key)? {
                    (Some(value), ConfigOrigin::Global)
                } else {
                    (field.default.clone(), ConfigOrigin::Default)
                }
            }
        };
        let secret_status = if matches!(field.kind, SchemaValueKind::EnvVar) {
            value.as_ref().and_then(|value| {
                value.as_str().map(|env_var| {
                    if std::env::var(env_var).is_ok() {
                        "set".to_string()
                    } else {
                        "missing".to_string()
                    }
                })
            })
        } else {
            None
        };
        Ok(ResolvedField {
            key: field.key.to_string(),
            value,
            origin,
            secret_status,
        })
    }

    fn collect_effective_values(
        &self,
        global_doc: &ConfigDocument,
        workspace_doc: &ConfigDocument,
        overrides: &ConfigOverrides,
    ) -> Result<BTreeMap<String, ConfigValue>, ConfigError> {
        let mut out = BTreeMap::new();
        for field in &self.schema.fields {
            if let Some(value) = overrides
                .get(&field.key)
                .cloned()
                .or(workspace_doc.get(&field.key)?)
                .or(global_doc.get(&field.key)?)
                .or(field.default.clone())
            {
                field.validate_value(&value)?;
                out.insert(field.key.to_string(), value);
            }
        }
        Ok(out)
    }

    fn validate_document(&self, doc: &ConfigDocument) -> Vec<ConfigIssue> {
        let mut issues = Vec::new();
        let set_keys = doc.flatten_keys();
        for key in set_keys {
            let Ok(config_key) = ConfigKey::parse(key.clone()) else {
                issues.push(ConfigIssue {
                    code: "config_invalid_key".to_string(),
                    key: Some(key),
                    message: "invalid config key".to_string(),
                });
                continue;
            };
            let Some(field) = self.schema.field(&config_key) else {
                issues.push(ConfigIssue {
                    code: "config_unknown_key".to_string(),
                    key: Some(key),
                    message: "unknown config key".to_string(),
                });
                continue;
            };
            match doc.get(&config_key) {
                Ok(Some(value)) => {
                    if let Err(err) = field.validate_value(&value) {
                        issues.push(ConfigIssue {
                            code: err.code.to_string(),
                            key: Some(key),
                            message: err.message,
                        });
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    issues.push(ConfigIssue {
                        code: err.code.to_string(),
                        key: Some(key),
                        message: err.message,
                    });
                }
            }
        }
        issues
    }

    fn provider_ids(&self) -> Vec<String> {
        let mut ids = BTreeSet::new();
        for field in &self.schema.fields {
            let mut segments = field.key.segments();
            if segments.next() == Some("providers") {
                if let Some(provider_id) = segments.next() {
                    ids.insert(provider_id.to_string());
                }
            }
        }
        ids.into_iter().collect()
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct RawConfigDocument {
    #[serde(default)]
    version: Option<u32>,
    #[serde(flatten)]
    values: toml::map::Map<String, toml::Value>,
}

#[derive(Debug, Deserialize)]
struct RawConfigSchema {
    version: u32,
    fields: Vec<RawSchemaField>,
}

#[derive(Debug, Deserialize)]
struct RawSchemaField {
    key: String,
    kind: String,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    default: Option<toml::Value>,
    #[serde(default)]
    enum_values: Option<Vec<String>>,
    #[serde(default)]
    min: Option<i64>,
    #[serde(default)]
    max: Option<i64>,
}

fn validate_key_segment(segment: &str) -> Result<(), ConfigError> {
    if segment
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        Ok(())
    } else {
        Err(ConfigError::invalid_key(format!(
            "invalid config key segment: {segment}"
        )))
    }
}

fn flatten_table_keys(
    prefix: &str,
    table: &toml::map::Map<String, toml::Value>,
    out: &mut Vec<String>,
) {
    for (key, value) in table {
        let current = if prefix.is_empty() {
            key.to_string()
        } else {
            format!("{prefix}.{key}")
        };
        if let Some(inner) = value.as_table() {
            flatten_table_keys(&current, inner, out);
        } else {
            out.push(current);
        }
    }
}

fn set_nested_value(
    table: &mut toml::map::Map<String, toml::Value>,
    segments: &[&str],
    value: toml::Value,
) {
    if segments.len() == 1 {
        table.insert(segments[0].to_string(), value);
        return;
    }
    let entry = table
        .entry(segments[0].to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    if !entry.is_table() {
        *entry = toml::Value::Table(toml::map::Map::new());
    }
    let inner = entry.as_table_mut().expect("table inserted above");
    set_nested_value(inner, &segments[1..], value);
}

fn unset_nested_value(table: &mut toml::map::Map<String, toml::Value>, segments: &[&str]) -> bool {
    if segments.is_empty() {
        return table.is_empty();
    }
    if segments.len() == 1 {
        table.remove(segments[0]);
        return table.is_empty();
    }
    let Some(value) = table.get_mut(segments[0]) else {
        return table.is_empty();
    };
    let Some(inner) = value.as_table_mut() else {
        return table.is_empty();
    };
    let should_prune = unset_nested_value(inner, &segments[1..]);
    if should_prune {
        table.remove(segments[0]);
    }
    table.is_empty()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    let Some(parent) = path.parent() else {
        return Err(ConfigError::io_error("config path has no parent directory"));
    };
    fs::create_dir_all(parent).map_err(|err| {
        ConfigError::io_error("failed to create config directory").with_source(err)
    })?;
    secure_dir(parent)?;

    let tmp_name = format!(
        ".tmp-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let tmp_path = parent.join(tmp_name);
    fs::write(&tmp_path, bytes).map_err(|err| {
        ConfigError::io_error("failed to write temp config file").with_source(err)
    })?;
    secure_file(&tmp_path)?;
    fs::rename(&tmp_path, path)
        .map_err(|err| ConfigError::io_error("failed to replace config file").with_source(err))?;
    secure_file(path)?;
    Ok(())
}

fn secure_dir(path: &Path) -> Result<(), ConfigError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o700);
        fs::set_permissions(path, perms).map_err(|err| {
            ConfigError::io_error("failed to secure config directory").with_source(err)
        })?;
    }
    Ok(())
}

fn secure_file(path: &Path) -> Result<(), ConfigError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(path, perms).map_err(|err| {
            ConfigError::io_error("failed to secure config file").with_source(err)
        })?;
    }
    Ok(())
}

fn parse_provider_surface(input: &str) -> Result<ProviderSurfaceKind, ConfigError> {
    match input {
        "openai-compatible" => Ok(ProviderSurfaceKind::OpenAiCompatible),
        "openrouter" => Ok(ProviderSurfaceKind::OpenRouter),
        other => Err(ConfigError::invalid_value(format!(
            "unknown provider surface: {other}"
        ))),
    }
}

fn parse_memory_backend(input: &str) -> Result<MemoryBackendKind, ConfigError> {
    match input {
        "file" => Ok(MemoryBackendKind::File),
        other => Err(ConfigError::invalid_value(format!(
            "unknown memory backend: {other}"
        ))),
    }
}

fn parse_prompt_cache_backend(input: &str) -> Result<PromptCacheBackendKind, ConfigError> {
    match input {
        "off" => Ok(PromptCacheBackendKind::Off),
        "memory" => Ok(PromptCacheBackendKind::Memory),
        other => Err(ConfigError::invalid_value(format!(
            "unknown prompt cache backend: {other}"
        ))),
    }
}

fn parse_prompt_cache_native(input: &str) -> Result<PromptCacheNativeMode, ConfigError> {
    match input {
        "auto" => Ok(PromptCacheNativeMode::Auto),
        "off" => Ok(PromptCacheNativeMode::Off),
        "required" => Ok(PromptCacheNativeMode::Required),
        other => Err(ConfigError::invalid_value(format!(
            "unknown prompt cache native mode: {other}"
        ))),
    }
}

fn parse_prompt_cache_layout(input: &str) -> Result<PromptCacheLayoutMode, ConfigError> {
    match input {
        "auto" => Ok(PromptCacheLayoutMode::Auto),
        "single_system" => Ok(PromptCacheLayoutMode::SingleSystem),
        "preserve_prefix_segments" => Ok(PromptCacheLayoutMode::PreservePrefixSegments),
        other => Err(ConfigError::invalid_value(format!(
            "unknown prompt cache layout mode: {other}"
        ))),
    }
}

fn parse_execution_mode(input: &str) -> Result<ExecutionMode, ConfigError> {
    match input {
        "interactive" => Ok(ExecutionMode::Interactive),
        "non_interactive" | "non-interactive" => Ok(ExecutionMode::NonInteractive),
        other => Err(ConfigError::invalid_value(format!(
            "unknown execution mode: {other}"
        ))),
    }
}

fn value_to_usize(value: i64) -> Result<usize, ConfigError> {
    usize::try_from(value)
        .map_err(|_| ConfigError::invalid_value(format!("value {value} cannot fit in usize")))
}

fn value_to_u64(value: i64) -> Result<u64, ConfigError> {
    u64::try_from(value)
        .map_err(|_| ConfigError::invalid_value(format!("value {value} cannot fit in u64")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_schema_loads() {
        let schema = ConfigSchema::builtin().unwrap();
        assert_eq!(schema.version, CONFIG_SCHEMA_VERSION);
        assert!(schema
            .fields
            .iter()
            .any(|field| field.key.as_str() == "security.execution_mode"));
    }

    #[test]
    fn config_key_rejects_empty_segment() {
        let err = ConfigKey::parse("memory..store").unwrap_err();
        assert_eq!(err.code, "config_invalid_key");
    }

    #[test]
    fn env_var_name_rejects_secret_value_shape() {
        let err = EnvVarName::parse("sk-test-secret").unwrap_err();
        assert_eq!(err.code, "config_invalid_value");
    }
}
