//! Skill-system domain types, runtime snapshot helpers, and legacy mirrors.

pub mod governance;
pub mod loader;
pub mod registry;
pub mod selection;
pub mod validator;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::state::AgentState;

/// Runtime state key that stores the resolved, thread-sticky skill snapshot.
pub const SKILLS_SNAPSHOT_KEY: &str = "skills_snapshot";
/// Runtime state key that stores selection/candidate reporting for the run.
pub const SKILLS_SELECTION_KEY: &str = "skills_selection";
/// Runtime state key that stores discovery, validation, and governance diagnostics.
pub const SKILLS_DIAGNOSTICS_KEY: &str = "skills_diagnostics";
/// Legacy runtime state key mirrored for compatibility with existing tests and runner seams.
pub const SKILLS_METADATA_KEY: &str = "skills_metadata";
/// Legacy runtime state key mirrored for compatibility with existing tests and runner seams.
pub const SKILLS_TOOLS_KEY: &str = "skills_tools";

/// Returns the compatibility version used for legacy source-only packages that
/// omit explicit version metadata.
pub fn default_compat_skill_version() -> String {
    "0.0.0-dev".to_string()
}

/// Returns the default `enabled` lifecycle state for newly installed skills.
pub fn default_skill_enabled() -> bool {
    true
}

/// Stable identity for a skill package version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct SkillIdentity {
    /// Human-stable lowercase skill name.
    pub name: String,
    /// Semantic version string for the installed package.
    #[serde(default = "default_compat_skill_version")]
    pub version: String,
}

impl SkillIdentity {
    /// Formats the skill identity as `name@version`.
    pub fn as_key(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

/// Risk buckets used by routing and isolation decisions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillRiskLevel {
    /// Low-risk skills are eligible for inline execution by default.
    #[default]
    Low,
    /// Medium-risk skills may still run inline, but lose tie-breaks to lower risk.
    Medium,
    /// High-risk skills prefer isolation.
    High,
    /// Critical-risk skills are isolated and may be quarantined more aggressively.
    Critical,
}

impl SkillRiskLevel {
    /// Parses a risk-level string from frontmatter.
    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }
}

/// Simple lexical trigger hints used by the deterministic selector.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillTriggerHints {
    /// Keywords that raise selection score when present in the user request.
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// Lazily loadable package asset paths discovered under the package root.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillAssetPaths {
    /// Relative files under `references/`.
    #[serde(default)]
    pub references: Vec<String>,
    /// Relative files under `examples/`.
    #[serde(default)]
    pub examples: Vec<String>,
    /// Relative files under `templates/`.
    #[serde(default)]
    pub templates: Vec<String>,
}

/// Parsed structured fragments extracted from `SKILL.md`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillFragmentSet {
    /// `## Role`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// `## When to Use`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    /// `## Inputs`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs: Option<String>,
    /// `## Constraints`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constraints: Option<String>,
    /// `## Workflow`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    /// `## Output`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// `## Examples`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<String>,
    /// `## References`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub references: Option<String>,
    /// Discovered package assets that remain lazily loaded.
    #[serde(default)]
    pub assets: SkillAssetPaths,
}

impl SkillFragmentSet {
    /// Returns the named fragment identifiers that are populated.
    pub fn available_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (name, value) in [
            ("role", self.role.as_ref()),
            ("when_to_use", self.when_to_use.as_ref()),
            ("inputs", self.inputs.as_ref()),
            ("constraints", self.constraints.as_ref()),
            ("workflow", self.workflow.as_ref()),
            ("output", self.output.as_ref()),
            ("examples", self.examples.as_ref()),
            ("references", self.references.as_ref()),
        ] {
            if value.is_some() {
                out.push(name.to_string());
            }
        }
        out
    }

    /// Returns the fragment content for a stable fragment identifier.
    pub fn get(&self, name: &str) -> Option<&str> {
        match name {
            "role" => self.role.as_deref(),
            "when_to_use" => self.when_to_use.as_deref(),
            "inputs" => self.inputs.as_deref(),
            "constraints" => self.constraints.as_deref(),
            "workflow" => self.workflow.as_deref(),
            "output" => self.output.as_deref(),
            "examples" => self.examples.as_deref(),
            "references" => self.references.as_deref(),
            _ => None,
        }
    }
}

/// Normalized manifest information for a skill package version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    /// Versioned package identity.
    pub identity: SkillIdentity,
    /// Human-readable description used in routing and provider exposure.
    pub description: String,
    /// Absolute on-disk package path.
    pub path: String,
    /// Source label from loader/registry resolution.
    pub source: String,
    /// Optional license string from frontmatter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Optional compatibility metadata string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<String>,
    /// Arbitrary string metadata exposed for routing and execution hints.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// Advisory tool names exposed to the model.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Deterministic lexical trigger hints.
    #[serde(default)]
    pub triggers: SkillTriggerHints,
    /// Declared risk level.
    #[serde(default)]
    pub risk_level: SkillRiskLevel,
    /// Optional output contract identifier used in routing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_contract: Option<String>,
    /// Whether the installed skill should be enabled by default.
    #[serde(default = "default_skill_enabled")]
    pub default_enabled: bool,
    /// Whether the skill must run in an isolated subagent execution mode.
    #[serde(default)]
    pub requires_isolation: bool,
}

/// Legacy provider/runtime-facing metadata mirror kept for compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// Skill name.
    pub name: String,
    /// Skill version string.
    #[serde(default = "default_compat_skill_version")]
    pub version: String,
    /// Human-readable description.
    pub description: String,
    /// Absolute on-disk package path.
    pub path: String,
    /// Source label from loader/registry resolution.
    pub source: String,
    /// Optional license string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Optional compatibility string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<String>,
    /// Arbitrary string metadata exposed to higher layers.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// Advisory tool names exposed to the model.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
}

impl From<&SkillManifest> for SkillMetadata {
    fn from(manifest: &SkillManifest) -> Self {
        Self {
            name: manifest.identity.name.clone(),
            version: manifest.identity.version.clone(),
            description: manifest.description.clone(),
            path: manifest.path.clone(),
            source: manifest.source.clone(),
            license: manifest.license.clone(),
            compatibility: manifest.compatibility.clone(),
            metadata: manifest.metadata.clone(),
            allowed_tools: manifest.allowed_tools.clone(),
        }
    }
}

/// Enforced policy for executable skill tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillToolPolicy {
    /// Allows filesystem tool usage.
    pub allow_filesystem: bool,
    /// Allows `execute`.
    pub allow_execute: bool,
    /// Allows future network-capable tools.
    pub allow_network: bool,
    /// Maximum declared skill steps.
    pub max_steps: usize,
    /// Per-skill timeout budget.
    pub timeout_ms: u64,
    /// Maximum output characters retained before local truncation.
    pub max_output_chars: usize,
}

impl Default for SkillToolPolicy {
    fn default() -> Self {
        Self {
            allow_filesystem: false,
            allow_execute: false,
            allow_network: false,
            max_steps: 8,
            timeout_ms: 1000,
            max_output_chars: 12000,
        }
    }
}

/// One declarative step inside an executable skill tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillToolStep {
    /// Runtime tool name invoked by the step.
    pub tool_name: String,
    /// Template arguments for the step.
    #[serde(default)]
    pub arguments: serde_json::Value,
}

/// Provider-visible executable skill tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillToolSpec {
    /// Tool name exposed to the provider.
    pub name: String,
    /// Tool description exposed to the provider.
    pub description: String,
    /// Input schema for runtime validation.
    pub input_schema: serde_json::Value,
    /// Declarative execution steps.
    #[serde(default)]
    pub steps: Vec<SkillToolStep>,
    /// Machine-checked tool policy.
    #[serde(default)]
    pub policy: SkillToolPolicy,
    /// Owning skill name.
    pub skill_name: String,
    /// Owning skill version.
    #[serde(default = "default_compat_skill_version")]
    pub skill_version: String,
    /// Loader/registry source label.
    pub source: String,
    /// Whether the tool must execute in isolated mode.
    #[serde(default)]
    pub requires_isolation: bool,
    /// Optional subagent type hint sourced from manifest metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_type: Option<String>,
}

/// Severity of a governance finding.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SkillGovernanceSeverity {
    /// Non-blocking finding that must remain visible.
    Warn,
    /// Blocking finding that forces quarantine.
    Fail,
}

/// One semantic governance finding emitted during validation or review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillGovernanceFinding {
    /// Stable finding code suitable for tests and operators.
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
    /// Finding severity.
    pub severity: SkillGovernanceSeverity,
}

/// Aggregated semantic governance outcome for a package version.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillGovernanceStatus {
    /// No semantic-review concerns were found.
    #[default]
    Pass,
    /// Review found non-blocking warnings.
    Warn,
    /// Review found blocking failures.
    Fail,
}

/// Final semantic-review outcome for a skill package.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillGovernanceOutcome {
    /// Aggregated review status.
    #[serde(default)]
    pub status: SkillGovernanceStatus,
    /// Detailed findings emitted by review.
    #[serde(default)]
    pub findings: Vec<SkillGovernanceFinding>,
}

impl SkillGovernanceOutcome {
    /// Recomputes the aggregate status from the detailed findings.
    pub fn canonicalize(&mut self) {
        self.findings.sort_by(|a, b| {
            a.severity
                .cmp(&b.severity)
                .then_with(|| a.code.cmp(&b.code))
                .then_with(|| a.message.cmp(&b.message))
        });
        self.status = if self
            .findings
            .iter()
            .any(|finding| finding.severity == SkillGovernanceSeverity::Fail)
        {
            SkillGovernanceStatus::Fail
        } else if self.findings.is_empty() {
            SkillGovernanceStatus::Pass
        } else {
            SkillGovernanceStatus::Warn
        };
    }
}

/// Lifecycle state recorded in the local registry.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SkillLifecycleState {
    /// Installed and eligible for selection.
    Enabled,
    /// Installed but skipped by default.
    Disabled,
    /// Installed for audit/rollback but not selectable by default.
    Quarantined,
}

impl Default for SkillLifecycleState {
    fn default() -> Self {
        Self::Enabled
    }
}

/// Fully parsed skill package with manifest, fragments, tools, and review data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPackage {
    /// Normalized package manifest.
    pub manifest: SkillManifest,
    /// Legacy metadata mirror.
    pub metadata: SkillMetadata,
    /// Structured prompt fragments and lazily loadable assets.
    #[serde(default)]
    pub fragments: SkillFragmentSet,
    /// Executable tool definitions, if any.
    #[serde(default)]
    pub tools: Vec<SkillToolSpec>,
    /// Semantic-review outcome for the package.
    #[serde(default)]
    pub governance: SkillGovernanceOutcome,
}

/// File-backed registry entry for an installed package version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRegistryEntry {
    /// Versioned package identity.
    pub identity: SkillIdentity,
    /// Package manifest stored in the registry index.
    pub manifest: SkillManifest,
    /// Absolute installed package path.
    pub package_path: String,
    /// Deterministic content hash over the installed package.
    pub content_hash: String,
    /// Source locations used during installation.
    #[serde(default)]
    pub installed_from: Vec<String>,
    /// Install timestamp in epoch milliseconds.
    pub installed_at_ms: i64,
    /// Registry lifecycle state.
    #[serde(default)]
    pub lifecycle: SkillLifecycleState,
    /// Optional operator-supplied lifecycle reason, such as quarantine cause.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle_reason: Option<String>,
    /// Governance outcome recorded at install time.
    #[serde(default)]
    pub governance: SkillGovernanceOutcome,
}

/// Registry index persisted under `registry.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillRegistry {
    /// Schema version for the on-disk registry.
    #[serde(default = "default_registry_schema_version")]
    pub version: u32,
    /// Installed skill entries.
    #[serde(default)]
    pub entries: Vec<SkillRegistryEntry>,
}

fn default_registry_schema_version() -> u32 {
    1
}

/// One diagnostic record emitted by loader, validation, or governance flows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillDiagnosticRecord {
    /// Skill name.
    pub name: String,
    /// Skill version.
    #[serde(default = "default_compat_skill_version")]
    pub version: String,
    /// Source label.
    pub source: String,
    /// Severity label.
    pub severity: String,
    /// Stable diagnostic code.
    pub code: String,
    /// Human-readable message.
    pub message: String,
}

/// Source-level loader diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSourceDiagnostics {
    /// Source label or path.
    pub source: String,
    /// Number of successfully loaded packages.
    pub loaded: usize,
    /// Number of skipped entries.
    pub skipped: usize,
    /// Loader or validation errors encountered for the source.
    #[serde(default)]
    pub errors: Vec<String>,
}

/// Override record emitted when a later source replaces an earlier package/tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillOverrideRecord {
    /// Overridden entity name.
    pub name: String,
    /// Overridden version when known.
    #[serde(default = "default_compat_skill_version")]
    pub version: String,
    /// Source label that lost the override race.
    pub overridden_source: String,
    /// Source label that won the override race.
    pub source: String,
}

/// Aggregated diagnostics emitted during loading, selection, and governance.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillsDiagnostics {
    /// Per-source loading diagnostics.
    #[serde(default)]
    pub sources: Vec<SkillSourceDiagnostics>,
    /// Override records for package/tool conflicts.
    #[serde(default)]
    pub overrides: Vec<SkillOverrideRecord>,
    /// Semantic-review warnings and failures.
    #[serde(default)]
    pub records: Vec<SkillDiagnosticRecord>,
}

/// Catalog of loaded packages plus compatibility mirrors.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoadedSkills {
    /// Fully parsed packages.
    #[serde(default)]
    pub packages: Vec<SkillPackage>,
    /// Legacy selected metadata mirror.
    #[serde(default)]
    pub metadata: Vec<SkillMetadata>,
    /// Legacy selected tool mirror.
    #[serde(default)]
    pub tools: Vec<SkillToolSpec>,
    /// Loader/governance diagnostics.
    #[serde(default)]
    pub diagnostics: SkillsDiagnostics,
}

impl LoadedSkills {
    /// Canonicalizes loaded skill data so serialized snapshots and cache keys
    /// stay stable across runs.
    pub fn canonicalize(&mut self) {
        self.packages.sort_by(|a, b| {
            a.manifest
                .identity
                .name
                .cmp(&b.manifest.identity.name)
                .then_with(|| a.manifest.identity.version.cmp(&b.manifest.identity.version))
                .then_with(|| a.manifest.source.cmp(&b.manifest.source))
                .then_with(|| a.manifest.path.cmp(&b.manifest.path))
        });
        self.metadata.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.version.cmp(&b.version))
                .then_with(|| a.source.cmp(&b.source))
                .then_with(|| a.path.cmp(&b.path))
        });
        self.tools.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.skill_name.cmp(&b.skill_name))
                .then_with(|| a.skill_version.cmp(&b.skill_version))
                .then_with(|| a.source.cmp(&b.source))
        });
        self.diagnostics.sources.sort_by(|a, b| a.source.cmp(&b.source));
        self.diagnostics.overrides.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.version.cmp(&b.version))
                .then_with(|| a.overridden_source.cmp(&b.overridden_source))
                .then_with(|| a.source.cmp(&b.source))
        });
        self.diagnostics.records.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.version.cmp(&b.version))
                .then_with(|| a.severity.cmp(&b.severity))
                .then_with(|| a.code.cmp(&b.code))
                .then_with(|| a.source.cmp(&b.source))
        });
    }
}

/// Candidate scoring detail emitted by deterministic selection.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillCandidateRecord {
    /// Candidate identity.
    pub identity: SkillIdentity,
    /// Candidate description.
    pub description: String,
    /// Source label.
    pub source: String,
    /// Deterministic numeric score.
    pub score: i64,
    /// Human-readable score reasons.
    #[serde(default)]
    pub reasons: Vec<String>,
    /// Governance outcome observed while ranking.
    #[serde(default)]
    pub governance: SkillGovernanceOutcome,
}

/// Execution modes supported by the released skill system.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillExecutionMode {
    /// Inline execution as a macro-tool over ordinary agent tools.
    #[default]
    Inline,
    /// Isolated execution via the `task` runtime path.
    Subagent,
}

/// Selected-skill record persisted in the resolved snapshot and run trace.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillSelectedRecord {
    /// Selected identity.
    pub identity: SkillIdentity,
    /// Description surfaced in trace and CLI output.
    pub description: String,
    /// Source label.
    pub source: String,
    /// Reasons the selector chose this skill.
    #[serde(default)]
    pub reasons: Vec<String>,
    /// Selected fragment identifiers.
    #[serde(default)]
    pub fragments: Vec<String>,
    /// Provider-visible tool names exposed for the selected skill.
    #[serde(default)]
    pub tool_names: Vec<String>,
    /// Declared execution mode for the selected skill.
    pub execution_mode: SkillExecutionMode,
    /// Governance outcome for the selected package.
    #[serde(default)]
    pub governance: SkillGovernanceOutcome,
    /// Optional content hash from the registry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

/// Skipped-skill record persisted in trace and diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillSkippedRecord {
    /// Skipped identity.
    pub identity: SkillIdentity,
    /// Source label.
    pub source: String,
    /// Stable skip reason.
    pub reason: String,
    /// Governance outcome for the skipped package.
    #[serde(default)]
    pub governance: SkillGovernanceOutcome,
}

/// Deterministic selection report used by `skill resolve` and run trace.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillSelectionReport {
    /// Ranked candidates considered for the run.
    #[serde(default)]
    pub candidates: Vec<SkillCandidateRecord>,
    /// Final selected skill set.
    #[serde(default)]
    pub selected: Vec<SkillSelectedRecord>,
    /// Skills explicitly skipped with a stable reason.
    #[serde(default)]
    pub skipped: Vec<SkillSkippedRecord>,
}

/// Resolved per-thread/per-run snapshot that drives provider-visible assembly.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResolvedSkillSnapshot {
    /// Stable hash ID over the effective selection and provider-visible output.
    pub snapshot_id: String,
    /// Selected metadata mirror used by legacy seams.
    #[serde(default)]
    pub metadata: Vec<SkillMetadata>,
    /// Selected provider-visible tool specs.
    #[serde(default)]
    pub tools: Vec<SkillToolSpec>,
    /// Selected packages retained for execution and trace.
    #[serde(default)]
    pub packages: Vec<SkillPackage>,
    /// Deterministic selection report.
    #[serde(default)]
    pub selection: SkillSelectionReport,
    /// Provider-visible injected block assembled from selected fragments.
    pub injection_block: String,
}

impl ResolvedSkillSnapshot {
    /// Canonicalizes all snapshot collections and recomputes the stable ID.
    pub fn canonicalize(&mut self) {
        self.metadata.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.version.cmp(&b.version))
                .then_with(|| a.source.cmp(&b.source))
        });
        self.tools.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.skill_name.cmp(&b.skill_name))
                .then_with(|| a.skill_version.cmp(&b.skill_version))
        });
        self.packages.sort_by(|a, b| {
            a.manifest
                .identity
                .name
                .cmp(&b.manifest.identity.name)
                .then_with(|| a.manifest.identity.version.cmp(&b.manifest.identity.version))
                .then_with(|| a.manifest.source.cmp(&b.manifest.source))
        });
        self.selection.candidates.sort_by(|a, b| {
            a.identity
                .name
                .cmp(&b.identity.name)
                .then_with(|| a.identity.version.cmp(&b.identity.version))
                .then_with(|| b.score.cmp(&a.score))
        });
        self.selection.selected.sort_by(|a, b| {
            a.identity
                .name
                .cmp(&b.identity.name)
                .then_with(|| a.identity.version.cmp(&b.identity.version))
                .then_with(|| a.source.cmp(&b.source))
        });
        self.selection.skipped.sort_by(|a, b| {
            a.identity
                .name
                .cmp(&b.identity.name)
                .then_with(|| a.identity.version.cmp(&b.identity.version))
                .then_with(|| a.reason.cmp(&b.reason))
        });
        self.snapshot_id = compute_snapshot_id(self);
    }

    /// Returns the selected skill names for existing event surfaces.
    pub fn selected_skill_names(&self) -> Vec<String> {
        self.selection
            .selected
            .iter()
            .map(|record| record.identity.name.clone())
            .collect()
    }

    /// Returns the selected tool names for quick membership checks.
    pub fn selected_tool_names(&self) -> BTreeSet<String> {
        self.tools.iter().map(|tool| tool.name.clone()).collect()
    }
}

/// Stores the resolved snapshot and compatibility mirrors in the agent state.
pub fn store_resolved_snapshot(state: &mut AgentState, snapshot: &ResolvedSkillSnapshot) -> Result<()> {
    let mut snapshot = snapshot.clone();
    snapshot.canonicalize();
    state.extra.insert(
        SKILLS_SNAPSHOT_KEY.to_string(),
        serde_json::to_value(&snapshot)?,
    );
    state.extra.insert(
        SKILLS_SELECTION_KEY.to_string(),
        serde_json::to_value(&snapshot.selection)?,
    );
    state.extra.insert(
        SKILLS_METADATA_KEY.to_string(),
        serde_json::to_value(&snapshot.metadata)?,
    );
    state.extra.insert(
        SKILLS_TOOLS_KEY.to_string(),
        serde_json::to_value(&snapshot.tools)?,
    );
    Ok(())
}

/// Restores the resolved snapshot from persisted runtime state.
pub fn restore_resolved_snapshot(state: &AgentState) -> Option<ResolvedSkillSnapshot> {
    let value = state.extra.get(SKILLS_SNAPSHOT_KEY)?;
    serde_json::from_value(value.clone()).ok()
}

/// Stores aggregated diagnostics in both the new and legacy state keys.
pub fn store_skills_diagnostics(state: &mut AgentState, diagnostics: &SkillsDiagnostics) -> Result<()> {
    let value = serde_json::to_value(diagnostics)?;
    state
        .extra
        .insert(SKILLS_DIAGNOSTICS_KEY.to_string(), value.clone());
    state
        .extra
        .insert("skills_diagnostics".to_string(), value);
    Ok(())
}

/// Restores stored diagnostics regardless of whether the new or legacy key is present.
pub fn restore_skills_diagnostics(state: &AgentState) -> Option<SkillsDiagnostics> {
    state
        .extra
        .get(SKILLS_DIAGNOSTICS_KEY)
        .or_else(|| state.extra.get("skills_diagnostics"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

/// Returns the selected-skill names currently stored in state.
pub fn selected_skill_names_from_state(state: &AgentState) -> Vec<String> {
    restore_resolved_snapshot(state)
        .map(|snapshot| snapshot.selected_skill_names())
        .or_else(|| {
            state.extra.get(SKILLS_METADATA_KEY).and_then(|value| {
                serde_json::from_value::<Vec<SkillMetadata>>(value.clone())
                    .ok()
                    .map(|skills| skills.into_iter().map(|skill| skill.name).collect())
            })
        })
        .unwrap_or_default()
}

/// Returns whether a tool name belongs to the selected-skill tool surface.
pub fn is_selected_skill_tool(state: &AgentState, tool_name: &str) -> bool {
    if let Some(snapshot) = restore_resolved_snapshot(state) {
        return snapshot.selected_tool_names().contains(tool_name);
    }
    state
        .extra
        .get(SKILLS_TOOLS_KEY)
        .and_then(|value| serde_json::from_value::<Vec<SkillToolSpec>>(value.clone()).ok())
        .map(|tools| tools.into_iter().any(|tool| tool.name == tool_name))
        .unwrap_or(false)
}

/// Attaches the skill selection trace to an existing run trace object.
pub fn attach_skills_trace_to_trace(trace: Option<Value>, state: &AgentState) -> Option<Value> {
    let Some(snapshot) = restore_resolved_snapshot(state) else {
        return trace;
    };
    let mut trace = trace.unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    let Some(map) = trace.as_object_mut() else {
        return Some(trace);
    };
    map.insert(
        "skills".to_string(),
        serde_json::json!({
            "snapshot_id": snapshot.snapshot_id,
            "candidates": snapshot.selection.candidates,
            "selected": snapshot.selection.selected,
            "skipped": snapshot.selection.skipped,
        }),
    );
    Some(trace)
}

/// Builds a stable hash for a resolved skill snapshot.
pub fn compute_snapshot_id(snapshot: &ResolvedSkillSnapshot) -> String {
    let payload = serde_json::json!({
        "metadata": snapshot.metadata,
        "tools": snapshot.tools,
        "selection": snapshot.selection,
        "injection_block": snapshot.injection_block,
    });
    let bytes = serde_json::to_vec(&payload).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}
