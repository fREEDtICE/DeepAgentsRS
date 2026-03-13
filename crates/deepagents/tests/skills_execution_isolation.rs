//! Execution-isolation coverage for package skill tools.
//!
//! These tests lock down the non-fatal failure policy introduced for package
//! skills so validation, permission, timeout, panic, truncation, and HITL
//! behavior stay stable as the runner evolves.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use deepagents::approval::ExecutionMode;
use deepagents::provider::mock::{MockProvider, MockScript, MockStep};
use deepagents::provider::protocol::{AgentProvider, AgentToolCall};
use deepagents::runtime::simple::{SimpleRuntime, SimpleRuntimeOptions};
use deepagents::runtime::skills_middleware::SkillsMiddleware;
use deepagents::runtime::{
    ResumableRunner, ResumableRunnerOptions, RunStatus, Runtime, RuntimeConfig, RuntimeMiddleware,
};
use deepagents::skills::loader::{load_skills, SkillsLoadOptions};
use deepagents::tools::{default_tools, Tool, ToolResult};
use deepagents::types::Message;
use deepagents::DeepAgent;

/// Writes a minimal package skill fixture with a `SKILL.md` and `tools.json`.
fn write_skill(dir: &std::path::Path, name: &str, desc: &str, tools_json: serde_json::Value) {
    std::fs::create_dir_all(dir.join(name)).unwrap();
    std::fs::write(
        dir.join(name).join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {desc}\n---\n\n# {name}\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join(name).join("tools.json"),
        serde_json::to_vec_pretty(&tools_json).unwrap(),
    )
    .unwrap();
}

/// Builds the runtime middleware stack used to expose package skills.
fn skill_middleware(source: &std::path::Path) -> Arc<dyn RuntimeMiddleware> {
    Arc::new(SkillsMiddleware::new(
        vec![source.to_string_lossy().to_string()],
        SkillsLoadOptions::default(),
    ))
}

/// Creates a simple runtime with the default tool set plus optional test-only
/// tools and runtime middleware.
fn runtime_with_script_and_tools(
    root: &std::path::Path,
    script: MockScript,
    extra_tools: Vec<Arc<dyn Tool>>,
    middlewares: Vec<Arc<dyn RuntimeMiddleware>>,
) -> SimpleRuntime {
    let backend = deepagents::create_local_sandbox_backend(root, None).unwrap();
    let mut tools = default_tools(backend.clone());
    tools.extend(extra_tools);
    let agent = DeepAgent::with_backend_and_tools(backend, tools);
    let provider: Arc<dyn AgentProvider> = Arc::new(MockProvider::from_script(script));
    SimpleRuntime::new(
        agent,
        provider,
        SimpleRuntimeOptions {
            config: RuntimeConfig {
                max_steps: 4,
                provider_timeout_ms: 1000,
            },
            approval: None,
            audit: None,
            root: root.to_string_lossy().to_string(),
            mode: ExecutionMode::NonInteractive,
        },
    )
    .with_runtime_middlewares(middlewares)
}

/// Creates a resumable runner so HITL behavior can be exercised against
/// middleware-backed package skills.
fn runner_with_script_and_tools(
    root: &std::path::Path,
    script: MockScript,
    extra_tools: Vec<Arc<dyn Tool>>,
    middlewares: Vec<Arc<dyn RuntimeMiddleware>>,
    interrupt_on: BTreeMap<String, bool>,
) -> ResumableRunner {
    let backend = deepagents::create_local_sandbox_backend(root, None).unwrap();
    let mut tools = default_tools(backend.clone());
    tools.extend(extra_tools);
    let agent = DeepAgent::with_backend_and_tools(backend, tools);
    let provider: Arc<dyn AgentProvider> = Arc::new(MockProvider::from_script(script));
    ResumableRunner::new(
        agent,
        provider,
        ResumableRunnerOptions {
            config: RuntimeConfig {
                max_steps: 4,
                provider_timeout_ms: 1000,
            },
            approval: None,
            audit: None,
            root: root.to_string_lossy().to_string(),
            mode: ExecutionMode::NonInteractive,
            interrupt_on,
        },
    )
    .with_runtime_middlewares(middlewares)
}

/// Provides the minimal user message needed to advance the mock provider.
fn user_message() -> Vec<Message> {
    vec![Message {
        role: "user".to_string(),
        content: "go".to_string(),
        content_blocks: None,
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        status: None,
    }]
}

/// Extracts the first recorded tool error for a named tool result.
fn first_tool_error(out: &deepagents::runtime::RunOutput, tool_name: &str) -> String {
    out.tool_results
        .iter()
        .find(|record| record.tool_name == tool_name)
        .and_then(|record| record.error.clone())
        .expect("tool error")
}

/// Sleeps for a configurable duration so skill timeout handling can be tested.
struct SleepTool;

#[async_trait]
impl Tool for SleepTool {
    /// Returns the tool name exposed to package skill steps.
    fn name(&self) -> &'static str {
        "sleep_ms"
    }

    /// Describes the synthetic sleep tool for debugging output.
    fn description(&self) -> &'static str {
        "Sleeps for a number of milliseconds."
    }

    /// Restricts the tool input to a single integer duration field.
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "ms": { "type": "integer" }
            },
            "required": ["ms"],
            "additionalProperties": false
        })
    }

    /// Sleeps for the requested duration and emits a short textual result.
    async fn call(&self, input: serde_json::Value) -> anyhow::Result<ToolResult> {
        let ms = input
            .get("ms")
            .and_then(|value| value.as_u64())
            .unwrap_or_default();
        tokio::time::sleep(Duration::from_millis(ms)).await;
        Ok(ToolResult {
            output: serde_json::json!({ "content": format!("slept {ms}") }),
            content_blocks: None,
        })
    }
}

/// Emits caller-supplied text so truncation behavior can be tested precisely.
struct EmitTextTool;

#[async_trait]
impl Tool for EmitTextTool {
    /// Returns the tool name exposed to package skill steps.
    fn name(&self) -> &'static str {
        "emit_text"
    }

    /// Describes the synthetic emit tool for debugging output.
    fn description(&self) -> &'static str {
        "Emits the provided text."
    }

    /// Restricts the tool input to a single string field.
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" }
            },
            "required": ["text"],
            "additionalProperties": false
        })
    }

    /// Returns the provided text unchanged in the tool output payload.
    async fn call(&self, input: serde_json::Value) -> anyhow::Result<ToolResult> {
        Ok(ToolResult {
            output: serde_json::json!({ "content": input["text"].as_str().unwrap_or_default() }),
            content_blocks: None,
        })
    }
}

/// Panics immediately so skill panic isolation can be exercised.
struct PanicTool;

#[async_trait]
impl Tool for PanicTool {
    /// Returns the tool name exposed to package skill steps.
    fn name(&self) -> &'static str {
        "panic_tool"
    }

    /// Describes the synthetic panic tool for debugging output.
    fn description(&self) -> &'static str {
        "Panics immediately."
    }

    /// Exposes an empty-object schema because the tool takes no arguments.
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    /// Panics to confirm the middleware converts unwinds into tool errors.
    async fn call(&self, _input: serde_json::Value) -> anyhow::Result<ToolResult> {
        panic!("panic_tool exploded");
    }
}

/// Confirms package skills reject runtime-only tools at load time.
#[test]
fn load_skills_rejects_runtime_only_step_tools() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("skills");
    write_skill(
        &source,
        "bad-skill",
        "Bad",
        serde_json::json!({
            "tools": [{
                "name": "bad-tool",
                "description": "Bad",
                "input_schema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                },
                "steps": [{ "tool_name": "task", "arguments": {} }],
                "policy": {}
            }]
        }),
    );

    let err = load_skills(
        &[source.to_string_lossy().to_string()],
        SkillsLoadOptions::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("skill_step_not_supported"));
    assert!(err.to_string().contains("task"));
}

/// Confirms schema validation rejects unexpected fields without aborting the
/// overall run.
#[tokio::test]
async fn skill_input_schema_rejects_unexpected_fields_without_failing_runner() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = root.join("skills");
    write_skill(
        &source,
        "typed-skill",
        "Typed",
        serde_json::json!({
            "tools": [{
                "name": "typed-skill",
                "description": "Typed",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "count": { "type": "integer" }
                    },
                    "required": ["count"],
                    "additionalProperties": false
                },
                "steps": [],
                "policy": {}
            }]
        }),
    );

    let runtime = runtime_with_script_and_tools(
        root,
        MockScript {
            steps: vec![
                MockStep::ToolCalls {
                    calls: vec![AgentToolCall {
                        tool_name: "typed-skill".to_string(),
                        arguments: serde_json::json!({ "count": 1, "extra": true }),
                        call_id: Some("typed-1".to_string()),
                    }],
                },
                MockStep::FinalText {
                    text: "done".to_string(),
                },
            ],
        },
        Vec::new(),
        vec![skill_middleware(&source)],
    );

    let out = runtime.run(user_message()).await;
    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(out.final_text, "done");
    assert!(first_tool_error(&out, "typed-skill").contains("unexpected field: extra"));
}

/// Confirms filesystem-denied skill steps fail explicitly and leave no side
/// effects behind.
#[tokio::test]
async fn skill_filesystem_permission_denied_has_no_side_effect() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = root.join("skills");
    write_skill(
        &source,
        "fs-skill",
        "Filesystem",
        serde_json::json!({
            "tools": [{
                "name": "fs-skill",
                "description": "Filesystem",
                "input_schema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                },
                "steps": [{
                    "tool_name": "write_file",
                    "arguments": { "file_path": "skill.txt", "content": "hi\n" }
                }],
                "policy": {
                    "allow_filesystem": false
                }
            }]
        }),
    );

    let runtime = runtime_with_script_and_tools(
        root,
        MockScript {
            steps: vec![
                MockStep::ToolCalls {
                    calls: vec![AgentToolCall {
                        tool_name: "fs-skill".to_string(),
                        arguments: serde_json::json!({}),
                        call_id: Some("fs-1".to_string()),
                    }],
                },
                MockStep::FinalText {
                    text: "done".to_string(),
                },
            ],
        },
        Vec::new(),
        vec![skill_middleware(&source)],
    );

    let out = runtime.run(user_message()).await;
    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(out.final_text, "done");
    assert!(first_tool_error(&out, "fs-skill").contains("allow_filesystem=true"));
    assert!(!root.join("skill.txt").exists());
}

/// Confirms execute-denied skill steps fail explicitly and leave no side
/// effects behind.
#[tokio::test]
async fn skill_execute_permission_denied_has_no_side_effect() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = root.join("skills");
    write_skill(
        &source,
        "exec-skill",
        "Execute",
        serde_json::json!({
            "tools": [{
                "name": "exec-skill",
                "description": "Execute",
                "input_schema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                },
                "steps": [{
                    "tool_name": "execute",
                    "arguments": { "command": "echo hi > exec-skill.txt" }
                }],
                "policy": {
                    "allow_execute": false
                }
            }]
        }),
    );

    let runtime = runtime_with_script_and_tools(
        root,
        MockScript {
            steps: vec![
                MockStep::ToolCalls {
                    calls: vec![AgentToolCall {
                        tool_name: "exec-skill".to_string(),
                        arguments: serde_json::json!({}),
                        call_id: Some("exec-1".to_string()),
                    }],
                },
                MockStep::FinalText {
                    text: "done".to_string(),
                },
            ],
        },
        Vec::new(),
        vec![skill_middleware(&source)],
    );

    let out = runtime.run(user_message()).await;
    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(out.final_text, "done");
    assert!(first_tool_error(&out, "exec-skill").contains("allow_execute=true"));
    assert!(!root.join("exec-skill.txt").exists());
}

/// Confirms step-level timeouts are surfaced as non-fatal tool errors.
#[tokio::test]
async fn skill_timeout_is_non_fatal() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = root.join("skills");
    write_skill(
        &source,
        "slow-skill",
        "Slow",
        serde_json::json!({
            "tools": [{
                "name": "slow-skill",
                "description": "Slow",
                "input_schema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                },
                "steps": [{
                    "tool_name": "sleep_ms",
                    "arguments": { "ms": 50 }
                }],
                "policy": {
                    "timeout_ms": 10
                }
            }]
        }),
    );

    let runtime = runtime_with_script_and_tools(
        root,
        MockScript {
            steps: vec![
                MockStep::ToolCalls {
                    calls: vec![AgentToolCall {
                        tool_name: "slow-skill".to_string(),
                        arguments: serde_json::json!({}),
                        call_id: Some("slow-1".to_string()),
                    }],
                },
                MockStep::FinalText {
                    text: "done".to_string(),
                },
            ],
        },
        vec![Arc::new(SleepTool)],
        vec![skill_middleware(&source)],
    );

    let out = runtime.run(user_message()).await;
    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(out.final_text, "done");
    assert!(first_tool_error(&out, "slow-skill").contains("skill_timeout: exceeded 10ms"));
}

/// Confirms package skills truncate large output in-place instead of using the
/// generic offload path.
#[tokio::test]
async fn skill_output_is_truncated_by_skill_policy() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = root.join("skills");
    write_skill(
        &source,
        "big-skill",
        "Big",
        serde_json::json!({
            "tools": [{
                "name": "big-skill",
                "description": "Big",
                "input_schema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                },
                "steps": [{
                    "tool_name": "emit_text",
                    "arguments": { "text": "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx" }
                }],
                "policy": {
                    "max_output_chars": 16
                }
            }]
        }),
    );

    let runtime = runtime_with_script_and_tools(
        root,
        MockScript {
            steps: vec![
                MockStep::ToolCalls {
                    calls: vec![AgentToolCall {
                        tool_name: "big-skill".to_string(),
                        arguments: serde_json::json!({}),
                        call_id: Some("big-1".to_string()),
                    }],
                },
                MockStep::FinalText {
                    text: "done".to_string(),
                },
            ],
        },
        vec![Arc::new(EmitTextTool)],
        vec![skill_middleware(&source)],
    );

    let out = runtime.run(user_message()).await;
    assert_eq!(out.status, RunStatus::Completed);
    let output = &out
        .tool_results
        .iter()
        .find(|record| record.tool_name == "big-skill")
        .expect("tool result")
        .output;
    assert_eq!(
        output.get("content").and_then(|value| value.as_str()),
        Some("xxxxxxxxxxxxxxxx")
    );
    assert_eq!(
        output.get("truncated").and_then(|value| value.as_bool()),
        Some(true)
    );
    assert!(output.get("offload_path").is_none());
}

/// Confirms panics inside skill steps are converted into tool errors and do
/// not abort the runner.
#[tokio::test]
async fn skill_panic_becomes_error_tool_result() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = root.join("skills");
    write_skill(
        &source,
        "panic-skill",
        "Panic",
        serde_json::json!({
            "tools": [{
                "name": "panic-skill",
                "description": "Panic",
                "input_schema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                },
                "steps": [{
                    "tool_name": "panic_tool",
                    "arguments": {}
                }],
                "policy": {}
            }]
        }),
    );

    let runtime = runtime_with_script_and_tools(
        root,
        MockScript {
            steps: vec![
                MockStep::ToolCalls {
                    calls: vec![AgentToolCall {
                        tool_name: "panic-skill".to_string(),
                        arguments: serde_json::json!({}),
                        call_id: Some("panic-1".to_string()),
                    }],
                },
                MockStep::FinalText {
                    text: "done".to_string(),
                },
            ],
        },
        vec![Arc::new(PanicTool)],
        vec![skill_middleware(&source)],
    );

    let out = runtime.run(user_message()).await;
    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(out.final_text, "done");
    assert!(first_tool_error(&out, "panic-skill").contains("skill_panic:"));
}

/// Confirms middleware-backed package skills can still be interrupted and
/// resumed through the standard HITL flow.
#[tokio::test]
async fn skill_tool_can_interrupt_via_hitl_before_execution() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = root.join("skills");
    write_skill(
        &source,
        "fs-skill",
        "Filesystem",
        serde_json::json!({
            "tools": [{
                "name": "fs-skill",
                "description": "Filesystem",
                "input_schema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                },
                "steps": [{
                    "tool_name": "write_file",
                    "arguments": { "file_path": "skill.txt", "content": "hi\n" }
                }],
                "policy": {
                    "allow_filesystem": true
                }
            }]
        }),
    );

    let mut interrupt_on = BTreeMap::new();
    interrupt_on.insert("fs-skill".to_string(), true);
    let mut runner = runner_with_script_and_tools(
        root,
        MockScript {
            steps: vec![
                MockStep::ToolCalls {
                    calls: vec![AgentToolCall {
                        tool_name: "fs-skill".to_string(),
                        arguments: serde_json::json!({}),
                        call_id: Some("skill-hitl".to_string()),
                    }],
                },
                MockStep::FinalText {
                    text: "done".to_string(),
                },
            ],
        },
        Vec::new(),
        vec![skill_middleware(&source)],
        interrupt_on,
    );

    runner.push_user_input("go".to_string());
    let out1 = runner.run().await;
    assert_eq!(out1.status, RunStatus::Interrupted);
    assert_eq!(out1.interrupts.len(), 1);
    assert_eq!(out1.interrupts[0].tool_name, "fs-skill");
    assert!(!root.join("skill.txt").exists());

    let out2 = runner
        .resume("skill-hitl", deepagents::runtime::HitlDecision::Approve)
        .await;
    assert_eq!(out2.status, RunStatus::Completed);
    assert_eq!(out2.final_text, "done");
    assert_eq!(
        std::fs::read_to_string(root.join("skill.txt")).unwrap(),
        "hi\n"
    );
}
