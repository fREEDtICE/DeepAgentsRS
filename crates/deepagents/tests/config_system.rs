use std::sync::Mutex;

use deepagents::config::{ConfigKey, ConfigManager, ConfigOverrides, ConfigScope, ConfigValue};
use deepagents::memory::{MemoryEvictionPolicy, MemoryRuntimeMode};

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn effective_config_prefers_override_workspace_global_default() {
    let _guard = ENV_LOCK.lock().unwrap();
    let global_dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::env::set_var("DEEPAGENTS_CONFIG_HOME", global_dir.path());

    let manager = ConfigManager::new(workspace.path()).unwrap();
    let key = ConfigKey::parse("runtime.max_steps").unwrap();
    manager
        .set(ConfigScope::Global, &key, ConfigValue::Integer(11))
        .unwrap();
    manager
        .set(ConfigScope::Workspace, &key, ConfigValue::Integer(22))
        .unwrap();

    let mut overrides = ConfigOverrides::new();
    overrides.set(key, ConfigValue::Integer(33));

    let effective = manager.resolve_effective(&overrides).unwrap();
    assert_eq!(effective.runtime.max_steps, 33);
}

#[test]
fn doctor_reports_missing_env_for_enabled_provider() {
    let _guard = ENV_LOCK.lock().unwrap();
    let global_dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::env::set_var("DEEPAGENTS_CONFIG_HOME", global_dir.path());
    std::env::remove_var("DEEPAGENTS_TEST_MISSING");

    let manager = ConfigManager::new(workspace.path()).unwrap();
    manager
        .set(
            ConfigScope::Workspace,
            &ConfigKey::parse("providers.openai-compatible.enabled").unwrap(),
            ConfigValue::Boolean(true),
        )
        .unwrap();
    manager
        .set(
            ConfigScope::Workspace,
            &ConfigKey::parse("providers.openai-compatible.api_key_env").unwrap(),
            ConfigValue::String("DEEPAGENTS_TEST_MISSING".to_string()),
        )
        .unwrap();

    let report = manager.doctor(&ConfigOverrides::new()).unwrap();
    assert!(report.issues.iter().any(|issue| {
        issue.code == "config_env_missing"
            && issue.key.as_deref() == Some("providers.openai-compatible.api_key_env")
    }));
}

#[test]
fn set_rejects_unknown_secret_key() {
    let _guard = ENV_LOCK.lock().unwrap();
    let global_dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::env::set_var("DEEPAGENTS_CONFIG_HOME", global_dir.path());

    let manager = ConfigManager::new(workspace.path()).unwrap();
    let key = ConfigKey::parse("providers.openai-compatible.api_key").unwrap();
    let err = manager
        .set(
            ConfigScope::Workspace,
            &key,
            ConfigValue::String("sk-secret".to_string()),
        )
        .unwrap_err();
    assert_eq!(err.code, "invalid_request");
}

#[test]
fn workspace_config_is_written_with_secure_permissions() {
    let _guard = ENV_LOCK.lock().unwrap();
    let global_dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::env::set_var("DEEPAGENTS_CONFIG_HOME", global_dir.path());

    let manager = ConfigManager::new(workspace.path()).unwrap();
    manager
        .set(
            ConfigScope::Workspace,
            &ConfigKey::parse("runtime.max_steps").unwrap(),
            ConfigValue::Integer(9),
        )
        .unwrap();

    let file = manager.workspace_config_path();
    assert!(file.exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let file_mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        let dir_mode = std::fs::metadata(file.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(dir_mode, 0o700);
    }
}

#[test]
fn effective_config_reads_memory_loader_fields() {
    let _guard = ENV_LOCK.lock().unwrap();
    let global_dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::env::set_var("DEEPAGENTS_CONFIG_HOME", global_dir.path());

    let manager = ConfigManager::new(workspace.path()).unwrap();
    manager
        .set(
            ConfigScope::Workspace,
            &ConfigKey::parse("memory.file.max_source_bytes").unwrap(),
            ConfigValue::Integer(4096),
        )
        .unwrap();
    manager
        .set(
            ConfigScope::Workspace,
            &ConfigKey::parse("memory.file.strict").unwrap(),
            ConfigValue::Boolean(false),
        )
        .unwrap();
    manager
        .set(
            ConfigScope::Workspace,
            &ConfigKey::parse("memory.file.runtime_mode").unwrap(),
            ConfigValue::String("scoped".to_string()),
        )
        .unwrap();

    let effective = manager.resolve_effective(&ConfigOverrides::new()).unwrap();
    assert_eq!(effective.memory.max_source_bytes, 4096);
    assert!(!effective.memory.strict);
    assert_eq!(effective.memory.runtime_mode, MemoryRuntimeMode::Scoped);
}

#[test]
fn effective_config_reads_memory_store_policy_fields() {
    let _guard = ENV_LOCK.lock().unwrap();
    let global_dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::env::set_var("DEEPAGENTS_CONFIG_HOME", global_dir.path());

    let manager = ConfigManager::new(workspace.path()).unwrap();
    for (key, value) in [
        ("memory.file.max_entries", ConfigValue::Integer(7)),
        ("memory.file.max_bytes_total", ConfigValue::Integer(3210)),
        (
            "memory.file.eviction",
            ConfigValue::String("ttl".to_string()),
        ),
        ("memory.file.ttl_secs", ConfigValue::Integer(42)),
    ] {
        manager
            .set(
                ConfigScope::Workspace,
                &ConfigKey::parse(key).unwrap(),
                value,
            )
            .unwrap();
    }

    let effective = manager.resolve_effective(&ConfigOverrides::new()).unwrap();
    assert_eq!(effective.memory.max_entries, 7);
    assert_eq!(effective.memory.max_bytes_total, 3210);
    assert_eq!(
        effective.memory.eviction,
        MemoryEvictionPolicy::Ttl { ttl_secs: 42 }
    );
    assert_eq!(effective.memory.store_policy().max_entries, 7);
}
