use deepagents::memory::{
    FileMemoryStore, MemoryEntry, MemoryPolicy, MemoryQuery, MemoryScope, MemoryStatus, MemoryStore,
    MemoryType,
};
use deepagents::runtime::{MemoryLoadOptions, MemoryMiddleware, RuntimeMiddleware};
use deepagents::state::AgentState;
use deepagents::types::Message;

#[tokio::test]
async fn file_memory_store_put_get_query_and_evict() {
    let temp = tempfile::tempdir().unwrap();
    let store_path = temp.path().join("memory_store.json");
    let store = FileMemoryStore::new(store_path).with_policy(MemoryPolicy {
        max_entries: 2,
        max_bytes_total: 10_000,
        eviction: deepagents::memory::MemoryEvictionPolicy::Lru,
    });

    store.load().await.unwrap();
    store
        .put(MemoryEntry {
            key: "a/one".to_string(),
            value: "v1".to_string(),
            title: None,
            scope: MemoryScope::User,
            scope_id: None,
            memory_type: MemoryType::Semantic,
            pinned: false,
            status: MemoryStatus::Active,
            confidence: None,
            salience: None,
            supersedes: None,
            owner_user_id: None,
            owner_workspace_id: None,
            owner_channel_account_id: None,
            tags: vec!["t1".to_string()],
            created_at: String::new(),
            updated_at: String::new(),
            last_accessed_at: String::new(),
            access_count: 0,
        })
        .await
        .unwrap();
    store
        .put(MemoryEntry {
            key: "b/two".to_string(),
            value: "v2".to_string(),
            title: None,
            scope: MemoryScope::User,
            scope_id: None,
            memory_type: MemoryType::Semantic,
            pinned: false,
            status: MemoryStatus::Active,
            confidence: None,
            salience: None,
            supersedes: None,
            owner_user_id: None,
            owner_workspace_id: None,
            owner_channel_account_id: None,
            tags: vec!["t2".to_string()],
            created_at: String::new(),
            updated_at: String::new(),
            last_accessed_at: String::new(),
            access_count: 0,
        })
        .await
        .unwrap();
    store
        .put(MemoryEntry {
            key: "c/three".to_string(),
            value: "v3".to_string(),
            title: None,
            scope: MemoryScope::User,
            scope_id: None,
            memory_type: MemoryType::Semantic,
            pinned: false,
            status: MemoryStatus::Active,
            confidence: None,
            salience: None,
            supersedes: None,
            owner_user_id: None,
            owner_workspace_id: None,
            owner_channel_account_id: None,
            tags: vec!["t3".to_string()],
            created_at: String::new(),
            updated_at: String::new(),
            last_accessed_at: String::new(),
            access_count: 0,
        })
        .await
        .unwrap();

    let report = store.evict_if_needed().await.unwrap();
    assert!(report.after_entries <= 2);
    store.flush().await.unwrap();

    let got = store.get("b/two").await.unwrap().unwrap();
    assert_eq!(got.value, "v2");

    let q = store
        .query(MemoryQuery {
            prefix: Some("b/".to_string()),
            tag: None,
            limit: Some(10),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].key, "b/two");
}

#[tokio::test]
async fn memory_middleware_injects_once_and_keeps_memory_private() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join(".deepagents")).unwrap();
    std::fs::write(
        temp.path().join(".deepagents").join("AGENTS.md"),
        "# Memory\nHello\n",
    )
    .unwrap();

    let options = MemoryLoadOptions {
        allow_host_paths: false,
        max_injected_chars: 2000,
        ..Default::default()
    };
    let mw = MemoryMiddleware::new(
        temp.path().to_string_lossy().to_string(),
        vec![".deepagents/AGENTS.md".to_string()],
        options,
    );

    let mut state = AgentState::default();
    let messages = vec![Message {
        role: "user".to_string(),
        content: "hi".to_string(),
        content_blocks: None,
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        status: None,
    }];

    let out1 = mw.before_run(messages.clone(), &mut state).await.unwrap();
    assert!(out1
        .iter()
        .any(|m| m.role == "system" && m.content.contains("DEEPAGENTS_MEMORY_INJECTED_V1")));
    assert!(state.private.memory_contents.is_some());
    assert!(state.extra.contains_key("memory_diagnostics"));

    let out2 = mw.before_run(out1.clone(), &mut state).await.unwrap();
    let count = out2
        .iter()
        .filter(|m| m.role == "system" && m.content.contains("DEEPAGENTS_MEMORY_INJECTED_V1"))
        .count();
    assert_eq!(count, 1);
}
