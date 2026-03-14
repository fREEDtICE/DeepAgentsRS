use deepagents::memory::{
    FileMemoryStore, MemoryActorInput, MemoryEntry, MemoryIdentityResolver, MemoryPolicy,
    MemoryQuery, MemoryScopeType, MemoryStore, MemoryType,
};
use deepagents::runtime::{MemoryLoadOptions, MemoryMiddleware, RuntimeMiddleware};
use deepagents::state::AgentState;
use deepagents::types::Message;

#[tokio::test]
async fn file_memory_store_put_get_query_delete_and_evict() {
    let temp = tempfile::tempdir().unwrap();
    let store_path = temp.path().join("memory_store.json");
    let store = FileMemoryStore::new(store_path).with_policy(MemoryPolicy {
        max_entries: 2,
        max_bytes_total: 10_000,
        eviction: deepagents::memory::MemoryEvictionPolicy::Fifo,
    });

    store.load().await.unwrap();
    let mut first = MemoryEntry::new("a/one", "v1");
    first.tags = vec!["t1".to_string()];
    store.put(first).await.unwrap();
    let mut second = MemoryEntry::new("b/two", "v2");
    second.tags = vec!["t2".to_string()];
    store.put(second).await.unwrap();
    let mut third = MemoryEntry::new("c/three", "v3");
    third.tags = vec!["t3".to_string()];
    let report = store.put_with_report(third).await.unwrap();
    assert_eq!(report.evicted, 1);
    assert_eq!(report.after_entries, 2);
    assert_eq!(report.evicted_keys, vec!["a/one".to_string()]);
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

    assert!(store.delete("b/two").await.unwrap());
    assert!(store.get("b/two").await.unwrap().is_none());
    let deleted = store.inspect("b/two").await.unwrap().unwrap();
    assert_eq!(deleted.status, deepagents::memory::MemoryStatus::Deleted);
    assert!(!store.delete("missing").await.unwrap());
}

#[tokio::test]
async fn file_memory_store_policy_can_be_overridden_and_persisted() {
    let temp = tempfile::tempdir().unwrap();
    let store_path = temp.path().join("memory_store.json");
    let store = FileMemoryStore::new(&store_path);

    store.load().await.unwrap();
    let first = MemoryEntry::new("a/one", "v1");
    store.put(first).await.unwrap();
    let second = MemoryEntry::new("b/two", "v2");
    store.put(second).await.unwrap();

    store
        .set_policy(MemoryPolicy {
            max_entries: 1,
            max_bytes_total: 10_000,
            eviction: deepagents::memory::MemoryEvictionPolicy::Fifo,
        })
        .await
        .unwrap();
    let report = store.evict_if_needed().await.unwrap();
    assert_eq!(report.evicted, 1);
    store.flush().await.unwrap();

    let reloaded = FileMemoryStore::new(store_path).with_policy(MemoryPolicy {
        max_entries: 99,
        max_bytes_total: 99_999,
        eviction: deepagents::memory::MemoryEvictionPolicy::Lru,
    });
    reloaded.load().await.unwrap();
    assert_eq!(reloaded.policy().max_entries, 1);
    assert_eq!(
        reloaded.policy().eviction,
        deepagents::memory::MemoryEvictionPolicy::Fifo
    );
}

#[tokio::test]
async fn file_memory_store_loads_legacy_v1_entries_with_schema_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let store_path = temp.path().join("memory_store.json");
    std::fs::write(
        &store_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 1,
            "policy": {
                "max_entries": 5,
                "max_bytes_total": 5000,
                "eviction": "Lru"
            },
            "entries": [{
                "key": "legacy",
                "value": "hello",
                "tags": ["imported"],
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z",
                "last_accessed_at": "2026-01-01T00:00:00Z",
                "access_count": 1
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let store = FileMemoryStore::new(store_path);
    store.load().await.unwrap();
    let entry = store.inspect("legacy").await.unwrap().unwrap();
    assert_eq!(
        entry.scope_type,
        deepagents::memory::MemoryScopeType::Workspace
    );
    assert_eq!(entry.scope_id, "__compat_workspace__");
    assert_eq!(entry.title, "legacy");
    assert_eq!(entry.status, deepagents::memory::MemoryStatus::Active);
}

#[test]
fn local_identity_resolver_and_access_rules_are_scope_aware() {
    let resolver = deepagents::memory::LocalIdentityResolver::new("/tmp/project");
    let actor = resolver.resolve_actor(&deepagents::memory::MemoryActorInput {
        user_id: Some("user_1".to_string()),
        thread_id: Some("thread_1".to_string()),
        workspace_ids: vec!["ws_a".to_string()],
    });

    let mut user_entry = MemoryEntry::new("user", "value");
    user_entry.scope_type = deepagents::memory::MemoryScopeType::User;
    user_entry.scope_id = "user_1".to_string();
    assert!(deepagents::memory::can_read_entry(&user_entry, &actor));
    assert!(deepagents::memory::can_write_scope(
        deepagents::memory::MemoryScopeType::User,
        "user_1",
        &actor
    ));

    let mut blocked_entry = MemoryEntry::new("blocked", "value");
    blocked_entry.scope_type = deepagents::memory::MemoryScopeType::User;
    blocked_entry.scope_id = "user_2".to_string();
    assert!(!deepagents::memory::can_read_entry(&blocked_entry, &actor));
    assert!(!deepagents::memory::can_write_scope(
        deepagents::memory::MemoryScopeType::User,
        "user_2",
        &actor
    ));

    let mut workspace_entry = MemoryEntry::new("workspace", "value");
    workspace_entry.scope_type = deepagents::memory::MemoryScopeType::Workspace;
    workspace_entry.scope_id = "ws_a".to_string();
    assert!(deepagents::memory::can_read_entry(&workspace_entry, &actor));

    let mut thread_entry = MemoryEntry::new("thread", "value");
    thread_entry.scope_type = deepagents::memory::MemoryScopeType::Thread;
    thread_entry.scope_id = "thread_1".to_string();
    assert!(deepagents::memory::can_read_entry(&thread_entry, &actor));
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

#[tokio::test]
async fn memory_middleware_scoped_mode_injects_ranked_pack() {
    let temp = tempfile::tempdir().unwrap();
    let store_path = temp.path().join(".deepagents").join("memory_store.json");
    std::fs::create_dir_all(store_path.parent().unwrap()).unwrap();
    std::fs::write(
        temp.path().join(".deepagents").join("AGENTS.md"),
        "# Legacy Memory\nshould_not_be_used = true\n",
    )
    .unwrap();

    let store = FileMemoryStore::new(&store_path);
    store.load().await.unwrap();

    let mut preference = MemoryEntry::new("reply_style", "Reply in concise Chinese.");
    preference.scope_type = MemoryScopeType::User;
    preference.scope_id = "user_123".to_string();
    preference.memory_type = MemoryType::Procedural;
    preference.title = "Preferred reply style".to_string();
    preference.pinned = true;
    preference.tags = vec!["preference".to_string(), "language".to_string()];
    store.put(preference).await.unwrap();

    let mut workspace = MemoryEntry::new("release_day", "The team ships every Friday.");
    workspace.scope_type = MemoryScopeType::Workspace;
    workspace.scope_id = "ws_team".to_string();
    workspace.memory_type = MemoryType::Semantic;
    workspace.title = "Release cadence".to_string();
    workspace.tags = vec!["project".to_string(), "release".to_string()];
    store.put(workspace).await.unwrap();

    let mut thread = MemoryEntry::new("current_topic", "This thread is about the release update.");
    thread.scope_type = MemoryScopeType::Thread;
    thread.scope_id = "thread_abc".to_string();
    thread.memory_type = MemoryType::Episodic;
    thread.title = "Current topic".to_string();
    store.put(thread).await.unwrap();
    store.flush().await.unwrap();

    let mw = MemoryMiddleware::new(
        temp.path().to_string_lossy().to_string(),
        vec![".deepagents/AGENTS.md".to_string()],
        MemoryLoadOptions {
            max_injected_chars: 2_000,
            runtime_mode: deepagents::memory::MemoryRuntimeMode::Scoped,
            store_path: store_path.clone(),
            actor: MemoryActorInput {
                user_id: Some("user_123".to_string()),
                thread_id: Some("thread_abc".to_string()),
                workspace_ids: vec!["ws_team".to_string()],
            },
            ..Default::default()
        },
    );

    let mut state = AgentState::default();
    let messages = vec![Message {
        role: "user".to_string(),
        content: "Please prepare a concise Chinese release update for the team.".to_string(),
        content_blocks: None,
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        status: None,
    }];

    let out = mw.before_run(messages, &mut state).await.unwrap();
    let memory = out
        .iter()
        .find(|message| {
            message.role == "system" && message.content.contains("DEEPAGENTS_MEMORY_INJECTED_V2")
        })
        .unwrap();
    assert!(memory.content.contains("<memory_pack>"));
    assert!(memory.content.contains("<thread_memory>"));
    assert!(memory.content.contains("<pinned_memory>"));
    assert!(memory.content.contains("<workspace_context>"));
    assert!(memory.content.contains("Reply in concise Chinese."));
    assert!(memory.content.contains("The team ships every Friday."));
    assert!(!memory.content.contains("should_not_be_used"));
    assert!(state.extra.contains_key("memory_retrieval"));
    assert!(!state.extra.contains_key("memory_diagnostics"));
}
