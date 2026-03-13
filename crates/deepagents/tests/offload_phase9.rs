use std::sync::Arc;

use async_trait::async_trait;

use deepagents::backends::{CompositeBackend, LocalSandbox, SandboxBackend};
use deepagents::provider::mock::{MockProvider, MockScript, MockStep};
use deepagents::provider::protocol::AgentToolCall;
use deepagents::runtime::{
    FilesystemRuntimeMiddleware, FilesystemRuntimeOptions, Runtime, RuntimeMiddleware,
};
use deepagents::tools::{default_tools, Tool, ToolResult};
use deepagents::DeepAgent;

struct EmitBigTool;

#[async_trait]
impl Tool for EmitBigTool {
    fn name(&self) -> &'static str {
        "emit_big"
    }

    fn description(&self) -> &'static str {
        "Emits a large text blob."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "n": {
                    "type": "integer",
                    "description": "Number of characters to generate (default: 0)."
                }
            },
            "required": [],
            "additionalProperties": false
        })
    }

    async fn call(&self, input: serde_json::Value) -> anyhow::Result<ToolResult> {
        let n = input.get("n").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        Ok(ToolResult {
            output: serde_json::json!({ "content": "x".repeat(n) }),
            content_blocks: None,
        })
    }
}

#[tokio::test]
async fn phase9_offload_writes_to_large_backend_and_replaces_tool_output() {
    let workspace = tempfile::tempdir().unwrap();
    let large = tempfile::tempdir().unwrap();

    let ws_backend: Arc<dyn SandboxBackend> =
        Arc::new(LocalSandbox::new(workspace.path()).unwrap());
    let large_backend: Arc<dyn SandboxBackend> = Arc::new(LocalSandbox::new(large.path()).unwrap());
    let backend: Arc<dyn SandboxBackend> = Arc::new(
        CompositeBackend::new(ws_backend.clone())
            .with_route("/large_tool_results", large_backend.clone()),
    );

    let mut tools = default_tools(backend.clone());
    tools.push(Arc::new(EmitBigTool));
    let agent = DeepAgent::with_backend_and_tools(backend.clone(), tools);

    let script = MockScript {
        steps: vec![
            MockStep::ToolCalls {
                calls: vec![AgentToolCall {
                    tool_name: "emit_big".to_string(),
                    arguments: serde_json::json!({ "n": 5000 }),
                    call_id: Some("a:b/c".to_string()),
                }],
            },
            MockStep::FinalText {
                text: "done".to_string(),
            },
        ],
    };
    let provider: Arc<dyn deepagents::provider::AgentProvider> =
        Arc::new(MockProvider::from_script(script));

    let fs_opts = FilesystemRuntimeOptions {
        enabled: true,
        tool_output_char_threshold: 200,
        large_result_prefix: "/large_tool_results".to_string(),
        ..Default::default()
    };

    let fs_mw: Arc<dyn RuntimeMiddleware> = Arc::new(FilesystemRuntimeMiddleware::new(fs_opts));

    let runtime = deepagents::runtime::simple::SimpleRuntime::new(
        agent,
        provider,
        deepagents::runtime::simple::SimpleRuntimeOptions {
            config: deepagents::runtime::RuntimeConfig {
                max_steps: 8,
                provider_timeout_ms: 1000,
            },
            approval: None,
            audit: None,
            root: workspace.path().to_string_lossy().to_string(),
            mode: deepagents::approval::ExecutionMode::NonInteractive,
        },
    )
    .with_runtime_middlewares(vec![fs_mw]);

    let out = runtime
        .run(vec![deepagents::types::Message {
            role: "user".to_string(),
            content: "go".to_string(),
            content_blocks: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }])
        .await;

    let tr = out
        .tool_results
        .iter()
        .find(|r| r.tool_name == "emit_big")
        .unwrap();
    let out_v = &tr.output;
    assert_eq!(out_v.get("offloaded").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        out_v.get("offload_path").and_then(|v| v.as_str()),
        Some("/large_tool_results/a_b_c")
    );
    let content = out_v.get("content").and_then(|v| v.as_str()).unwrap_or("");
    assert!(content.contains("offloaded"));
    assert!(content.len() < 200);

    let large_file = large.path().join("a_b_c");
    assert!(large_file.exists());
    let bytes = std::fs::read_to_string(&large_file).unwrap();
    assert_eq!(bytes.len(), 5000);

    assert!(!workspace.path().join("large_tool_results").exists());
}

#[tokio::test]
async fn phase9_offload_creates_large_tool_results_dir_in_workspace_backend() {
    let workspace = tempfile::tempdir().unwrap();

    let backend: Arc<dyn SandboxBackend> = Arc::new(LocalSandbox::new(workspace.path()).unwrap());

    let mut tools = default_tools(backend.clone());
    tools.push(Arc::new(EmitBigTool));
    let agent = DeepAgent::with_backend_and_tools(backend.clone(), tools);

    let script = MockScript {
        steps: vec![
            MockStep::ToolCalls {
                calls: vec![AgentToolCall {
                    tool_name: "emit_big".to_string(),
                    arguments: serde_json::json!({ "n": 5000 }),
                    call_id: Some("a:b/c".to_string()),
                }],
            },
            MockStep::FinalText {
                text: "done".to_string(),
            },
        ],
    };
    let provider: Arc<dyn deepagents::provider::AgentProvider> =
        Arc::new(MockProvider::from_script(script));

    let fs_opts = FilesystemRuntimeOptions {
        tool_output_char_threshold: 200,
        ..Default::default()
    };
    let fs_mw: Arc<dyn RuntimeMiddleware> = Arc::new(FilesystemRuntimeMiddleware::new(fs_opts));

    assert!(!workspace.path().join("large_tool_results").exists());

    let runtime = deepagents::runtime::simple::SimpleRuntime::new(
        agent,
        provider,
        deepagents::runtime::simple::SimpleRuntimeOptions {
            config: deepagents::runtime::RuntimeConfig {
                max_steps: 8,
                provider_timeout_ms: 1000,
            },
            approval: None,
            audit: None,
            root: workspace.path().to_string_lossy().to_string(),
            mode: deepagents::approval::ExecutionMode::NonInteractive,
        },
    )
    .with_runtime_middlewares(vec![fs_mw]);

    let out = runtime
        .run(vec![deepagents::types::Message {
            role: "user".to_string(),
            content: "go".to_string(),
            content_blocks: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }])
        .await;

    let tr = out
        .tool_results
        .iter()
        .find(|r| r.tool_name == "emit_big")
        .unwrap();
    let out_v = &tr.output;
    assert_eq!(out_v.get("offloaded").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        out_v.get("offload_path").and_then(|v| v.as_str()),
        Some("/large_tool_results/a_b_c")
    );

    let large_file = workspace.path().join("large_tool_results").join("a_b_c");
    assert!(large_file.exists());
    let bytes = std::fs::read_to_string(&large_file).unwrap();
    assert_eq!(bytes.len(), 5000);
}
