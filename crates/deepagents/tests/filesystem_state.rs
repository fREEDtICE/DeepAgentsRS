use deepagents::state::{
    DefaultFilesystemReducer, FileDelta, FileRecord, FilesystemDelta, FilesystemState, StateReducer,
};

#[test]
fn reducer_upsert_overwrites() {
    let mut state = FilesystemState::default();
    state.files.insert(
        "/a".to_string(),
        FileRecord {
            content: vec!["old".to_string()],
            created_at: None,
            modified_at: None,
            deleted: false,
            truncated: false,
        },
    );

    let mut delta = FilesystemDelta::default();
    delta.files.insert(
        "/a".to_string(),
        FileDelta {
            upsert: Some(FileRecord {
                content: vec!["new".to_string()],
                created_at: None,
                modified_at: Some("t".to_string()),
                deleted: false,
                truncated: false,
            }),
            delete: false,
        },
    );

    DefaultFilesystemReducer.reduce(&mut state, delta);
    assert_eq!(
        state.files.get("/a").unwrap().content,
        vec!["new".to_string()]
    );
}

#[test]
fn reducer_delete_marks_deleted() {
    let mut state = FilesystemState::default();
    state.files.insert(
        "/a".to_string(),
        FileRecord {
            content: vec!["x".to_string()],
            created_at: None,
            modified_at: None,
            deleted: false,
            truncated: false,
        },
    );

    let mut delta = FilesystemDelta::default();
    delta.files.insert(
        "/a".to_string(),
        FileDelta {
            upsert: Some(FileRecord {
                content: Vec::new(),
                created_at: None,
                modified_at: Some("t".to_string()),
                deleted: true,
                truncated: false,
            }),
            delete: true,
        },
    );

    DefaultFilesystemReducer.reduce(&mut state, delta);
    let rec = state.files.get("/a").unwrap();
    assert!(rec.deleted);
    assert!(rec.content.is_empty());
}
