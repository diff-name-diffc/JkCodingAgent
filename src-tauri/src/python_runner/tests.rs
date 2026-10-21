use super::*;
use crate::agent::rig_ext::agents::session_workspace_dir_name;
use crate::test_util::TempDirGuard;

#[test]
fn chat_python_inherits_category_directories_and_keeps_sessions_separate() {
    let dir = TempDirGuard::new("python-chat-workspace");
    let db = DispatcherDb::new(dir.path().join("test.sqlite3")).unwrap();
    let project = dir.path().join("shared-project");
    let artifacts = dir.path().join("shared-artifacts");
    std::fs::create_dir(&project).unwrap();
    std::fs::create_dir(&artifacts).unwrap();
    let mut config = db.list_chat_category_agent_configs().unwrap().remove(0);
    config.project_directories = vec![
        project.display().to_string(),
        artifacts.display().to_string(),
    ];
    db.save_chat_category_agent_configs(&[config.clone()])
        .unwrap();
    let first = db
        .create_chat_session("first", Some(&config.category_id))
        .unwrap();
    let second = db
        .create_chat_session("second", Some(&config.category_id))
        .unwrap();
    let root_dir = dir.path().join("app-data");
    let first_workspace = workspace::resolve_chat_workspace(&db, &root_dir, &first.id)
        .unwrap()
        .unwrap();
    let second_workspace = workspace::resolve_chat_workspace(&db, &root_dir, &second.id)
        .unwrap()
        .unwrap();
    assert_ne!(first_workspace.session_root, second_workspace.session_root);
    assert_eq!(
        first_workspace.project_roots,
        second_workspace.project_roots
    );
    assert_eq!(
        first_workspace.project_roots,
        vec![
            project.canonicalize().unwrap(),
            artifacts.canonicalize().unwrap()
        ]
    );
    assert!(first_workspace
        .session_root
        .ends_with(Path::new("plain-chat-browser").join(session_workspace_dir_name(&first.id))));
    let paths = prepare_paths(&root_dir, "run-one", Some(&first_workspace)).unwrap();
    let prompt = build_python_agent_system_prompt(Some(&first_workspace), &paths);
    assert!(prompt.contains("普通聊天目录规则（固定）"));
    assert!(prompt.contains(
        &serde_json::Value::String(project.canonicalize().unwrap().display().to_string())
            .to_string()
    ));
    assert!(prompt.contains("Python 相对路径以该目录为基准"));
    assert!(!prompt.contains("不要访问项目文件"));
    assert!(paths.run_dir.starts_with(&first_workspace.session_root));
    assert!(paths.temp_dir.as_ref().unwrap().is_dir());
    assert!(!root_dir.join("python-runner/runs").exists());

    std::fs::remove_dir(&artifacts).unwrap();
    let error = workspace::resolve_chat_workspace(&db, &root_dir, &first.id).unwrap_err();
    assert!(format!("{error:#}").contains("共享工程目录不存在或无法访问"));
}

#[test]
fn project_python_keeps_the_original_directory_and_prompt_contract() {
    let dir = TempDirGuard::new("python-project-workspace");
    let db = DispatcherDb::new(dir.path().join("test.sqlite3")).unwrap();
    let session = db.create_project_session("project-one", "project").unwrap();
    let workspace = workspace::resolve_chat_workspace(&db, dir.path(), &session.id).unwrap();
    assert!(workspace.is_none());
    assert!(!dir.path().join("plain-chat-browser").exists());
    let paths = prepare_paths(dir.path(), "run-one", None).unwrap();
    assert_eq!(paths.run_dir, dir.path().join("python-runner/runs/run-one"));
    let prompt = build_python_agent_system_prompt(None, &paths);
    assert!(prompt.ends_with("不要建议使用 shell；不要安装与错误无关的包；不要访问项目文件。"));
    assert!(!prompt.contains("普通聊天目录规则"));
}

#[tokio::test]
async fn python_shared_directory_writer_waits_and_cancellation_releases_the_queue() {
    let dir = TempDirGuard::new("python-chat-resource-lease");
    let root = dir.path().canonicalize().unwrap();
    let first = ChatWorkspace {
        session_root: root.join("session-one"),
        project_roots: vec![root.join("project-one"), root.join("shared-artifacts")],
    };
    let second = ChatWorkspace {
        session_root: root.join("session-two"),
        project_roots: vec![root.join("project-two"), root.join("shared-artifacts")],
    };
    let (_first_tx, first_rx) = watch::channel(false);
    let lease = workspace::acquire_chat_workspace(Some(&first), first_rx)
        .await
        .unwrap();
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let waiting = workspace::acquire_chat_workspace(Some(&second), cancel_rx);
    tokio::pin!(waiting);
    assert!(timeout(Duration::from_millis(20), waiting.as_mut())
        .await
        .is_err());
    cancel_tx.send(true).unwrap();
    let error = timeout(Duration::from_secs(1), waiting.as_mut())
        .await
        .unwrap()
        .err()
        .unwrap();
    assert!(error.to_string().contains("已取消"));
    drop(lease);
    let (_retry_tx, retry_rx) = watch::channel(false);
    let retry = timeout(
        Duration::from_secs(1),
        workspace::acquire_chat_workspace(Some(&second), retry_rx),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(retry.is_some());
}

async fn assert_cleanup_while_queued_does_not_recreate_run_files(delete_session: bool) {
    let dir = TempDirGuard::new("python-cleanup-race");
    let db = DispatcherDb::new(dir.path().join("test.sqlite3")).unwrap();
    let session = db.create_chat_session("queued python", None).unwrap();
    let message = db
        .add_visible_message_from_segments(
            &session.id,
            "assistant",
            crate::agent::db::content::content_to_segments_json("print(1)"),
        )
        .unwrap();
    let record = PythonCodeRunRecord {
        run_id: Uuid::new_v4().to_string(),
        workspace_id: session.id.clone(),
        message_id: message.id,
        code_block_index: 0,
        code_hash: code_hash("print(1)"),
        code: "print(1)".to_string(),
        status: "running".to_string(),
        stdout: String::new(),
        stderr: String::new(),
        installed_packages_json: "[]".to_string(),
        tool_events_json: "[]".to_string(),
        explanation_markdown: String::new(),
        error_reason: None,
        created_at: Utc::now().to_rfc3339(),
        updated_at: Utc::now().to_rfc3339(),
    };
    db.upsert_python_code_run(&record).unwrap();
    let root = dir.path().join("app-data");
    let chat = workspace::resolve_chat_workspace(&db, &root, &session.id)
        .unwrap()
        .unwrap();
    assert!(!chat.session_root.exists(), "解析不得在租约前创建会话目录");
    let (_cleanup_tx, cleanup_rx) = watch::channel(false);
    let cleanup_lease = workspace::acquire_chat_workspace(Some(&chat), cleanup_rx)
        .await
        .unwrap();
    let (_run_tx, run_rx) = watch::channel(false);
    let waiting = workspace::acquire_chat_workspace(Some(&chat), run_rx);
    tokio::pin!(waiting);
    assert!(timeout(Duration::from_millis(20), waiting.as_mut())
        .await
        .is_err());
    if delete_session {
        db.delete_session(&session.id).unwrap();
    } else {
        db.clear_messages(&session.id).unwrap();
    }
    drop(cleanup_lease);
    let _run_lease = timeout(Duration::from_secs(1), waiting.as_mut())
        .await
        .unwrap()
        .unwrap();
    let error = workspace::prepare_current_run_paths(&db, &root, &record, Some(&chat)).unwrap_err();
    assert!(error.to_string().contains("执行目标已被清理或替换"));
    assert!(!chat.session_root.exists());
    assert!(!root.join("python-runner").exists());
    assert!(!persist_existing_run_record(&db, &record).await.unwrap());
    assert!(db
        .list_python_code_runs(&session.id, None)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn clearing_messages_while_python_waits_prevents_run_files_and_ghost_updates() {
    assert_cleanup_while_queued_does_not_recreate_run_files(false).await;
}

#[tokio::test]
async fn deleting_session_while_python_waits_prevents_run_files_and_ghost_updates() {
    assert_cleanup_while_queued_does_not_recreate_run_files(true).await;
}
