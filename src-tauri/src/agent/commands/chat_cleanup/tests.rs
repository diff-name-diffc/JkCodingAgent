use super::*;
use crate::test_util::TempDirGuard;

struct Fixture {
    _guard: TempDirGuard,
    app_root: PathBuf,
    shared_root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let guard = TempDirGuard::new("chat-cleanup");
        let base = guard.path().canonicalize().unwrap();
        let app_root = base.join("app");
        let shared_root = base.join("shared");
        std::fs::create_dir_all(&app_root).unwrap();
        std::fs::create_dir_all(&shared_root).unwrap();
        std::fs::write(shared_root.join("deliverable.txt"), "成果保留").unwrap();
        Self {
            _guard: guard,
            app_root,
            shared_root,
        }
    }

    fn session(&self, id: &str) -> PathBuf {
        let path = ChatWorkspace::session_directory(&self.app_root, id);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("temporary.txt"), "临时产物").unwrap();
        path
    }

    fn plan(&self, id: &str) -> CleanupPlan {
        CleanupPlan::new(&self.app_root, id, DispatcherSessionKind::Chat)
            .unwrap()
            .unwrap()
    }
}

#[test]
fn removes_only_current_session_and_preserves_other_sessions_and_shared_deliverables() {
    let fixture = Fixture::new();
    let current = fixture.session("current");
    let other = fixture.session("other");
    for folder in ["python-runs/run-1", "browser-runs/run-1", ".tmp"] {
        let directory = current.join(folder);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("output.txt"), "temporary").unwrap();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&fixture.shared_root, current.join("shared-link")).unwrap();

    fixture.plan("current").remove_directory().unwrap();

    assert!(!current.exists());
    assert!(other.join("temporary.txt").is_file());
    assert!(fixture.shared_root.join("deliverable.txt").is_file());
}

#[test]
fn project_sessions_do_not_produce_cleanup_plans() {
    let fixture = Fixture::new();
    let root = fixture.session("project-session");
    let plan = CleanupPlan::new(
        &fixture.app_root,
        "project-session",
        DispatcherSessionKind::Project,
    )
    .unwrap();
    assert!(plan.is_none());
    assert!(root.join("temporary.txt").is_file());
    assert!(fixture.shared_root.join("deliverable.txt").is_file());
}

#[tokio::test(start_paused = true)]
async fn active_python_or_shell_lease_rejects_cleanup_before_data_is_removed() {
    let fixture = Fixture::new();
    let session_root = fixture.session("busy");
    let (_tx, rx) = watch::channel(false);
    let running = ResourceArbiter::shared()
        .acquire(
            vec![Claim {
                resource: Resource::LocalFilesystem(session_root.clone()),
                write: true,
            }],
            rx,
            Instant::now() + Duration::from_secs(60),
        )
        .await
        .unwrap();

    let error = fixture.plan("busy").acquire().await.err().unwrap();
    assert!(error.contains("正在被 Python 或其他工具使用"));
    assert!(session_root.join("temporary.txt").is_file());

    drop(running);
    let cleanup = fixture.plan("busy").acquire().await.unwrap();
    // DB 失败时只丢弃守卫，不得提前删文件。
    drop(cleanup);
    assert!(session_root.join("temporary.txt").is_file());
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn root_symlink_locks_canonical_target_but_only_unlinks_the_session_alias() {
    let fixture = Fixture::new();
    let session_root = ChatWorkspace::session_directory(&fixture.app_root, "linked");
    std::fs::create_dir_all(session_root.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&fixture.shared_root, &session_root).unwrap();
    let plan = fixture.plan("linked");
    assert_eq!(plan.resource_root, fixture.shared_root);
    let (_tx, rx) = watch::channel(false);
    let running = ResourceArbiter::shared()
        .acquire(
            vec![Claim {
                resource: Resource::File(fixture.shared_root.clone()),
                write: true,
            }],
            rx,
            Instant::now() + Duration::from_secs(60),
        )
        .await
        .unwrap();
    assert!(plan.acquire().await.is_err());
    drop(running);

    fixture
        .plan("linked")
        .acquire()
        .await
        .unwrap()
        .after_commit("测试会话删除");
    assert!(std::fs::symlink_metadata(&session_root).is_err());
    assert!(fixture.shared_root.join("deliverable.txt").is_file());
}

#[cfg(unix)]
#[tokio::test]
async fn post_commit_cleanup_failure_does_not_report_database_deletion_as_failed() {
    let fixture = Fixture::new();
    let db = DispatcherDb::new(fixture.app_root.join("test.sqlite3")).unwrap();
    let session = db.create_chat_session("cleanup", None).unwrap();
    let session_root = fixture.session(&session.id);
    let cleanup = prepare(&db, fixture.app_root.clone(), &session.id)
        .await
        .unwrap()
        .unwrap();
    let parent = session_root.parent().unwrap();
    std::fs::rename(parent, fixture.app_root.join("old-session-directory")).unwrap();
    std::os::unix::fs::symlink(&fixture.shared_root, parent).unwrap();

    db.delete_session(&session.id).unwrap();
    // 目录父路径被替换后，清理明确留痕但不更改已经成功的 DB 操作结果。
    cleanup.after_commit("会话删除");
    assert!(db.get_dispatcher_session(&session.id).unwrap().is_none());
    assert!(fixture.shared_root.join("deliverable.txt").is_file());
}

#[test]
fn missing_session_directory_is_an_idempotent_cleanup() {
    let fixture = Fixture::new();
    let plan = fixture.plan("never-executed");
    plan.remove_directory().unwrap();
    plan.remove_directory().unwrap();
    assert!(!plan.session_root.exists());
}
