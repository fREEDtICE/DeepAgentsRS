//! Validation helpers for package-based skills and executable `tools.json`
//! definitions.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use serde::Deserialize;

use crate::skills::{SkillMetadata, SkillToolPolicy, SkillToolSpec, SkillToolStep};

const MAX_SKILL_FILE_SIZE: u64 = 10 * 1024 * 1024;
const RUNTIME_ONLY_PACKAGE_STEP_TOOLS: [&str; 3] = ["task", "compact_conversation", "write_todos"];

#[derive(Debug, Clone)]
/// Controls how strictly package frontmatter is validated during loading.
pub struct SkillValidationOptions {
    pub strict: bool,
}

impl Default for SkillValidationOptions {
    fn default() -> Self {
        Self { strict: true }
    }
}

#[derive(Debug, Clone)]
/// Represents a validated package skill with metadata and executable tools.
pub struct SkillPackage {
    pub metadata: SkillMetadata,
    pub tools: Vec<SkillToolSpec>,
}

/// Classifies a package-skill step according to the runtime policy boundary
/// that must guard it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageSkillStepKind {
    AgentOwned,
    Filesystem,
    Execute,
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

/// Loads and validates a single package skill directory from disk.
pub fn load_skill_dir(
    path: &Path,
    source: &str,
    options: SkillValidationOptions,
) -> Result<SkillPackage> {
    validate_dir_safety(path).map_err(|error| anyhow::anyhow!("{}: {}", path.display(), error))?;
    let skill_md = path.join("SKILL.md");
    if !skill_md.exists() {
        return Err(anyhow::anyhow!("{}: missing SKILL.md", skill_md.display()));
    }
    let md_meta = std::fs::metadata(&skill_md)
        .map_err(|error| anyhow::anyhow!("{}: {}", skill_md.display(), error))?;
    if md_meta.len() > MAX_SKILL_FILE_SIZE {
        return Err(anyhow::anyhow!(
            "{}: SKILL.md too large",
            skill_md.display()
        ));
    }
    let content = std::fs::read_to_string(&skill_md)
        .map_err(|error| anyhow::anyhow!("{}: {}", skill_md.display(), error))?;
    let frontmatter = parse_frontmatter(&content)
        .map_err(|error| anyhow::anyhow!("{}: frontmatter: {}", skill_md.display(), error))?;
    let metadata = build_metadata(frontmatter, path, source, options.strict)
        .map_err(|error| anyhow::anyhow!("{}: {}", skill_md.display(), error))?;

    let tools_json = path.join("tools.json");
    let tools = if tools_json.exists() {
        let raw = std::fs::read(&tools_json)
            .map_err(|error| anyhow::anyhow!("{}: {}", tools_json.display(), error))?;
        let file: ToolsFile = serde_json::from_slice(&raw)
            .map_err(|error| anyhow::anyhow!("{}: {}", tools_json.display(), error))?;
        file.tools
            .into_iter()
            .enumerate()
            .map(|(index, tool)| {
                let tool_name = tool.name.clone();
                to_tool_spec(tool, &metadata).map_err(|error| {
                    anyhow::anyhow!(
                        "{}: tools[{index}] ({tool_name}): {}",
                        tools_json.display(),
                        error
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        Vec::new()
    };

    Ok(SkillPackage { metadata, tools })
}

/// Validates the supported subset of JSON Schema accepted for package-skill
/// tool inputs.
pub fn validate_package_skill_input_schema(schema: &serde_json::Value) -> Result<()> {
    let schema_obj = schema
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("schema must be object"))?;
    let typ = schema_obj
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("object");
    if typ != "object" {
        return Err(anyhow::anyhow!("schema type must be object"));
    }

    let properties = match schema_obj.get("properties") {
        Some(value) => Some(
            value
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("properties must be object"))?,
        ),
        None => None,
    };
    if let Some(properties) = properties {
        for (name, property_schema) in properties {
            validate_property_schema(name, property_schema)?;
        }
    }

    let required = parse_required_fields(schema_obj)?;
    if !required.is_empty() {
        let Some(properties) = properties else {
            return Err(anyhow::anyhow!(
                "required fields require properties definitions"
            ));
        };
        for key in required {
            if !properties.contains_key(&key) {
                return Err(anyhow::anyhow!(
                    "required field not declared in properties: {}",
                    key
                ));
            }
        }
    }

    if let Some(value) = schema_obj.get("additionalProperties") {
        if !value.is_boolean() {
            return Err(anyhow::anyhow!("additionalProperties must be boolean"));
        }
    }

    Ok(())
}

/// Validates a runtime input object against the supported package-skill schema
/// contract.
pub fn validate_package_skill_input(
    schema: &serde_json::Value,
    input: &serde_json::Value,
) -> Result<()> {
    validate_package_skill_input_schema(schema)?;

    let schema_obj = schema
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("schema must be object"))?;
    let input_obj = input
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("input must be object"))?;
    let properties = schema_obj
        .get("properties")
        .and_then(|value| value.as_object());
    let required = parse_required_fields(schema_obj)?;
    for key in required {
        if !input_obj.contains_key(&key) {
            return Err(anyhow::anyhow!("missing required field: {}", key));
        }
    }

    let additional_properties = schema_obj
        .get("additionalProperties")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    if !additional_properties {
        for key in input_obj.keys() {
            if properties.is_none_or(|properties| !properties.contains_key(key)) {
                return Err(anyhow::anyhow!("unexpected field: {}", key));
            }
        }
    }

    if let Some(properties) = properties {
        for (key, property_schema) in properties {
            if let Some(value) = input_obj.get(key) {
                validate_input_value(key, property_schema, value)?;
            }
        }
    }

    Ok(())
}

/// Rejects runtime-only tools from package-skill step definitions and returns
/// the policy bucket for tools that remain valid in package skills.
pub fn classify_package_skill_step_tool(name: &str) -> Result<PackageSkillStepKind> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow::anyhow!("step tool_name required"));
    }
    if RUNTIME_ONLY_PACKAGE_STEP_TOOLS.contains(&name) {
        return Err(anyhow::anyhow!(
            "skill_step_not_supported: runtime-only tool {} cannot be used in package skills",
            name
        ));
    }
    if name == "execute" {
        return Ok(PackageSkillStepKind::Execute);
    }
    if is_filesystem_tool(name) {
        return Ok(PackageSkillStepKind::Filesystem);
    }
    Ok(PackageSkillStepKind::AgentOwned)
}

/// Rejects symlinked skill directories so loading stays within the declared
/// source tree.
fn validate_dir_safety(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(anyhow::anyhow!("symlink not allowed"));
    }
    Ok(())
}

/// Extracts the YAML frontmatter used to define package skill metadata.
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

/// Builds normalized skill metadata from parsed frontmatter.
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
                    "name"
                        | "description"
                        | "license"
                        | "compatibility"
                        | "metadata"
                        | "allowed-tools"
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

/// Computes the canonical `SKILL.md` path recorded in metadata.
fn skill_md_path(path: &Path) -> Result<String> {
    Ok(path.join("SKILL.md").to_string_lossy().to_string())
}

/// Enforces the package skill naming rules and directory-name invariant.
fn validate_skill_name(name: &str, path: &Path) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        return Err(anyhow::anyhow!("invalid name length"));
    }
    if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
        return Err(anyhow::anyhow!("invalid name format"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
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

/// Enforces the package skill description length constraint.
fn validate_description(desc: &str) -> Result<()> {
    if desc.is_empty() || desc.len() > 1024 {
        return Err(anyhow::anyhow!("invalid description length"));
    }
    Ok(())
}

/// Reads a required string field from package skill frontmatter.
fn get_str(map: &serde_yaml::Mapping, key: &str) -> Result<String> {
    let v = map
        .get(serde_yaml::Value::String(key.to_string()))
        .ok_or_else(|| anyhow::anyhow!("missing field: {}", key))?;
    v.as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("invalid field: {}", key))
}

/// Reads an optional trimmed string field from package skill frontmatter.
fn get_opt_str(map: &serde_yaml::Mapping, key: &str) -> Option<String> {
    map.get(serde_yaml::Value::String(key.to_string()))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Parses the optional arbitrary metadata map from package skill frontmatter.
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

/// Parses the optional allowed-tools declaration into a normalized list.
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
        let parts = s
            .split_whitespace()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        return Ok(parts);
    }
    Err(anyhow::anyhow!("allowed-tools must be list or string"))
}

/// Converts a parsed `tools.json` record into the runtime skill tool shape.
fn to_tool_spec(tool: ToolFileSpec, metadata: &SkillMetadata) -> Result<SkillToolSpec> {
    if tool.name.trim().is_empty() {
        return Err(anyhow::anyhow!("tool name required"));
    }
    if tool.description.trim().is_empty() {
        return Err(anyhow::anyhow!("tool description required"));
    }
    validate_package_skill_input_schema(&tool.input_schema)?;
    let policy = resolve_policy(tool.policy);
    let steps = tool
        .steps
        .into_iter()
        .map(|s| {
            classify_package_skill_step_tool(&s.tool_name)?;
            if !s.arguments.is_object() {
                return Err(anyhow::anyhow!("step arguments must be object"));
            }
            Ok(SkillToolStep {
                tool_name: s.tool_name.trim().to_string(),
                arguments: s.arguments,
            })
        })
        .collect::<Result<Vec<_>>>()?;
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

/// Applies package skill policy defaults and boundary normalization.
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

/// Validates a single property schema inside the supported JSON Schema subset.
fn validate_property_schema(name: &str, schema: &serde_json::Value) -> Result<()> {
    let schema_obj = schema
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("property schema for {} must be object", name))?;
    if let Some(typ) = schema_obj.get("type").and_then(|value| value.as_str()) {
        validate_supported_schema_type(name, typ)?;
    }
    if let Some(enum_values) = schema_obj.get("enum") {
        let enum_values = enum_values
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("enum for {} must be array", name))?;
        if enum_values.is_empty() {
            return Err(anyhow::anyhow!("enum for {} must not be empty", name));
        }
        if let Some(typ) = schema_obj.get("type").and_then(|value| value.as_str()) {
            for value in enum_values {
                if !matches_schema_type(value, typ) {
                    return Err(anyhow::anyhow!(
                        "enum value for {} must match declared type {}",
                        name,
                        typ
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Parses the optional `required` array from an object schema.
fn parse_required_fields(
    schema_obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<String>> {
    let Some(required) = schema_obj.get("required") else {
        return Ok(Vec::new());
    };
    let required = required
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("required must be array"))?;
    let mut out = Vec::with_capacity(required.len());
    for item in required {
        let item = item
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("required must contain strings"))?;
        out.push(item.to_string());
    }
    Ok(out)
}

/// Validates a runtime input value against one property schema.
fn validate_input_value(
    name: &str,
    property_schema: &serde_json::Value,
    value: &serde_json::Value,
) -> Result<()> {
    let schema_obj = property_schema
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("property schema for {} must be object", name))?;
    if let Some(typ) = schema_obj
        .get("type")
        .and_then(|schema_type| schema_type.as_str())
    {
        if !matches_schema_type(value, typ) {
            return Err(anyhow::anyhow!("field {} must be {}", name, typ));
        }
    }
    if let Some(enum_values) = schema_obj.get("enum") {
        let enum_values = enum_values
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("enum for {} must be array", name))?;
        if !enum_values.iter().any(|candidate| candidate == value) {
            return Err(anyhow::anyhow!(
                "field {} must match one of the declared enum values",
                name
            ));
        }
    }
    Ok(())
}

/// Validates that a declared JSON Schema type is part of the supported subset.
fn validate_supported_schema_type(name: &str, typ: &str) -> Result<()> {
    if matches!(
        typ,
        "string" | "number" | "integer" | "boolean" | "object" | "array" | "null"
    ) {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "property {} uses unsupported schema type {}",
        name,
        typ
    ))
}

/// Checks whether a runtime value matches one supported JSON Schema type.
fn matches_schema_type(value: &serde_json::Value, typ: &str) -> bool {
    match typ {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "null" => value.is_null(),
        _ => false,
    }
}

/// Identifies filesystem tools that require explicit package-skill policy.
fn is_filesystem_tool(name: &str) -> bool {
    matches!(
        name,
        "ls" | "read_file" | "write_file" | "edit_file" | "delete_file" | "glob" | "grep"
    )
}
