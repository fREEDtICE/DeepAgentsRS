//! Black-box CLI coverage for the package-skill authoring lifecycle.
//!
//! These tests lock down the release-facing `skill init`, `skill validate`,
//! and `skill list` workflows so the package-only skills story stays
//! scriptable, deterministic, and CI-friendly.

use std::process::{Command, ExitStatus};

/// Runs the `deepagents` CLI and parses stdout as JSON.
fn run_skill_command(args: &[&str]) -> (ExitStatus, serde_json::Value, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_deepagents"))
        .args(args)
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!("stdout was not valid JSON: {error}\nstdout={stdout}\nstderr={stderr}")
    });
    (output.status, value, stderr)
}

/// Writes a minimal package skill fixture under the given source directory.
fn write_skill(
    source_root: &std::path::Path,
    skill_name: &str,
    description: &str,
    tool_content: &str,
) {
    let skill_dir = source_root.join(skill_name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: {skill_name}\ndescription: {description}\n---\n\n# {skill_name}\n"),
    )
    .unwrap();
    std::fs::write(
        skill_dir.join("tools.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "tools": [{
                "name": skill_name,
                "description": description,
                "input_schema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                },
                "steps": [{
                    "tool_name": "emit_text",
                    "arguments": { "text": tool_content }
                }],
                "policy": {}
            }]
        }))
        .unwrap(),
    )
    .unwrap();
}

/// Confirms `skill init` creates a valid package and both `validate` and `list`
/// expose the package through the release JSON surface.
#[test]
fn phase6_skill_init_validate_and_list_are_scriptable() {
    let temp = tempfile::tempdir().unwrap();
    let source_root = temp.path();
    let skill_dir = source_root.join("sample-skill");

    let (status, init_json, stderr) = run_skill_command(&[
        "skill",
        "init",
        skill_dir.to_string_lossy().as_ref(),
        "--pretty",
    ]);
    assert!(status.success(), "stderr={stderr}");
    assert_eq!(
        init_json.get("ok").and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        init_json
            .get("skill")
            .and_then(|value| value.get("name"))
            .and_then(|value| value.as_str()),
        Some("sample-skill")
    );
    assert!(skill_dir.join("SKILL.md").exists());
    assert!(skill_dir.join("tools.json").exists());

    let (status, validate_json, stderr) = run_skill_command(&[
        "skill",
        "validate",
        "--source",
        source_root.to_string_lossy().as_ref(),
        "--pretty",
    ]);
    assert!(status.success(), "stderr={stderr}");
    assert_eq!(
        validate_json
            .get("summary")
            .and_then(|value| value.get("skills"))
            .and_then(|value| value.as_u64()),
        Some(1)
    );
    assert_eq!(
        validate_json
            .get("summary")
            .and_then(|value| value.get("tools"))
            .and_then(|value| value.as_u64()),
        Some(1)
    );

    let (status, list_json, stderr) = run_skill_command(&[
        "skill",
        "list",
        "--source",
        source_root.to_string_lossy().as_ref(),
        "--pretty",
    ]);
    assert!(status.success(), "stderr={stderr}");
    assert_eq!(
        list_json
            .get("skills")
            .and_then(|value| value.as_array())
            .map(|skills| skills.len()),
        Some(1)
    );
    assert_eq!(
        list_json
            .get("tools")
            .and_then(|value| value.as_array())
            .map(|tools| tools.len()),
        Some(1)
    );
}

/// Confirms `skill list` reports deterministic last-one-wins override
/// diagnostics for duplicate skill names across sources.
#[test]
fn phase6_skill_list_reports_override_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    let source_a = temp.path().join("A");
    let source_b = temp.path().join("B");
    std::fs::create_dir_all(&source_a).unwrap();
    std::fs::create_dir_all(&source_b).unwrap();

    write_skill(&source_a, "web-research", "A implementation", "A_IMPL");
    write_skill(&source_b, "web-research", "B implementation", "B_IMPL");

    let (status, json, stderr) = run_skill_command(&[
        "skill",
        "list",
        "--source",
        source_a.to_string_lossy().as_ref(),
        "--source",
        source_b.to_string_lossy().as_ref(),
        "--pretty",
    ]);
    assert!(status.success(), "stderr={stderr}");
    assert_eq!(
        json.get("skills")
            .and_then(|value| value.as_array())
            .map(|skills| skills.len()),
        Some(1)
    );
    assert_eq!(
        json.get("skills")
            .and_then(|value| value.as_array())
            .and_then(|skills| skills.first())
            .and_then(|value| value.get("description"))
            .and_then(|value| value.as_str()),
        Some("B implementation")
    );
    assert_eq!(
        json.get("diagnostics")
            .and_then(|value| value.get("overrides"))
            .and_then(|value| value.as_array())
            .map(|overrides| overrides.len()),
        Some(1)
    );
}

/// Confirms `skill validate` returns a non-zero exit code and a machine-readable
/// error payload that pinpoints the failing file and field.
#[test]
fn phase6_skill_validate_reports_precise_json_errors() {
    let temp = tempfile::tempdir().unwrap();
    let skill_dir = temp.path().join("bad-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\ndescription: missing name\n---\n\n# bad-skill\n",
    )
    .unwrap();

    let (status, json, stderr) = run_skill_command(&[
        "skill",
        "validate",
        "--source",
        temp.path().to_string_lossy().as_ref(),
        "--pretty",
    ]);
    assert!(!status.success(), "stderr={stderr}");
    assert_eq!(
        json.get("error")
            .and_then(|value| value.get("code"))
            .and_then(|value| value.as_str()),
        Some("skill_validation_failed")
    );
    let message = json
        .get("error")
        .and_then(|value| value.get("message"))
        .and_then(|value| value.as_str())
        .unwrap();
    assert!(message.contains("SKILL.md"));
    assert!(message.contains("name"));
}

/// Confirms invalid sources fail predictably with a classified error payload.
#[test]
fn phase6_skill_validate_classifies_invalid_sources() {
    let missing = tempfile::tempdir().unwrap().path().join("missing-source");
    let (status, json, stderr) = run_skill_command(&[
        "skill",
        "validate",
        "--source",
        missing.to_string_lossy().as_ref(),
        "--pretty",
    ]);
    assert!(!status.success(), "stderr={stderr}");
    assert_eq!(
        json.get("error")
            .and_then(|value| value.get("code"))
            .and_then(|value| value.as_str()),
        Some("invalid_source")
    );
}
