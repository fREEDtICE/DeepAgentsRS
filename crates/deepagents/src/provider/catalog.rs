use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::llm::{OpenAiCompatibleConfig, OpenRouterConfig};

use super::init::{build_provider_bundle, ProviderInitBundle, ProviderInitSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelLevel {
    Lite,
    Normal,
    Pro,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelLevelIntent {
    pub level: ModelLevel,
    pub feature_tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSurfaceKind {
    OpenAiCompatible,
    OpenRouter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderLevelTarget {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url_override: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env_override: Option<String>,
}

impl ProviderLevelTarget {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            base_url_override: None,
            api_key_env_override: None,
        }
    }

    pub fn with_base_url_override(mut self, base_url: impl Into<String>) -> Self {
        self.base_url_override = Some(base_url.into());
        self
    }

    pub fn with_api_key_env_override(mut self, env_var: impl Into<String>) -> Self {
        self.api_key_env_override = Some(env_var.into());
        self
    }
}

pub type ProviderLevelMap = BTreeMap<ModelLevel, ProviderLevelTarget>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCatalogEntry {
    pub provider_id: String,
    pub surface: ProviderSurfaceKind,
    #[serde(default)]
    pub credentials_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_api_key_env: Option<String>,
    #[serde(default)]
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
            levels: BTreeMap::new(),
        }
    }

    pub fn with_credentials_required(mut self, required: bool) -> Self {
        self.credentials_required = required;
        self
    }

    pub fn with_default_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.default_base_url = Some(base_url.into());
        self
    }

    pub fn with_default_api_key_env(mut self, env_var: impl Into<String>) -> Self {
        self.default_api_key_env = Some(env_var.into());
        self
    }

    pub fn with_level(mut self, level: ModelLevel, target: ProviderLevelTarget) -> Self {
        self.levels.insert(level, target);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderBasicConfig {
    pub provider_id: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
}

impl ProviderBasicConfig {
    pub fn new(provider_id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            enabled: true,
            base_url: None,
            api_key: None,
            api_key_env: None,
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

    pub fn with_api_key_env(mut self, env_var: impl Into<String>) -> Self {
        self.api_key_env = Some(env_var.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelLevelResolutionDiagnostics {
    pub feature_tag: String,
    pub requested_level: ModelLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_hint: Option<String>,
    #[serde(default)]
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

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModelLevelResolutionError {
    #[error("model_level_provider_not_configured: {provider_id}")]
    ProviderNotConfigured { provider_id: String },
    #[error("model_level_provider_disabled: {provider_id}")]
    ProviderDisabled { provider_id: String },
    #[error("model_level_provider_unknown: {provider_id}")]
    ProviderUnknown { provider_id: String },
    #[error("model_level_unsupported_level: {provider_id} {level:?}")]
    UnsupportedLevel {
        provider_id: String,
        level: ModelLevel,
    },
    #[error("model_level_credentials_missing: {provider_id}")]
    CredentialsMissing {
        provider_id: String,
        env_var: Option<String>,
    },
    #[error("model_level_no_provider_available: {level:?}")]
    NoProviderAvailable { level: ModelLevel },
}

impl ModelLevelResolutionError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ProviderNotConfigured { .. } => "model_level_provider_not_configured",
            Self::ProviderDisabled { .. } => "model_level_provider_disabled",
            Self::ProviderUnknown { .. } => "model_level_provider_unknown",
            Self::UnsupportedLevel { .. } => "model_level_unsupported_level",
            Self::CredentialsMissing { .. } => "model_level_credentials_missing",
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
        ProviderCatalogEntry::new("openrouter", ProviderSurfaceKind::OpenRouter)
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
    resolve_model_level_selection_with_catalog(
        intent,
        provider_configs,
        &default_model_level_catalog(),
    )
}

pub fn resolve_model_level_selection_with_catalog(
    intent: &ModelLevelIntent,
    provider_configs: &[ProviderBasicConfig],
    catalog: &[ProviderCatalogEntry],
) -> Result<ResolvedProviderSelection, ModelLevelResolutionError> {
    let config_by_id = provider_configs
        .iter()
        .map(|cfg| (cfg.provider_id.as_str(), cfg))
        .collect::<BTreeMap<_, _>>();
    let catalog_by_id = catalog
        .iter()
        .map(|entry| (entry.provider_id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();

    let candidate_providers = if let Some(provider_hint) = intent.provider_hint.as_deref() {
        vec![provider_hint.to_string()]
    } else {
        provider_configs
            .iter()
            .map(|cfg| cfg.provider_id.clone())
            .collect::<Vec<_>>()
    };

    if candidate_providers.is_empty() {
        return Err(ModelLevelResolutionError::NoProviderAvailable {
            level: intent.level,
        });
    }

    for provider_id in &candidate_providers {
        let Some(config) = config_by_id.get(provider_id.as_str()) else {
            if intent.provider_hint.is_some() {
                return Err(ModelLevelResolutionError::ProviderNotConfigured {
                    provider_id: provider_id.clone(),
                });
            }
            continue;
        };
        if !config.enabled {
            if intent.provider_hint.is_some() {
                return Err(ModelLevelResolutionError::ProviderDisabled {
                    provider_id: provider_id.clone(),
                });
            }
            continue;
        }
        let Some(entry) = catalog_by_id.get(provider_id.as_str()) else {
            if intent.provider_hint.is_some() {
                return Err(ModelLevelResolutionError::ProviderUnknown {
                    provider_id: provider_id.clone(),
                });
            }
            continue;
        };
        let Some(target) = entry.levels.get(&intent.level) else {
            if intent.provider_hint.is_some() {
                return Err(ModelLevelResolutionError::UnsupportedLevel {
                    provider_id: provider_id.clone(),
                    level: intent.level,
                });
            }
            continue;
        };

        let resolved_api_key = resolve_api_key(config, entry, target).map_err(|env_var| {
            ModelLevelResolutionError::CredentialsMissing {
                provider_id: provider_id.clone(),
                env_var,
            }
        })?;

        let init_spec = match entry.surface {
            ProviderSurfaceKind::OpenAiCompatible => {
                let mut cfg = OpenAiCompatibleConfig::new(target.model.clone());
                if let Some(base_url) = config
                    .base_url
                    .as_ref()
                    .or(target.base_url_override.as_ref())
                    .or(entry.default_base_url.as_ref())
                {
                    cfg = cfg.with_base_url(base_url.clone());
                }
                if let Some(api_key) = resolved_api_key {
                    cfg = cfg.with_api_key(api_key);
                }
                ProviderInitSpec::OpenAiCompatible { config: cfg }
            }
            ProviderSurfaceKind::OpenRouter => {
                let mut cfg = OpenRouterConfig::new(target.model.clone());
                if let Some(base_url) = config
                    .base_url
                    .as_ref()
                    .or(target.base_url_override.as_ref())
                    .or(entry.default_base_url.as_ref())
                {
                    cfg = cfg.with_base_url(base_url.clone());
                }
                if let Some(api_key) = resolved_api_key {
                    cfg = cfg.with_api_key(api_key);
                }
                ProviderInitSpec::OpenRouter { config: cfg }
            }
        };

        return Ok(ResolvedProviderSelection {
            provider_id: provider_id.clone(),
            requested_level: intent.level,
            diagnostics: ModelLevelResolutionDiagnostics {
                feature_tag: intent.feature_tag.clone(),
                requested_level: intent.level,
                provider_hint: intent.provider_hint.clone(),
                candidate_providers: candidate_providers.clone(),
                chosen_provider: provider_id.clone(),
                chosen_model: target.model.clone(),
            },
            init_spec,
        });
    }

    Err(ModelLevelResolutionError::NoProviderAvailable {
        level: intent.level,
    })
}

fn resolve_api_key(
    config: &ProviderBasicConfig,
    entry: &ProviderCatalogEntry,
    target: &ProviderLevelTarget,
) -> Result<Option<String>, Option<String>> {
    if let Some(api_key) = config.api_key.clone() {
        return Ok(Some(api_key));
    }

    let env_var = config
        .api_key_env
        .clone()
        .or_else(|| target.api_key_env_override.clone())
        .or_else(|| entry.default_api_key_env.clone());

    let api_key = match env_var.as_deref() {
        Some(name) => std::env::var(name).ok(),
        None => None,
    };

    if entry.credentials_required && api_key.is_none() {
        return Err(env_var);
    }

    Ok(api_key)
}
