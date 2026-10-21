use super::super::DispatcherDb;

/// 带守卫的临时库路径：sqlite/wal/shm 都落在守卫目录内，用例结束整目录
/// 回收（取代旧的手工 remove_file 清理）。
fn temp_db(tag: &str) -> (std::path::PathBuf, crate::test_util::TempDirGuard) {
    let dir = crate::test_util::TempDirGuard::new(&format!("aha-schema-{tag}"));
    (dir.path().join("jkbot.sqlite3"), dir)
}

/// 数据库版本高于当前基线（降级安装，或 schema 重置前的旧开发库）时
/// 必须拒绝打开，而不是静默继续。
#[test]
fn newer_database_version_is_rejected() {
    let (path, _dir) = temp_db("newer-version");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", super::SCHEMA_VERSION + 1)
            .unwrap();
    }
    let result = DispatcherDb::new(path.clone());
    assert!(result.is_err(), "更高版本的数据库必须报错拒绝打开");
    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("不兼容"),
        "错误信息应说明版本不兼容：{message}"
    );
    assert!(
        message.contains("删除数据库文件"),
        "错误信息应引导删除数据库文件重建：{message}"
    );
}

/// 迁移链清除前的旧开发库（user_version=0 但已有核心表）必须拒绝打开，
/// 错误信息引导运行重置脚本，而不是在旧表上继续建库。
#[test]
fn legacy_pre_baseline_database_is_rejected() {
    let (path, _dir) = temp_db("legacy-pre-baseline");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE dispatcher_sessions (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                kind TEXT NOT NULL DEFAULT 'project',
                title TEXT NOT NULL,
                category TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );",
        )
        .unwrap();
    }
    let result = DispatcherDb::new(path.clone());
    assert!(result.is_err(), "旧开发库必须报错拒绝打开");
    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("删除数据库文件"),
        "错误信息应引导删除数据库文件重建：{message}"
    );
}

/// 全新库一次建到基线形态：版本号、内置种子（聊天分类/场景配置/浏览器
/// 子智能体）与工具运行追踪的外键齐备。
#[test]
fn fresh_database_creates_baseline_and_seeds() {
    let (path, _dir) = temp_db("fresh-baseline");
    let db = DispatcherDb::new(path.clone()).unwrap();
    let conn = db.conn().expect("db conn");

    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, super::SCHEMA_VERSION);

    let categories: i64 = conn
        .query_row("SELECT COUNT(*) FROM chat_categories", [], |row| row.get(0))
        .unwrap();
    assert_eq!(categories, 5, "内置聊天分类应全部种子");

    let configs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chat_category_agent_configs
             WHERE TRIM(system_prompt) != ''",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(configs, 5, "内置分类应带场景级提示词配置");

    let browser_agent: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sub_agents s
             JOIN global_sub_agents g ON g.sub_agent_id = s.id",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(browser_agent, 1, "浏览器子智能体应种子并全局启用");

    for (table, column) in [
        ("dispatcher_tool_runs", "parent_run_id"),
        ("dispatcher_tool_runs", "origin"),
        ("dispatcher_tool_runs", "step_id"),
        ("dispatcher_tool_runs", "sequence"),
        ("dispatcher_tool_artifacts", "tool_run_id"),
        ("chat_category_agent_configs", "project_directories_json"),
    ] {
        let sql = format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1");
        let exists: i64 = conn
            .query_row(&sql, rusqlite::params![column], |row| row.get(0))
            .unwrap();
        assert_eq!(exists, 1, "baseline must contain {table}.{column}");
    }
    let delivery_index: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index'
               AND name = 'idx_tool_completions_delivery'
               AND tbl_name = 'dispatcher_tool_completions'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(delivery_index, 1, "baseline must contain the v9 投递索引");
    let parent_fk: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_foreign_key_list('dispatcher_tool_runs')
             WHERE \"from\" = 'parent_run_id' AND \"table\" = 'dispatcher_tool_runs'
               AND on_delete = 'CASCADE'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(parent_fk, 1);
    let artifact_fk: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_foreign_key_list('dispatcher_tool_artifacts')
             WHERE \"from\" = 'tool_run_id' AND \"table\" = 'dispatcher_tool_runs'
               AND on_delete = 'CASCADE'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(artifact_fk, 1);

    drop(conn);
    drop(db);
}

#[test]
fn v17_upgrade_preserves_category_settings_and_defaults_directories_to_empty() {
    let (path, _dir) = temp_db("v18-upgrade");
    {
        let db = DispatcherDb::new(path.clone()).unwrap();
        let conn = db.conn().unwrap();
        conn.execute_batch(
            "ALTER TABLE chat_category_agent_configs DROP COLUMN project_directories_json;
             UPDATE chat_category_agent_configs SET system_prompt = '用户定制提示词' WHERE category_id = 'tech';
             PRAGMA user_version = 17;",
        )
        .unwrap();
    }
    let db = DispatcherDb::new(path.clone()).unwrap();
    let config = db
        .list_chat_category_agent_configs()
        .unwrap()
        .into_iter()
        .find(|config| config.category_id == "tech")
        .unwrap();
    assert_eq!(config.system_prompt, "用户定制提示词");
    assert!(config.project_directories.is_empty());
    assert_eq!(
        db.conn()
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
            .unwrap(),
        super::SCHEMA_VERSION
    );
}

#[test]
fn v18_migration_retries_without_overwriting_directories() {
    let (path, _dir) = temp_db("v18-retry");
    let db = DispatcherDb::new(path).unwrap();
    let mut conn = db.conn().unwrap();
    conn.execute(
        "UPDATE chat_category_agent_configs SET project_directories_json = '[\"/retained\"]'
         WHERE category_id = 'tech'",
        [],
    )
    .unwrap();
    let tx = conn.transaction().unwrap();
    super::migration_v18::migrate(&tx).unwrap();
    super::migration_v18::migrate(&tx).unwrap();
    tx.commit().unwrap();
    let value: String = conn
        .query_row(
            "SELECT project_directories_json FROM chat_category_agent_configs WHERE category_id = 'tech'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(value, "[\"/retained\"]");
}

/// v17 存量行：逐字等于冻结默认提示词的同步为新默认，用户定制行保持原样。
#[test]
fn v18_migration_syncs_only_untouched_default_prompt() {
    let (path, _dir) = temp_db("v18-prompt-sync");
    let db = DispatcherDb::new(path).unwrap();
    let mut conn = db.conn().unwrap();
    conn.execute(
        "UPDATE chat_category_agent_configs SET system_prompt = ?1 WHERE category_id = 'tech'",
        rusqlite::params![super::migration_v18::V17_DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT],
    )
    .unwrap();
    let custom = "用户定制提示词";
    conn.execute(
        "UPDATE chat_category_agent_configs SET system_prompt = ?1 WHERE category_id = 'general'",
        rusqlite::params![custom],
    )
    .unwrap();
    let tx = conn.transaction().unwrap();
    super::migration_v18::migrate(&tx).unwrap();
    // 幂等：重复迁移不应再次改动（定制行仍原样）。
    super::migration_v18::migrate(&tx).unwrap();
    tx.commit().unwrap();
    let prompt_of = |category_id: &str| -> String {
        conn.query_row(
            "SELECT system_prompt FROM chat_category_agent_configs WHERE category_id = ?1",
            rusqlite::params![category_id],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        prompt_of("tech"),
        crate::agent::config::DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT,
        "未定制的 v17 默认提示词应同步为新默认"
    );
    assert_eq!(prompt_of("general"), custom, "用户定制的提示词必须保持原样");
}

#[test]
fn current_schema_without_project_directories_is_rejected() {
    let (path, _dir) = temp_db("v18-shape");
    {
        let db = DispatcherDb::new(path.clone()).unwrap();
        db.conn()
            .unwrap()
            .execute_batch(
                "ALTER TABLE chat_category_agent_configs DROP COLUMN project_directories_json",
            )
            .unwrap();
    }
    let error = DispatcherDb::new(path).unwrap_err();
    assert!(error.to_string().contains("v18"), "{error:#}");
}

/// 同版本库重复打开走 fast path：不重复建表/种子，版本号不变。
#[test]
fn reopen_at_current_version_keeps_state() {
    let (path, _dir) = temp_db("reopen");
    {
        let db = DispatcherDb::new(path.clone()).unwrap();
        drop(db);
    }
    let db = DispatcherDb::new(path.clone()).unwrap();
    let conn = db.conn().expect("db conn");
    let categories: i64 = conn
        .query_row("SELECT COUNT(*) FROM chat_categories", [], |row| row.get(0))
        .unwrap();
    assert_eq!(categories, 5, "重复打开不得重复种子");
    drop(conn);
    drop(db);
}

/// 低于基线的旧版本库（v1→v15 迁移链已清除）必须拒绝打开并引导
/// 重置，而不是静默按新形态使用或误建；拒绝打开不得改动库内容。
#[test]
fn legacy_version_database_is_rejected() {
    let (path, _dir) = temp_db("legacy-version");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE workflow_plans (id TEXT PRIMARY KEY);
             PRAGMA user_version = 15;",
        )
        .unwrap();
    }
    let result = DispatcherDb::new(path.clone());
    assert!(result.is_err(), "低于基线的旧库必须报错拒绝打开");
    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("删除数据库文件"),
        "错误信息应引导删除数据库文件重建：{message}"
    );
    let conn = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
            .unwrap(),
        15,
        "拒绝打开不得改动库内容"
    );
    drop(conn);
}
