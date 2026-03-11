use std::sync::{Arc, Mutex};

use deepagents::approval::ExecutionMode;
use deepagents::provider::protocol::{Provider, ProviderRequest, ProviderStep};
use deepagents::provider::ProviderToolCall;
use deepagents::runtime::simple::SimpleRuntime;
use deepagents::runtime::skills_middleware::SkillsMiddleware;
use deepagents::runtime::{Runtime, RuntimeConfig};
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
                    calls: vec![ProviderToolCall {
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
        Vec::new(),
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
    let tools_json = r#"{
        "tools": [{
            "name": "echo-skill",
            "description": "Echo",
            "input_schema": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] },
            "steps": [{ "tool_name": "read_file", "arguments": { "file_path": "README.md", "limit": 1 } }],
            "policy": { "allow_filesystem": true }
        }]
    }"#;
    write_skill(&src, "echo-skill", "Echo", Some(tools_json));
    std::fs::write(
        temp.path().join("README.md"),
        "Project: DeepAgents\nhello\n",
    )
    .unwrap();

    let sources = vec![src.to_string_lossy().to_string()];
    let options = SkillsLoadOptions::default();
    let skills_mw: Arc<dyn deepagents::runtime::RuntimeMiddleware> =
        Arc::new(SkillsMiddleware::new(sources, options));

    let captured: Arc<Mutex<Option<ProviderRequest>>> = Arc::new(Mutex::new(None));
    let provider = Arc::new(CaptureProvider {
        captured: captured.clone(),
    });

    let backend = deepagents::create_local_sandbox_backend(temp.path(), None).unwrap();
    let agent = deepagents::create_deep_agent_with_backend(backend);
    let runtime = SimpleRuntime::new(
        agent,
        provider,
        Vec::new(),
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
    assert!(req.tool_specs.iter().any(|t| t.name == "echo-skill"));
    assert!(req
        .messages
        .iter()
        .any(|m| m.content.contains("DEEPAGENTS_SKILLS_INJECTED_V1")));
}

struct CaptureProvider {
    captured: Arc<Mutex<Option<ProviderRequest>>>,
}

#[async_trait::async_trait]
impl Provider for CaptureProvider {
    async fn step(&self, req: ProviderRequest) -> Result<ProviderStep, anyhow::Error> {
        *self.captured.lock().unwrap() = Some(req);
        Ok(ProviderStep::FinalText {
            text: "ok".to_string(),
        })
    }
}
