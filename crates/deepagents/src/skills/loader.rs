use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::skills::{
    LoadedSkills, SkillMetadata, SkillOverrideRecord, SkillSourceDiagnostics, SkillToolSpec, SkillsDiagnostics,
};
use crate::skills::validator::{load_skill_dir, SkillPackage, SkillValidationOptions};

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
    let mut skill_map: HashMap<String, SkillPackage> = HashMap::new();
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
        let entries = std::fs::read_dir(&source_path)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            match load_skill_dir(&path, &source_name, SkillValidationOptions { strict: options.strict }) {
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
                        return Err(anyhow::anyhow!("{}: {}", path.display(), e));
                    }
                    source_diag.skipped += 1;
                    source_diag.errors.push(format!("{}: {}", path.display(), e));
                }
            }
        }
        diagnostics.sources.push(source_diag);
    }

    let mut metadata: Vec<SkillMetadata> = Vec::new();
    let mut tool_map: HashMap<String, SkillToolSpec> = HashMap::new();
    for (_, pkg) in skill_map {
        metadata.push(pkg.metadata.clone());
        for tool in pkg.tools {
            if is_core_tool(&tool.name) {
                return Err(anyhow::anyhow!("skill tool conflicts with core tool: {}", tool.name));
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
    Ok(LoadedSkills {
        metadata,
        tools,
        diagnostics,
    })
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
        "ls"
            | "read_file"
            | "write_file"
            | "edit_file"
            | "delete_file"
            | "glob"
            | "grep"
            | "execute"
            | "task"
    )
}
