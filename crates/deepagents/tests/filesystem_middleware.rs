use deepagents::state::AgentState;

#[tokio::test]
async fn filesystem_middleware_updates_state_on_write_edit_delete() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let agent = deepagents::create_deep_agent(&root).unwrap();
    let mut state = AgentState::default();

    let p = root.join("a.txt").to_string_lossy().to_string();

    let (out, delta) = agent
        .call_tool_stateful(
            "write_file",
            serde_json::json!({
              "file_path": p,
              "content": "hello\nworld\n"
            }),
            &mut state,
        )
        .await
        .unwrap();
    assert!(delta.is_some());

    let key = out
        .output
        .get("path")
        .and_then(|p| p.as_str())
        .unwrap()
        .to_string();
    let rec = state.filesystem.files.get(&key).unwrap();
    assert_eq!(rec.content, vec!["hello".to_string(), "world".to_string()]);
    assert!(!rec.deleted);

    agent
        .call_tool_stateful(
            "edit_file",
            serde_json::json!({
              "file_path": key.clone(),
              "old_string": "world",
              "new_string": "rust"
            }),
            &mut state,
        )
        .await
        .unwrap();
    let rec = state.filesystem.files.get(&key).unwrap();
    assert_eq!(rec.content, vec!["hello".to_string(), "rust".to_string()]);

    agent
        .call_tool_stateful(
            "delete_file",
            serde_json::json!({ "file_path": key.clone() }),
            &mut state,
        )
        .await
        .unwrap();
    let rec = state.filesystem.files.get(&key).unwrap();
    assert!(rec.deleted);
}

#[tokio::test]
async fn tool_schema_rejects_missing_fields() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let agent = deepagents::create_deep_agent(&root).unwrap();

    let err = agent
        .call_tool("write_file", serde_json::json!({ "file_path": "a.txt" }))
        .await
        .err()
        .unwrap();
    assert!(err.to_string().contains("missing field"));
}
