use std::sync::{Arc, Mutex};

use deepagents::approval::ExecutionMode;
use deepagents::provider::protocol::{AgentProvider, AgentProviderRequest, AgentStep};
use deepagents::provider::AgentToolCall;
use deepagents::runtime::simple::SimpleRuntime;
use deepagents::runtime::skills_middleware::SkillsMiddleware;
use deepagents::runtime::{Runtime, RuntimeConfig, RuntimeMiddleware};
use deepagents::skills::loader::{load_skills, SkillsLoadOptions};
use deepagents::types::Message;

fn write_skill(dir: &std::path::Path, name: &str, desc: &str, tools_json: Option<&str>) {
    std::fs::create_dir_all(dir.join(name)).unwrap();
    let skill_md = format!(
        "---\nname: {}\ndescription: {}\n---\n\n# {}\n",
        name, desc, name
    );
    std::fs::write(dir.join(name).join("SKILL.md"), skill_md).unwrap();
    if let Some(tj) = tools_json {
        std::fs::write(dir.join(name).join("tools.json"), tj).unwrap();
    }
}

#[test]
fn load_skills_last_one_wins() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("A");
    let b = temp.path().join("B");
    write_skill(&a, "web-research", "A impl", None);
    write_skill(&b, "web-research", "B impl", None);
    let sources = vec![
        a.to_string_lossy().to_string(),
        b.to_string_lossy().to_string(),
    ];
    let loaded = load_skills(&sources, SkillsLoadOptions::default()).unwrap();
    let skill = loaded
        .metadata
        .iter()
        .find(|s| s.name == "web-research")
        .unwrap();
    assert_eq!(skill.description, "B impl");
    assert_eq!(loaded.metadata.len(), 1);
}

#[test]
fn load_skills_requires_skill_md() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("skills");
    std::fs::create_dir_all(src.join("bad-skill")).unwrap();
    let sources = vec![src.to_string_lossy().to_string()];
    let err = load_skills(&sources, SkillsLoadOptions::default()).unwrap_err();
    assert!(err.to_string().contains("missing SKILL.md"));
}

#[test]
fn prompt_only_skill_has_no_tools() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("skills");
    write_skill(&src, "note-skill", "Prompt only", None);
    let sources = vec![src.to_string_lossy().to_string()];
    let loaded = load_skills(&sources, SkillsLoadOptions::default()).unwrap();
    assert!(loaded.tools.is_empty());
}

#[test]
fn load_skills_canonicalizes_metadata_tools_and_overrides() {
    let temp = tempfile::tempdir().unwrap();
    let source_a = temp.path().join("A");
    let source_b = temp.path().join("B");

    write_skill(
        &source_a,
        "zeta-skill",
        "Zeta",
        Some(
            r#"{
                "tools": [{
                    "name": "shared-tool",
                    "description": "Shared tool from zeta",
                    "input_schema": { "type": "object", "properties": {}, "required": [] },
                    "steps": [],
                    "policy": {}
                }]
            }"#,
        ),
    );
    write_skill(
        &source_a,
        "beta-skill",
        "Beta from A",
        Some(
            r#"{
                "tools": [{
                    "name": "shared-tool",
                    "description": "Shared tool from beta",
                    "input_schema": { "type": "object", "properties": {}, "required": [] },
                    "steps": [],
                    "policy": {}
                }]
            }"#,
        ),
    );
    write_skill(
        &source_a,
        "alpha-skill",
        "Alpha from A",
        Some(
            r#"{
                "tools": [{
                    "name": "alpha-tool",
                    "description": "Alpha tool from A",
                    "input_schema": { "type": "object", "properties": {}, "required": [] },
                    "steps": [],
                    "policy": {}
                }]
            }"#,
        ),
    );
    write_skill(
        &source_b,
        "alpha-skill",
        "Alpha from B",
        Some(
            r#"{
                "tools": [{
                    "name": "alpha-tool",
                    "description": "Alpha tool from B",
                    "input_schema": { "type": "object", "properties": {}, "required": [] },
                    "steps": [],
                    "policy": {}
                }]
            }"#,
        ),
    );

    let loaded = load_skills(
        &[
            source_a.to_string_lossy().to_string(),
            source_b.to_string_lossy().to_string(),
        ],
        SkillsLoadOptions::default(),
    )
    .unwrap();

    let metadata_names = loaded
        .metadata
        .iter()
        .map(|skill| skill.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        metadata_names,
        vec!["alpha-skill", "beta-skill", "zeta-skill"]
    );

    let tool_names = loaded
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(tool_names, vec!["alpha-tool", "shared-tool"]);

    let override_names = loaded
        .diagnostics
        .overrides
        .iter()
        .map(|record| record.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(override_names, vec!["alpha-skill", "shared-tool"]);
}

#[tokio::test]
async fn skills_tool_executes_steps() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("skills");
    let tools_json = r#"{
        "tools": [{
            "name": "read-readme",
            "description": "Read README",
            "input_schema": { "type": "object", "properties": {}, "required": [] },
            "steps": [{ "tool_name": "read_file", "arguments": { "file_path": "README.md", "limit": 1 } }],
            "policy": { "allow_filesystem": true }
        }]
    }"#;
    write_skill(&src, "read-readme", "Read README", Some(tools_json));
    std::fs::write(
        temp.path().join("README.md"),
        "Project: DeepAgents\nhello\n",
    )
    .unwrap();

    let sources = vec![src.to_string_lossy().to_string()];
    let options = SkillsLoadOptions::default();
    let skills_mw: Arc<dyn deepagents::runtime::RuntimeMiddleware> =
        Arc::new(SkillsMiddleware::new(sources, options));

    let provider = Arc::new(deepagents::provider::mock::MockProvider::from_script(
        deepagents::provider::mock::MockScript {
            steps: vec![
                deepagents::provider::mock::MockStep::ToolCalls {
                    calls: vec![AgentToolCall {
                        tool_name: "read-readme".to_string(),
                        arguments: serde_json::json!({}),
                        call_id: Some("c1".to_string()),
                    }],
                },
                deepagents::provider::mock::MockStep::FinalText {
                    text: "done".to_string(),
                },
            ],
        },
    ));

    let backend = deepagents::create_local_sandbox_backend(temp.path(), None).unwrap();
    let agent = deepagents::create_deep_agent_with_backend(backend);
    let runtime = SimpleRuntime::new(
        agent,
        provider,
        deepagents::runtime::simple::SimpleRuntimeOptions {
            config: RuntimeConfig {
                max_steps: 4,
                provider_timeout_ms: 1000,
            },
            approval: None,
            audit: None,
            root: temp.path().to_string_lossy().to_string(),
            mode: ExecutionMode::NonInteractive,
        },
    )
    .with_runtime_middlewares(vec![skills_mw]);

    let out = runtime
        .run(vec![Message {
            role: "user".to_string(),
            content: "read".to_string(),
            content_blocks: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }])
        .await;
    assert!(out.error.is_none());
    let result = out.tool_results.first().unwrap();
    let content = result
        .output
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap();
    assert!(content.contains("Project: DeepAgents"));
}

#[tokio::test]
async fn skills_tools_are_injected_into_tool_specs() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("skills");
    write_skill(
        &src,
        "zeta-skill",
        "Zeta",
        Some(
            r#"{
                "tools": [{
                    "name": "zeta-tool",
                    "description": "Zeta",
                    "input_schema": { "type": "object", "properties": {}, "required": [] },
                    "steps": [],
                    "policy": {}
                }]
            }"#,
        ),
    );
    write_skill(
        &src,
        "alpha-skill",
        "Alpha",
        Some(
            r#"{
                "tools": [{
                    "name": "alpha-tool",
                    "description": "Alpha",
                    "input_schema": { "type": "object", "properties": {}, "required": [] },
                    "steps": [],
                    "policy": {}
                }]
            }"#,
        ),
    );
    std::fs::write(
        temp.path().join("README.md"),
        "Project: DeepAgents\nhello\n",
    )
    .unwrap();

    let sources = vec![src.to_string_lossy().to_string()];
    let options = SkillsLoadOptions::default();
    let skills_mw: Arc<dyn deepagents::runtime::RuntimeMiddleware> =
        Arc::new(SkillsMiddleware::new(sources, options));

    let captured: Arc<Mutex<Option<AgentProviderRequest>>> = Arc::new(Mutex::new(None));
    let provider = Arc::new(CaptureProvider {
        captured: captured.clone(),
    });

    let backend = deepagents::create_local_sandbox_backend(temp.path(), None).unwrap();
    let agent = deepagents::create_deep_agent_with_backend(backend);
    let runtime = SimpleRuntime::new(
        agent,
        provider,
        deepagents::runtime::simple::SimpleRuntimeOptions {
            config: RuntimeConfig {
                max_steps: 1,
                provider_timeout_ms: 1000,
            },
            approval: None,
            audit: None,
            root: temp.path().to_string_lossy().to_string(),
            mode: ExecutionMode::NonInteractive,
        },
    )
    .with_runtime_middlewares(vec![skills_mw]);

    let _ = runtime
        .run(vec![Message {
            role: "user".to_string(),
            content: "ping".to_string(),
            content_blocks: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }])
        .await;

    let req = captured.lock().unwrap().clone().unwrap();
    let skill_tool_names = req
        .tool_specs
        .iter()
        .filter(|tool| tool.name == "alpha-tool" || tool.name == "zeta-tool")
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(skill_tool_names, vec!["alpha-tool", "zeta-tool"]);
    assert_eq!(
        req.messages
            .iter()
            .find(|message| message.content.contains("DEEPAGENTS_SKILLS_INJECTED_V1"))
            .map(|message| message.content.as_str()),
        Some(
            "DEEPAGENTS_SKILLS_INJECTED_V1\n## Skills\n- alpha-skill: Alpha (source: skills)\n- zeta-skill: Zeta (source: skills)\n"
        )
    );
    assert!(req
        .messages
        .iter()
        .any(|m| m.content.contains("DEEPAGENTS_SKILLS_INJECTED_V1")));
}

#[tokio::test]
async fn skills_middleware_restores_and_rewrites_snapshot_deterministically() {
    let middleware = SkillsMiddleware::new(
        vec!["/definitely/missing".to_string()],
        SkillsLoadOptions::default(),
    );
    let mut state = deepagents::state::AgentState::default();
    state.extra.insert(
        "skills_metadata".to_string(),
        serde_json::json!([
            {
                "name": "zeta-skill",
                "description": "Zeta",
                "path": "/tmp/zeta",
                "source": "restored",
                "allowed_tools": []
            },
            {
                "name": "alpha-skill",
                "description": "Alpha",
                "path": "/tmp/alpha",
                "source": "restored",
                "allowed_tools": []
            }
        ]),
    );
    state.extra.insert(
        "skills_tools".to_string(),
        serde_json::json!([
            {
                "name": "zeta-tool",
                "description": "Zeta tool",
                "input_schema": { "type": "object", "properties": {}, "required": [] },
                "steps": [],
                "policy": {
                    "allow_filesystem": false,
                    "allow_execute": false,
                    "allow_network": false,
                    "max_steps": 8,
                    "timeout_ms": 1000,
                    "max_output_chars": 12000
                },
                "skill_name": "zeta-skill",
                "source": "restored"
            },
            {
                "name": "alpha-tool",
                "description": "Alpha tool",
                "input_schema": { "type": "object", "properties": {}, "required": [] },
                "steps": [],
                "policy": {
                    "allow_filesystem": false,
                    "allow_execute": false,
                    "allow_network": false,
                    "max_steps": 8,
                    "timeout_ms": 1000,
                    "max_output_chars": 12000
                },
                "skill_name": "alpha-skill",
                "source": "restored"
            }
        ]),
    );

    let messages = vec![
        Message {
            role: "system".to_string(),
            content:
                "DEEPAGENTS_SKILLS_INJECTED_V1\n## Skills\n- zeta-skill: Zeta (source: restored)\n"
                    .to_string(),
            content_blocks: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        },
        Message {
            role: "system".to_string(),
            content: "DEEPAGENTS_SKILLS_INJECTED_V1\nduplicate".to_string(),
            content_blocks: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        },
        Message {
            role: "user".to_string(),
            content: "ping".to_string(),
            content_blocks: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        },
    ];

    let messages =
        <SkillsMiddleware as RuntimeMiddleware>::before_run(&middleware, messages, &mut state)
            .await
            .unwrap();

    let markers = messages
        .iter()
        .filter(|message| message.content.contains("DEEPAGENTS_SKILLS_INJECTED_V1"))
        .collect::<Vec<_>>();
    assert_eq!(markers.len(), 1);
    assert_eq!(
        markers[0].content,
        "DEEPAGENTS_SKILLS_INJECTED_V1\n## Skills\n- alpha-skill: Alpha (source: restored)\n- zeta-skill: Zeta (source: restored)\n"
    );

    let metadata = serde_json::from_value::<Vec<deepagents::skills::SkillMetadata>>(
        state.extra.get("skills_metadata").cloned().unwrap(),
    )
    .unwrap();
    assert_eq!(
        metadata
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha-skill", "zeta-skill"]
    );
    let tools = serde_json::from_value::<Vec<deepagents::skills::SkillToolSpec>>(
        state.extra.get("skills_tools").cloned().unwrap(),
    )
    .unwrap();
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha-tool", "zeta-tool"]
    );
    let diagnostics = serde_json::from_value::<deepagents::skills::SkillsDiagnostics>(
        state.extra.get("skills_diagnostics").cloned().unwrap(),
    )
    .unwrap();
    assert!(diagnostics.sources.is_empty());
    assert!(diagnostics.overrides.is_empty());
}

struct CaptureProvider {
    captured: Arc<Mutex<Option<AgentProviderRequest>>>,
}

#[async_trait::async_trait]
impl AgentProvider for CaptureProvider {
    async fn step(&self, req: AgentProviderRequest) -> Result<AgentStep, anyhow::Error> {
        *self.captured.lock().unwrap() = Some(req);
        Ok(AgentStep::FinalText {
            text: "ok".to_string(),
        })
    }
}
