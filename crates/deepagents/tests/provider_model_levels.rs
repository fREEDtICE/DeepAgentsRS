use deepagents::provider::{
    resolve_model_level_selection, resolve_model_level_selection_with_catalog, ModelLevel,
    ModelLevelIntent, ModelLevelResolutionError, ProviderBasicConfig, ProviderCatalogEntry,
    ProviderLevelTarget, ProviderSurfaceKind,
};

fn custom_catalog() -> Vec<ProviderCatalogEntry> {
    vec![
        ProviderCatalogEntry::new("alpha", ProviderSurfaceKind::OpenAiCompatible)
            .with_credentials_required(true)
            .with_default_base_url("https://alpha.example/v1")
            .with_default_api_key_env("DEEPAGENTS_ALPHA_KEY")
            .with_level(ModelLevel::Lite, ProviderLevelTarget::new("alpha-lite"))
            .with_level(ModelLevel::Normal, ProviderLevelTarget::new("alpha-normal"))
            .with_level(ModelLevel::Pro, ProviderLevelTarget::new("alpha-pro")),
        ProviderCatalogEntry::new("beta", ProviderSurfaceKind::OpenAiCompatible)
            .with_credentials_required(true)
            .with_default_base_url("https://beta.example/v1")
            .with_default_api_key_env("DEEPAGENTS_BETA_KEY")
            .with_level(ModelLevel::Lite, ProviderLevelTarget::new("beta-lite"))
            .with_level(ModelLevel::Normal, ProviderLevelTarget::new("beta-normal"))
            .with_level(ModelLevel::Pro, ProviderLevelTarget::new("beta-pro")),
    ]
}

#[test]
fn builtin_openai_compatible_normal_resolves_to_exact_spec() {
    let provider_configs =
        vec![ProviderBasicConfig::new("openai-compatible").with_api_key("test-key")];
    let intent = ModelLevelIntent::new(ModelLevel::Normal, "agent_turn");

    let selection = resolve_model_level_selection(&intent, &provider_configs).unwrap();

    assert_eq!(selection.provider_id, "openai-compatible");
    assert_eq!(selection.diagnostics.chosen_provider, "openai-compatible");
    assert_eq!(selection.diagnostics.chosen_model, "gpt-4o");

    match selection.init_spec {
        deepagents::provider::ProviderInitSpec::OpenAiCompatible { config } => {
            assert_eq!(config.model, "gpt-4o");
            assert_eq!(config.base_url, "https://api.openai.com/v1");
            assert_eq!(config.api_key.as_deref(), Some("test-key"));
        }
        _ => panic!("expected openai-compatible init spec"),
    }
}

#[test]
fn provider_hint_is_respected_with_custom_catalog() {
    let provider_configs = vec![
        ProviderBasicConfig::new("alpha").with_api_key("alpha-key"),
        ProviderBasicConfig::new("beta").with_api_key("beta-key"),
    ];
    let catalog = custom_catalog();
    let intent = ModelLevelIntent::new(ModelLevel::Pro, "escalation").with_provider_hint("beta");

    let selection =
        resolve_model_level_selection_with_catalog(&intent, &provider_configs, &catalog).unwrap();

    assert_eq!(selection.provider_id, "beta");
    assert_eq!(selection.diagnostics.candidate_providers, vec!["beta"]);
    assert_eq!(selection.diagnostics.chosen_model, "beta-pro");

    match selection.init_spec {
        deepagents::provider::ProviderInitSpec::OpenAiCompatible { config } => {
            assert_eq!(config.model, "beta-pro");
            assert_eq!(config.base_url, "https://beta.example/v1");
            assert_eq!(config.api_key.as_deref(), Some("beta-key"));
        }
        _ => panic!("expected openai-compatible init spec"),
    }
}

#[test]
fn disabled_provider_is_skipped_without_hint() {
    let provider_configs = vec![
        ProviderBasicConfig::new("alpha")
            .with_enabled(false)
            .with_api_key("alpha-key"),
        ProviderBasicConfig::new("beta").with_api_key("beta-key"),
    ];
    let catalog = custom_catalog();
    let intent = ModelLevelIntent::new(ModelLevel::Lite, "summarization");

    let selection =
        resolve_model_level_selection_with_catalog(&intent, &provider_configs, &catalog).unwrap();

    assert_eq!(selection.provider_id, "beta");
    assert_eq!(selection.diagnostics.chosen_model, "beta-lite");
    assert_eq!(
        selection.diagnostics.candidate_providers,
        vec!["alpha".to_string(), "beta".to_string()]
    );
}

#[test]
fn missing_credentials_returns_specific_error_for_hint() {
    std::env::remove_var("DEEPAGENTS_MISSING_KEY");

    let provider_configs = vec![ProviderBasicConfig::new("missing")];
    let catalog = vec![
        ProviderCatalogEntry::new("missing", ProviderSurfaceKind::OpenAiCompatible)
            .with_credentials_required(true)
            .with_default_base_url("https://missing.example/v1")
            .with_default_api_key_env("DEEPAGENTS_MISSING_KEY")
            .with_level(
                ModelLevel::Normal,
                ProviderLevelTarget::new("missing-normal"),
            ),
    ];
    let intent =
        ModelLevelIntent::new(ModelLevel::Normal, "agent_turn").with_provider_hint("missing");

    let err = resolve_model_level_selection_with_catalog(&intent, &provider_configs, &catalog)
        .unwrap_err();

    assert_eq!(err.code(), "model_level_credentials_missing");
    assert_eq!(
        err,
        ModelLevelResolutionError::CredentialsMissing {
            provider_id: "missing".to_string(),
            env_var: Some("DEEPAGENTS_MISSING_KEY".to_string()),
        }
    );
}

#[test]
fn resolved_selection_builds_bundle_with_logical_provider_id() {
    let provider_configs = vec![ProviderBasicConfig::new("beta").with_api_key("beta-key")];
    let catalog = custom_catalog();
    let intent = ModelLevelIntent::new(ModelLevel::Normal, "agent_turn").with_provider_hint("beta");

    let selection =
        resolve_model_level_selection_with_catalog(&intent, &provider_configs, &catalog).unwrap();
    let bundle = selection.build_bundle();

    assert_eq!(bundle.diagnostics.provider_id, "beta");
    assert!(bundle.diagnostics.supports_structured_output());
    assert!(bundle.diagnostics.supports_tool_choice());
}
