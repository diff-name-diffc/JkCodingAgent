use super::*;
use crate::agent::log_snapshot::model::LogSource;
use crate::agent::log_snapshot::update::UpdateContext;
use crate::agent::rig_ext::review::RigReviewContext;
use crate::ssh_tool::SshSessionManager;

fn context(
    root: &std::path::Path,
    pool: r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>,
) -> UpdateContext {
    UpdateContext {
        workspace_id: "session-one".into(),
        session_root: root.to_path_buf(),
        extra_allowed_dirs: Vec::new(),
        ssh_manager: SshSessionManager::new(std::sync::Arc::new(pool)),
        review_context: RigReviewContext::unconfigured(),
        app_handle: None,
    }
}

fn options(source: &std::path::Path) -> UpdateOptions {
    UpdateOptions {
        snapshot_id: Some("incident".into()),
        label: None,
        sources: vec![LogSource::LocalFile {
            path: source.to_string_lossy().into_owned(),
        }],
        since: None,
        until: None,
        tail: None,
        max_total_bytes: 1024 * 1024,
        timeout_secs: 30,
    }
}

#[tokio::test]
async fn cleanup_block_coordinates_active_and_future_updates() {
    let guard = crate::test_util::TempDirGuard::new("log-snapshot-lifecycle");
    let root = guard.path().join("session");
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("app.log");
    std::fs::write(&source, "ERROR one\n").unwrap();
    let pool = r2d2::Pool::builder()
        .max_size(1)
        .build(r2d2_sqlite::SqliteConnectionManager::memory())
        .unwrap();
    let manager = LogSnapshotManager::new();
    let store = SnapshotStore::new(root.clone(), "session-one".into());
    let permit = manager
        .inner
        .update_slots
        .clone()
        .acquire_owned()
        .await
        .unwrap();
    manager
        .start_update(context(&root, pool.clone()), options(&source))
        .await
        .unwrap();
    let error = manager.block_workspace("session-one").err().unwrap();
    assert!(error.contains("仍在更新"), "{error}");
    drop(permit);
    let status = manager.status(store.clone(), "incident", 30).await.unwrap();
    assert_eq!(status.update.unwrap().state, UpdateState::Ready);

    let block = manager.block_workspace("session-one").unwrap();
    let error = manager
        .start_update(context(&root, pool.clone()), options(&source))
        .await
        .unwrap_err();
    assert!(error.contains("删除或清空"), "{error}");
    block.finish(false);
    manager
        .start_update(context(&root, pool.clone()), options(&source))
        .await
        .unwrap();
    let status = manager.status(store.clone(), "incident", 30).await.unwrap();
    assert_eq!(status.current.unwrap().generation, 2);

    let block = manager.block_workspace("session-one").unwrap();
    block.finish(true);
    let error = manager
        .start_update(context(&root, pool), options(&source))
        .await
        .unwrap_err();
    assert!(error.contains("删除或清空"), "{error}");
}

#[tokio::test]
async fn update_recovers_interrupted_staging_and_unpublished_generation() {
    let guard = crate::test_util::TempDirGuard::new("log-snapshot-recovery");
    let root = guard.path().join("session");
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("app.log");
    std::fs::write(&source, "ERROR recovered\n").unwrap();
    let pool = r2d2::Pool::builder()
        .max_size(1)
        .build(r2d2_sqlite::SqliteConnectionManager::memory())
        .unwrap();
    let manager = LogSnapshotManager::new();
    let store = SnapshotStore::new(root.clone(), "session-one".into());
    let stale = store.staging_root("incident", "stale").unwrap();
    std::fs::create_dir_all(stale.join("raw")).unwrap();
    std::fs::write(stale.join("raw/stale.log"), "stale").unwrap();
    let orphan = store.generation_root("incident", 1).unwrap();
    std::fs::create_dir_all(orphan.join("raw")).unwrap();
    std::fs::write(orphan.join("orphan.txt"), "orphan").unwrap();

    manager
        .start_update(context(&root, pool), options(&source))
        .await
        .unwrap();
    let status = manager.status(store.clone(), "incident", 30).await.unwrap();
    assert_eq!(status.current.unwrap().generation, 1);
    assert!(!stale.exists());
    assert!(!orphan.join("orphan.txt").exists());
    assert!(orphan.join("index.sqlite3").exists());
}
