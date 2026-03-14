//! Semantic-review heuristics for skill packages.

use crate::skills::{
    SkillGovernanceFinding, SkillGovernanceOutcome, SkillGovernanceSeverity, SkillPackage,
};

/// Reviews a skill package for semantic safety issues that are not covered by
/// structural validation alone.
pub fn review_skill_package(package: &SkillPackage) -> SkillGovernanceOutcome {
    let mut outcome = SkillGovernanceOutcome::default();
    let text = assemble_package_text(package);

    for (needle, code, message, severity) in [
        (
            "ignore system",
            "policy_override_attempt",
            "skill content attempts to ignore system instructions",
            SkillGovernanceSeverity::Fail,
        ),
        (
            "ignore developer",
            "policy_override_attempt",
            "skill content attempts to ignore developer instructions",
            SkillGovernanceSeverity::Fail,
        ),
        (
            "bypass approval",
            "approval_bypass_attempt",
            "skill content suggests bypassing approval boundaries",
            SkillGovernanceSeverity::Fail,
        ),
        (
            "override policy",
            "policy_override_attempt",
            "skill content suggests overriding platform policy",
            SkillGovernanceSeverity::Fail,
        ),
        (
            "unrestricted access",
            "privilege_expansion_claim",
            "skill content claims unrestricted access",
            SkillGovernanceSeverity::Fail,
        ),
        (
            "full filesystem access",
            "privilege_expansion_claim",
            "skill content claims full filesystem access",
            SkillGovernanceSeverity::Warn,
        ),
        (
            "run any command",
            "privilege_expansion_claim",
            "skill content claims broad command execution authority",
            SkillGovernanceSeverity::Warn,
        ),
    ] {
        if text.contains(needle) {
            outcome.findings.push(SkillGovernanceFinding {
                code: code.to_string(),
                message: message.to_string(),
                severity,
            });
        }
    }

    let declared_allowed = package
        .manifest
        .allowed_tools
        .iter()
        .map(|tool| tool.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let step_tools = package
        .tools
        .iter()
        .flat_map(|tool| tool.steps.iter().map(|step| step.tool_name.as_str()))
        .collect::<std::collections::BTreeSet<_>>();

    if declared_allowed.contains("execute")
        && !package.tools.iter().any(|tool| tool.policy.allow_execute)
    {
        outcome.findings.push(SkillGovernanceFinding {
            code: "advisory_policy_drift".to_string(),
            message: "allowed-tools advertises execute without enforced allow_execute policy"
                .to_string(),
            severity: SkillGovernanceSeverity::Warn,
        });
    }

    if step_tools.contains("execute") && !declared_allowed.contains("execute") {
        outcome.findings.push(SkillGovernanceFinding {
            code: "advisory_policy_drift".to_string(),
            message: "execute is used in tools.json but omitted from allowed-tools".to_string(),
            severity: SkillGovernanceSeverity::Warn,
        });
    }

    if package.tools.is_empty()
        && ["execute", "shell", "command", "write_file", "filesystem"]
            .iter()
            .any(|needle| text.contains(needle))
    {
        outcome.findings.push(SkillGovernanceFinding {
            code: "prompt_only_capability_claim".to_string(),
            message:
                "prompt-only skill claims executable capabilities without a governed tool surface"
                    .to_string(),
            severity: SkillGovernanceSeverity::Warn,
        });
    }

    outcome.canonicalize();
    outcome
}

/// Builds one normalized text blob so the heuristic checks remain stable.
fn assemble_package_text(package: &SkillPackage) -> String {
    let mut parts = vec![package.manifest.description.to_ascii_lowercase()];
    for value in [
        package.fragments.role.as_deref(),
        package.fragments.when_to_use.as_deref(),
        package.fragments.inputs.as_deref(),
        package.fragments.constraints.as_deref(),
        package.fragments.workflow.as_deref(),
        package.fragments.output.as_deref(),
        package.fragments.examples.as_deref(),
        package.fragments.references.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        parts.push(value.to_ascii_lowercase());
    }
    parts.join("\n")
}
