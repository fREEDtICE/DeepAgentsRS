//! File-backed local skill registry and lifecycle operations.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use chrono::Utc;
use semver::Version;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::skills::governance::review_skill_package;
use crate::skills::loader::SkillsLoadOptions;
use crate::skills::validator::{load_skill_dir, SkillValidationOptions};
use crate::skills::{
    LoadedSkills, SkillLifecycleState, SkillPackage, SkillRegistry, SkillRegistryEntry,
};

const REGISTRY_FILE: &str = "registry.json";
const PACKAGES_DIR: &str = "packages";

/// Summary returned by registry installation flows.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct SkillInstallReport {
    /// Entries newly installed into the registry.
    #[serde(default)]
    pub installed: Vec<SkillRegistryEntry>,
    /// Entries already present with identical content.
    #[serde(default)]
    pub unchanged: Vec<SkillRegistryEntry>,
}

/// Loads the file-backed registry if present, or returns an empty registry.
pub fn load_registry(registry_dir: &Path) -> Result<SkillRegistry> {
    let path = registry_dir.join(REGISTRY_FILE);
    if !path.exists() {
        return Ok(SkillRegistry::default());
    }
    let bytes = std::fs::read(&path)?;
    let mut registry: SkillRegistry = serde_json::from_slice(&bytes)?;
    registry.entries.sort_by(|a, b| {
        a.identity
            .name
            .cmp(&b.identity.name)
            .then_with(|| compare_versions(&a.identity.version, &b.identity.version))
    });
    Ok(registry)
}

/// Persists the registry index and ensures the package directory exists.
pub fn save_registry(registry_dir: &Path, registry: &SkillRegistry) -> Result<()> {
    std::fs::create_dir_all(registry_dir.join(PACKAGES_DIR))?;
    let path = registry_dir.join(REGISTRY_FILE);
    std::fs::write(path, serde_json::to_vec_pretty(registry)?)?;
    Ok(())
}

/// Installs validated skill packages from source directories into the registry.
pub fn install_sources_into_registry(
    sources: &[String],
    registry_dir: &Path,
    options: SkillsLoadOptions,
) -> Result<SkillInstallReport> {
    let mut report = SkillInstallReport::default();
    let mut registry = load_registry(registry_dir)?;

    for source in sources {
        let source_path = PathBuf::from(source);
        if !source_path.exists() || !source_path.is_dir() {
            return Err(anyhow!("invalid_source: {}", source));
        }
        let mut entries = std::fs::read_dir(&source_path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort();
        for path in entries {
            if !path.is_dir() {
                continue;
            }
            let source_name = source_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or(source);
            let mut package = load_skill_dir(
                &path,
                source_name,
                SkillValidationOptions {
                    strict: options.strict,
                    allow_versionless_compat: false,
                },
            )?;
            package.governance = review_skill_package(&package);
            let hash = compute_package_hash(&path)?;
            let lifecycle =
                if package.governance.status == crate::skills::SkillGovernanceStatus::Fail {
                    SkillLifecycleState::Quarantined
                } else if package.manifest.default_enabled {
                    SkillLifecycleState::Enabled
                } else {
                    SkillLifecycleState::Disabled
                };

            match registry
                .entries
                .iter()
                .position(|entry| entry.identity == package.manifest.identity)
            {
                Some(index) => {
                    if registry.entries[index].content_hash != hash {
                        return Err(anyhow!(
                            "registry_conflict: {} already installed with different content hash",
                            package.manifest.identity.as_key()
                        ));
                    }
                    report.unchanged.push(registry.entries[index].clone());
                }
                None => {
                    let install_dir = registry_package_dir(
                        registry_dir,
                        &package.manifest.identity.name,
                        &package.manifest.identity.version,
                    );
                    copy_dir_all(&path, &install_dir)?;
                    let entry = SkillRegistryEntry {
                        identity: package.manifest.identity.clone(),
                        manifest: package.manifest.clone(),
                        package_path: install_dir.to_string_lossy().to_string(),
                        content_hash: hash,
                        installed_from: vec![source.to_string()],
                        installed_at_ms: Utc::now().timestamp_millis(),
                        lifecycle,
                        lifecycle_reason: if lifecycle == SkillLifecycleState::Quarantined {
                            Some("semantic_review_failed".to_string())
                        } else {
                            None
                        },
                        governance: package.governance.clone(),
                    };
                    registry.entries.push(entry.clone());
                    report.installed.push(entry);
                }
            }
        }
    }

    registry.entries.sort_by(|a, b| {
        a.identity
            .name
            .cmp(&b.identity.name)
            .then_with(|| compare_versions(&a.identity.version, &b.identity.version))
    });
    save_registry(registry_dir, &registry)?;
    Ok(report)
}

/// Loads installed registry packages together with the registry metadata that
/// controls lifecycle behavior.
pub fn load_registry_packages(
    registry_dir: &Path,
) -> Result<Vec<(SkillRegistryEntry, SkillPackage)>> {
    let registry = load_registry(registry_dir)?;
    let mut out = Vec::new();
    for entry in registry.entries {
        let mut package = load_skill_dir(
            Path::new(&entry.package_path),
            &entry.manifest.source,
            SkillValidationOptions {
                strict: true,
                allow_versionless_compat: false,
            },
        )?;
        package.governance = entry.governance.clone();
        out.push((entry, package));
    }
    out.sort_by(|a, b| {
        a.0.identity
            .name
            .cmp(&b.0.identity.name)
            .then_with(|| compare_versions(&a.0.identity.version, &b.0.identity.version))
    });
    Ok(out)
}

/// Returns all entries currently present in the registry.
pub fn registry_status(registry_dir: &Path) -> Result<Vec<SkillRegistryEntry>> {
    let mut entries = load_registry(registry_dir)?.entries;
    entries.sort_by(|a, b| {
        a.identity
            .name
            .cmp(&b.identity.name)
            .then_with(|| compare_versions(&a.identity.version, &b.identity.version))
    });
    Ok(entries)
}

/// Returns all installed versions for one skill name.
pub fn registry_versions(registry_dir: &Path, name: &str) -> Result<Vec<SkillRegistryEntry>> {
    let mut entries = load_registry(registry_dir)?
        .entries
        .into_iter()
        .filter(|entry| entry.identity.name == name)
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| compare_versions(&a.identity.version, &b.identity.version));
    Ok(entries)
}

/// Updates lifecycle state for the targeted registry entries.
pub fn set_registry_lifecycle(
    registry_dir: &Path,
    name: &str,
    version: Option<&str>,
    lifecycle: SkillLifecycleState,
    reason: Option<String>,
) -> Result<Vec<SkillRegistryEntry>> {
    let mut registry = load_registry(registry_dir)?;
    let mut changed = Vec::new();
    for entry in &mut registry.entries {
        if entry.identity.name != name {
            continue;
        }
        if version.is_some() && version != Some(entry.identity.version.as_str()) {
            continue;
        }
        if lifecycle == SkillLifecycleState::Enabled
            && entry.governance.status == crate::skills::SkillGovernanceStatus::Fail
        {
            return Err(anyhow!(
                "governance_blocked: {} cannot be enabled because semantic review failed",
                entry.identity.as_key()
            ));
        }
        entry.lifecycle = lifecycle;
        entry.lifecycle_reason = reason.clone();
        changed.push(entry.clone());
    }
    if changed.is_empty() {
        return Err(anyhow!(
            "registry_entry_not_found: {}{}",
            name,
            version.map(|value| format!("@{value}")).unwrap_or_default()
        ));
    }
    save_registry(registry_dir, &registry)?;
    Ok(changed)
}

/// Removes a versioned package from the registry and deletes the installed files.
pub fn remove_registry_entry(
    registry_dir: &Path,
    name: &str,
    version: &str,
) -> Result<SkillRegistryEntry> {
    let mut registry = load_registry(registry_dir)?;
    let Some(index) = registry
        .entries
        .iter()
        .position(|entry| entry.identity.name == name && entry.identity.version == version)
    else {
        return Err(anyhow!("registry_entry_not_found: {name}@{version}"));
    };
    let entry = registry.entries.remove(index);
    let package_dir = PathBuf::from(&entry.package_path);
    if package_dir.exists() {
        std::fs::remove_dir_all(&package_dir)?;
    }
    save_registry(registry_dir, &registry)?;
    Ok(entry)
}

/// Builds a legacy `LoadedSkills` view from registry packages so older CLI
/// surfaces can still summarize installed content.
pub fn registry_loaded_skills(registry_dir: &Path) -> Result<LoadedSkills> {
    let mut loaded = LoadedSkills::default();
    for (entry, package) in load_registry_packages(registry_dir)? {
        loaded.metadata.push(package.metadata.clone());
        if entry.lifecycle != SkillLifecycleState::Quarantined {
            loaded.tools.extend(package.tools.clone());
        }
        loaded.packages.push(package.clone());
        for finding in &package.governance.findings {
            loaded
                .diagnostics
                .records
                .push(crate::skills::SkillDiagnosticRecord {
                    name: package.manifest.identity.name.clone(),
                    version: package.manifest.identity.version.clone(),
                    source: package.manifest.source.clone(),
                    severity: format!("{:?}", finding.severity).to_ascii_lowercase(),
                    code: finding.code.clone(),
                    message: finding.message.clone(),
                });
        }
    }
    loaded.canonicalize();
    Ok(loaded)
}

/// Parses `name` or `name@version` tokens from CLI-facing lifecycle commands.
pub fn parse_identity_token(token: &str) -> Result<(String, Option<String>)> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("invalid_identity: empty"));
    }
    match trimmed.split_once('@') {
        Some((name, version)) if !name.is_empty() && !version.is_empty() => {
            Ok((name.to_string(), Some(version.to_string())))
        }
        Some(_) => Err(anyhow!("invalid_identity: {}", token)),
        None => Ok((trimmed.to_string(), None)),
    }
}

/// Returns the on-disk package directory for a registry entry.
pub fn registry_package_dir(registry_dir: &Path, name: &str, version: &str) -> PathBuf {
    registry_dir.join(PACKAGES_DIR).join(name).join(version)
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in WalkDir::new(src).follow_links(false) {
        let entry = entry?;
        let relative = entry.path().strip_prefix(src)?;
        let target = dst.join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if entry.file_type().is_symlink() {
            return Err(anyhow!(
                "registry_install_symlink_not_allowed: {}",
                entry.path().display()
            ));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(entry.path(), &target)?;
    }
    Ok(())
}

fn compute_package_hash(root: &Path) -> Result<String> {
    let mut files = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()?;
    files.sort_by(|a, b| a.path().cmp(b.path()));
    let mut hasher = Sha256::new();
    for entry in files {
        if entry.file_type().is_dir() {
            continue;
        }
        if entry.file_type().is_symlink() {
            return Err(anyhow!(
                "registry_install_symlink_not_allowed: {}",
                entry.path().display()
            ));
        }
        let relative = entry.path().strip_prefix(root)?.to_string_lossy();
        hasher.update(relative.as_bytes());
        hasher.update(&std::fs::read(entry.path())?);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    match (Version::parse(a), Version::parse(b)) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        _ => a.cmp(b),
    }
}
