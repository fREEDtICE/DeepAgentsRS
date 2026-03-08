use std::sync::Arc;

use deepagents::backends::{FilesystemBackend, LocalSandbox, SandboxBackend};

#[tokio::test]
async fn local_backend_read_write_edit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let backend = LocalSandbox::new(&root).unwrap();

    let file_path = root.join("a.txt").to_string_lossy().to_string();
    let wr = backend.write_file(&file_path, "hello\nworld\n").await.unwrap();
    assert!(wr.error.is_none());

    let content = backend.read(&file_path, 0, 100).await.unwrap();
    assert!(content.contains("     1→hello"));

    let er = backend
        .edit_file(&file_path, "world", "rust")
        .await
        .unwrap();
    assert_eq!(er.occurrences, Some(1));

    let content2 = backend.read(&file_path, 0, 100).await.unwrap();
    assert!(content2.contains("rust"));

    let dr = backend.delete_file(&file_path).await.unwrap();
    assert!(dr.error.is_none());
    assert!(backend.read(&file_path, 0, 10).await.is_err());
}

#[tokio::test]
async fn local_backend_glob_and_grep() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let backend = LocalSandbox::new(&root).unwrap();

    let file_path = root.join("src").join("b.txt");
    std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    backend
        .write_file(file_path.to_string_lossy().as_ref(), "todo: fix\nok\n")
        .await
        .unwrap();

    let files = backend.glob("**/*.txt").await.unwrap();
    assert_eq!(files.len(), 1);

    let matches = backend
        .grep("todo:", Some(root.to_string_lossy().as_ref()), Some("**/*.txt"))
        .await
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].line, 1);
}

#[tokio::test]
async fn local_backend_execute_allow_list() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let backend: Arc<dyn SandboxBackend> = Arc::new(
        LocalSandbox::new(&root)
            .unwrap()
            .with_shell_allow_list(Some(vec!["echo".to_string()])),
    );

    let ok = backend.execute("echo hello", Some(5)).await.unwrap();
    assert_eq!(ok.exit_code, 0);
    assert!(ok.output.contains("hello"));

    let err = backend.execute("ls", Some(5)).await.err().unwrap();
    assert!(err.to_string().contains("command_not_allowed"));
}
