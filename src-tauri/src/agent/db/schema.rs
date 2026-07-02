//! 数据库 schema 初始化与版本迁移（PRAGMA user_version 方案）。
//!
//! `init()` 负责建表与数据迁移；其余 `ensure_*` / `migrate_*` 为迁移助手，
//! 仅被 `init()` 调用，故保持模块私有。

use std::collections::HashSet;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

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
        const SCHEMA_VERSION: i32 = 18;
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

            CREATE TABLE IF NOT EXISTS dispatcher_tool_runs (
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                tool_call_id TEXT NOT NULL,
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
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_runs_workspace_created
            ON dispatcher_tool_runs(workspace_id, created_at);

            CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_runs_call
            ON dispatcher_tool_runs(workspace_id, tool_call_id);

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

            CREATE TABLE IF NOT EXISTS chat_category_agent_configs (
                category_id TEXT PRIMARY KEY,
                allowed_tools_json TEXT NOT NULL DEFAULT '[]',
                system_prompt TEXT NOT NULL DEFAULT '',
                sub_agent_ids_json TEXT NOT NULL DEFAULT '[]',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY (category_id) REFERENCES chat_categories(id) ON DELETE CASCADE
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

            CREATE TABLE IF NOT EXISTS global_sub_agents (
                sub_agent_id TEXT PRIMARY KEY,
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
            ensure_python_code_runs_table_tx(&tx)?;

            ensure_chat_categories_table_tx(&tx)?;

            ensure_column_exists_tx(
                &tx,
                "chat_category_agent_configs",
                "sub_agent_ids_json",
                "TEXT NOT NULL DEFAULT '[]'",
            )?;

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

            // v16: 废弃 context_sub_agents 与 session_sub_agents 关联表。
            //      子智能体改为「全局启用 + 聊天分类级配置」两来源，不再有上下文/单会话关联。
            //      旧库升级时移除这两张残留表；新建库不会有它们（IF EXISTS 保证幂等）。
            tx.execute_batch(
                "DROP TABLE IF EXISTS context_sub_agents;
                 DROP TABLE IF EXISTS session_sub_agents;",
            )
            .context("drop obsolete sub_agent association tables")?;

            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS dispatcher_tool_runs (
                    id TEXT PRIMARY KEY,
                    workspace_id TEXT NOT NULL,
                    tool_call_id TEXT NOT NULL,
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
                    updated_at TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_runs_workspace_created
                ON dispatcher_tool_runs(workspace_id, created_at);

                CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_runs_call
                ON dispatcher_tool_runs(workspace_id, tool_call_id);",
            )
            .context("create dispatcher tool runs table")?;

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

        // v9 → v10: remove the obsolete `context` column from sub_agents.
        //           (Historically sub-agent scoping moved through a context_sub_agents
        //            table; that table has since been removed in favor of per-chat-category
        //            config plus global enablement. Here we only drop the legacy column.)
        let has_context_col = conn
            .prepare("SELECT COUNT(*) FROM pragma_table_info('sub_agents') WHERE name = 'context'")
            .and_then(|mut stmt| stmt.query_row([], |row| row.get::<_, i64>(0)))
            .unwrap_or(0);

        if has_context_col > 0 {
            conn.execute_batch("ALTER TABLE sub_agents DROP COLUMN context;")
                .context("v10 migration: drop obsolete sub_agents.context column")?;
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

        // v12 → v13: per-chat-category plain chat tools and system prompt.
        {
            let tx = conn
                .transaction()
                .context("v13: begin chat category agent config migration")?;
            ensure_chat_category_agent_configs_table_tx(&tx)?;
            backfill_chat_category_agent_configs_tx(&tx)?;
            tx.commit()
                .context("v13: commit chat category agent config migration")?;
        }

        // v13 → v14: initialize built-in chat categories with scenario-specific
        // prompts and tool sets. Do not overwrite user-customized prompts.
        {
            let tx = conn
                .transaction()
                .context("v14: begin scenario chat category config migration")?;
            apply_scenario_chat_category_defaults_tx(&tx)?;
            tx.commit()
                .context("v14: commit scenario chat category config migration")?;
        }

        drop_obsolete_planning_columns(&conn)?;

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

fn drop_column_if_exists(conn: &Connection, table: &str, column: &str) -> Result<()> {
    let exists = conn
        .prepare(&format!(
            "SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"
        ))
        .and_then(|mut stmt| stmt.query_row(params![column], |row| row.get::<_, i64>(0)))
        .unwrap_or(0)
        > 0;
    if !exists {
        return Ok(());
    }
    conn.execute_batch(&format!("ALTER TABLE {table} DROP COLUMN {column};"))
        .with_context(|| format!("drop obsolete column {table}.{column}"))
}

fn drop_obsolete_planning_columns(conn: &Connection) -> Result<()> {
    for table in ["dispatcher_sessions", "project_sessions"] {
        for column in [
            "mode",
            "active_plan_path",
            "checklist_json",
            "plan_interaction_json",
        ] {
            drop_column_if_exists(conn, table, column)?;
        }
    }
    Ok(())
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

fn apply_scenario_chat_category_defaults_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    ensure_chat_category_agent_configs_table_tx(tx)?;
    backfill_chat_category_agent_configs_tx(tx)?;
    let ts = now();
    for category_id in ["general", "life", "work", "tech", "learning"] {
        let Some(default) = scenario_chat_category_agent_config(category_id) else {
            continue;
        };
        let tools_json = serde_json::to_string(default.tools)
            .context("serialize scenario chat category tools")?;
        tx.execute(
            "
            UPDATE chat_category_agent_configs
            SET allowed_tools_json = ?1, system_prompt = ?2, updated_at = ?3
            WHERE category_id = ?4
              AND (TRIM(system_prompt) = '' OR system_prompt = ?5)
            ",
            params![
                tools_json,
                default.system_prompt,
                ts,
                category_id,
                crate::agent::config::DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT,
            ],
        )
        .with_context(|| format!("apply scenario config for {category_id}"))?;
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
            FROM dispatcher_settings_v2
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
