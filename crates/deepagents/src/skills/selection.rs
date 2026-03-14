//! Deterministic skill selection, overlay resolution, and prompt assembly.

use std::collections::BTreeMap;

use anyhow::Result;

use crate::skills::loader::{load_skill_packages_from_sources, SkillsLoadOptions};
use crate::skills::registry::{load_registry_packages, parse_identity_token};
use crate::skills::{
    compute_snapshot_id, restore_resolved_snapshot, LoadedSkills, ResolvedSkillSnapshot,
    SkillCandidateRecord, SkillExecutionMode, SkillIdentity, SkillLifecycleState, SkillPackage,
    SkillSelectedRecord, SkillSelectionReport, SkillSkippedRecord,
};
use crate::state::AgentState;
use crate::types::Message;

/// Selection mode requested by the CLI/runtime.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillSelectionMode {
    /// Fully disable skill selection and exposure.
    Off,
    /// Select only explicitly pinned skills.
    Manual,
    /// Use deterministic routing.
    #[default]
    Auto,
}

/// Runtime options that control snapshot resolution and selection behavior.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillResolverOptions {
    /// Optional file-backed registry root.
    pub registry_dir: Option<String>,
    /// Direct source overlays applied only for the current invocation.
    pub sources: Vec<String>,
    /// Source loader options.
    pub source_options: SkillsLoadOptions,
    /// Explicit pinned skills supplied by the caller.
    pub explicit_skills: Vec<String>,
    /// Explicitly disabled skills supplied by the caller.
    pub disabled_skills: Vec<String>,
    /// Selection mode for the run.
    pub selection_mode: SkillSelectionMode,
    /// Maximum active skills allowed for one run.
    pub max_active: usize,
    /// Whether the sticky snapshot should be ignored and recomputed.
    pub refresh_snapshot: bool,
}

impl Default for SkillResolverOptions {
    fn default() -> Self {
        Self {
            registry_dir: None,
            sources: Vec::new(),
            source_options: SkillsLoadOptions::default(),
            explicit_skills: Vec::new(),
            disabled_skills: Vec::new(),
            selection_mode: SkillSelectionMode::Auto,
            max_active: 3,
            refresh_snapshot: false,
        }
    }
}

#[derive(Debug, Clone)]
struct CatalogEntry {
    package: SkillPackage,
    lifecycle: SkillLifecycleState,
    content_hash: Option<String>,
}

/// Resolves the active skill snapshot for a run from registry content,
/// invocation overlays, and the persisted thread state.
pub fn resolve_skill_snapshot(
    messages: &[Message],
    state: &AgentState,
    options: &SkillResolverOptions,
) -> Result<Option<ResolvedSkillSnapshot>> {
    if options.selection_mode == SkillSelectionMode::Off {
        return Ok(None);
    }
    if !options.refresh_snapshot
        && options.sources.is_empty()
        && options.explicit_skills.is_empty()
        && options.disabled_skills.is_empty()
        && options.registry_dir.is_some()
    {
        if let Some(snapshot) = restore_resolved_snapshot(state) {
            return Ok(Some(snapshot));
        }
    }

    let catalog = build_catalog(options)?;
    if catalog.is_empty() {
        return Ok(None);
    }

    let input = user_request_text(messages);
    let input_lower = input.to_ascii_lowercase();
    let pins = options
        .explicit_skills
        .iter()
        .map(|token| parse_identity_token(token))
        .collect::<Result<Vec<_>>>()?;
    let disables = options
        .disabled_skills
        .iter()
        .map(|token| parse_identity_token(token))
        .collect::<Result<Vec<_>>>()?;

    let sticky_names = if options.refresh_snapshot {
        Vec::new()
    } else {
        restore_resolved_snapshot(state)
            .map(|snapshot| {
                snapshot
                    .selection
                    .selected
                    .into_iter()
                    .map(|record| record.identity.as_key())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };

    let mut report = SkillSelectionReport::default();
    let mut selected_entries = Vec::new();

    for entry in catalog.values() {
        if matches_disable(&entry.package.manifest.identity, &disables) {
            report.skipped.push(SkillSkippedRecord {
                identity: entry.package.manifest.identity.clone(),
                source: entry.package.manifest.source.clone(),
                reason: "disabled_by_request".to_string(),
                governance: entry.package.governance.clone(),
            });
            continue;
        }
        if entry.lifecycle == SkillLifecycleState::Quarantined {
            report.skipped.push(SkillSkippedRecord {
                identity: entry.package.manifest.identity.clone(),
                source: entry.package.manifest.source.clone(),
                reason: "quarantined".to_string(),
                governance: entry.package.governance.clone(),
            });
            continue;
        }
        if entry.lifecycle == SkillLifecycleState::Disabled && !matches_pin(&entry.package.manifest.identity, &pins) {
            report.skipped.push(SkillSkippedRecord {
                identity: entry.package.manifest.identity.clone(),
                source: entry.package.manifest.source.clone(),
                reason: "disabled".to_string(),
                governance: entry.package.governance.clone(),
            });
            continue;
        }

        let (score, reasons) = candidate_score(entry, &pins, &sticky_names, &input_lower);
        report.candidates.push(SkillCandidateRecord {
            identity: entry.package.manifest.identity.clone(),
            description: entry.package.manifest.description.clone(),
            source: entry.package.manifest.source.clone(),
            score,
            reasons: reasons.clone(),
            governance: entry.package.governance.clone(),
        });

        match options.selection_mode {
            SkillSelectionMode::Manual => {
                if matches_pin(&entry.package.manifest.identity, &pins) {
                    selected_entries.push((entry.clone(), reasons));
                } else {
                    report.skipped.push(SkillSkippedRecord {
                        identity: entry.package.manifest.identity.clone(),
                        source: entry.package.manifest.source.clone(),
                        reason: "manual_not_pinned".to_string(),
                        governance: entry.package.governance.clone(),
                    });
                }
            }
            SkillSelectionMode::Auto => {
                if options.registry_dir.is_none() && !options.sources.is_empty() && pins.is_empty() {
                    selected_entries.push((entry.clone(), vec!["source_compat_default".to_string()]));
                } else if score > 0 {
                    selected_entries.push((entry.clone(), reasons));
                } else {
                    report.skipped.push(SkillSkippedRecord {
                        identity: entry.package.manifest.identity.clone(),
                        source: entry.package.manifest.source.clone(),
                        reason: "score_below_threshold".to_string(),
                        governance: entry.package.governance.clone(),
                    });
                }
            }
            SkillSelectionMode::Off => {}
        }
    }

    selected_entries.sort_by(|(left, _), (right, _)| {
        let left_rank = candidate_rank(left, &pins, &sticky_names, &input_lower);
        let right_rank = candidate_rank(right, &pins, &sticky_names, &input_lower);
        right_rank
            .explicit_pin
            .cmp(&left_rank.explicit_pin)
            .then_with(|| right_rank.score.cmp(&left_rank.score))
            .then_with(|| left_rank.risk.cmp(&right_rank.risk))
            .then_with(|| {
                left.package
                    .manifest
                    .identity
                    .name
                    .cmp(&right.package.manifest.identity.name)
            })
            .then_with(|| {
                left.package
                    .manifest
                    .identity
                    .version
                    .cmp(&right.package.manifest.identity.version)
            })
    });
    selected_entries.truncate(options.max_active.max(1));

    if selected_entries.is_empty() {
        let mut snapshot = ResolvedSkillSnapshot::default();
        snapshot.injection_block = "DEEPAGENTS_SKILLS_INJECTED_V2\n## Skills\n".to_string();
        snapshot.selection = report;
        snapshot.canonicalize();
        return Ok(Some(snapshot));
    }

    let mut snapshot = ResolvedSkillSnapshot::default();
    for (entry, reasons) in selected_entries {
        let fragments = choose_fragments(&entry.package, &input_lower);
        let execution_mode = if entry.package.manifest.requires_isolation
            || entry.package.manifest.risk_level >= crate::skills::SkillRiskLevel::High
        {
            SkillExecutionMode::Subagent
        } else {
            SkillExecutionMode::Inline
        };
        snapshot.metadata.push(entry.package.metadata.clone());
        snapshot.tools.extend(
            entry.package
                .tools
                .iter()
                .cloned()
                .map(|mut tool| {
                    tool.requires_isolation = execution_mode == SkillExecutionMode::Subagent;
                    tool
                }),
        );
        snapshot.packages.push(entry.package.clone());
        report.selected.push(SkillSelectedRecord {
            identity: entry.package.manifest.identity.clone(),
            description: entry.package.manifest.description.clone(),
            source: entry.package.manifest.source.clone(),
            reasons,
            fragments,
            tool_names: entry.package.tools.iter().map(|tool| tool.name.clone()).collect(),
            execution_mode,
            governance: entry.package.governance.clone(),
            content_hash: entry.content_hash.clone(),
        });
    }
    snapshot.selection = report;
    snapshot.injection_block = build_injection_block(&snapshot);
    snapshot.snapshot_id = compute_snapshot_id(&snapshot);
    snapshot.canonicalize();
    Ok(Some(snapshot))
}

/// Exposes a legacy loaded-skills view of the selected snapshot.
pub fn selected_loaded_skills(snapshot: &ResolvedSkillSnapshot) -> LoadedSkills {
    let mut loaded = LoadedSkills {
        packages: snapshot.packages.clone(),
        metadata: snapshot.metadata.clone(),
        tools: snapshot.tools.clone(),
        diagnostics: Default::default(),
    };
    loaded.canonicalize();
    loaded
}

fn build_catalog(options: &SkillResolverOptions) -> Result<BTreeMap<String, CatalogEntry>> {
    let mut out = BTreeMap::new();

    if let Some(registry_dir) = &options.registry_dir {
        for (entry, package) in load_registry_packages(std::path::Path::new(registry_dir))? {
            out.insert(
                package.manifest.identity.as_key(),
                CatalogEntry {
                    package,
                    lifecycle: entry.lifecycle,
                    content_hash: Some(entry.content_hash),
                },
            );
        }
    }

    if !options.sources.is_empty() {
        for package in load_skill_packages_from_sources(&options.sources, options.source_options.clone())? {
            out.insert(
                package.manifest.identity.as_key(),
                CatalogEntry {
                    package,
                    lifecycle: SkillLifecycleState::Enabled,
                    content_hash: None,
                },
            );
        }
    }

    Ok(out)
}

fn candidate_score(
    entry: &CatalogEntry,
    pins: &[(String, Option<String>)],
    sticky_names: &[String],
    input_lower: &str,
) -> (i64, Vec<String>) {
    let mut score = 0_i64;
    let mut reasons = Vec::new();
    if matches_pin(&entry.package.manifest.identity, pins) {
        score += 10_000;
        reasons.push("explicit_pin".to_string());
    }
    if sticky_names
        .iter()
        .any(|value| value == &entry.package.manifest.identity.as_key())
    {
        score += 500;
        reasons.push("snapshot_affinity".to_string());
    }
    for keyword in &entry.package.manifest.triggers.keywords {
        let keyword_lower = keyword.to_ascii_lowercase();
        if !keyword_lower.is_empty() && input_lower.contains(&keyword_lower) {
            score += 100;
            reasons.push(format!("keyword:{keyword_lower}"));
        }
    }
    if let Some(contract) = &entry.package.manifest.output_contract {
        let contract_lower = contract.to_ascii_lowercase();
        if input_lower.contains(&contract_lower) {
            score += 50;
            reasons.push(format!("output_contract:{contract_lower}"));
        }
    }
    for name in &entry.package.manifest.allowed_tools {
        let name_lower = name.to_ascii_lowercase();
        if !name_lower.is_empty() && input_lower.contains(&name_lower) {
            score += 10;
            reasons.push(format!("tool_scope:{name_lower}"));
        }
    }
    let desc = entry.package.manifest.description.to_ascii_lowercase();
    for token in input_lower.split_whitespace() {
        if token.len() < 4 {
            continue;
        }
        if desc.contains(token) {
            score += 5;
            reasons.push(format!("description:{token}"));
        }
    }
    (score, dedup_reasons(reasons))
}

struct CandidateRank {
    explicit_pin: bool,
    score: i64,
    risk: crate::skills::SkillRiskLevel,
}

fn candidate_rank(
    entry: &CatalogEntry,
    pins: &[(String, Option<String>)],
    sticky_names: &[String],
    input_lower: &str,
) -> CandidateRank {
    let (score, _) = candidate_score(entry, pins, sticky_names, input_lower);
    CandidateRank {
        explicit_pin: matches_pin(&entry.package.manifest.identity, pins),
        score,
        risk: entry.package.manifest.risk_level,
    }
}

fn choose_fragments(package: &SkillPackage, input_lower: &str) -> Vec<String> {
    let mut out = Vec::new();
    for name in ["role", "constraints", "workflow", "output"] {
        if package.fragments.get(name).is_some() {
            out.push(name.to_string());
        }
    }
    for (keyword, fragment) in [
        ("when", "when_to_use"),
        ("input", "inputs"),
        ("example", "examples"),
        ("reference", "references"),
        ("template", "references"),
    ] {
        if input_lower.contains(keyword)
            && package.fragments.get(fragment).is_some()
            && !out.iter().any(|value| value == fragment)
        {
            out.push(fragment.to_string());
        }
    }
    if out.is_empty() {
        out.extend(package.fragments.available_names().into_iter().take(3));
    }
    out
}

fn build_injection_block(snapshot: &ResolvedSkillSnapshot) -> String {
    let mut out = String::new();
    out.push_str("DEEPAGENTS_SKILLS_INJECTED_V2\n");
    out.push_str("## Skills\n");
    for selected in &snapshot.selection.selected {
        out.push_str("- ");
        out.push_str(&selected.identity.as_key());
        out.push_str(": ");
        out.push_str(&selected.description);
        out.push_str(" (source: ");
        out.push_str(&selected.source);
        out.push_str(")\n");
        if !selected.fragments.is_empty() {
            out.push_str("  Selected fragments: ");
            out.push_str(&selected.fragments.join(", "));
            out.push('\n');
        }
    }
    for selected in &snapshot.selection.selected {
        if selected.fragments.is_empty() {
            continue;
        }
        let Some(package) = snapshot
            .packages
            .iter()
            .find(|package| package.manifest.identity == selected.identity)
        else {
            continue;
        };
        out.push_str("### ");
        out.push_str(&selected.identity.as_key());
        out.push('\n');
        for fragment_name in &selected.fragments {
            if let Some(content) = package.fragments.get(fragment_name) {
                out.push_str("#### ");
                out.push_str(fragment_name);
                out.push('\n');
                out.push_str(content.trim());
                out.push('\n');
            }
        }
    }
    out
}

fn matches_pin(identity: &SkillIdentity, pins: &[(String, Option<String>)]) -> bool {
    pins.iter().any(|(name, version)| {
        identity.name == *name && version.as_deref().is_none_or(|value| value == identity.version)
    })
}

fn matches_disable(identity: &SkillIdentity, disables: &[(String, Option<String>)]) -> bool {
    disables.iter().any(|(name, version)| {
        identity.name == *name && version.as_deref().is_none_or(|value| value == identity.version)
    })
}

fn user_request_text(messages: &[Message]) -> String {
    messages
        .iter()
        .filter(|message| message.role == "user")
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn dedup_reasons(reasons: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for reason in reasons {
        if seen.insert(reason.clone()) {
            out.push(reason);
        }
    }
    out
}
