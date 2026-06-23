//! 数据库 schema 初始化与版本迁移（PRAGMA user_version 方案）。
//!
//! `init()` 负责建表与数据迁移；其余 `ensure_*` / `migrate_*` 为迁移助手，
//! 仅被 `init()` 调用，故保持模块私有。

use std::collections::HashSet;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use super::content::safe_absolute_image_path;
use super::util::now;
use super::DispatcherDb;

impl DispatcherDb {
    pub(super) fn init(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create db directory {}", parent.display()))?;
        }
        let mut conn = self.conn()?;

        // Fast path: if schema is already at the expected version, skip all DDL.
        const SCHEMA_VERSION: i32 = 12;
        let current_version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or(0);
        if current_version >= SCHEMA_VERSION {
            // Still need to set WAL mode on first open of each connection.
            conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
                .context("set pragmas")?;
            return Ok(());
        }

        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS dispatcher_sessions (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                kind TEXT NOT NULL DEFAULT 'project',
                title TEXT NOT NULL,
                mode TEXT NOT NULL DEFAULT 'default',
                active_plan_path TEXT,
                checklist_json TEXT,
                plan_interaction_json TEXT,
                category TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_sessions_project
            ON dispatcher_sessions(project_id, updated_at DESC);

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

            CREATE INDEX IF NOT EXISTS idx_chat_images_workspace
            ON chat_images(workspace_id, created_at);

            CREATE INDEX IF NOT EXISTS idx_chat_images_message
            ON chat_images(message_id);

            CREATE TABLE IF NOT EXISTS dispatcher_tool_artifacts (
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                message_id TEXT,
                tool_call_id TEXT,
                tool_name TEXT,
                title TEXT NOT NULL,
                kind TEXT NOT NULL,
                preview TEXT NOT NULL DEFAULT '',
                content TEXT NOT NULL,
                char_count INTEGER NOT NULL DEFAULT 0,
                line_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_artifacts_workspace_created
            ON dispatcher_tool_artifacts(workspace_id, created_at);

            CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_artifacts_message
            ON dispatcher_tool_artifacts(message_id);

            CREATE TABLE IF NOT EXISTS dispatcher_settings (
                id TEXT PRIMARY KEY DEFAULT 'default',
                api_base TEXT NOT NULL DEFAULT '',
                api_key TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL DEFAULT '',
                summary_model TEXT NOT NULL DEFAULT 'deepseek-v4-flash',
                vision_model TEXT NOT NULL DEFAULT '',
                asr_api_key TEXT NOT NULL DEFAULT '',
                asr_websocket_url TEXT NOT NULL DEFAULT '',
                auto_approve_dispatch INTEGER NOT NULL DEFAULT 0,
                context_debug INTEGER NOT NULL DEFAULT 0,
                image_model_url TEXT NOT NULL DEFAULT 'https://dashscope.aliyuncs.com/api/v1',
                image_model_api_key TEXT NOT NULL DEFAULT '',
                image_model TEXT NOT NULL DEFAULT 'qwen-image-2.0-pro',
                image_edit_model TEXT NOT NULL DEFAULT '',
                chat_model_url TEXT NOT NULL DEFAULT '',
                chat_model_api_key TEXT NOT NULL DEFAULT '',
                chat_model_name TEXT NOT NULL DEFAULT '',
                summary_model_url TEXT NOT NULL DEFAULT '',
                summary_model_api_key TEXT NOT NULL DEFAULT '',
                summary_model_name TEXT NOT NULL DEFAULT '',
                vision_model_url TEXT NOT NULL DEFAULT '',
                vision_model_api_key TEXT NOT NULL DEFAULT '',
                vision_model_name TEXT NOT NULL DEFAULT '',
                image_model_config_url TEXT NOT NULL DEFAULT 'https://dashscope.aliyuncs.com/api/v1',
                image_model_config_api_key TEXT NOT NULL DEFAULT '',
                image_model_config_name TEXT NOT NULL DEFAULT 'qwen-image-2.0-pro',
                image_edit_model_url TEXT NOT NULL DEFAULT 'https://dashscope.aliyuncs.com/api/v1',
                image_edit_model_api_key TEXT NOT NULL DEFAULT '',
                image_edit_model_name TEXT NOT NULL DEFAULT 'qwen-image-2.0-pro',
                asr_model_url TEXT NOT NULL DEFAULT '',
                asr_model_api_key TEXT NOT NULL DEFAULT '',
                asr_model_name TEXT NOT NULL DEFAULT 'fun-asr-realtime',
                tts_model_url TEXT NOT NULL DEFAULT '',
                tts_model_api_key TEXT NOT NULL DEFAULT '',
                tts_model_name TEXT NOT NULL DEFAULT '',
                embedding_model_url TEXT NOT NULL DEFAULT '',
                embedding_model_api_key TEXT NOT NULL DEFAULT '',
                embedding_model_name TEXT NOT NULL DEFAULT '',
                chat_model_configs_json TEXT NOT NULL DEFAULT '[]',
                summary_model_configs_json TEXT NOT NULL DEFAULT '[]',
                vision_model_configs_json TEXT NOT NULL DEFAULT '[]',
                image_model_configs_json TEXT NOT NULL DEFAULT '[]',
                image_edit_model_configs_json TEXT NOT NULL DEFAULT '[]',
                asr_model_configs_json TEXT NOT NULL DEFAULT '[]',
                tts_model_configs_json TEXT NOT NULL DEFAULT '[]',
                embedding_model_configs_json TEXT NOT NULL DEFAULT '[]',
                allowed_tools_json TEXT NOT NULL DEFAULT '[]'
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

            CREATE TABLE IF NOT EXISTS chat_categories (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                icon TEXT NOT NULL DEFAULT '',
                color TEXT NOT NULL DEFAULT '',
                sort_order INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sub_agents (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                description TEXT NOT NULL,
                config_json TEXT NOT NULL,
                enabled     INTEGER NOT NULL DEFAULT 1,
                created_at  INTEGER NOT NULL,
                updated_at  INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS session_sub_agents (
                session_id   TEXT NOT NULL,
                sub_agent_id TEXT NOT NULL,
                PRIMARY KEY (session_id, sub_agent_id),
                FOREIGN KEY (session_id) REFERENCES dispatcher_sessions(id) ON DELETE CASCADE,
                FOREIGN KEY (sub_agent_id) REFERENCES sub_agents(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS global_sub_agents (
                sub_agent_id TEXT PRIMARY KEY,
                FOREIGN KEY (sub_agent_id) REFERENCES sub_agents(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS context_sub_agents (
                context      TEXT NOT NULL,
                sub_agent_id TEXT NOT NULL,
                PRIMARY KEY (context, sub_agent_id),
                FOREIGN KEY (sub_agent_id) REFERENCES sub_agents(id) ON DELETE CASCADE
            );
            ",
        )
        .context("initialize dispatcher sqlite schema")?;

        // Run all column additions and data migrations in a single transaction
        // so that a failure midway rolls back cleanly instead of leaving partial schema.
        {
            let tx = conn.transaction().context("begin migration transaction")?;

            migrate_session_token_usage_primary_key_on_tx(&tx)?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "summary_model",
                "TEXT NOT NULL DEFAULT 'deepseek-v4-flash'",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "vision_model",
                "TEXT NOT NULL DEFAULT ''",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "asr_api_key",
                "TEXT NOT NULL DEFAULT ''",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "asr_websocket_url",
                "TEXT NOT NULL DEFAULT ''",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "context_debug",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "image_model_url",
                "TEXT NOT NULL DEFAULT 'https://dashscope.aliyuncs.com/api/v1'",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "image_model_api_key",
                "TEXT NOT NULL DEFAULT ''",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "image_model",
                "TEXT NOT NULL DEFAULT 'qwen-image-2.0-pro'",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "image_edit_model",
                "TEXT NOT NULL DEFAULT ''",
            )?;
            ensure_dispatcher_model_config_columns_tx(&tx)?;
            migrate_dispatcher_model_configs_tx(&tx)?;
            ensure_column_exists_tx(&tx, "dispatcher_messages", "context_payload", "TEXT")?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_messages",
                "segments_json",
                "TEXT NOT NULL DEFAULT '[]'",
            )?;
            ensure_column_exists_tx(&tx, "dispatcher_messages", "thinking_content", "TEXT")?;
            ensure_column_exists_tx(&tx, "dispatcher_messages", "thinking_elapsed_ms", "INTEGER")?;
            ensure_column_exists_tx(&tx, "dispatcher_messages", "tool_result_mode", "TEXT")?;
            ensure_column_exists_tx(&tx, "dispatcher_messages", "tool_artifacts_json", "TEXT")?;
            ensure_column_exists_tx(&tx, "dispatcher_messages", "usage_stats_json", "TEXT")?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_messages",
                "context_cleared",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_sessions",
                "kind",
                "TEXT NOT NULL DEFAULT 'project'",
            )?;
            tx.execute(
                "CREATE INDEX IF NOT EXISTS idx_dispatcher_sessions_project_kind
                 ON dispatcher_sessions(project_id, kind, updated_at DESC)",
                [],
            )
            .context("create dispatcher session project kind index")?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_sessions",
                "mode",
                "TEXT NOT NULL DEFAULT 'default'",
            )?;
            ensure_column_exists_tx(&tx, "dispatcher_sessions", "active_plan_path", "TEXT")?;
            ensure_column_exists_tx(&tx, "dispatcher_sessions", "checklist_json", "TEXT")?;
            ensure_column_exists_tx(&tx, "dispatcher_sessions", "plan_interaction_json", "TEXT")?;
            ensure_python_code_runs_table_tx(&tx)?;

            ensure_chat_categories_table_tx(&tx)?;

            ensure_column_exists_tx(
                &tx,
                "dispatcher_sessions",
                "category",
                "TEXT NOT NULL DEFAULT ''",
            )?;

            tx.execute(
                "UPDATE dispatcher_sessions SET category = 'tech'
                 WHERE kind = 'chat' AND (category IS NULL OR category = '')",
                [],
            )
            .context("migrate existing chat sessions to tech category")?;

            crate::agent::sub_agent::db::ensure_sub_agent_tables_tx(&tx)?;
            crate::agent::sub_agent::db::seed_browser_agent_if_missing_tx(&tx)?;

            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "allowed_tools_json",
                "TEXT NOT NULL DEFAULT '[]'",
            )?;

            tx.commit().context("commit migration transaction")?;
        }

        // v5 → v6: split dispatcher_sessions → chat_sessions + project_sessions,
        // delete all project-kind sessions, keep chat data only.
        if conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chat_sessions'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            == 0
        {
            let mut paths = HashSet::new();
            {
                let mut stmt = conn
                    .prepare(
                        "SELECT ci.path FROM chat_images ci
                         INNER JOIN dispatcher_sessions ds ON ci.workspace_id = ds.id
                         WHERE ds.kind = 'project'",
                    )
                    .context("v6: load project image paths")?;
                let indexed = stmt
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .unwrap_or_default();
                paths.extend(indexed);
            }
            conn.execute_batch(
                "
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
                    mode TEXT NOT NULL DEFAULT 'default',
                    active_plan_path TEXT,
                    checklist_json TEXT,
                    plan_interaction_json TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_project_sessions_project_updated
                    ON project_sessions(project_id, updated_at DESC);

                INSERT OR IGNORE INTO chat_sessions (id, title, category, created_at, updated_at)
                SELECT id, title, category, created_at, updated_at
                FROM dispatcher_sessions
                WHERE kind = 'chat';

                DELETE FROM dispatcher_tool_artifacts
                    WHERE workspace_id IN (SELECT id FROM dispatcher_sessions WHERE kind = 'project');
                DELETE FROM dispatcher_session_token_usage
                    WHERE workspace_id IN (SELECT id FROM dispatcher_sessions WHERE kind = 'project');
                DELETE FROM python_code_runs
                    WHERE workspace_id IN (SELECT id FROM dispatcher_sessions WHERE kind = 'project');
                DELETE FROM chat_images
                    WHERE workspace_id IN (SELECT id FROM dispatcher_sessions WHERE kind = 'project');
                DELETE FROM dispatcher_messages
                    WHERE workspace_id IN (SELECT id FROM dispatcher_sessions WHERE kind = 'project');
                DELETE FROM dispatcher_sessions WHERE kind = 'project';

                UPDATE chat_sessions SET category = 'tech'
                    WHERE category IS NULL OR category = '';
                ",
            )
            .context("v6 migration: split sessions and delete project-kind data")?;
            for path in paths {
                if let Ok(safe) = safe_absolute_image_path(&path) {
                    let _ = std::fs::remove_file(&safe);
                }
            }
        }

        // v6 → v7: add session_keywords table for keyword extraction per session.
        if conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='session_keywords'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            == 0
        {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS session_keywords (
                    workspace_id TEXT NOT NULL,
                    keyword TEXT NOT NULL,
                    weight REAL NOT NULL DEFAULT 1.0,
                    created_at TEXT NOT NULL,
                    PRIMARY KEY (workspace_id, keyword),
                    FOREIGN KEY (workspace_id) REFERENCES dispatcher_sessions(id) ON DELETE CASCADE
                );
                CREATE INDEX IF NOT EXISTS idx_session_keywords_workspace
                    ON session_keywords(workspace_id, weight DESC);
                CREATE INDEX IF NOT EXISTS idx_session_keywords_keyword
                    ON session_keywords(keyword);
                ",
            )
            .context("v7 migration: create session_keywords table")?;
        }

        // v7 → v8: add sub_agents, session_sub_agents, global_sub_agents tables.
        if conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sub_agents'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            == 0
        {
            {
                let tx = conn
                    .transaction()
                    .context("v8: begin sub_agent migration")?;
                crate::agent::sub_agent::db::ensure_sub_agent_tables_tx(&tx)?;
                crate::agent::sub_agent::db::seed_browser_agent_if_missing_tx(&tx)?;
                tx.commit().context("v8: commit sub_agent migration")?;
            }
        }

        // v8 → v9: aha settings v2 — split project / chat agent configs.
        //         Adds dispatcher_settings_v2 table and context column to sub_agents.
        if conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='dispatcher_settings_v2'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            == 0
        {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS dispatcher_settings_v2 (
                    id TEXT PRIMARY KEY DEFAULT 'default',

                    -- Shared models (JSON arrays of DispatcherModelConfig)
                    shared_vision_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    shared_image_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    shared_image_edit_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    shared_asr_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    shared_tts_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    shared_embedding_model_configs_json TEXT NOT NULL DEFAULT '[]',

                    -- Project context
                    project_chat_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    project_summary_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    project_allowed_tools_json TEXT NOT NULL DEFAULT '[]',

                    -- Chat context
                    chat_agent_chat_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    chat_agent_summary_model_configs_json TEXT NOT NULL DEFAULT '[]',
                    chat_agent_allowed_tools_json TEXT NOT NULL DEFAULT '[]',

                    -- Shared behavior flags
                    auto_approve_dispatch INTEGER NOT NULL DEFAULT 0,
                    context_debug INTEGER NOT NULL DEFAULT 0
                );

                ALTER TABLE sub_agents ADD COLUMN context TEXT NOT NULL DEFAULT 'project';
                ",
            )
            .context("v9 migration: create dispatcher_settings_v2 and add sub_agents.context")?;
        }

        // v9 → v10: remove context column from sub_agents; sub-agents are now global
        //           entities. Context associations move to context_sub_agents table.
        let has_context_col = conn
            .prepare("SELECT COUNT(*) FROM pragma_table_info('sub_agents') WHERE name = 'context'")
            .and_then(|mut stmt| stmt.query_row([], |row| row.get::<_, i64>(0)))
            .unwrap_or(0);

        if has_context_col > 0 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS context_sub_agents (
                    context      TEXT NOT NULL,
                    sub_agent_id TEXT NOT NULL,
                    PRIMARY KEY (context, sub_agent_id),
                    FOREIGN KEY (sub_agent_id) REFERENCES sub_agents(id) ON DELETE CASCADE
                );

                INSERT OR IGNORE INTO context_sub_agents (context, sub_agent_id)
                    SELECT context, id FROM sub_agents WHERE context IS NOT NULL;

                ALTER TABLE sub_agents DROP COLUMN context;
                ",
            )
            .context("v10 migration: contextualize sub_agents → context_sub_agents")?;
        }

        // v10 → v11: refresh browser-agent default timeout from 180s → 600s (10 min).
        {
            let tx = conn
                .transaction()
                .context("v11: begin browser-agent timeout refresh")?;
            crate::agent::sub_agent::db::seed_browser_agent_force_tx(&tx)?;
            tx.commit()
                .context("v11: commit browser-agent timeout refresh")?;
        }

        // v11 → v12: SSH 命令安全审查 AI 配置（单个模型配置 + 可编辑系统提示词）。
        //             对已有行，ALTER ADD COLUMN ... DEFAULT 安全地补齐默认值。
        let has_review_model_col = conn
            .prepare("SELECT COUNT(*) FROM pragma_table_info('dispatcher_settings_v2') WHERE name = 'review_model_config_json'")
            .and_then(|mut stmt| stmt.query_row([], |row| row.get::<_, i64>(0)))
            .unwrap_or(0);
        if has_review_model_col == 0 {
            conn.execute_batch(
                "
                ALTER TABLE dispatcher_settings_v2
                    ADD COLUMN review_model_config_json TEXT NOT NULL DEFAULT '';
                ALTER TABLE dispatcher_settings_v2
                    ADD COLUMN review_system_prompt TEXT NOT NULL DEFAULT '';
                ",
            )
            .context("v12 migration: add ssh review config columns")?;
        }

        // Mark schema as fully migrated (outside the transaction — PRAGMA is auto-commit).
        conn.execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION))
            .context("set user_version")?;

        Ok(())
    }
}

fn migrate_session_token_usage_primary_key_on_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    let primary_key_columns = table_primary_key_columns(tx, "dispatcher_session_token_usage")?;
    if primary_key_columns
        .iter()
        .map(String::as_str)
        .eq(["workspace_id", "model", "source_kind"])
    {
        return Ok(());
    }

    migrate_session_token_usage_primary_key_inner(tx)
}

fn migrate_session_token_usage_primary_key_inner(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        ALTER TABLE dispatcher_session_token_usage RENAME TO dispatcher_session_token_usage_old;

        CREATE TABLE dispatcher_session_token_usage (
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

        INSERT INTO dispatcher_session_token_usage (
            workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
            cached_tokens, context_window_tokens, context_window_capacity, updated_at
        )
        SELECT
            workspace_id,
            model,
            COALESCE(NULLIF(source_kind, ''), 'primary'),
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cached_tokens,
            context_window_tokens,
            context_window_capacity,
            updated_at
        FROM dispatcher_session_token_usage_old;

        DROP TABLE dispatcher_session_token_usage_old;

        CREATE INDEX IF NOT EXISTS idx_dispatcher_session_token_usage_workspace_updated
        ON dispatcher_session_token_usage(workspace_id, updated_at DESC);
        ",
    )
    .context("migrate dispatcher session token usage primary key")
}

fn table_primary_key_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .with_context(|| format!("read table info for {table}"))?;
    let columns = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(5)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .with_context(|| format!("collect table info for {table}"))?;

    let mut primary_key_columns = columns
        .into_iter()
        .filter(|(position, _)| *position > 0)
        .collect::<Vec<_>>();
    primary_key_columns.sort_by_key(|(position, _)| *position);
    Ok(primary_key_columns
        .into_iter()
        .map(|(_, name)| name)
        .collect())
}

fn ensure_column_exists(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    match conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    ) {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(_, Some(message)))
            if message.contains("duplicate column name") =>
        {
            Ok(())
        }
        Err(error) => {
            Err(error).with_context(|| format!("ensure column {column} exists on table {table}"))
        }
    }
}

/// Transaction-scoped variant used from within `init()`'s outer migration transaction.
fn ensure_column_exists_tx(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    // Transaction deref's to Connection, so delegate directly.
    ensure_column_exists(tx, table, column, definition)
}

fn ensure_python_code_runs_table_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "
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
        ",
    )
    .context("ensure python code runs table")
}

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

fn ensure_dispatcher_model_config_columns(conn: &Connection) -> Result<()> {
    for (column, definition) in [
        ("chat_model_url", "TEXT NOT NULL DEFAULT ''"),
        ("chat_model_api_key", "TEXT NOT NULL DEFAULT ''"),
        ("chat_model_name", "TEXT NOT NULL DEFAULT ''"),
        ("summary_model_url", "TEXT NOT NULL DEFAULT ''"),
        ("summary_model_api_key", "TEXT NOT NULL DEFAULT ''"),
        ("summary_model_name", "TEXT NOT NULL DEFAULT ''"),
        ("vision_model_url", "TEXT NOT NULL DEFAULT ''"),
        ("vision_model_api_key", "TEXT NOT NULL DEFAULT ''"),
        ("vision_model_name", "TEXT NOT NULL DEFAULT ''"),
        (
            "image_model_config_url",
            "TEXT NOT NULL DEFAULT 'https://dashscope.aliyuncs.com/api/v1'",
        ),
        ("image_model_config_api_key", "TEXT NOT NULL DEFAULT ''"),
        (
            "image_model_config_name",
            "TEXT NOT NULL DEFAULT 'qwen-image-2.0-pro'",
        ),
        (
            "image_edit_model_url",
            "TEXT NOT NULL DEFAULT 'https://dashscope.aliyuncs.com/api/v1'",
        ),
        ("image_edit_model_api_key", "TEXT NOT NULL DEFAULT ''"),
        (
            "image_edit_model_name",
            "TEXT NOT NULL DEFAULT 'qwen-image-2.0-pro'",
        ),
        ("asr_model_url", "TEXT NOT NULL DEFAULT ''"),
        ("asr_model_api_key", "TEXT NOT NULL DEFAULT ''"),
        ("asr_model_name", "TEXT NOT NULL DEFAULT 'fun-asr-realtime'"),
        ("tts_model_url", "TEXT NOT NULL DEFAULT ''"),
        ("tts_model_api_key", "TEXT NOT NULL DEFAULT ''"),
        ("tts_model_name", "TEXT NOT NULL DEFAULT ''"),
        ("embedding_model_url", "TEXT NOT NULL DEFAULT ''"),
        ("embedding_model_api_key", "TEXT NOT NULL DEFAULT ''"),
        ("embedding_model_name", "TEXT NOT NULL DEFAULT ''"),
        ("chat_model_configs_json", "TEXT NOT NULL DEFAULT '[]'"),
        ("summary_model_configs_json", "TEXT NOT NULL DEFAULT '[]'"),
        ("vision_model_configs_json", "TEXT NOT NULL DEFAULT '[]'"),
        ("image_model_configs_json", "TEXT NOT NULL DEFAULT '[]'"),
        (
            "image_edit_model_configs_json",
            "TEXT NOT NULL DEFAULT '[]'",
        ),
        ("asr_model_configs_json", "TEXT NOT NULL DEFAULT '[]'"),
        ("tts_model_configs_json", "TEXT NOT NULL DEFAULT '[]'"),
        ("embedding_model_configs_json", "TEXT NOT NULL DEFAULT '[]'"),
    ] {
        ensure_column_exists(conn, "dispatcher_settings", column, definition)?;
    }

    Ok(())
}

/// Transaction-scoped variant used from within `init()`'s outer migration transaction.
fn ensure_dispatcher_model_config_columns_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    ensure_dispatcher_model_config_columns(tx)
}

fn migrate_dispatcher_model_configs(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE dispatcher_settings SET
            chat_model_url = CASE WHEN trim(chat_model_url) = '' THEN api_base ELSE chat_model_url END,
            chat_model_api_key = CASE WHEN trim(chat_model_api_key) = '' THEN api_key ELSE chat_model_api_key END,
            chat_model_name = CASE WHEN trim(chat_model_name) = '' THEN model ELSE chat_model_name END,
            summary_model_url = CASE WHEN trim(summary_model_url) = '' THEN api_base ELSE summary_model_url END,
            summary_model_api_key = CASE WHEN trim(summary_model_api_key) = '' THEN api_key ELSE summary_model_api_key END,
            summary_model_name = CASE WHEN trim(summary_model_name) = '' THEN summary_model ELSE summary_model_name END,
            vision_model_url = CASE WHEN trim(vision_model_url) = '' THEN api_base ELSE vision_model_url END,
            vision_model_api_key = CASE WHEN trim(vision_model_api_key) = '' THEN api_key ELSE vision_model_api_key END,
            vision_model_name = CASE WHEN trim(vision_model_name) = '' THEN vision_model ELSE vision_model_name END,
            image_model_config_url = CASE WHEN trim(image_model_config_url) = '' THEN image_model_url ELSE image_model_config_url END,
            image_model_config_api_key = CASE WHEN trim(image_model_config_api_key) = '' THEN image_model_api_key ELSE image_model_config_api_key END,
            image_model_config_name = CASE WHEN trim(image_model_config_name) = '' THEN image_model ELSE image_model_config_name END,
            image_edit_model_url = CASE WHEN trim(image_edit_model_url) = '' THEN image_model_url ELSE image_edit_model_url END,
            image_edit_model_api_key = CASE WHEN trim(image_edit_model_api_key) = '' THEN image_model_api_key ELSE image_edit_model_api_key END,
            image_edit_model_name = CASE WHEN trim(image_edit_model_name) = '' THEN COALESCE(NULLIF(trim(image_edit_model), ''), image_model) ELSE image_edit_model_name END,
            asr_model_url = CASE WHEN trim(asr_model_url) = '' THEN asr_websocket_url ELSE asr_model_url END,
            asr_model_api_key = CASE WHEN trim(asr_model_api_key) = '' THEN asr_api_key ELSE asr_model_api_key END,
            asr_model_name = CASE WHEN trim(asr_model_name) = '' THEN 'fun-asr-realtime' ELSE asr_model_name END",
        [],
    )
    .context("migrate dispatcher model configs")?;

    conn.execute(
        "UPDATE dispatcher_settings SET
            chat_model_configs_json = CASE WHEN trim(chat_model_configs_json) = '' THEN '[]' ELSE chat_model_configs_json END,
            summary_model_configs_json = CASE WHEN trim(summary_model_configs_json) = '' THEN '[]' ELSE summary_model_configs_json END,
            vision_model_configs_json = CASE WHEN trim(vision_model_configs_json) = '' THEN '[]' ELSE vision_model_configs_json END,
            image_model_configs_json = CASE WHEN trim(image_model_configs_json) = '' THEN '[]' ELSE image_model_configs_json END,
            image_edit_model_configs_json = CASE WHEN trim(image_edit_model_configs_json) = '' THEN '[]' ELSE image_edit_model_configs_json END,
            asr_model_configs_json = CASE WHEN trim(asr_model_configs_json) = '' THEN '[]' ELSE asr_model_configs_json END,
            tts_model_configs_json = CASE WHEN trim(tts_model_configs_json) = '' THEN '[]' ELSE tts_model_configs_json END,
            embedding_model_configs_json = CASE WHEN trim(embedding_model_configs_json) = '' THEN '[]' ELSE embedding_model_configs_json END",
        [],
    )
    .context("migrate dispatcher model config lists")?;

    Ok(())
}

/// Transaction-scoped variant used from within `init()`'s outer migration transaction.
fn migrate_dispatcher_model_configs_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    migrate_dispatcher_model_configs(tx)
}
