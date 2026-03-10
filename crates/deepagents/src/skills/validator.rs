use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use serde::Deserialize;

use crate::skills::{SkillMetadata, SkillToolPolicy, SkillToolSpec, SkillToolStep};

const MAX_SKILL_FILE_SIZE: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct SkillValidationOptions {
    pub strict: bool,
}

impl Default for SkillValidationOptions {
    fn default() -> Self {
        Self { strict: true }
    }
}

#[derive(Debug, Clone)]
pub struct SkillPackage {
    pub metadata: SkillMetadata,
    pub tools: Vec<SkillToolSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolsFile {
    tools: Vec<ToolFileSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolFileSpec {
    name: String,
    description: String,
    input_schema: serde_json::Value,
    #[serde(default)]
    steps: Vec<ToolStepFileSpec>,
    #[serde(default)]
    policy: Option<ToolPolicyFileSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolStepFileSpec {
    tool_name: String,
    #[serde(default)]
    arguments: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolPolicyFileSpec {
    allow_filesystem: Option<bool>,
    allow_execute: Option<bool>,
    allow_network: Option<bool>,
    max_steps: Option<usize>,
    timeout_ms: Option<u64>,
    max_output_chars: Option<usize>,
}

pub fn load_skill_dir(path: &Path, source: &str, options: SkillValidationOptions) -> Result<SkillPackage> {
    validate_dir_safety(path)?;
    let skill_md = path.join("SKILL.md");
    if !skill_md.exists() {
        return Err(anyhow::anyhow!("missing SKILL.md"));
    }
    let md_meta = std::fs::metadata(&skill_md)?;
    if md_meta.len() > MAX_SKILL_FILE_SIZE {
        return Err(anyhow::anyhow!("SKILL.md too large"));
    }
    let content = std::fs::read_to_string(&skill_md)?;
    let frontmatter = parse_frontmatter(&content)?;
    let metadata = build_metadata(frontmatter, path, source, options.strict)?;

    let tools_json = path.join("tools.json");
    let tools = if tools_json.exists() {
        let raw = std::fs::read(&tools_json)?;
        let file: ToolsFile = serde_json::from_slice(&raw)?;
        file.tools
            .into_iter()
            .map(|t| to_tool_spec(t, &metadata))
            .collect::<Result<Vec<_>>>()?
    } else {
        Vec::new()
    };

    Ok(SkillPackage { metadata, tools })
}

fn validate_dir_safety(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(anyhow::anyhow!("symlink not allowed"));
    }
    Ok(())
}

fn parse_frontmatter(content: &str) -> Result<serde_yaml::Value> {
    let mut lines = content.lines();
    let first = lines.next().unwrap_or("");
    if first.trim() != "---" {
        return Err(anyhow::anyhow!("missing frontmatter"));
    }
    let mut yaml_lines = Vec::new();
    for line in lines {
        if line.trim() == "---" {
            let yaml_str = yaml_lines.join("\n");
            let v: serde_yaml::Value = serde_yaml::from_str(&yaml_str)?;
            return Ok(v);
        }
        yaml_lines.push(line.to_string());
    }
    Err(anyhow::anyhow!("unterminated frontmatter"))
}

fn build_metadata(
    frontmatter: serde_yaml::Value,
    path: &Path,
    source: &str,
    strict: bool,
) -> Result<SkillMetadata> {
    let map = frontmatter
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("frontmatter must be mapping"))?;

    let name = get_str(map, "name")?;
    let description = get_str(map, "description")?;
    validate_skill_name(&name, path)?;
    validate_description(&description)?;

    let license = get_opt_str(map, "license");
    let compatibility = get_opt_str(map, "compatibility");
    let metadata = get_metadata(map, "metadata")?;
    let allowed_tools = get_allowed_tools(map, "allowed-tools")?;

    if strict {
        for key in map.keys() {
            if let Some(k) = key.as_str() {
                if !matches!(
                    k,
                    "name" | "description" | "license" | "compatibility" | "metadata" | "allowed-tools"
                ) {
                    return Err(anyhow::anyhow!("unknown frontmatter field: {}", k));
                }
            }
        }
    }

    Ok(SkillMetadata {
        name,
        description,
        path: skill_md_path(path)?,
        source: source.to_string(),
        license,
        compatibility,
        metadata,
        allowed_tools,
    })
}

fn skill_md_path(path: &Path) -> Result<String> {
    Ok(path.join("SKILL.md").to_string_lossy().to_string())
}

fn validate_skill_name(name: &str, path: &Path) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        return Err(anyhow::anyhow!("invalid name length"));
    }
    if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
        return Err(anyhow::anyhow!("invalid name format"));
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(anyhow::anyhow!("invalid name charset"));
    }
    let dir_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid skill dir"))?;
    if dir_name != name {
        return Err(anyhow::anyhow!("skill name must match directory"));
    }
    Ok(())
}

fn validate_description(desc: &str) -> Result<()> {
    if desc.is_empty() || desc.len() > 1024 {
        return Err(anyhow::anyhow!("invalid description length"));
    }
    Ok(())
}

fn get_str(map: &serde_yaml::Mapping, key: &str) -> Result<String> {
    let v = map
        .get(serde_yaml::Value::String(key.to_string()))
        .ok_or_else(|| anyhow::anyhow!("missing field: {}", key))?;
    v.as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("invalid field: {}", key))
}

fn get_opt_str(map: &serde_yaml::Mapping, key: &str) -> Option<String> {
    map.get(serde_yaml::Value::String(key.to_string()))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn get_metadata(map: &serde_yaml::Mapping, key: &str) -> Result<BTreeMap<String, String>> {
    let Some(v) = map.get(serde_yaml::Value::String(key.to_string())) else {
        return Ok(BTreeMap::new());
    };
    let Some(m) = v.as_mapping() else {
        return Err(anyhow::anyhow!("metadata must be mapping"));
    };
    let mut out = BTreeMap::new();
    for (k, v) in m {
        let key = k
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("metadata key must be string"))?
            .to_string();
        let val = v
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("metadata value must be string"))?
            .to_string();
        out.insert(key, val);
    }
    Ok(out)
}

fn get_allowed_tools(map: &serde_yaml::Mapping, key: &str) -> Result<Vec<String>> {
    let Some(v) = map.get(serde_yaml::Value::String(key.to_string())) else {
        return Ok(Vec::new());
    };
    if let Some(seq) = v.as_sequence() {
        let mut out = Vec::new();
        for item in seq {
            let s = item
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("allowed-tools item must be string"))?;
            out.push(s.to_string());
        }
        return Ok(out);
    }
    if let Some(s) = v.as_str() {
        let parts = s.split_whitespace().filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
        return Ok(parts);
    }
    Err(anyhow::anyhow!("allowed-tools must be list or string"))
}

fn to_tool_spec(tool: ToolFileSpec, metadata: &SkillMetadata) -> Result<SkillToolSpec> {
    if tool.name.trim().is_empty() {
        return Err(anyhow::anyhow!("tool name required"));
    }
    if tool.description.trim().is_empty() {
        return Err(anyhow::anyhow!("tool description required"));
    }
    let policy = resolve_policy(tool.policy);
    let steps = tool
        .steps
        .into_iter()
        .map(|s| SkillToolStep {
            tool_name: s.tool_name,
            arguments: s.arguments,
        })
        .collect::<Vec<_>>();
    Ok(SkillToolSpec {
        name: tool.name,
        description: tool.description,
        input_schema: tool.input_schema,
        steps,
        policy,
        skill_name: metadata.name.clone(),
        source: metadata.source.clone(),
    })
}

fn resolve_policy(policy: Option<ToolPolicyFileSpec>) -> SkillToolPolicy {
    let mut out = SkillToolPolicy::default();
    if let Some(p) = policy {
        if let Some(v) = p.allow_filesystem {
            out.allow_filesystem = v;
        }
        if let Some(v) = p.allow_execute {
            out.allow_execute = v;
        }
        if let Some(v) = p.allow_network {
            out.allow_network = v;
        }
        if let Some(v) = p.max_steps {
            out.max_steps = v.max(1);
        }
        if let Some(v) = p.timeout_ms {
            out.timeout_ms = v.max(1);
        }
        if let Some(v) = p.max_output_chars {
            out.max_output_chars = v.max(1);
        }
    }
    out
}
