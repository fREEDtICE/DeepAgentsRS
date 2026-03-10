use std::sync::Arc;

use deepagents::backends::{LocalSandbox, SandboxBackend};
use deepagents::state::AgentState;
use deepagents::DeepAgent;

#[tokio::test]
async fn read_file_text_output_unchanged_and_has_no_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.txt");
    std::fs::write(&p, "hello\nworld\n").unwrap();
    let backend: Arc<dyn SandboxBackend> = Arc::new(LocalSandbox::new(dir.path()).unwrap());
    let agent = DeepAgent::with_backend(backend);
    let mut state = AgentState::default();
    let (res, _delta) = agent
        .call_tool_stateful(
            "read_file",
            serde_json::json!({"file_path": p.to_string_lossy().to_string(), "offset": 0, "limit": 1}),
            &mut state,
        )
        .await
        .unwrap();
    assert_eq!(
        res.output.get("type").and_then(|v| v.as_str()),
        Some("text")
    );
    assert!(res
        .output
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap()
        .contains('→'));
    assert!(res.content_blocks.is_none());
}

#[tokio::test]
async fn read_file_image_returns_image_output_and_base64_block() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("img.png");
    std::fs::write(&p, vec![0u8, 1u8, 2u8]).unwrap();
    let backend: Arc<dyn SandboxBackend> = Arc::new(LocalSandbox::new(dir.path()).unwrap());
    let agent = DeepAgent::with_backend(backend);
    let mut state = AgentState::default();
    let (res, _delta) = agent
        .call_tool_stateful(
            "read_file",
            serde_json::json!({"file_path": p.to_string_lossy().to_string(), "mode": "auto"}),
            &mut state,
        )
        .await
        .unwrap();
    assert_eq!(
        res.output.get("type").and_then(|v| v.as_str()),
        Some("image")
    );
    assert_eq!(
        res.output.get("mime_type").and_then(|v| v.as_str()),
        Some("image/png")
    );
    let blocks = res.content_blocks.as_ref().unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].block_type, "image_base64");
    assert_eq!(blocks[0].mime_type.as_deref(), Some("image/png"));
    assert_eq!(blocks[0].base64.as_deref(), Some("AAEC"));
}

#[tokio::test]
async fn read_file_image_respects_max_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("big.png");
    std::fs::write(&p, vec![0u8; 20]).unwrap();
    let backend: Arc<dyn SandboxBackend> = Arc::new(LocalSandbox::new(dir.path()).unwrap());
    let agent = DeepAgent::with_backend(backend);
    let mut state = AgentState::default();
    let err = agent
        .call_tool_stateful(
            "read_file",
            serde_json::json!({"file_path": p.to_string_lossy().to_string(), "mode": "image", "max_bytes": 10}),
            &mut state,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("too_large"));
}
