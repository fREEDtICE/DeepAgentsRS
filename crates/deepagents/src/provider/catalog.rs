use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::init::{build_provider_bundle, ProviderInitBundle, ProviderInitSpec};
use super::llm::MultimodalInputRoles;
use super::openai_compatible::OpenAiCompatibleConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelLevel {
    Lite,
    Normal,
    Pro,
}

impl ModelLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lite => "lite",
            Self::Normal => "normal",
            Self::Pro => "pro",
        }
    }
}

impl fmt::Display for ModelLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelLevelIntent {
    pub level: ModelLevel,
    pub feature_tag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_hint: Option<String>,
}

impl ModelLevelIntent {
    pub fn new(level: ModelLevel, feature_tag: impl Into<String>) -> Self {
        Self {
            level,
            feature_tag: feature_tag.into(),
            provider_hint: None,
        }
    }

    pub fn with_provider_hint(mut self, provider_hint: impl Into<String>) -> Self {
        self.provider_hint = Some(provider_hint.into());
        self
    }
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderBasicConfig {
    pub provider_id: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multimodal_input_roles: Option<MultimodalInputRoles>,
}

impl ProviderBasicConfig {
    pub fn new(provider_id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            enabled: true,
            base_url: None,
            api_key: None,
            api_key_env: None,
            multimodal_input_roles: None,
        }
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    pub fn with_api_key_env(mut self, api_key_env: impl Into<String>) -> Self {
        self.api_key_env = Some(api_key_env.into());
        self
    }

    pub fn with_multimodal_input_roles(mut self, roles: MultimodalInputRoles) -> Self {
        self.multimodal_input_roles = Some(roles);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSurfaceKind {
    OpenAiCompatible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderLevelTarget {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url_override: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env_override: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multimodal_input_roles: Option<MultimodalInputRoles>,
}

impl ProviderLevelTarget {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            base_url_override: None,
            api_key_env_override: None,
            multimodal_input_roles: None,
        }
    }

    pub fn with_base_url_override(mut self, base_url: impl Into<String>) -> Self {
        self.base_url_override = Some(base_url.into());
        self
    }

    pub fn with_api_key_env_override(mut self, env_name: impl Into<String>) -> Self {
        self.api_key_env_override = Some(env_name.into());
        self
    }

    pub fn with_multimodal_input_roles(mut self, roles: MultimodalInputRoles) -> Self {
        self.multimodal_input_roles = Some(roles);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProviderLevelMap {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lite: Option<ProviderLevelTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normal: Option<ProviderLevelTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pro: Option<ProviderLevelTarget>,
}

impl ProviderLevelMap {
    pub fn get(&self, level: ModelLevel) -> Option<&ProviderLevelTarget> {
        match level {
            ModelLevel::Lite => self.lite.as_ref(),
            ModelLevel::Normal => self.normal.as_ref(),
            ModelLevel::Pro => self.pro.as_ref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCatalogEntry {
    pub provider_id: String,
    pub surface: ProviderSurfaceKind,
    #[serde(default)]
    pub credentials_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_api_key_env: Option<String>,
    pub levels: ProviderLevelMap,
}

impl ProviderCatalogEntry {
    pub fn new(provider_id: impl Into<String>, surface: ProviderSurfaceKind) -> Self {
        Self {
            provider_id: provider_id.into(),
            surface,
            credentials_required: false,
            default_base_url: None,
            default_api_key_env: None,
            levels: ProviderLevelMap::default(),
        }
    }

    pub fn with_credentials_required(mut self, credentials_required: bool) -> Self {
        self.credentials_required = credentials_required;
        self
    }

    pub fn with_default_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.default_base_url = Some(base_url.into());
        self
    }

    pub fn with_default_api_key_env(mut self, env_name: impl Into<String>) -> Self {
        self.default_api_key_env = Some(env_name.into());
        self
    }

    pub fn with_level(mut self, level: ModelLevel, target: ProviderLevelTarget) -> Self {
        match level {
            ModelLevel::Lite => self.levels.lite = Some(target),
            ModelLevel::Normal => self.levels.normal = Some(target),
            ModelLevel::Pro => self.levels.pro = Some(target),
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelLevelResolutionDiagnostics {
    pub feature_tag: String,
    pub requested_level: ModelLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_hint: Option<String>,
    pub candidate_providers: Vec<String>,
    pub chosen_provider: String,
    pub chosen_model: String,
}

#[derive(Debug, Clone)]
pub struct ResolvedProviderSelection {
    pub provider_id: String,
    pub requested_level: ModelLevel,
    pub init_spec: ProviderInitSpec,
    pub diagnostics: ModelLevelResolutionDiagnostics,
}

impl ResolvedProviderSelection {
    pub fn build_bundle(&self) -> ProviderInitBundle {
        build_provider_bundle(self.provider_id.clone(), self.init_spec.clone())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ModelLevelResolutionError {
    #[error("model_level_provider_hint_unknown: {provider_id}")]
    ProviderHintUnknown { provider_id: String },
    #[error("model_level_provider_not_configured: {provider_id}")]
    ProviderNotConfigured { provider_id: String },
    #[error("model_level_provider_disabled: {provider_id}")]
    ProviderDisabled { provider_id: String },
    #[error("model_level_credentials_missing: {provider_id}")]
    CredentialsMissing {
        provider_id: String,
        env_var: Option<String>,
    },
    #[error("model_level_not_supported_by_provider: {provider_id}:{level}")]
    LevelUnsupported {
        provider_id: String,
        level: ModelLevel,
    },
    #[error("model_level_no_provider_available: {level}")]
    NoProviderAvailable {
        level: ModelLevel,
        attempted: Vec<String>,
    },
}

impl ModelLevelResolutionError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ProviderHintUnknown { .. } => "model_level_provider_hint_unknown",
            Self::ProviderNotConfigured { .. } => "model_level_provider_not_configured",
            Self::ProviderDisabled { .. } => "model_level_provider_disabled",
            Self::CredentialsMissing { .. } => "model_level_credentials_missing",
            Self::LevelUnsupported { .. } => "model_level_not_supported_by_provider",
            Self::NoProviderAvailable { .. } => "model_level_no_provider_available",
        }
    }
}

pub fn default_model_level_catalog() -> Vec<ProviderCatalogEntry> {
    vec![
        ProviderCatalogEntry::new("openai-compatible", ProviderSurfaceKind::OpenAiCompatible)
            .with_credentials_required(true)
            .with_default_base_url("https://api.openai.com/v1")
            .with_default_api_key_env("OPENAI_API_KEY")
            .with_level(ModelLevel::Lite, ProviderLevelTarget::new("gpt-4o-mini"))
            .with_level(ModelLevel::Normal, ProviderLevelTarget::new("gpt-4o"))
            .with_level(ModelLevel::Pro, ProviderLevelTarget::new("o3")),
        ProviderCatalogEntry::new("openrouter", ProviderSurfaceKind::OpenAiCompatible)
            .with_credentials_required(true)
            .with_default_base_url("https://openrouter.ai/api/v1")
            .with_default_api_key_env("OPENROUTER_API_KEY")
            .with_level(
                ModelLevel::Lite,
                ProviderLevelTarget::new("openai/gpt-4o-mini"),
            )
            .with_level(
                ModelLevel::Normal,
                ProviderLevelTarget::new("openai/gpt-4o"),
            )
            .with_level(ModelLevel::Pro, ProviderLevelTarget::new("openai/o3")),
    ]
}

pub fn resolve_model_level_selection(
    intent: &ModelLevelIntent,
    provider_configs: &[ProviderBasicConfig],
) -> Result<ResolvedProviderSelection, ModelLevelResolutionError> {
    let catalog = default_model_level_catalog();
    resolve_model_level_selection_with_catalog(intent, provider_configs, &catalog)
}

pub fn resolve_model_level_selection_with_catalog(
    intent: &ModelLevelIntent,
    provider_configs: &[ProviderBasicConfig],
    catalog: &[ProviderCatalogEntry],
) -> Result<ResolvedProviderSelection, ModelLevelResolutionError> {
    let candidate_providers = candidate_provider_ids(intent, catalog);
    let attempted = candidate_providers.clone();

    if let Some(provider_hint) = &intent.provider_hint {
        let entry = catalog
            .iter()
            .find(|entry| entry.provider_id == *provider_hint)
            .ok_or_else(|| ModelLevelResolutionError::ProviderHintUnknown {
                provider_id: provider_hint.clone(),
            })?;
        let provider_config = provider_configs
            .iter()
            .find(|config| config.provider_id == *provider_hint)
            .ok_or_else(|| ModelLevelResolutionError::ProviderNotConfigured {
                provider_id: provider_hint.clone(),
            })?;
        if !provider_config.enabled {
            return Err(ModelLevelResolutionError::ProviderDisabled {
                provider_id: provider_hint.clone(),
            });
        }
        let target = entry.levels.get(intent.level).ok_or_else(|| {
            ModelLevelResolutionError::LevelUnsupported {
                provider_id: provider_hint.clone(),
                level: intent.level,
            }
        })?;
        let init_spec = build_init_spec(entry, provider_config, target).ok_or_else(|| {
            let env_var = effective_api_key_env(entry, provider_config, target);
            ModelLevelResolutionError::CredentialsMissing {
                provider_id: provider_hint.clone(),
                env_var,
            }
        })?;
        return Ok(ResolvedProviderSelection {
            provider_id: provider_hint.clone(),
            requested_level: intent.level,
            diagnostics: ModelLevelResolutionDiagnostics {
                feature_tag: intent.feature_tag.clone(),
                requested_level: intent.level,
                provider_hint: intent.provider_hint.clone(),
                candidate_providers: candidate_providers.clone(),
                chosen_provider: provider_hint.clone(),
                chosen_model: target.model.clone(),
            },
            init_spec,
        });
    }

    for provider_id in candidate_providers.iter() {
        let Some(entry) = catalog
            .iter()
            .find(|entry| entry.provider_id == *provider_id)
        else {
            continue;
        };
        let Some(provider_config) = provider_configs
            .iter()
            .find(|config| config.provider_id == *provider_id)
        else {
            continue;
        };
        if !provider_config.enabled {
            continue;
        }
        let Some(target) = entry.levels.get(intent.level) else {
            continue;
        };
        let Some(init_spec) = build_init_spec(entry, provider_config, target) else {
            continue;
        };
        return Ok(ResolvedProviderSelection {
            provider_id: provider_id.clone(),
            requested_level: intent.level,
            diagnostics: ModelLevelResolutionDiagnostics {
                feature_tag: intent.feature_tag.clone(),
                requested_level: intent.level,
                provider_hint: None,
                candidate_providers: candidate_providers.clone(),
                chosen_provider: provider_id.clone(),
                chosen_model: target.model.clone(),
            },
            init_spec,
        });
    }

    Err(ModelLevelResolutionError::NoProviderAvailable {
        level: intent.level,
        attempted,
    })
}

fn candidate_provider_ids(
    intent: &ModelLevelIntent,
    catalog: &[ProviderCatalogEntry],
) -> Vec<String> {
    if let Some(provider_hint) = &intent.provider_hint {
        return vec![provider_hint.clone()];
    }

    catalog
        .iter()
        .filter(|entry| entry.levels.get(intent.level).is_some())
        .map(|entry| entry.provider_id.clone())
        .collect()
}

fn effective_api_key_env(
    entry: &ProviderCatalogEntry,
    provider_config: &ProviderBasicConfig,
    target: &ProviderLevelTarget,
) -> Option<String> {
    provider_config
        .api_key_env
        .clone()
        .or_else(|| target.api_key_env_override.clone())
        .or_else(|| entry.default_api_key_env.clone())
}

fn resolve_api_key(
    entry: &ProviderCatalogEntry,
    provider_config: &ProviderBasicConfig,
    target: &ProviderLevelTarget,
) -> Option<String> {
    if let Some(api_key) = provider_config.api_key.clone() {
        return Some(api_key);
    }

    let env_name = effective_api_key_env(entry, provider_config, target)?;
    std::env::var(&env_name).ok()
}

fn build_init_spec(
    entry: &ProviderCatalogEntry,
    provider_config: &ProviderBasicConfig,
    target: &ProviderLevelTarget,
) -> Option<ProviderInitSpec> {
    let resolved_api_key = resolve_api_key(entry, provider_config, target);
    if entry.credentials_required && resolved_api_key.is_none() {
        return None;
    }

    match entry.surface {
        ProviderSurfaceKind::OpenAiCompatible => {
            let mut config = OpenAiCompatibleConfig::new(target.model.clone());
            if let Some(base_url) = provider_config
                .base_url
                .clone()
                .or_else(|| target.base_url_override.clone())
                .or_else(|| entry.default_base_url.clone())
            {
                config = config.with_base_url(base_url);
            }
            if let Some(api_key) = resolved_api_key {
                config = config.with_api_key(api_key);
            }
            if let Some(roles) = provider_config
                .multimodal_input_roles
                .or(target.multimodal_input_roles)
            {
                config = config.with_multimodal_input_roles(roles);
            }
            Some(ProviderInitSpec::OpenAiCompatible { config })
        }
    }
}
