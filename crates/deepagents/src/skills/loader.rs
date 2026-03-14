//! Source-based skill package loading and compatibility views.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::skills::governance::review_skill_package;
use crate::skills::validator::{load_skill_dir, SkillValidationOptions};
use crate::skills::{
    LoadedSkills, SkillDiagnosticRecord, SkillOverrideRecord, SkillPackage, SkillSourceDiagnostics,
    SkillToolSpec, SkillsDiagnostics,
};

/// Controls source-based skill loading behavior.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillsLoadOptions {
    /// Whether invalid source directories should be recorded and skipped.
    pub skip_invalid_sources: bool,
    /// Whether unknown frontmatter and schema fields should fail fast.
    pub strict: bool,
    /// Whether versionless packages should be normalized to `0.0.0-dev`.
    pub allow_versionless_compat: bool,
}

impl Default for SkillsLoadOptions {
    fn default() -> Self {
        Self {
            skip_invalid_sources: false,
            strict: true,
            allow_versionless_compat: true,
        }
    }
}

/// Loads fully parsed packages from one or more source roots.
pub fn load_skill_packages_from_sources(
    sources: &[String],
    options: SkillsLoadOptions,
) -> Result<Vec<SkillPackage>> {
    let loaded = load_skills(sources, options)?;
    Ok(loaded.packages)
}

/// Loads source-based skills and exposes both the new package view and the
/// legacy metadata/tool mirrors used by older callers.
pub fn load_skills(sources: &[String], options: SkillsLoadOptions) -> Result<LoadedSkills> {
    let mut package_map: BTreeMap<String, SkillPackage> = BTreeMap::new();
    let mut diagnostics = SkillsDiagnostics::default();

    for src in sources {
        let source_path = PathBuf::from(src);
        let mut source_diag = SkillSourceDiagnostics {
            source: src.clone(),
            loaded: 0,
            skipped: 0,
            errors: Vec::new(),
        };

        if !source_path.exists() || !source_path.is_dir() {
            let err = format!("invalid_source: {}", src);
            if options.skip_invalid_sources {
                source_diag.skipped += 1;
                source_diag.errors.push(err);
                diagnostics.sources.push(source_diag);
                continue;
            }
            return Err(anyhow::anyhow!(err));
        }

        let source_name = source_name(&source_path);
        let mut entries = std::fs::read_dir(&source_path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort();
        for path in entries {
            if !path.is_dir() {
                continue;
            }
            match load_skill_dir(
                &path,
                &source_name,
                SkillValidationOptions {
                    strict: options.strict,
                    allow_versionless_compat: options.allow_versionless_compat,
                },
            ) {
                Ok(mut package) => {
                    package.governance = review_skill_package(&package);
                    let key = package.manifest.identity.as_key();
                    if let Some(prev) = package_map.get(&key) {
                        diagnostics.overrides.push(SkillOverrideRecord {
                            name: package.manifest.identity.name.clone(),
                            version: package.manifest.identity.version.clone(),
                            overridden_source: prev.manifest.source.clone(),
                            source: package.manifest.source.clone(),
                        });
                    }
                    for finding in &package.governance.findings {
                        diagnostics.records.push(SkillDiagnosticRecord {
                            name: package.manifest.identity.name.clone(),
                            version: package.manifest.identity.version.clone(),
                            source: package.manifest.source.clone(),
                            severity: format!("{:?}", finding.severity).to_ascii_lowercase(),
                            code: finding.code.clone(),
                            message: finding.message.clone(),
                        });
                    }
                    source_diag.loaded += 1;
                    package_map.insert(key, package);
                }
                Err(error) => {
                    if options.strict {
                        return Err(error);
                    }
                    source_diag.skipped += 1;
                    source_diag.errors.push(error.to_string());
                }
            }
        }
        diagnostics.sources.push(source_diag);
    }

    let mut loaded = LoadedSkills::default();
    let mut tool_map: BTreeMap<String, SkillToolSpec> = BTreeMap::new();
    for (_, package) in package_map {
        loaded.metadata.push(package.metadata.clone());
        loaded.packages.push(package.clone());
        for tool in &package.tools {
            if is_core_tool(&tool.name) {
                return Err(anyhow::anyhow!(
                    "tool_conflict_with_core: skill tool {} from skill {} conflicts with core tool",
                    tool.name,
                    tool.skill_name
                ));
            }
            if let Some(prev) = tool_map.get(&tool.name) {
                loaded.diagnostics.overrides.push(SkillOverrideRecord {
                    name: tool.name.clone(),
                    version: tool.skill_version.clone(),
                    overridden_source: prev.source.clone(),
                    source: tool.source.clone(),
                });
            }
            tool_map.insert(tool.name.clone(), tool.clone());
        }
    }

    loaded.tools = tool_map.into_values().collect::<Vec<_>>();
    loaded.diagnostics.sources = diagnostics.sources;
    loaded.diagnostics.records.extend(diagnostics.records);
    loaded
        .diagnostics
        .overrides
        .extend(diagnostics.overrides);
    loaded.canonicalize();
    Ok(loaded)
}

fn source_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn is_core_tool(name: &str) -> bool {
    matches!(
        name,
        "ls"
            | "read_file"
            | "write_file"
            | "edit_file"
            | "delete_file"
            | "glob"
            | "grep"
            | "execute"
            | "task"
            | "compact_conversation"
    )
}
