use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::skills::validator::{load_skill_dir, SkillPackage, SkillValidationOptions};
use crate::skills::{
    LoadedSkills, SkillMetadata, SkillOverrideRecord, SkillSourceDiagnostics, SkillToolSpec,
    SkillsDiagnostics,
};

#[derive(Debug, Clone)]
pub struct SkillsLoadOptions {
    pub skip_invalid_sources: bool,
    pub strict: bool,
}

impl Default for SkillsLoadOptions {
    fn default() -> Self {
        Self {
            skip_invalid_sources: false,
            strict: true,
        }
    }
}

pub fn load_skills(sources: &[String], options: SkillsLoadOptions) -> Result<LoadedSkills> {
    let mut skill_map: BTreeMap<String, SkillPackage> = BTreeMap::new();
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
                },
            ) {
                Ok(pkg) => {
                    if let Some(prev) = skill_map.get(&pkg.metadata.name) {
                        diagnostics.overrides.push(SkillOverrideRecord {
                            name: pkg.metadata.name.clone(),
                            overridden_source: prev.metadata.source.clone(),
                            source: pkg.metadata.source.clone(),
                        });
                    }
                    skill_map.insert(pkg.metadata.name.clone(), pkg);
                    source_diag.loaded += 1;
                }
                Err(e) => {
                    if options.strict {
                        return Err(e);
                    }
                    source_diag.skipped += 1;
                    source_diag.errors.push(e.to_string());
                }
            }
        }
        diagnostics.sources.push(source_diag);
    }

    let mut metadata: Vec<SkillMetadata> = Vec::new();
    let mut tool_map: BTreeMap<String, SkillToolSpec> = BTreeMap::new();
    for (_, pkg) in skill_map {
        metadata.push(pkg.metadata.clone());
        for tool in pkg.tools {
            if is_core_tool(&tool.name) {
                return Err(anyhow::anyhow!(
                    "tool_conflict_with_core: skill tool {} from skill {} conflicts with core tool",
                    tool.name,
                    tool.skill_name
                ));
            }
            if let Some(prev) = tool_map.get(&tool.name) {
                diagnostics.overrides.push(SkillOverrideRecord {
                    name: tool.name.clone(),
                    overridden_source: prev.source.clone(),
                    source: tool.source.clone(),
                });
            }
            tool_map.insert(tool.name.clone(), tool);
        }
    }

    let tools = tool_map.into_values().collect::<Vec<_>>();
    let mut loaded = LoadedSkills {
        metadata,
        tools,
        diagnostics,
    };
    loaded.canonicalize();
    Ok(loaded)
}

fn source_name(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn is_core_tool(name: &str) -> bool {
    matches!(
        name,
        "ls" | "read_file"
            | "write_file"
            | "edit_file"
            | "delete_file"
            | "glob"
            | "grep"
            | "execute"
            | "task"
    )
}
