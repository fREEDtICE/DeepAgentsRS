//! Validation helpers for package-based skills and executable `tools.json`
//! definitions.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, Result};
use semver::Version;
use serde::Deserialize;
use walkdir::WalkDir;

use crate::skills::{
    default_compat_skill_version, SkillFragmentSet, SkillGovernanceFinding, SkillGovernanceOutcome,
    SkillGovernanceSeverity, SkillIdentity, SkillManifest, SkillMetadata, SkillPackage,
    SkillRiskLevel, SkillToolPolicy, SkillToolSpec, SkillToolStep, SkillTriggerHints,
};

const MAX_SKILL_FILE_SIZE: u64 = 10 * 1024 * 1024;
const RUNTIME_ONLY_PACKAGE_STEP_TOOLS: [&str; 3] = ["task", "compact_conversation", "write_todos"];

/// Controls how strictly package frontmatter is validated during loading.
#[derive(Debug, Clone)]
pub struct SkillValidationOptions {
    /// Whether unknown fields should fail fast.
    pub strict: bool,
    /// Whether versionless packages should be normalized to `0.0.0-dev`.
    pub allow_versionless_compat: bool,
}

impl Default for SkillValidationOptions {
    fn default() -> Self {
        Self {
            strict: true,
            allow_versionless_compat: true,
        }
    }
}

/// Classifies a package-skill step according to the runtime policy boundary
/// that must guard it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageSkillStepKind {
    /// Tool is owned directly by the base agent tool registry.
    AgentOwned,
    /// Tool touches the filesystem and requires `allow_filesystem`.
    Filesystem,
    /// Tool is `execute` and requires `allow_execute`.
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
    validate_dir_safety(path).map_err(|error| anyhow!("{}: {}", path.display(), error))?;
    let skill_md = path.join("SKILL.md");
    if !skill_md.exists() {
        return Err(anyhow!("{}: missing SKILL.md", skill_md.display()));
    }
    let md_meta = std::fs::metadata(&skill_md)
        .map_err(|error| anyhow!("{}: {}", skill_md.display(), error))?;
    if md_meta.len() > MAX_SKILL_FILE_SIZE {
        return Err(anyhow!("{}: SKILL.md too large", skill_md.display()));
    }

    let content = std::fs::read_to_string(&skill_md)
        .map_err(|error| anyhow!("{}: {}", skill_md.display(), error))?;
    let (frontmatter, body) = parse_frontmatter_and_body(&content)
        .map_err(|error| anyhow!("{}: frontmatter: {}", skill_md.display(), error))?;
    let (manifest, compat_versionless) = build_manifest(frontmatter, path, source, &options)
        .map_err(|error| anyhow!("{}: {}", skill_md.display(), error))?;
    let mut fragments = extract_fragments(body);
    fragments.assets = discover_asset_paths(path)?;

    let tools_json = path.join("tools.json");
    let tools = if tools_json.exists() {
        let raw = std::fs::read(&tools_json)
            .map_err(|error| anyhow!("{}: {}", tools_json.display(), error))?;
        let file: ToolsFile = serde_json::from_slice(&raw)
            .map_err(|error| anyhow!("{}: {}", tools_json.display(), error))?;
        file.tools
            .into_iter()
            .enumerate()
            .map(|(index, tool)| {
                let tool_name = tool.name.clone();
                to_tool_spec(tool, &manifest).map_err(|error| {
                    anyhow!(
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

    let mut governance = SkillGovernanceOutcome::default();
    if compat_versionless {
        governance.findings.push(SkillGovernanceFinding {
            code: "compat_versionless".to_string(),
            message: "versionless package normalized to 0.0.0-dev in source compatibility mode"
                .to_string(),
            severity: SkillGovernanceSeverity::Warn,
        });
        governance.canonicalize();
    }

    Ok(SkillPackage {
        metadata: SkillMetadata::from(&manifest),
        manifest,
        fragments,
        tools,
        governance,
    })
}

/// Validates the supported subset of JSON Schema accepted for package-skill
/// tool inputs.
pub fn validate_package_skill_input_schema(schema: &serde_json::Value) -> Result<()> {
    let schema_obj = schema
        .as_object()
        .ok_or_else(|| anyhow!("schema must be object"))?;
    let typ = schema_obj
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("object");
    if typ != "object" {
        return Err(anyhow!("schema type must be object"));
    }

    let properties = match schema_obj.get("properties") {
        Some(value) => Some(
            value
                .as_object()
                .ok_or_else(|| anyhow!("properties must be object"))?,
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
            return Err(anyhow!(
                "required fields require properties definitions"
            ));
        };
        for key in required {
            if !properties.contains_key(&key) {
                return Err(anyhow!("required field not declared in properties: {}", key));
            }
        }
    }

    if let Some(value) = schema_obj.get("additionalProperties") {
        if !value.is_boolean() {
            return Err(anyhow!("additionalProperties must be boolean"));
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
        .ok_or_else(|| anyhow!("schema must be object"))?;
    let input_obj = input
        .as_object()
        .ok_or_else(|| anyhow!("input must be object"))?;
    let properties = schema_obj.get("properties").and_then(|value| value.as_object());
    let required = parse_required_fields(schema_obj)?;
    for key in required {
        if !input_obj.contains_key(&key) {
            return Err(anyhow!("missing required field: {}", key));
        }
    }

    let additional_properties = schema_obj
        .get("additionalProperties")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    if !additional_properties {
        for key in input_obj.keys() {
            if properties.is_none_or(|properties| !properties.contains_key(key)) {
                return Err(anyhow!("unexpected field: {}", key));
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
        return Err(anyhow!("step tool_name required"));
    }
    if RUNTIME_ONLY_PACKAGE_STEP_TOOLS.contains(&name) {
        return Err(anyhow!(
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
        return Err(anyhow!("symlink not allowed"));
    }
    Ok(())
}

fn parse_frontmatter_and_body(content: &str) -> Result<(serde_yaml::Value, &str)> {
    let mut lines = content.lines();
    let first = lines.next().unwrap_or("");
    if first.trim() != "---" {
        return Err(anyhow!("missing frontmatter"));
    }
    let mut yaml_lines = Vec::new();
    let mut offset = first.len() + 1;
    for line in lines {
        if line.trim() == "---" {
            let yaml_str = yaml_lines.join("\n");
            let value: serde_yaml::Value = serde_yaml::from_str(&yaml_str)?;
            let body = &content[offset + line.len() + 1..];
            return Ok((value, body));
        }
        yaml_lines.push(line.to_string());
        offset += line.len() + 1;
    }
    Err(anyhow!("unterminated frontmatter"))
}

fn build_manifest(
    frontmatter: serde_yaml::Value,
    path: &Path,
    source: &str,
    options: &SkillValidationOptions,
) -> Result<(SkillManifest, bool)> {
    let map = frontmatter
        .as_mapping()
        .ok_or_else(|| anyhow!("frontmatter must be mapping"))?;

    let name = get_str(map, "name")?;
    validate_skill_name(&name)?;
    let version = match get_opt_str(map, "version") {
        Some(version) => {
            Version::parse(&version).map_err(|error| anyhow!("invalid version: {error}"))?;
            (version, false)
        }
        None if options.allow_versionless_compat => (default_compat_skill_version(), true),
        None => return Err(anyhow!("missing required field: version")),
    };

    let description = get_str(map, "description")?;
    validate_description(&description)?;

    if options.strict {
        for key in map.keys() {
            if let Some(key) = key.as_str() {
                if !matches!(
                    key,
                    "name"
                        | "version"
                        | "description"
                        | "license"
                        | "compatibility"
                        | "metadata"
                        | "allowed-tools"
                        | "triggers"
                        | "risk-level"
                        | "output-contract"
                        | "default-enabled"
                        | "requires-isolation"
                ) {
                    return Err(anyhow!("unknown frontmatter field: {}", key));
                }
            }
        }
    }

    let manifest = SkillManifest {
        identity: SkillIdentity {
            name,
            version: version.0,
        },
        description,
        path: path.to_string_lossy().to_string(),
        source: source.to_string(),
        license: get_opt_str(map, "license"),
        compatibility: get_opt_str(map, "compatibility"),
        metadata: get_metadata(map, "metadata")?,
        allowed_tools: get_allowed_tools(map, "allowed-tools")?,
        triggers: get_triggers(map, "triggers")?,
        risk_level: get_risk_level(map, "risk-level")?,
        output_contract: get_opt_str(map, "output-contract"),
        default_enabled: get_opt_bool(map, "default-enabled").unwrap_or(true),
        requires_isolation: get_opt_bool(map, "requires-isolation").unwrap_or(false),
    };

    Ok((manifest, version.1))
}

fn extract_fragments(body: &str) -> SkillFragmentSet {
    let mut fragments = SkillFragmentSet::default();
    let mut current: Option<&str> = None;
    let mut buckets: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for line in body.lines() {
        if let Some(section) = line.strip_prefix("## ") {
            current = match section.trim() {
                "Role" => Some("role"),
                "When to Use" => Some("when_to_use"),
                "Inputs" => Some("inputs"),
                "Constraints" => Some("constraints"),
                "Workflow" => Some("workflow"),
                "Output" => Some("output"),
                "Examples" => Some("examples"),
                "References" => Some("references"),
                _ => None,
            };
            continue;
        }
        if let Some(current) = current {
            buckets.entry(current).or_default().push(line.to_string());
        }
    }
    for (name, lines) in buckets {
        let content = lines.join("\n").trim().to_string();
        if content.is_empty() {
            continue;
        }
        match name {
            "role" => fragments.role = Some(content),
            "when_to_use" => fragments.when_to_use = Some(content),
            "inputs" => fragments.inputs = Some(content),
            "constraints" => fragments.constraints = Some(content),
            "workflow" => fragments.workflow = Some(content),
            "output" => fragments.output = Some(content),
            "examples" => fragments.examples = Some(content),
            "references" => fragments.references = Some(content),
            _ => {}
        }
    }
    fragments
}

fn discover_asset_paths(root: &Path) -> Result<crate::skills::SkillAssetPaths> {
    Ok(crate::skills::SkillAssetPaths {
        references: collect_asset_dir(root, "references")?,
        examples: collect_asset_dir(root, "examples")?,
        templates: collect_asset_dir(root, "templates")?,
    })
}

fn collect_asset_dir(root: &Path, name: &str) -> Result<Vec<String>> {
    let dir = root.join(name);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    if std::fs::symlink_metadata(&dir)?.file_type().is_symlink() {
        return Err(anyhow!("asset directory symlink not allowed: {}", dir.display()));
    }
    let mut out = WalkDir::new(&dir)
        .follow_links(false)
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()?;
    out.sort_by(|a, b| a.path().cmp(b.path()));
    let mut paths = Vec::new();
    for entry in out {
        if entry.file_type().is_dir() {
            continue;
        }
        if entry.file_type().is_symlink() {
            return Err(anyhow!("asset symlink not allowed: {}", entry.path().display()));
        }
        let relative = entry.path().strip_prefix(root).map_err(|_| {
            anyhow!("asset path escaped package root: {}", entry.path().display())
        })?;
        paths.push(relative.to_string_lossy().to_string());
    }
    Ok(paths)
}

fn to_tool_spec(file: ToolFileSpec, manifest: &SkillManifest) -> Result<SkillToolSpec> {
    validate_package_skill_input_schema(&file.input_schema)?;

    let policy = to_policy(file.policy);
    let steps = file
        .steps
        .into_iter()
        .map(|step| {
            classify_package_skill_step_tool(&step.tool_name)?;
            if !step.arguments.is_object() {
                return Err(anyhow!("step arguments must be object"));
            }
            Ok(SkillToolStep {
                tool_name: step.tool_name,
                arguments: step.arguments,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(SkillToolSpec {
        name: file.name,
        description: file.description,
        input_schema: file.input_schema,
        steps,
        policy,
        skill_name: manifest.identity.name.clone(),
        skill_version: manifest.identity.version.clone(),
        source: manifest.source.clone(),
        requires_isolation: manifest.requires_isolation,
        subagent_type: manifest.metadata.get("subagent_type").cloned(),
    })
}

fn to_policy(value: Option<ToolPolicyFileSpec>) -> SkillToolPolicy {
    let mut policy = SkillToolPolicy::default();
    if let Some(value) = value {
        if let Some(v) = value.allow_filesystem {
            policy.allow_filesystem = v;
        }
        if let Some(v) = value.allow_execute {
            policy.allow_execute = v;
        }
        if let Some(v) = value.allow_network {
            policy.allow_network = v;
        }
        if let Some(v) = value.max_steps {
            policy.max_steps = v;
        }
        if let Some(v) = value.timeout_ms {
            policy.timeout_ms = v;
        }
        if let Some(v) = value.max_output_chars {
            policy.max_output_chars = v;
        }
    }
    policy
}

fn validate_skill_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(anyhow!("name required"));
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(anyhow!(
            "name must use lowercase ASCII letters, digits, and '-'"
        ));
    }
    Ok(())
}

fn validate_description(description: &str) -> Result<()> {
    if description.trim().is_empty() {
        return Err(anyhow!("description required"));
    }
    Ok(())
}

fn get_str(map: &serde_yaml::Mapping, key: &str) -> Result<String> {
    get_opt_str(map, key).ok_or_else(|| anyhow!("missing required field: {}", key))
}

fn get_opt_str(map: &serde_yaml::Mapping, key: &str) -> Option<String> {
    map.get(serde_yaml::Value::String(key.to_string()))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

fn get_opt_bool(map: &serde_yaml::Mapping, key: &str) -> Option<bool> {
    map.get(serde_yaml::Value::String(key.to_string()))
        .and_then(|value| value.as_bool())
}

fn get_metadata(map: &serde_yaml::Mapping, key: &str) -> Result<BTreeMap<String, String>> {
    let Some(value) = map.get(serde_yaml::Value::String(key.to_string())) else {
        return Ok(BTreeMap::new());
    };
    let mapping = value
        .as_mapping()
        .ok_or_else(|| anyhow!("{key} must be mapping"))?;
    let mut out = BTreeMap::new();
    for (key, value) in mapping {
        let key = key
            .as_str()
            .ok_or_else(|| anyhow!("{key:?}: metadata key must be string"))?;
        let value = match value {
            serde_yaml::Value::String(value) => value.clone(),
            serde_yaml::Value::Bool(value) => value.to_string(),
            serde_yaml::Value::Number(value) => value.to_string(),
            other => serde_yaml::to_string(other)?.trim().to_string(),
        };
        out.insert(key.to_string(), value);
    }
    Ok(out)
}

fn get_allowed_tools(map: &serde_yaml::Mapping, key: &str) -> Result<Vec<String>> {
    let Some(value) = map.get(serde_yaml::Value::String(key.to_string())) else {
        return Ok(Vec::new());
    };
    let sequence = value
        .as_sequence()
        .ok_or_else(|| anyhow!("{key} must be list"))?;
    sequence
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(|value| value.to_string())
                .ok_or_else(|| anyhow!("{key} entries must be strings"))
        })
        .collect()
}

fn get_triggers(map: &serde_yaml::Mapping, key: &str) -> Result<SkillTriggerHints> {
    let Some(value) = map.get(serde_yaml::Value::String(key.to_string())) else {
        return Ok(SkillTriggerHints::default());
    };
    let mapping = value
        .as_mapping()
        .ok_or_else(|| anyhow!("{key} must be mapping"))?;
    let keywords = mapping
        .get(serde_yaml::Value::String("keywords".to_string()))
        .map(|value| {
            value
                .as_sequence()
                .ok_or_else(|| anyhow!("triggers.keywords must be list"))?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(|value| value.to_string())
                        .ok_or_else(|| anyhow!("triggers.keywords entries must be strings"))
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(SkillTriggerHints { keywords })
}

fn get_risk_level(map: &serde_yaml::Mapping, key: &str) -> Result<SkillRiskLevel> {
    let Some(value) = get_opt_str(map, key) else {
        return Ok(SkillRiskLevel::Low);
    };
    SkillRiskLevel::parse(&value).ok_or_else(|| anyhow!("invalid risk-level: {}", value))
}

fn validate_property_schema(name: &str, schema: &serde_json::Value) -> Result<()> {
    let schema_obj = schema
        .as_object()
        .ok_or_else(|| anyhow!("property schema for {} must be object", name))?;
    let Some(typ) = schema_obj.get("type").and_then(|value| value.as_str()) else {
        return Err(anyhow!("property {} missing type", name));
    };
    if !matches!(typ, "string" | "integer" | "number" | "boolean" | "object" | "array") {
        return Err(anyhow!("property {} has unsupported type: {}", name, typ));
    }
    Ok(())
}

fn validate_input_value(
    name: &str,
    schema: &serde_json::Value,
    value: &serde_json::Value,
) -> Result<()> {
    let schema_obj = schema
        .as_object()
        .ok_or_else(|| anyhow!("property schema for {} must be object", name))?;
    let typ = schema_obj
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("string");
    let ok = match typ {
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(anyhow!("invalid type for {}: expected {}", name, typ))
    }
}

fn parse_required_fields(schema_obj: &serde_json::Map<String, serde_json::Value>) -> Result<Vec<String>> {
    let Some(required) = schema_obj.get("required") else {
        return Ok(Vec::new());
    };
    let sequence = required
        .as_array()
        .ok_or_else(|| anyhow!("required must be array"))?;
    sequence
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(|value| value.to_string())
                .ok_or_else(|| anyhow!("required entries must be strings"))
        })
        .collect()
}

fn is_filesystem_tool(name: &str) -> bool {
    matches!(
        name,
        "ls" | "read_file" | "write_file" | "edit_file" | "delete_file" | "glob" | "grep"
    )
}
