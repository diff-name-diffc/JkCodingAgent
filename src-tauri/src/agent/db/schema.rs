//! 数据库 schema 初始化与版本管理（PRAGMA user_version 方案）。
//!
//! 当前基线为 **v3**（历史 v0→v33 迁移链已按产品决策清除）。`init()` 的
//! 路径：同版本库直接复用；低于基线但存在迁移块的版本逐级前向迁移
//! （当前为 v1→v2、v2→v3）；再早的旧开发库一律拒绝打开（提示运行
//! `scripts/reset-dev-data.sh`）；user_version=0 且无表则按基线全新建库。
//!
//! ## 后续 schema 变更规范（详见 AGENTS.md「存储 schema 迁移」）
//!
//! 1. 更新本文件的基线 DDL，使新装库直接得到新形态；
//! 2. 递增 `SCHEMA_VERSION`，并在 `init()` 的迁移挂载点追加
//!    `if current_version < N` 的事务块（DDL/回填 + `user_version` 推进同事务）；
//! 3. 迁移块必须幂等、可重试；破坏性变更（DROP/清空）前先做整库快照备份。

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use super::util::now;
use super::DispatcherDb;

/// 当前 schema 版本号（PRAGMA user_version）。
///
/// v3：dispatcher_settings 新增 `theme` 列，外观主题偏好并入
/// `AhaSettingsV2`（原存 app_config 的 `app_settings` 键，迁移时搬移）。
/// v2：chat_images 的 message_id 改为可空（工具生成图先登记后关联消息）、
/// 删除从未写入的 vector_embedding_json / text_description 两列。
/// v1 开发库经 `migrate_v1_to_v2` 前向迁移；更早的旧开发库仍报错引导
/// `scripts/reset-dev-data.sh`。
pub(crate) const SCHEMA_VERSION: i32 = 3;

impl DispatcherDb {
    pub(super) fn init(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create db directory {}", parent.display()))?;
        }
        // 连接池 with_init 已逐连接设置 WAL / busy_timeout / foreign_keys=ON，
        // 这里无需重复声明 PRAGMA。
        let mut conn = self.conn()?;

        // 读取失败说明数据库文件损坏 / IO 异常：不能按 0（全新库）处理，
        // 否则会在损坏文件上继续建表，掩盖真实故障。
        let current_version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .context("read PRAGMA user_version（数据库可能已损坏）")?;

        if current_version == SCHEMA_VERSION {
            return Ok(());
        }
        if current_version > SCHEMA_VERSION {
            // 版本号更高有两种来源：应用降级打开了新版写入的库，或（当前开发阶段
            // 的实际情况）schema 基线重置前的旧开发库——其 user_version 落在 2..=33，
            // 迁移链已清除，无法识别也无法迁移。两种情况都不能继续读写：轻则查询
            // 报错、重则写入不兼容数据损坏库。
            anyhow::bail!(
                "数据库版本({current_version})与当前基线版本({SCHEMA_VERSION})不兼容。\
                 若为 schema 重置前的旧开发库，请退出应用并运行 scripts/reset-dev-data.sh 重置开发数据；\
                 否则请使用写入该数据库的更新版本应用打开。"
            );
        }
        if current_version == 0 {
            // user_version=0 但库里已有任何表 → 迁移链清除前的旧开发库（v1 之前
            // 的版本号都大于 0，正常旧库会命中上方的版本比较分支）。检查「任何表」
            // 而非单看 dispatcher_sessions：残缺旧库可能缺核心表但残留其它表，
            // 若误判为全新库，CREATE TABLE IF NOT EXISTS 不会清理残留，会形成
            // 新旧 schema 混合。
            let table_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type='table' AND name NOT LIKE 'sqlite_%'",
                    [],
                    |row| row.get(0),
                )
                .context("inspect sqlite_master（数据库可能已损坏）")?;
            if table_count > 0 {
                anyhow::bail!(
                    "检测到 schema 重置前的旧开发数据库（user_version=0 但已有数据表），无法自动迁移。\
                     请退出应用并运行 scripts/reset-dev-data.sh 重置开发数据后重试。"
                );
            }
            create_baseline(&mut conn)?;
            return Ok(());
        }

        // 0 < current_version < SCHEMA_VERSION：前向迁移块按版本递增挂载。
        if current_version < 2 {
            self.migrate_v1_to_v2(&mut conn)?;
        }
        if current_version < 3 {
            self.migrate_v2_to_v3(&mut conn)?;
            return Ok(());
        }

        anyhow::bail!(
            "数据库版本({current_version})低于当前基线版本({SCHEMA_VERSION})且尚无对应迁移路径；\
             开发阶段请运行 scripts/reset-dev-data.sh 重置开发数据。"
        )
    }

    /// v1 → v2：chat_images 的 message_id 改为可空（工具生成图先登记后
    /// 关联消息）、删除从未写入的 vector_embedding_json / text_description。
    ///
    /// 事务内重建表 + `user_version` 推进，失败整体回滚、幂等可重试；
    /// 数据经 INSERT SELECT 全量保留。破坏性 DDL（DROP TABLE）前按规范
    /// 先做整库快照备份（VACUUM INTO，不能在事务内执行）；备份失败只留痕
    /// 不阻断——迁移本身不丢数据。
    fn migrate_v1_to_v2(&self, conn: &mut Connection) -> Result<()> {
        let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
        let file_stem = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("jkbot.sqlite3");
        let backup_path = self
            .path
            .with_file_name(format!("{file_stem}.pre-v2-backup-{stamp}"));
        if let Err(error) = conn.execute(
            "VACUUM INTO ?1",
            params![backup_path.to_string_lossy().to_string()],
        ) {
            eprintln!(
                "v1→v2 迁移前整库快照失败（迁移经 INSERT SELECT 保留全部数据，继续）：{error}"
            );
        }

        let tx = conn
            .transaction()
            .context("begin v1→v2 migration transaction")?;
        tx.execute_batch(
            "CREATE TABLE chat_images_v2 (
                id TEXT PRIMARY KEY,
                image_id TEXT NOT NULL UNIQUE,
                workspace_id TEXT NOT NULL,
                message_id TEXT,
                segment_index INTEGER NOT NULL DEFAULT 0,
                path TEXT NOT NULL,
                alt TEXT,
                width INTEGER,
                height INTEGER,
                mime_type TEXT,
                source TEXT,
                generation_prompt TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY (message_id) REFERENCES dispatcher_messages(id) ON DELETE CASCADE
            );
            INSERT INTO chat_images_v2 (
                id, image_id, workspace_id, message_id, segment_index, path, alt,
                width, height, mime_type, source, generation_prompt, created_at
            )
            SELECT id, image_id, workspace_id, message_id, segment_index, path, alt,
                   width, height, mime_type, source, generation_prompt, created_at
            FROM chat_images;
            DROP TABLE chat_images;
            ALTER TABLE chat_images_v2 RENAME TO chat_images;
            CREATE INDEX IF NOT EXISTS idx_chat_images_workspace
            ON chat_images(workspace_id, created_at);
            CREATE INDEX IF NOT EXISTS idx_chat_images_message
            ON chat_images(message_id);",
        )
        .context("migrate chat_images to v2 shape")?;
        tx.pragma_update(None, "user_version", 2)
            .context("advance user_version to 2")?;
        tx.commit().context("commit v1→v2 migration")
    }

    /// v2 → v3：dispatcher_settings 新增 `theme` 列，并把旧存于
    /// app_config `app_settings` 键的外观主题搬进 `AhaSettingsV2.theme`，
    /// 设置统一走单一命令面（aha_get/save_settings_v2）。
    ///
    /// 事务内 ADD COLUMN + 主题回填 + 删除旧键 + `user_version` 推进，
    /// 失败整体回滚、幂等可重试。迁移会删除 app_config 行（数据已搬移），
    /// 按规范先做整库快照备份（VACUUM INTO，不能在事务内执行）；
    /// 备份失败只留痕不阻断——迁移本身保留全部用户数据。
    fn migrate_v2_to_v3(&self, conn: &mut Connection) -> Result<()> {
        let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
        let file_stem = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("jkbot.sqlite3");
        let backup_path = self
            .path
            .with_file_name(format!("{file_stem}.pre-v3-backup-{stamp}"));
        if let Err(error) = conn.execute(
            "VACUUM INTO ?1",
            params![backup_path.to_string_lossy().to_string()],
        ) {
            eprintln!(
                "v2→v3 迁移前整库快照失败（迁移仅搬移主题数据、不丢失用户数据，继续）：{error}"
            );
        }

        let tx = conn
            .transaction()
            .context("begin v2→v3 migration transaction")?;
        tx.execute_batch(
            "ALTER TABLE dispatcher_settings
             ADD COLUMN theme TEXT NOT NULL DEFAULT 'system';",
        )
        .context("add dispatcher_settings.theme column")?;

        // 旧主题搬移：app_config `app_settings` 键形如 {"theme":"dark"}。
        // 仅接受合法偏好值；dispatcher_settings 行可能尚不存在（用户只改过
        // 主题、未触发过智能体设置保存），用 upsert 兜底建默认行。
        let legacy: Option<String> = tx
            .query_row(
                "SELECT value_json FROM app_config WHERE key = 'app_settings'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("read legacy app_settings key")?;
        if let Some(raw) = legacy {
            let theme = serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|value| {
                    value
                        .get("theme")?
                        .as_str()
                        .map(str::trim)
                        .map(str::to_string)
                })
                .filter(|value| matches!(value.as_str(), "system" | "light" | "dark"));
            if let Some(theme) = theme {
                tx.execute(
                    "INSERT INTO dispatcher_settings (id, theme) VALUES ('default', ?1)
                     ON CONFLICT(id) DO UPDATE SET theme = ?1",
                    params![theme],
                )
                .context("carry legacy theme into dispatcher_settings")?;
            }
            tx.execute("DELETE FROM app_config WHERE key = 'app_settings'", [])
                .context("drop legacy app_settings key")?;
        }

        tx.pragma_update(None, "user_version", 3)
            .context("advance user_version to 3")?;
        tx.commit().context("commit v2→v3 migration")
    }
}

/// 全新建库：单事务内执行基线 DDL + 领域建表助手 + 内置种子数据，
/// 并把 user_version 推进到 SCHEMA_VERSION（与建表同事务，失败整体回滚）。
fn create_baseline(conn: &mut Connection) -> Result<()> {
    let tx = conn
        .transaction()
        .context("begin baseline schema transaction")?;
    tx.execute_batch(BASELINE_DDL)
        .context("initialize baseline schema")?;

    ensure_chat_categories_table_tx(&tx)?;
    ensure_chat_category_agent_configs_table_tx(&tx)?;
    backfill_chat_category_agent_configs_tx(&tx)?;

    crate::agent::sub_agent::db::ensure_sub_agent_tables_tx(&tx)?;
    crate::agent::sub_agent::db::ensure_sub_agent_trace_table_tx(&tx)?;
    crate::agent::sub_agent::db::seed_browser_agent_if_missing_tx(&tx)?;

    crate::ssh_tool::db::ensure_ssh_tables_tx(&tx).context("create ssh global tables")?;
    super::projects::ensure_projects_table_tx(&tx)?;
    super::mcp_servers::ensure_mcp_servers_table_tx(&tx)?;
    super::app_config::ensure_app_config_table_tx(&tx)?;

    tx.pragma_update(None, "user_version", SCHEMA_VERSION)
        .context("set baseline user_version")?;
    tx.commit().context("commit baseline schema")
}

/// 基线 DDL：核心表按「当前最终形态」一次性建齐。领域模块自管的表
/// （sub_agent / ssh / projects / mcp_servers / app_config）由各自的
/// `ensure_*_tx` 助手在 `create_baseline` 中补齐，DDL 保持单一出处。
const BASELINE_DDL: &str = "
CREATE TABLE IF NOT EXISTS dispatcher_sessions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT 'project',
    title TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_dispatcher_sessions_project
ON dispatcher_sessions(project_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_dispatcher_sessions_project_kind
ON dispatcher_sessions(project_id, kind, updated_at DESC);

CREATE TABLE IF NOT EXISTS dispatcher_messages (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    role TEXT NOT NULL,
    segments_json TEXT NOT NULL DEFAULT '[]',
    thinking_content TEXT,
    thinking_elapsed_ms INTEGER,
    context_payload TEXT,
    tool_call_id TEXT,
    tool_name TEXT,
    tool_result_mode TEXT,
    tool_artifacts_json TEXT,
    tool_calls_json TEXT,
    usage_stats_json TEXT,
    visible INTEGER NOT NULL DEFAULT 1,
    context_cleared INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_dispatcher_messages_workspace_created
ON dispatcher_messages(workspace_id, created_at);
CREATE INDEX IF NOT EXISTS idx_dispatcher_messages_workspace_context_role
ON dispatcher_messages(workspace_id, context_cleared, role, created_at);

CREATE TABLE IF NOT EXISTS chat_images (
    id TEXT PRIMARY KEY,
    image_id TEXT NOT NULL UNIQUE,
    workspace_id TEXT NOT NULL,
    message_id TEXT,
    segment_index INTEGER NOT NULL DEFAULT 0,
    path TEXT NOT NULL,
    alt TEXT,
    width INTEGER,
    height INTEGER,
    mime_type TEXT,
    source TEXT,
    generation_prompt TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (message_id) REFERENCES dispatcher_messages(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_chat_images_workspace
ON chat_images(workspace_id, created_at);
CREATE INDEX IF NOT EXISTS idx_chat_images_message
ON chat_images(message_id);

CREATE TABLE IF NOT EXISTS dispatcher_tool_runs (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    tool_call_id TEXT NOT NULL,
    parent_run_id TEXT,
    origin TEXT NOT NULL DEFAULT 'model' CHECK(length(trim(origin)) > 0),
    step_id TEXT,
    sequence INTEGER NOT NULL DEFAULT 0 CHECK(sequence >= 0),
    tool_name TEXT NOT NULL,
    provider TEXT NOT NULL,
    category TEXT NOT NULL,
    status TEXT NOT NULL,
    arguments_json TEXT NOT NULL DEFAULT '{}',
    effective_arguments_json TEXT NOT NULL DEFAULT '{}',
    result_mode TEXT,
    message_id TEXT,
    error_kind TEXT,
    error_message TEXT,
    action_kind TEXT,
    started_at TEXT,
    finished_at TEXT,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (parent_run_id) REFERENCES dispatcher_tool_runs(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_runs_workspace_created
ON dispatcher_tool_runs(workspace_id, created_at);
CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_runs_call
ON dispatcher_tool_runs(workspace_id, tool_call_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_dispatcher_tool_runs_parent_sequence
ON dispatcher_tool_runs(parent_run_id, sequence)
WHERE parent_run_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS dispatcher_tool_artifacts (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    message_id TEXT,
    tool_call_id TEXT,
    tool_run_id TEXT,
    tool_name TEXT,
    title TEXT NOT NULL,
    kind TEXT NOT NULL,
    preview TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL,
    char_count INTEGER NOT NULL DEFAULT 0,
    line_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    FOREIGN KEY (tool_run_id) REFERENCES dispatcher_tool_runs(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_artifacts_workspace_created
ON dispatcher_tool_artifacts(workspace_id, created_at);
CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_artifacts_message
ON dispatcher_tool_artifacts(message_id);
CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_artifacts_run
ON dispatcher_tool_artifacts(tool_run_id, created_at);

CREATE TABLE IF NOT EXISTS dispatcher_settings (
    id TEXT PRIMARY KEY DEFAULT 'default',
    shared_vision_model_configs_json TEXT NOT NULL DEFAULT '[]',
    shared_image_model_configs_json TEXT NOT NULL DEFAULT '[]',
    shared_image_edit_model_configs_json TEXT NOT NULL DEFAULT '[]',
    shared_asr_model_configs_json TEXT NOT NULL DEFAULT '[]',
    shared_tts_model_configs_json TEXT NOT NULL DEFAULT '[]',
    shared_embedding_model_configs_json TEXT NOT NULL DEFAULT '[]',
    project_chat_model_configs_json TEXT NOT NULL DEFAULT '[]',
    project_summary_model_configs_json TEXT NOT NULL DEFAULT '[]',
    project_allowed_tools_json TEXT NOT NULL DEFAULT '[]',
    chat_agent_chat_model_configs_json TEXT NOT NULL DEFAULT '[]',
    chat_agent_summary_model_configs_json TEXT NOT NULL DEFAULT '[]',
    chat_agent_allowed_tools_json TEXT NOT NULL DEFAULT '[]',
    context_debug INTEGER NOT NULL DEFAULT 0,
    review_model_config_json TEXT NOT NULL DEFAULT '',
    review_system_prompt TEXT NOT NULL DEFAULT '',
    model_library_json TEXT NOT NULL DEFAULT '[]',
    graph_execution_config_json TEXT NOT NULL DEFAULT '{}',
    theme TEXT NOT NULL DEFAULT 'system'
);

CREATE TABLE IF NOT EXISTS dispatcher_session_token_usage (
    workspace_id TEXT NOT NULL,
    model TEXT NOT NULL,
    source_kind TEXT NOT NULL DEFAULT 'primary',
    prompt_tokens INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    cached_tokens INTEGER NOT NULL DEFAULT 0,
    context_window_tokens INTEGER NOT NULL DEFAULT 0,
    context_window_capacity INTEGER NOT NULL DEFAULT 1000000,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, model, source_kind)
);
CREATE INDEX IF NOT EXISTS idx_dispatcher_session_token_usage_workspace_updated
ON dispatcher_session_token_usage(workspace_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS python_code_runs (
    run_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    code_block_index INTEGER NOT NULL,
    code_hash TEXT NOT NULL,
    code TEXT NOT NULL,
    status TEXT NOT NULL,
    stdout TEXT NOT NULL DEFAULT '',
    stderr TEXT NOT NULL DEFAULT '',
    installed_packages_json TEXT NOT NULL DEFAULT '[]',
    tool_events_json TEXT NOT NULL DEFAULT '[]',
    explanation_markdown TEXT NOT NULL DEFAULT '',
    error_reason TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, message_id, code_block_index),
    FOREIGN KEY (message_id) REFERENCES dispatcher_messages(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_python_code_runs_workspace_updated
ON python_code_runs(workspace_id, updated_at DESC);

-- 会话读模型：dispatcher_sessions 是统一锚点（消息/关键词/轨迹的外键目标），
-- chat_sessions / project_sessions 为两类会话的列表读模型，写入路径双写。
CREATE TABLE IF NOT EXISTS chat_sessions (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT 'tech',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_chat_sessions_updated
ON chat_sessions(updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_chat_sessions_category_updated
ON chat_sessions(category, updated_at DESC);

CREATE TABLE IF NOT EXISTS project_sessions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    title TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_project_sessions_project_updated
ON project_sessions(project_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS session_keywords (
    session_id TEXT NOT NULL,
    keyword TEXT NOT NULL,
    weight REAL NOT NULL DEFAULT 1.0,
    created_at TEXT NOT NULL,
    PRIMARY KEY (session_id, keyword),
    FOREIGN KEY (session_id) REFERENCES dispatcher_sessions(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_session_keywords_session
ON session_keywords(session_id, weight DESC);
CREATE INDEX IF NOT EXISTS idx_session_keywords_keyword
ON session_keywords(keyword);

-- 执行图（PI graph v3：run/attempt 模型 + 验收回执 + 继承快照）。
CREATE TABLE IF NOT EXISTS graph_plans (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT NOT NULL DEFAULT '',
    definition_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'draft',
    state_json TEXT NOT NULL DEFAULT '{}',
    initial_state_json TEXT NOT NULL DEFAULT '{}',
    requirement TEXT NOT NULL DEFAULT '',
    inherits_plan_id TEXT,
    inherits_run_id TEXT,
    latest_run_id TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_graph_plans_ws
ON graph_plans(workspace_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS graph_runs (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    attempt_no INTEGER NOT NULL,
    status TEXT NOT NULL,
    mode TEXT NOT NULL DEFAULT 'full',
    verdict_status TEXT NOT NULL DEFAULT '',
    verdict_reason TEXT NOT NULL DEFAULT '',
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    UNIQUE(plan_id, attempt_no),
    FOREIGN KEY(plan_id) REFERENCES graph_plans(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_graph_runs_plan
ON graph_runs(plan_id, attempt_no DESC);

CREATE TABLE IF NOT EXISTS graph_node_runs (
    run_id TEXT NOT NULL,
    plan_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    phase TEXT NOT NULL DEFAULT 'starting',
    model_ref TEXT NOT NULL,
    model_label TEXT NOT NULL,
    model_category TEXT NOT NULL,
    base_tool_group TEXT NOT NULL,
    special_tools_json TEXT NOT NULL DEFAULT '[]',
    input_text TEXT NOT NULL DEFAULT '',
    output_text TEXT NOT NULL DEFAULT '',
    error_text TEXT,
    started_at INTEGER,
    finished_at INTEGER,
    duration_ms INTEGER,
    usage_json TEXT NOT NULL DEFAULT '{}',
    affected_files_json TEXT NOT NULL DEFAULT '[]',
    tool_call_count INTEGER NOT NULL DEFAULT 0,
    retry_count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(run_id, node_id),
    FOREIGN KEY(run_id) REFERENCES graph_runs(id) ON DELETE CASCADE,
    FOREIGN KEY(plan_id) REFERENCES graph_plans(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_graph_node_runs_plan
ON graph_node_runs(plan_id);

CREATE TABLE IF NOT EXISTS graph_node_activities (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    kind TEXT NOT NULL,
    status TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL DEFAULT '',
    payload_json TEXT NOT NULL DEFAULT '{}',
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    UNIQUE(run_id, node_id, sequence),
    FOREIGN KEY(run_id) REFERENCES graph_runs(id) ON DELETE CASCADE
);
";

fn ensure_chat_categories_table_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS chat_categories (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            icon TEXT NOT NULL DEFAULT '',
            color TEXT NOT NULL DEFAULT '',
            sort_order INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        ",
    )
    .context("create chat_categories table")?;

    let ts = now();
    let defaults: Vec<(&str, &str, &str, &str, i32)> = vec![
        ("general", "综合", "MessageSquare", "", 0),
        ("life", "生活", "Heart", "#F43F5E", 1),
        ("work", "工作", "Briefcase", "#3B82F6", 2),
        ("tech", "技术", "Code2", "#8B5CF6", 3),
        ("learning", "学习", "GraduationCap", "#22C55E", 4),
    ];
    let mut stmt = tx
        .prepare(
            "INSERT OR IGNORE INTO chat_categories (id, name, icon, color, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .context("prepare seed categories statement")?;
    for (id, name, icon, color, sort_order) in defaults {
        stmt.execute(params![id, name, icon, color, sort_order, &ts, &ts])
            .with_context(|| format!("seed chat category {id}"))?;
    }

    Ok(())
}

pub(super) fn ensure_chat_category_agent_configs_table_tx(
    tx: &rusqlite::Transaction<'_>,
) -> Result<()> {
    tx.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS chat_category_agent_configs (
            category_id TEXT PRIMARY KEY,
            allowed_tools_json TEXT NOT NULL DEFAULT '[]',
            system_prompt TEXT NOT NULL DEFAULT '',
            sub_agent_ids_json TEXT NOT NULL DEFAULT '[]',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (category_id) REFERENCES chat_categories(id) ON DELETE CASCADE
        );
        ",
    )
    .context("create chat_category_agent_configs table")
}

pub(super) fn backfill_chat_category_agent_configs_tx(
    tx: &rusqlite::Transaction<'_>,
) -> Result<()> {
    let ts = now();
    let mut stmt = tx
        .prepare("SELECT id FROM chat_categories")
        .context("prepare chat categories for config backfill")?;
    let category_ids = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("load chat categories for config backfill")?;
    drop(stmt);

    for category_id in category_ids {
        let (allowed_tools_json, system_prompt) =
            default_chat_category_agent_config_tx(tx, &category_id)?;
        tx.execute(
            "
            INSERT OR IGNORE INTO chat_category_agent_configs (
                category_id,
                allowed_tools_json,
                system_prompt,
                sub_agent_ids_json,
                created_at,
                updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?5)
            ",
            params![category_id, allowed_tools_json, system_prompt, "[]", ts],
        )
        .with_context(|| format!("backfill chat category agent config {category_id}"))?;
    }
    Ok(())
}

pub(super) fn default_chat_category_agent_config_tx(
    tx: &rusqlite::Transaction<'_>,
    category_id: &str,
) -> Result<(String, String)> {
    let row = tx
        .query_row(
            "
            SELECT chat_agent_allowed_tools_json
            FROM dispatcher_settings
            WHERE id = 'default'
            ",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .context("load default chat agent config")?;

    let allowed_tools_json = row.unwrap_or_else(|| "[]".to_string());
    if let Some(default) = scenario_chat_category_agent_config(category_id) {
        return Ok((
            serde_json::to_string(default.tools)
                .context("serialize scenario chat category tools")?,
            default.system_prompt.to_string(),
        ));
    }

    Ok((
        allowed_tools_json,
        crate::agent::config::DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT.to_string(),
    ))
}

struct ScenarioChatCategoryAgentConfig {
    tools: &'static [&'static str],
    system_prompt: &'static str,
}

fn scenario_chat_category_agent_config(
    category_id: &str,
) -> Option<ScenarioChatCategoryAgentConfig> {
    match category_id {
        "general" => Some(ScenarioChatCategoryAgentConfig {
            tools: &[
                "browser_open_url",
                "browser_read_text",
                "browser_close",
                "list_sub_agents",
                "call_sub_agent",
            ],
            system_prompt: r#"# 综合聊天

你是一个高信息密度的通用助手，适合处理日常问答、快速判断、文本整理和轻量信息检索。

工作方式：
- 先直接回答用户问题；缺少事实依据时再使用浏览器读取公开信息。
- 回答保持简洁、清晰、可执行，默认使用简体中文。
- 不主动执行本地命令；除非用户明确要求或问题需要本地验证。
- 如任务明显属于专业领域，可先列出可用子智能体，再选择合适的子智能体协助。
"#,
        }),
        "life" => Some(ScenarioChatCategoryAgentConfig {
            tools: &[
                "browser_open_url",
                "browser_read_text",
                "browser_visual_analyze",
                "browser_close",
            ],
            system_prompt: r#"# 生活助理

你是面向生活场景的助理，适合规划、比较、行程、消费决策、健康常识和日常文本处理。

工作方式：
- 对会影响时间、金钱或安全的建议，优先检索当前信息并说明依据。
- 给出选择时用清晰的取舍标准，而不是堆砌选项。
- 遇到医疗、法律、金融等高风险问题时，只做信息整理和风险提示，不替代专业意见。
- 输出务实、温和、简洁，默认使用简体中文。
"#,
        }),
        "work" => Some(ScenarioChatCategoryAgentConfig {
            tools: &[
                "browser_open_url",
                "browser_read_text",
                "browser_click",
                "browser_type",
                "browser_press",
                "browser_wait_for",
                "browser_close",
                "local_zsh",
                "ssh_list_servers",
            ],
            system_prompt: r#"# 工作助理

你是面向工作流的执行型助理，适合处理资料整理、流程推进、网页操作、轻量自动化和远程环境巡检。

工作方式：
- 先明确目标、约束和交付物，再选择工具。
- 使用浏览器工具时遵循 ref 流程：先 browser_read_text，再基于 ref 点击、输入或等待。
- 本地命令仅在 .jkcodingagent/local_env/zsh 中执行，命令要短小、可审计，避免高风险操作。
- SSH 默认只做服务器列表和只读巡检；执行变更前必须说明影响并等待用户确认。
- 默认使用简体中文，输出结论优先。
"#,
        }),
        "tech" => Some(ScenarioChatCategoryAgentConfig {
            tools: &[
                "local_zsh",
                "browser_open_url",
                "browser_read_text",
                "browser_click",
                "browser_type",
                "browser_press",
                "browser_wait_for",
                "browser_visual_analyze",
                "browser_close",
                "ssh_list_servers",
                "ssh_exec",
                "list_sub_agents",
                "call_sub_agent",
            ],
            system_prompt: r#"# 技术助手

你是面向工程问题的技术助手，适合排查错误、解释代码、验证命令、阅读文档和推进技术方案。

工作方式：
- 事实优先，必要时用浏览器查看官方文档或公开资料；不要编造 API、参数或版本信息。
- 本地验证优先使用 local_zsh，并保持命令小步、可复现、可审计。
- 远程命令必须先确认目标服务器；涉及写入、删除、部署、重启等操作前必须说明影响并等待用户确认。
- 对复杂任务先拆解，再给出可执行步骤；回答默认简体中文，保持工程化、直接、少废话。
"#,
        }),
        "learning" => Some(ScenarioChatCategoryAgentConfig {
            tools: &[
                "browser_open_url",
                "browser_read_text",
                "browser_visual_analyze",
                "browser_close",
                "local_zsh",
            ],
            system_prompt: r#"# 学习教练

你是学习型助手，适合讲解概念、制定学习路径、做题辅导、资料检索和小实验验证。

工作方式：
- 先判断用户当前水平，再用递进方式解释：直觉、例子、关键细节、练习。
- 复杂概念要拆成短段落，并给出可验证的小任务。
- 需要最新资料时优先用浏览器读取可信来源；需要演示时可用 local_zsh 做小实验。
- 不替用户跳过思考：给答案，也给判断依据和可迁移的方法。
"#,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::DispatcherDb;

    fn temp_db_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("aha-schema-{tag}-{}.sqlite3", uuid::Uuid::new_v4()))
    }

    fn cleanup_db_files(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    /// 数据库版本高于当前基线（降级安装，或 schema 重置前的旧开发库）时
    /// 必须拒绝打开，而不是静默继续。
    #[test]
    fn newer_database_version_is_rejected() {
        let path = temp_db_path("newer-version");
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
            message.contains("reset-dev-data.sh"),
            "错误信息应引导重置开发数据：{message}"
        );
        cleanup_db_files(&path);
    }

    /// 迁移链清除前的旧开发库（user_version=0 但已有核心表）必须拒绝打开，
    /// 错误信息引导运行重置脚本，而不是在旧表上继续建库。
    #[test]
    fn legacy_pre_baseline_database_is_rejected() {
        let path = temp_db_path("legacy-pre-baseline");
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
            message.contains("reset-dev-data"),
            "错误信息应引导运行重置脚本：{message}"
        );
        cleanup_db_files(&path);
    }

    /// v1 库（旧形态 chat_images：message_id NOT NULL + 两个未用列）打开时
    /// 沿迁移链前向迁移到当前基线：chat_images 数据全量保留、message_id
    /// 可空、快照备份生成、重复打开幂等；随后 v2→v3 继续为
    /// dispatcher_settings 补 theme 列，并把旧 app_config 中的主题搬移进来。
    #[test]
    fn v1_database_migrates_through_chain_to_baseline() {
        let path = temp_db_path("v1-to-baseline");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.pragma_update(None, "foreign_keys", "ON").unwrap();
            // 与 v1 基线等价的最小表组：迁移链触及 chat_images /
            // dispatcher_settings / app_config，其余表形态由基线路径负责，
            // 这里只需要 FK 目标表存在。
            conn.execute_batch(
                "CREATE TABLE dispatcher_messages (
                    id TEXT PRIMARY KEY,
                    workspace_id TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );
                CREATE TABLE chat_images (
                    id TEXT PRIMARY KEY,
                    image_id TEXT NOT NULL UNIQUE,
                    workspace_id TEXT NOT NULL,
                    message_id TEXT NOT NULL,
                    segment_index INTEGER NOT NULL,
                    path TEXT NOT NULL,
                    alt TEXT,
                    width INTEGER,
                    height INTEGER,
                    mime_type TEXT,
                    source TEXT,
                    generation_prompt TEXT,
                    vector_embedding_json TEXT,
                    text_description TEXT,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY (message_id) REFERENCES dispatcher_messages(id) ON DELETE CASCADE
                );
                CREATE TABLE dispatcher_settings (
                    id TEXT PRIMARY KEY DEFAULT 'default',
                    shared_vision_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    shared_image_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    shared_image_edit_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    shared_asr_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    shared_tts_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    shared_embedding_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    project_chat_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    project_summary_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    project_allowed_tools_json TEXT NOT NULL DEFAULT '[]',
                    chat_agent_chat_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    chat_agent_summary_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    chat_agent_allowed_tools_json TEXT NOT NULL DEFAULT '[]',
                    context_debug INTEGER NOT NULL DEFAULT 0,
                    review_model_config_json TEXT NOT NULL DEFAULT '',
                    review_system_prompt TEXT NOT NULL DEFAULT '',
                    model_library_json TEXT NOT NULL DEFAULT '[]',
                    graph_execution_config_json TEXT NOT NULL DEFAULT '{}'
                );
                CREATE TABLE app_config (
                    key TEXT PRIMARY KEY,
                    value_json TEXT NOT NULL
                );
                INSERT INTO dispatcher_messages VALUES ('msg-1', 'ws-1', '2026-01-01T00:00:00Z');
                INSERT INTO chat_images (
                    id, image_id, workspace_id, message_id, segment_index, path,
                    vector_embedding_json, text_description, created_at
                ) VALUES (
                    'row-1', 'image-1', 'ws-1', 'msg-1', 0, '/tmp/a.png',
                    NULL, '旧描述', '2026-01-01T00:00:01Z'
                );
                INSERT INTO app_config VALUES ('app_settings', '{\"theme\":\"dark\"}');
                PRAGMA user_version = 1;",
            )
            .unwrap();
        }

        let db = DispatcherDb::new(path.clone()).unwrap();
        {
            let conn = db.conn().unwrap();
            let version: i32 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, super::SCHEMA_VERSION);

            // 数据保留，未用列已随表重建消失。
            let (image_id, message_id, legacy_column): (String, Option<String>, i64) = conn
                .query_row(
                    "SELECT image_id, message_id,
                            (SELECT COUNT(*) FROM pragma_table_info('chat_images')
                             WHERE name IN ('vector_embedding_json', 'text_description'))
                     FROM chat_images",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(image_id, "image-1");
            assert_eq!(message_id.as_deref(), Some("msg-1"));
            assert_eq!(legacy_column, 0, "未用列应随 v2 迁移移除");

            // v2→v3：旧主题搬移进 dispatcher_settings.theme，旧键删除。
            let theme: String = conn
                .query_row(
                    "SELECT theme FROM dispatcher_settings WHERE id = 'default'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(theme, "dark", "旧 app_config 主题应搬移进设置");
            let legacy_rows: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM app_config WHERE key = 'app_settings'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(legacy_rows, 0, "旧 app_settings 键应随迁移删除");
        }
        // Drop db（连接池）后重开验证幂等，并清理迁移快照。
        drop(db);
        let _ = DispatcherDb::new(path.clone());
        cleanup_db_files(&path);
        let dir = path.parent().unwrap();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.contains("v1-to-baseline-")
                && (name.contains("pre-v2-backup") || name.contains("pre-v3-backup"))
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// v2 库打开时迁移到 v3：dispatcher_settings 补 theme 列。旧
    /// app_config `app_settings` 键存在时主题搬移进既有行；不存在时落
    /// 默认值 `system`。
    #[test]
    fn v2_database_migrates_theme_into_settings() {
        let path = temp_db_path("v2-to-v3");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE dispatcher_settings (
                    id TEXT PRIMARY KEY DEFAULT 'default',
                    shared_vision_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    shared_image_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    shared_image_edit_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    shared_asr_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    shared_tts_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    shared_embedding_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    project_chat_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    project_summary_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    project_allowed_tools_json TEXT NOT NULL DEFAULT '[]',
                    chat_agent_chat_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    chat_agent_summary_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    chat_agent_allowed_tools_json TEXT NOT NULL DEFAULT '[]',
                    context_debug INTEGER NOT NULL DEFAULT 0,
                    review_model_config_json TEXT NOT NULL DEFAULT '',
                    review_system_prompt TEXT NOT NULL DEFAULT '',
                    model_library_json TEXT NOT NULL DEFAULT '[]',
                    graph_execution_config_json TEXT NOT NULL DEFAULT '{}'
                );
                CREATE TABLE app_config (
                    key TEXT PRIMARY KEY,
                    value_json TEXT NOT NULL
                );
                INSERT INTO dispatcher_settings (id, context_debug) VALUES ('default', 1);
                INSERT INTO app_config VALUES ('app_settings', '{\"theme\":\"light\"}');
                PRAGMA user_version = 2;",
            )
            .unwrap();
        }

        let db = DispatcherDb::new(path.clone()).unwrap();
        {
            let conn = db.conn().unwrap();
            let version: i32 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, 3);

            // 既有行保留原值，只追加 theme；旧键删除。
            let (theme, context_debug): (String, i64) = conn
                .query_row(
                    "SELECT theme, context_debug FROM dispatcher_settings WHERE id = 'default'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(theme, "light", "旧主题应搬移进既有设置行");
            assert_eq!(context_debug, 1, "迁移不应改动其它列");
            let legacy_rows: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM app_config WHERE key = 'app_settings'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(legacy_rows, 0);
        }
        drop(db);
        cleanup_db_files(&path);
        let dir = path.parent().unwrap();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.contains("v2-to-v3-") && name.contains("pre-v3-backup") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// v2 库没有旧主题键（全新安装后从未写过 app_settings）时，迁移补列
    /// 落默认值 `system`，不创建多余行。
    #[test]
    fn v2_database_without_legacy_theme_defaults_system() {
        let path = temp_db_path("v2-to-v3-no-legacy");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE dispatcher_settings (
                    id TEXT PRIMARY KEY DEFAULT 'default',
                    context_debug INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE app_config (
                    key TEXT PRIMARY KEY,
                    value_json TEXT NOT NULL
                );
                PRAGMA user_version = 2;",
            )
            .unwrap();
        }

        let db = DispatcherDb::new(path.clone()).unwrap();
        let conn = db.conn().unwrap();
        let theme_default: String = conn
            .query_row(
                "SELECT dflt_value FROM pragma_table_info('dispatcher_settings')
                 WHERE name = 'theme'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("theme 列应存在");
        assert_eq!(theme_default, "'system'", "新增列默认值应为 system");
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM dispatcher_settings", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0, "无旧主题时迁移不应创建设置行");
        drop(conn);
        drop(db);
        cleanup_db_files(&path);
        let dir = path.parent().unwrap();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.contains("v2-to-v3-no-legacy-") && name.contains("pre-v3-backup") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// 全新库一次建到基线形态：版本号、内置种子（聊天分类/场景配置/浏览器
    /// 子智能体）与工具运行追踪的外键齐备。
    #[test]
    fn fresh_database_creates_baseline_and_seeds() {
        let path = temp_db_path("fresh-baseline");
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
        ] {
            let sql = format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1");
            let exists: i64 = conn
                .query_row(&sql, rusqlite::params![column], |row| row.get(0))
                .unwrap();
            assert_eq!(exists, 1, "baseline must contain {table}.{column}");
        }
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
        cleanup_db_files(&path);
    }

    /// 同版本库重复打开走 fast path：不重复建表/种子，版本号不变。
    #[test]
    fn reopen_at_current_version_keeps_state() {
        let path = temp_db_path("reopen");
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
        cleanup_db_files(&path);
    }
}
