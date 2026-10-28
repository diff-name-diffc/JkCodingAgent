use super::*;
use crate::agent::log_snapshot::model::LogSource;
use crate::agent::log_snapshot::query::{MatchMode, OutputMode, SearchEngine};
use crate::agent::log_snapshot::store::SnapshotStore;
use crate::agent::log_snapshot::update::UpdateContext;
use crate::agent::rig_ext::review::RigReviewContext;
use crate::ssh_tool::SshSessionManager;

struct Fixture {
    manager: LogSnapshotManager,
    context: UpdateContext,
    store: SnapshotStore,
    source: std::path::PathBuf,
    _pool: Arc<r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>>,
    _guard: crate::test_util::TempDirGuard,
}

impl Fixture {
    fn new() -> Self {
        let guard = crate::test_util::TempDirGuard::new("log-snapshot-manager");
        let session_root = guard.path().join("session");
        std::fs::create_dir_all(&session_root).unwrap();
        let source = session_root.join("application.log");
        std::fs::write(
            &source,
            "2026-01-01 INFO boot\n2026-01-01 ERROR trace=abc-123456 timeout\nstack line\n",
        )
        .unwrap();
        let pool = Arc::new(
            r2d2::Pool::builder()
                .max_size(1)
                .build(r2d2_sqlite::SqliteConnectionManager::memory())
                .unwrap(),
        );
        let context = UpdateContext {
            workspace_id: "session-one".into(),
            session_root: session_root.clone(),
            extra_allowed_dirs: Vec::new(),
            ssh_manager: SshSessionManager::new(pool.clone()),
            review_context: RigReviewContext::unconfigured(),
            app_handle: None,
        };
        let store = SnapshotStore::new(session_root, "session-one".into());
        Self {
            manager: LogSnapshotManager::new(),
            context,
            store,
            source,
            _pool: pool,
            _guard: guard,
        }
    }

    fn options(&self) -> UpdateOptions {
        UpdateOptions {
            snapshot_id: Some("incident".into()),
            label: Some("支付故障".into()),
            sources: vec![LogSource::LocalFile {
                path: self.source.to_string_lossy().into_owned(),
            }],
            since: None,
            until: None,
            tail: None,
            max_total_bytes: 8 * 1024 * 1024,
            timeout_secs: 30,
        }
    }

    async fn update_ready(&self, sources: Vec<LogSource>) -> SnapshotStatus {
        let mut options = self.options();
        options.sources = sources;
        self.manager
            .start_update(self.context.clone(), options)
            .await
            .unwrap();
        let status = self
            .manager
            .status(self.store.clone(), "incident", 30)
            .await
            .unwrap();
        assert_eq!(status.update.as_ref().unwrap().state, UpdateState::Ready);
        status
    }

    fn search(&self, pattern: &str, mode: MatchMode, engine: SearchEngine) -> LogSearchRequest {
        LogSearchRequest {
            snapshot_id: "incident".into(),
            patterns: vec![pattern.into()],
            match_mode: mode,
            case_sensitive: false,
            output_mode: OutputMode::Matches,
            engine,
            file_ids: Vec::new(),
            context_before: 1,
            context_after: 1,
            max_matches: 50,
            max_files: 50,
            max_line_chars: 2000,
            cursor: None,
        }
    }
}

#[tokio::test]
async fn repeated_update_replaces_generation_and_preserves_exact_search_and_read() {
    let fixture = Fixture::new();
    let first = fixture
        .update_ready(fixture.options().sources.clone())
        .await;
    assert_eq!(first.current.as_ref().unwrap().generation, 1);
    let metrics = first.current.as_ref().unwrap().metrics.as_ref().unwrap();
    assert!(metrics.indexed_lines > 0);
    assert!(metrics.index_bytes > 0);
    assert!(metrics.index_ratio.unwrap() > 0.0);
    assert!(
        metrics.total_duration_ms
            >= metrics.review_duration_ms
                + metrics.download_duration_ms
                + metrics.index_duration_ms
                + metrics.publish_duration_ms
    );
    let evidence = &first.current.as_ref().unwrap().files[0].evidence;
    assert!(evidence.fingerprint_before.is_some());
    assert!(evidence.fingerprint_after.is_some());
    assert_eq!(evidence.source_changed, Some(false));
    assert!(evidence.collection.is_some());
    assert!(first.update.as_ref().unwrap().metrics.is_some());

    let result = fixture
        .manager
        .search(
            fixture.store.clone(),
            fixture.search("c-123", MatchMode::Fixed, SearchEngine::Auto),
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.engine, "fts5-trigram");
    assert!(result.scan_complete);
    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].line.line_number, 2);
    assert_eq!(result.matches[0].before.len(), 1);
    assert_eq!(result.matches[0].after.len(), 1);

    let read = fixture
        .manager
        .read(
            fixture.store.clone(),
            LogReadRequest {
                snapshot_id: "incident".into(),
                file_id: 1,
                line_number: Some(2),
                before: 1,
                after: 1,
                byte_offset: None,
                max_bytes: 64 * 1024,
                max_line_chars: 2000,
            },
        )
        .await
        .unwrap();
    assert_eq!(read.lines.len(), 3);
    assert!(read.lines[1].text.contains("abc-123456"));

    std::fs::write(
        &fixture.source,
        "2026-01-02 INFO restart\n2026-01-02 WARN resolved request=xyz-999\n",
    )
    .unwrap();
    let second = fixture.update_ready(Vec::new()).await;
    assert_eq!(second.current.as_ref().unwrap().generation, 2);
    assert!(!fixture
        .store
        .generation_root("incident", 1)
        .unwrap()
        .exists());

    let old = fixture
        .manager
        .search(
            fixture.store.clone(),
            fixture.search("abc-123456", MatchMode::Fixed, SearchEngine::Scan),
            None,
        )
        .await
        .unwrap();
    assert!(old.scan_complete);
    assert!(old.matches.is_empty());
    let new = fixture
        .manager
        .search(
            fixture.store.clone(),
            fixture.search(r"request=xyz-\d+", MatchMode::Regex, SearchEngine::Auto),
            None,
        )
        .await
        .unwrap();
    assert_eq!(new.engine, "ripgrep");
    assert_eq!(new.matches.len(), 1);
    assert!(new.matches[0].line.text.contains("xyz-999"));
}

#[tokio::test]
async fn byte_read_returns_original_long_line() {
    let fixture = Fixture::new();
    let long_payload = "x".repeat(2 * 1024 * 1024 + 17);
    std::fs::write(&fixture.source, format!("prefix\nERROR {long_payload}\n")).unwrap();
    fixture
        .update_ready(fixture.options().sources.clone())
        .await;
    let result = fixture
        .manager
        .search(
            fixture.store.clone(),
            fixture.search("ERROR", MatchMode::Fixed, SearchEngine::Auto),
            None,
        )
        .await
        .unwrap();
    let offset = result.matches[0].line.byte_offset;
    let read = fixture
        .manager
        .read(
            fixture.store.clone(),
            LogReadRequest {
                snapshot_id: "incident".into(),
                file_id: 1,
                line_number: None,
                before: 0,
                after: 0,
                byte_offset: Some(offset),
                max_bytes: 1024 * 1024,
                max_line_chars: 2000,
            },
        )
        .await
        .unwrap();
    let text = read.text.unwrap();
    assert!(text.starts_with("ERROR xxx"));
    assert_eq!(read.next_byte_offset, Some(offset + 1024 * 1024));
}

#[tokio::test]
async fn byte_read_aligns_next_offset_to_utf8_boundary() {
    let fixture = Fixture::new();
    // 3 字节 UTF-8 字符（“中”）：窗口切在字符中间时，文本与下一页游标都
    // 必须对齐到字符边界，不产生 U+FFFD。
    let payload = "中".repeat(4096);
    std::fs::write(&fixture.source, format!("{payload}\n")).unwrap();
    fixture
        .update_ready(fixture.options().sources.clone())
        .await;
    // 1025 不是 3 的倍数：恰好把最后一个字符切成两半。
    let read = fixture
        .manager
        .read(
            fixture.store.clone(),
            LogReadRequest {
                snapshot_id: "incident".into(),
                file_id: 1,
                line_number: None,
                before: 0,
                after: 0,
                byte_offset: Some(0),
                max_bytes: 1025,
                max_line_chars: 20_000,
            },
        )
        .await
        .unwrap();
    let text = read.text.unwrap();
    assert!(!text.contains('\u{fffd}'), "文本不应含替换符：{text:?}");
    let bytes = text.as_bytes().len();
    assert_eq!(bytes, 1023, "1025 窗口应回退到最近的字符边界");
    assert_eq!(read.next_byte_offset, Some(1023));
}

#[tokio::test]
async fn index_and_scan_share_pagination_and_count_semantics() {
    let fixture = Fixture::new();
    std::fs::write(
        &fixture.source,
        "ERROR 1\nok\nERROR 2\nok\nERROR 3\nok\nERROR 4\nok\nERROR 5\n",
    )
    .unwrap();
    fixture
        .update_ready(fixture.options().sources.clone())
        .await;
    for engine in [SearchEngine::Auto, SearchEngine::Scan] {
        let mut cursor = None;
        let mut pages = 0;
        let mut line_numbers = Vec::new();
        loop {
            let mut request = fixture.search("ERROR", MatchMode::Fixed, engine);
            request.max_matches = 2;
            request.cursor = cursor.clone();
            let result = fixture
                .manager
                .search(fixture.store.clone(), request, None)
                .await
                .unwrap();
            line_numbers.extend(result.matches.iter().map(|item| item.line.line_number));
            pages += 1;
            cursor = result.next_cursor.clone();
            if cursor.is_none() {
                assert!(result.scan_complete, "engine={engine:?}");
                break;
            }
            assert!(!result.scan_complete, "engine={engine:?}");
            assert!(pages < 4, "pagination did not converge");
        }
        assert_eq!(pages, 3, "engine={engine:?}");
        assert_eq!(line_numbers, vec![1, 3, 5, 7, 9], "engine={engine:?}");
    }
    let mut count = fixture.search(r"ERROR [0-9]", MatchMode::Regex, SearchEngine::Auto);
    count.output_mode = OutputMode::Count;
    let result = fixture
        .manager
        .search(fixture.store.clone(), count, None)
        .await
        .unwrap();
    assert_eq!(result.total_match_lines, Some(5));
    assert_eq!(result.files[0].match_count, 5);
    assert!(result.scan_complete);
}

#[tokio::test]
async fn mixed_sources_handle_empty_crlf_invalid_utf8_and_case() {
    let fixture = Fixture::new();
    let root = fixture.source.parent().unwrap();
    let second = root.join("second.log");
    let empty = root.join("empty.log");
    std::fs::write(&fixture.source, "ERROR One\r\nplain\r\n").unwrap();
    std::fs::write(&second, b"ERROR Two\ninvalid \xff bytes\n").unwrap();
    std::fs::write(&empty, "").unwrap();
    let sources = vec![
        LogSource::LocalFile {
            path: fixture.source.to_string_lossy().into_owned(),
        },
        LogSource::LocalFile {
            path: second.to_string_lossy().into_owned(),
        },
        LogSource::LocalFile {
            path: empty.to_string_lossy().into_owned(),
        },
    ];
    let status = fixture.update_ready(sources).await;
    let manifest = status.current.unwrap();
    assert_eq!(manifest.files.len(), 3);
    assert_eq!(manifest.files[0].line_count, 2);
    assert_eq!(manifest.files[2].line_count, 0);

    let mut request = fixture.search("ERROR", MatchMode::Fixed, SearchEngine::Auto);
    request.case_sensitive = true;
    let result = fixture
        .manager
        .search(fixture.store.clone(), request.clone(), None)
        .await
        .unwrap();
    assert_eq!(result.matches.len(), 2);
    request.patterns = vec!["error".into()];
    let lowercase = fixture
        .manager
        .search(fixture.store.clone(), request.clone(), None)
        .await
        .unwrap();
    assert!(lowercase.scan_complete);
    assert!(lowercase.matches.is_empty());
    request.case_sensitive = false;
    let insensitive = fixture
        .manager
        .search(fixture.store.clone(), request, None)
        .await
        .unwrap();
    assert_eq!(insensitive.matches.len(), 2);

    let read = fixture
        .manager
        .read(
            fixture.store.clone(),
            LogReadRequest {
                snapshot_id: "incident".into(),
                file_id: 2,
                line_number: Some(2),
                before: 0,
                after: 0,
                byte_offset: None,
                max_bytes: 64 * 1024,
                max_line_chars: 2000,
            },
        )
        .await
        .unwrap();
    assert!(read.lines[0].text.contains('\u{fffd}'));
}

#[tokio::test]
async fn failed_update_preserves_last_ready_generation() {
    let fixture = Fixture::new();
    fixture
        .update_ready(fixture.options().sources.clone())
        .await;
    std::fs::write(&fixture.source, [0x1f, 0x8b, 0x08, 0x00]).unwrap();
    let mut options = fixture.options();
    options.sources = Vec::new();
    fixture
        .manager
        .start_update(fixture.context.clone(), options)
        .await
        .unwrap();
    let failed = fixture
        .manager
        .status(fixture.store.clone(), "incident", 30)
        .await
        .unwrap();
    assert_eq!(failed.update.unwrap().state, UpdateState::Failed);
    assert_eq!(failed.current.as_ref().unwrap().generation, 1);

    std::fs::write(&fixture.source, "x".repeat(2 * 1024 * 1024)).unwrap();
    let mut options = fixture.options();
    options.sources = Vec::new();
    options.max_total_bytes = 1024 * 1024;
    fixture
        .manager
        .start_update(fixture.context.clone(), options)
        .await
        .unwrap();
    let quota = fixture
        .manager
        .status(fixture.store.clone(), "incident", 30)
        .await
        .unwrap();
    assert_eq!(quota.update.unwrap().state, UpdateState::Failed);
    assert_eq!(quota.current.as_ref().unwrap().generation, 1);
    let old = fixture
        .manager
        .search(
            fixture.store.clone(),
            fixture.search("abc-123456", MatchMode::Fixed, SearchEngine::Auto),
            None,
        )
        .await
        .unwrap();
    assert_eq!(old.matches.len(), 1);
}

#[tokio::test]
async fn queued_update_can_be_cancelled_deterministically() {
    let fixture = Fixture::new();
    let permit = fixture
        .manager
        .inner
        .update_slots
        .clone()
        .acquire_owned()
        .await
        .unwrap();
    let mut options = fixture.options();
    options.snapshot_id = Some("queued".into());
    fixture
        .manager
        .start_update(fixture.context.clone(), options)
        .await
        .unwrap();
    assert!(fixture.manager.cancel("session-one", "queued").unwrap());
    drop(permit);
    let status = fixture
        .manager
        .status(fixture.store.clone(), "queued", 30)
        .await
        .unwrap();
    assert_eq!(status.update.unwrap().state, UpdateState::Cancelled);
    assert!(status.current.is_none());
}

#[tokio::test]
async fn interrupted_update_is_not_reported_as_ready() {
    let fixture = Fixture::new();
    let now = Utc::now().to_rfc3339();
    let record = UpdateRecord {
        job_id: "interrupted".into(),
        snapshot_id: "incident".into(),
        generation: 1,
        state: UpdateState::Syncing,
        created_at: now.clone(),
        updated_at: now,
        raw_bytes: 10,
        file_count: 1,
        metrics: None,
        error: None,
    };
    fixture.store.write_update(&record).unwrap();
    let status = fixture
        .manager
        .status(fixture.store.clone(), "incident", 0)
        .await
        .unwrap();
    assert_eq!(status.update.unwrap().state, UpdateState::Failed);
    assert!(!status.active);
}

#[tokio::test]
async fn cursor_is_rejected_after_generation_changes() {
    let fixture = Fixture::new();
    std::fs::write(&fixture.source, "ERROR one\nERROR two\n").unwrap();
    fixture
        .update_ready(fixture.options().sources.clone())
        .await;
    let mut request = fixture.search("ERROR", MatchMode::Fixed, SearchEngine::Auto);
    request.max_matches = 1;
    let page = fixture
        .manager
        .search(fixture.store.clone(), request.clone(), None)
        .await
        .unwrap();
    request.cursor = page.next_cursor;
    assert!(request.cursor.is_some());
    std::fs::write(&fixture.source, "ERROR new\n").unwrap();
    fixture.update_ready(Vec::new()).await;
    let error = fixture
        .manager
        .search(fixture.store.clone(), request, None)
        .await
        .unwrap_err();
    assert!(
        error.contains("不属于当前日志快照、代次或查询条件"),
        "{error}"
    );
}

#[tokio::test]
async fn source_warnings_are_exposed_in_search_and_read() {
    let fixture = Fixture::new();
    fixture
        .update_ready(fixture.options().sources.clone())
        .await;
    let root = fixture.store.generation_root("incident", 1).unwrap();
    let mut manifest = fixture
        .store
        .load_current_generation("incident")
        .unwrap()
        .unwrap()
        .manifest;
    manifest.warnings.push("测试来源在采集期间变化".into());
    fixture
        .store
        .write_generation_manifest(&root, &manifest)
        .unwrap();
    let result = fixture
        .manager
        .search(
            fixture.store.clone(),
            fixture.search("ERROR", MatchMode::Fixed, SearchEngine::Auto),
            None,
        )
        .await
        .unwrap();
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.contains("测试来源")));
    let read = fixture
        .manager
        .read(
            fixture.store.clone(),
            LogReadRequest {
                snapshot_id: "incident".into(),
                file_id: 1,
                line_number: Some(2),
                before: 0,
                after: 0,
                byte_offset: None,
                max_bytes: 64 * 1024,
                max_line_chars: 2000,
            },
        )
        .await
        .unwrap();
    assert!(read
        .warnings
        .iter()
        .any(|warning| warning.contains("测试来源")));
}
