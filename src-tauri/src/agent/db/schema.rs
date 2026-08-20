//! 数据库 schema 初始化与版本迁移（PRAGMA user_version 方案）。
//!
//! `init()` 负责建表与数据迁移；其余 `ensure_*` / `migrate_*` 为迁移助手，
//! 仅被 `init()` 调用，故保持模块私有。

use std::collections::HashSet;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use super::content::safe_absolute_image_path;
use super::settings::DispatcherModelConfig;
use super::util::now;
use super::DispatcherDb;

/// 当前 schema 版本号（PRAGMA user_version），init() 内迁移按版本号阶梯推进。
/// pub(crate)：跨模块的迁移测试直接引用本常量，避免硬编码版本号漂移。
/// v30：SSH 服务器 / 主机密钥 / 审计从按项目分键的 JSON 文件收敛为全局表。
/// v31：受管项目注册表从 projects.json 收敛为全局表（会话级联删除的锚点）。
/// v32：MCP 全局注册表建表，普通聊天工作区的旧「全局」mcp.json 导入。
/// v33：应用级键值配置（app_config）：theme / 全局浏览器选项 / RAG 配置入库。
pub(crate) const SCHEMA_VERSION: i32 = 33;

impl DispatcherDb {
    pub(super) fn init(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create db directory {}", parent.display()))?;
        }
        let mut conn = self.conn()?;

        // Fast path: if schema is already at the expected version, skip all DDL.
        let current_version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or(0);
        if current_version > SCHEMA_VERSION {
            // 数据库由更高版本的应用写入，旧代码无法识别新 schema：继续读写
            // 轻则查询报错、重则写入不兼容数据损坏库，直接报错提示升级。
            anyhow::bail!(
                "数据库版本({current_version})高于当前程序支持的版本({SCHEMA_VERSION})，请升级应用后再打开"
            );
        }
        if current_version == SCHEMA_VERSION {
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

            CREATE TABLE IF NOT EXISTS dispatcher_settings_v2 (
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
                auto_approve_dispatch INTEGER NOT NULL DEFAULT 0,
                context_debug INTEGER NOT NULL DEFAULT 0,
                review_model_config_json TEXT NOT NULL DEFAULT '',
                review_system_prompt TEXT NOT NULL DEFAULT '',
                model_library_json TEXT NOT NULL DEFAULT '[]'
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

            CREATE TABLE IF NOT EXISTS ssh_servers (
                id TEXT PRIMARY KEY,
                config_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS ssh_host_keys (
                server_id TEXT PRIMARY KEY,
                fingerprint TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS ssh_audit_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                server_id TEXT NOT NULL,
                record_json TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_ssh_audit_log_server
            ON ssh_audit_log(server_id, id DESC);

            CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                path TEXT NOT NULL UNIQUE,
                branch TEXT,
                last_opened_at INTEGER NOT NULL DEFAULT 0,
                sort_order INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS mcp_servers (
                name TEXT PRIMARY KEY,
                enabled INTEGER NOT NULL DEFAULT 1,
                config_json TEXT NOT NULL,
                sort_order INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS app_config (
                key TEXT PRIMARY KEY,
                value_json TEXT NOT NULL
            );
            ",
        )
        .context("initialize dispatcher sqlite schema")?;

        // Run all column additions and data migrations in a single transaction
        // so that a failure midway rolls back cleanly instead of leaving partial schema.
        {
            let tx = conn.transaction().context("begin migration transaction")?;

            migrate_session_token_usage_primary_key_on_tx(&tx)?;
            migrate_legacy_dispatcher_settings_tx(&tx)?;
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
            // 旧库缺少 kind 列时默认 'chat' 而非 'project'：后续 v6 迁移会删除
            // 全部 kind='project' 会话及其消息/图片，默认 'project' 会把未经
            // v5→v6 的旧库全部聊天数据不可恢复地清空；'chat' 保留数据。
            ensure_column_exists_tx(
                &tx,
                "dispatcher_sessions",
                "kind",
                "TEXT NOT NULL DEFAULT 'chat'",
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
        // 列名 session_id（存储会话 id）；旧库中的 workspace_id 列由 v28 迁移改名。
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

        // v19 → v20: persist completed/failed sub-agent execution traces by
        // parent tool_call_id so historical message cards remain inspectable.
        if current_version < 20 {
            let tx = conn
                .transaction()
                .context("v20: begin sub-agent trace migration")?;
            crate::agent::sub_agent::db::ensure_sub_agent_trace_table_tx(&tx)?;
            tx.commit()
                .context("v20: commit sub-agent trace migration")?;
        }

        // v20 → v21: categorized model library（按模型调用方式分类的模型库，
        //             供设置中心「模型服务」页管理、「模型用途」页引用）。
        if current_version < 21 {
            let has_model_library_col = conn
                .prepare("SELECT COUNT(*) FROM pragma_table_info('dispatcher_settings_v2') WHERE name = 'model_library_json'")
                .and_then(|mut stmt| stmt.query_row([], |row| row.get::<_, i64>(0)))
                .unwrap_or(0);
            if has_model_library_col == 0 {
                conn.execute_batch(
                    "
                    ALTER TABLE dispatcher_settings_v2
                        ADD COLUMN model_library_json TEXT NOT NULL DEFAULT '[]';
                    ",
                )
                .context("v21 migration: add model library column")?;
            }
        }

        // v24/v25 都是「DROP 重建图表格」的破坏性迁移（按产品决策清空旧图
        // 数据）。在首个破坏性迁移之前，对仍存有图数据的库做一次整库快照备份
        // （VACUUM INTO），为不可逆清空兜底；同时打印各表行数便于事后审计。
        // 注意 current_version < 24 的库会在同一次 init 内连续执行 v24 与 v25
        // 两次 DROP，一次备份即可覆盖。
        if current_version < 25 {
            snapshot_before_graph_rebuild(&conn, &self.path, current_version);
        }

        // v21 → v24: PI SDK 执行图 v2。按产品决策清空全部旧图数据，重建为
        // run/attempt 模型，后续每次重跑均保留独立节点记录与活动时间线。
        if current_version < 24 {
            let tx = conn
                .transaction()
                .context("v24: begin PI graph migration")?;
            tx.execute_batch(
                "
                DROP TABLE IF EXISTS graph_node_activities;
                DROP TABLE IF EXISTS graph_node_runs;
                DROP TABLE IF EXISTS graph_runs;
                DROP TABLE IF EXISTS graph_plans;

                CREATE TABLE graph_plans (
                    id TEXT PRIMARY KEY,
                    workspace_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    summary TEXT NOT NULL DEFAULT '',
                    definition_json TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'draft',
                    state_json TEXT NOT NULL DEFAULT '{}',
                    latest_run_id TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE INDEX idx_graph_plans_ws
                    ON graph_plans(workspace_id, updated_at DESC);

                CREATE TABLE graph_runs (
                    id TEXT PRIMARY KEY,
                    plan_id TEXT NOT NULL,
                    attempt_no INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    started_at INTEGER NOT NULL,
                    finished_at INTEGER,
                    UNIQUE(plan_id, attempt_no),
                    FOREIGN KEY(plan_id) REFERENCES graph_plans(id) ON DELETE CASCADE
                );
                CREATE INDEX idx_graph_runs_plan
                    ON graph_runs(plan_id, attempt_no DESC);

                CREATE TABLE graph_node_runs (
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
                    PRIMARY KEY(run_id, node_id),
                    FOREIGN KEY(run_id) REFERENCES graph_runs(id) ON DELETE CASCADE,
                    FOREIGN KEY(plan_id) REFERENCES graph_plans(id) ON DELETE CASCADE
                );

                CREATE TABLE graph_node_activities (
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
                CREATE INDEX idx_graph_activities_node
                    ON graph_node_activities(run_id, node_id, sequence);
                ",
            )
            .context("v24: rebuild PI graph tables")?;
            tx.commit().context("v24: commit PI graph migration")?;
        }

        // 废弃列清理（幂等）放在破坏性的 v25 迁移之前：即便失败，版本号也不会
        // 前进，下次启动可安全重试，不会触发 v25 的 DROP 重建。
        drop_obsolete_planning_columns(&conn)?;

        // v24 → v25: 执行图 v3（闭环编排）。按产品决策清空全部旧图数据：
        // 图定义升级为 v3（expectedFiles/exportPolicy/inheritsFrom），plan 携带
        // 需求快照与继承来源，run 携带模式（full/resume）与验收结论（verdict），
        // 节点记录携带重试计数。
        if current_version < 25 {
            let tx = conn
                .transaction()
                .context("v25: begin PI graph v3 migration")?;
            tx.execute_batch(
                "
                DROP TABLE IF EXISTS graph_node_activities;
                DROP TABLE IF EXISTS graph_node_runs;
                DROP TABLE IF EXISTS graph_runs;
                DROP TABLE IF EXISTS graph_plans;

                CREATE TABLE graph_plans (
                    id TEXT PRIMARY KEY,
                    workspace_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    summary TEXT NOT NULL DEFAULT '',
                    definition_json TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'draft',
                    state_json TEXT NOT NULL DEFAULT '{}',
                    requirement TEXT NOT NULL DEFAULT '',
                    inherits_plan_id TEXT,
                    inherits_run_id TEXT,
                    latest_run_id TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE INDEX idx_graph_plans_ws
                    ON graph_plans(workspace_id, updated_at DESC);

                CREATE TABLE graph_runs (
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
                CREATE INDEX idx_graph_runs_plan
                    ON graph_runs(plan_id, attempt_no DESC);

                CREATE TABLE graph_node_runs (
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

                CREATE TABLE graph_node_activities (
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
                ",
            )
            .context("v25: rebuild PI graph tables for v3")?;

            // settings 列变更与版本号更新并入同一事务：若 ALTER 失败或提交前进程退出，
            // 整体回滚，下次启动不会在「表已重建但版本号未更新」的中间状态上
            // 再次 DROP 重建（会丢弃两次启动之间写入的图数据）。
            let has_graph_config_col: i64 = tx
                .prepare("SELECT COUNT(*) FROM pragma_table_info('dispatcher_settings_v2') WHERE name = 'graph_execution_config_json'")
                .and_then(|mut stmt| stmt.query_row([], |row| row.get::<_, i64>(0)))
                .context("v25: check dispatcher_settings_v2.graph_execution_config_json")?;
            if has_graph_config_col == 0 {
                tx.execute_batch(
                    "
                    ALTER TABLE dispatcher_settings_v2
                        ADD COLUMN graph_execution_config_json TEXT NOT NULL DEFAULT '{}';
                    ",
                )
                .context("v25 migration: add graph execution config column")?;
            }
            tx.pragma_update(None, "user_version", 25)
                .context("v25: set user_version")?;
            tx.commit().context("v25: commit PI graph v3 migration")?;
        }

        // v25 → v26: 图表加固（幂等）。
        // 1) graph_node_runs(plan_id) 无索引，而清理/联表查询按 plan_id 过滤会全表扫描；
        // 2) idx_graph_activities_node 与 UNIQUE(run_id,node_id,sequence) 约束自动索引重复；
        // 3) graph_plans 增列 initial_state_json：full 模式重跑修复图时恢复提交时种入的
        //    初始继承快照（运行中 state_json 会被部分产物持续写入，不能作为重跑起点）。
        if current_version < 26 {
            let tx = conn
                .transaction()
                .context("v26: begin graph hardening migration")?;
            tx.execute_batch(
                "
                CREATE INDEX IF NOT EXISTS idx_graph_node_runs_plan
                    ON graph_node_runs(plan_id);
                DROP INDEX IF EXISTS idx_graph_activities_node;
                ",
            )
            .context("v26: graph index maintenance")?;
            let has_initial_state_col: i64 = tx
                .prepare("SELECT COUNT(*) FROM pragma_table_info('graph_plans') WHERE name = 'initial_state_json'")
                .and_then(|mut stmt| stmt.query_row([], |row| row.get::<_, i64>(0)))
                .context("v26: check graph_plans.initial_state_json")?;
            if has_initial_state_col == 0 {
                tx.execute_batch(
                    "ALTER TABLE graph_plans ADD COLUMN initial_state_json TEXT NOT NULL DEFAULT '{}';",
                )
                .context("v26: add graph_plans.initial_state_json")?;
                // 存量回填：普通图初始即空；修复图只能以当前 state 近似（提交时的
                // 原始快照在旧版本中未单独留存）。
                tx.execute(
                    "UPDATE graph_plans SET initial_state_json = CASE WHEN inherits_plan_id IS NULL THEN '{}' ELSE state_json END",
                    params![],
                )
                .context("v26: backfill graph_plans.initial_state_json")?;
            }
            tx.pragma_update(None, "user_version", 26)
                .context("v26: set user_version")?;
            tx.commit()
                .context("v26: commit graph hardening migration")?;
        }

        // v26 → v27: 清理 v6 迁移遗漏的悬空 dispatcher_tool_runs 记录。
        // v6（已发布，语义不再改动）删除 project 会话的消息/产物/token 用量等
        // 数据时遗漏了无外键约束的 dispatcher_tool_runs，会话删除后其
        // workspace_id 成为悬空引用；此处统一删除工作区已不存在的记录。
        if current_version < 27 {
            let tx = conn
                .transaction()
                .context("v27: begin tool runs dangling cleanup migration")?;
            let removed = tx
                .execute(
                    "DELETE FROM dispatcher_tool_runs
                     WHERE workspace_id NOT IN (SELECT id FROM dispatcher_sessions)",
                    [],
                )
                .context("v27: delete dangling dispatcher_tool_runs")?;
            if removed > 0 {
                eprintln!("[db] v27 迁移：清理悬空 dispatcher_tool_runs 记录 {removed} 条");
            }
            tx.pragma_update(None, "user_version", 27)
                .context("v27: set user_version")?;
            tx.commit()
                .context("v27: commit tool runs dangling cleanup migration")?;
        }

        // v27 → v28: session_keywords.workspace_id 改名为 session_id。
        // 该列实际存储会话 id（FK → dispatcher_sessions(id)，查询中按
        // sk.workspace_id = ds.id 关联），旧名与语义严重不符。SQLite ≥ 3.25 的
        // RENAME COLUMN 会同步更新 PK/FK 中的列引用；索引按列序号引用不受影响，
        // 但会话维度索引顺势重建为新名。改名前先清理会话已删除的悬空行（否则
        // foreign_key_check 会把历史残留算成违例），改名后以
        // PRAGMA foreign_key_check 兜底校验。新建库直接以 session_id 建表
        // （见上方建表语句），此块对其为空操作，天然幂等。
        if current_version < 28 {
            let tx = conn
                .transaction()
                .context("v28: begin session_keywords rename migration")?;
            if table_has_column(&tx, "session_keywords", "workspace_id")? {
                let removed = tx
                    .execute(
                        "DELETE FROM session_keywords
                         WHERE workspace_id NOT IN (SELECT id FROM dispatcher_sessions)",
                        [],
                    )
                    .context("v28: delete dangling session_keywords")?;
                if removed > 0 {
                    eprintln!("[db] v28 迁移：清理悬空 session_keywords 记录 {removed} 条");
                }
                tx.execute_batch(
                    "
                    ALTER TABLE session_keywords RENAME COLUMN workspace_id TO session_id;
                    DROP INDEX IF EXISTS idx_session_keywords_workspace;
                    CREATE INDEX IF NOT EXISTS idx_session_keywords_session
                        ON session_keywords(session_id, weight DESC);
                    ",
                )
                .context("v28: rename session_keywords.workspace_id to session_id")?;
            }
            let fk_violations: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM pragma_foreign_key_check
                     WHERE \"table\" = 'session_keywords'",
                    [],
                    |row| row.get(0),
                )
                .context("v28: foreign_key_check session_keywords")?;
            if fk_violations > 0 {
                anyhow::bail!(
                    "v28: session_keywords 外键校验发现 {fk_violations} 条违例，迁移中止"
                );
            }
            tx.pragma_update(None, "user_version", 28)
                .context("v28: set user_version")?;
            tx.commit()
                .context("v28: commit session_keywords rename migration")?;
        }

        // v28 → v29：工具运行从扁平台账升级为可审计的父子调用树。
        // 新列均具有兼容默认值，存量根调用无需回填；artifact.tool_run_id
        // 保持 NULL，只有新运行产生的产物才要求绑定真实 run。
        if current_version < 29 {
            let tx = conn
                .transaction()
                .context("v29: begin tool run trace migration")?;
            if !table_has_column(&tx, "dispatcher_tool_runs", "parent_run_id")? {
                tx.execute_batch(
                    "ALTER TABLE dispatcher_tool_runs
                         ADD COLUMN parent_run_id TEXT
                         REFERENCES dispatcher_tool_runs(id) ON DELETE CASCADE;",
                )
                .context("v29: add dispatcher_tool_runs.parent_run_id")?;
            }
            if !table_has_column(&tx, "dispatcher_tool_runs", "origin")? {
                tx.execute_batch(
                    "ALTER TABLE dispatcher_tool_runs
                         ADD COLUMN origin TEXT NOT NULL DEFAULT 'model'
                         CHECK(length(trim(origin)) > 0);",
                )
                .context("v29: add dispatcher_tool_runs.origin")?;
            }
            if !table_has_column(&tx, "dispatcher_tool_runs", "step_id")? {
                tx.execute_batch("ALTER TABLE dispatcher_tool_runs ADD COLUMN step_id TEXT;")
                    .context("v29: add dispatcher_tool_runs.step_id")?;
            }
            if !table_has_column(&tx, "dispatcher_tool_runs", "sequence")? {
                tx.execute_batch(
                    "ALTER TABLE dispatcher_tool_runs
                         ADD COLUMN sequence INTEGER NOT NULL DEFAULT 0 CHECK(sequence >= 0);",
                )
                .context("v29: add dispatcher_tool_runs.sequence")?;
            }
            if !table_has_column(&tx, "dispatcher_tool_artifacts", "tool_run_id")? {
                tx.execute_batch(
                    "ALTER TABLE dispatcher_tool_artifacts
                         ADD COLUMN tool_run_id TEXT
                         REFERENCES dispatcher_tool_runs(id) ON DELETE CASCADE;",
                )
                .context("v29: add dispatcher_tool_artifacts.tool_run_id")?;
            }
            tx.execute_batch(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_dispatcher_tool_runs_parent_sequence
                     ON dispatcher_tool_runs(parent_run_id, sequence)
                     WHERE parent_run_id IS NOT NULL;
                 CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_artifacts_run
                     ON dispatcher_tool_artifacts(tool_run_id, created_at);",
            )
            .context("v29: create tool run trace indexes")?;

            let fk_violations: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM pragma_foreign_key_check
                     WHERE \"table\" IN ('dispatcher_tool_runs', 'dispatcher_tool_artifacts')",
                    [],
                    |row| row.get(0),
                )
                .context("v29: foreign_key_check tool run trace tables")?;
            if fk_violations > 0 {
                anyhow::bail!("v29: 工具运行追踪表外键校验发现 {fk_violations} 条违例，迁移中止");
            }
            tx.pragma_update(None, "user_version", 29)
                .context("v29: set user_version")?;
            tx.commit()
                .context("v29: commit tool run trace migration")?;
        }

        // v29 → v30：SSH 配置收敛为全局 SQLite 权威源（应用生命周期配置，
        // 不再按项目路径分键存文件）。建表 + 一次性导入旧文件数据；导入过程
        // 只读文件，旧凭据文件在 commit 成功后清理（明文凭据不宜多处留存）。
        if current_version < 30 {
            let tx = conn
                .transaction()
                .context("v30: begin ssh global store migration")?;
            crate::ssh_tool::db::ensure_ssh_tables_tx(&tx)
                .context("v30: create ssh global tables")?;
            let root_dir = self
                .path
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_default();
            let outcome = crate::ssh_tool::db::import_legacy_ssh_store(&tx, &root_dir)
                .map_err(anyhow::Error::msg)
                .context("v30: import legacy ssh files")?;
            tx.pragma_update(None, "user_version", 30)
                .context("v30: set user_version")?;
            tx.commit()
                .context("v30: commit ssh global store migration")?;
            if outcome.servers > 0 || outcome.audit_records > 0 {
                eprintln!(
                    "[ssh-tool] 已迁移 {} 台 SSH 服务器、{} 条审计记录到全局数据库",
                    outcome.servers, outcome.audit_records
                );
            }
            crate::ssh_tool::db::cleanup_legacy_files(&outcome, &root_dir);
        }

        // v30 → v31：受管项目注册表入库（projects.json → projects 表）。
        // 注册表是会话 project_id 外键的锚点，入库后项目删除才能做级联清理。
        // 导入在事务内完成；projects.json 在 commit 成功后删除（best-effort）。
        if current_version < 31 {
            let tx = conn
                .transaction()
                .context("v31: begin projects registry migration")?;
            super::projects::ensure_projects_table_tx(&tx)
                .context("v31: create projects table")?;
            let root_dir = self
                .path
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_default();
            let imported =
                super::projects::import_legacy_projects_json(&tx, &root_dir)
                    .context("v31: import legacy projects.json")?;
            tx.pragma_update(None, "user_version", 31)
                .context("v31: set user_version")?;
            tx.commit()
                .context("v31: commit projects registry migration")?;
            if imported > 0 {
                eprintln!("[projects] 已迁移 {imported} 个受管项目到全局数据库");
            }
            // DB 已是权威源：旧文件无论导入条数都清理，失败仅告警。
            let legacy = root_dir.join("projects.json");
            if legacy.exists() {
                if let Err(error) = std::fs::remove_file(&legacy) {
                    eprintln!(
                        "[projects] 迁移后清理 projects.json 失败（{}）：{error}；建议手动删除。",
                        legacy.display()
                    );
                }
            }
        }

        // v31 → v32：MCP 全局注册表建表并导入普通聊天工作区的旧「全局」
        // mcp.json。此后全局 MCP 存 SQLite，项目级 mcp.json 保留并在解析时
        // 覆盖同名全局条目。
        if current_version < 32 {
            let tx = conn
                .transaction()
                .context("v32: begin global mcp registry migration")?;
            super::mcp_servers::ensure_mcp_servers_table_tx(&tx)
                .context("v32: create mcp_servers table")?;
            let root_dir = self
                .path
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_default();
            let (imported, found_legacy) =
                super::mcp_servers::import_legacy_global_mcp_json(&tx, &root_dir)
                    .context("v32: import legacy global mcp json")?;
            tx.pragma_update(None, "user_version", 32)
                .context("v32: set user_version")?;
            tx.commit()
                .context("v32: commit global mcp registry migration")?;
            if imported > 0 {
                eprintln!("[mcp] 已迁移 {imported} 个全局 MCP 服务器到全局数据库");
            }
            if found_legacy {
                super::mcp_servers::cleanup_legacy_global_mcp_file(&root_dir);
            }
        }

        // v32 → v33：应用级键值配置建表并导入散落的 JSON 文件（settings.json →
        // app_settings、rag/config.json → rag、项目 config.toml 的首个非默认
        // [browser] → browser）。导入在事务内，文件在 commit 后清理。
        if current_version < 33 {
            let tx = conn
                .transaction()
                .context("v33: begin app config migration")?;
            super::app_config::ensure_app_config_table_tx(&tx)
                .context("v33: create app_config table")?;
            let root_dir = self
                .path
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_default();
            let cleanup = super::app_config::import_legacy_app_config_files(&tx, &root_dir)
                .context("v33: import legacy app config files")?;
            tx.pragma_update(None, "user_version", 33)
                .context("v33: set user_version")?;
            tx.commit()
                .context("v33: commit app config migration")?;
            super::app_config::cleanup_legacy_files(&cleanup);
        }

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
    // 旧表形态可能缺少后续引入的列，先探测再用常量兜底，确保迁移在任何旧
    // 数据形态下都不因缺列硬失败。
    let has_source_kind = table_has_column(conn, "dispatcher_session_token_usage", "source_kind")?;
    let has_cached_tokens =
        table_has_column(conn, "dispatcher_session_token_usage", "cached_tokens")?;
    let has_context_window_tokens = table_has_column(
        conn,
        "dispatcher_session_token_usage",
        "context_window_tokens",
    )?;
    let has_context_window_capacity = table_has_column(
        conn,
        "dispatcher_session_token_usage",
        "context_window_capacity",
    )?;
    let has_updated_at = table_has_column(conn, "dispatcher_session_token_usage", "updated_at")?;

    // 空串与 NULL 的 source_kind 归一为 'primary' 后，旧数据可能存在重复的
    // (workspace_id, model, source_kind) 组合（旧主键更弱时）；直接 INSERT 会触发
    // UNIQUE 冲突导致整个迁移事务回滚。按归一化主键 GROUP BY 聚合去重：
    // token 列求和无损合并，容量取 MAX，updated_at 取最新。
    let source_kind_expr = if has_source_kind {
        "COALESCE(NULLIF(source_kind, ''), 'primary')"
    } else {
        "'primary'"
    };
    let cached_expr = if has_cached_tokens {
        "SUM(cached_tokens)"
    } else {
        "0"
    };
    let context_tokens_expr = if has_context_window_tokens {
        "SUM(context_window_tokens)"
    } else {
        "0"
    };
    let capacity_expr = if has_context_window_capacity {
        "MAX(context_window_capacity)"
    } else {
        "1000000"
    };
    let updated_at_expr = if has_updated_at {
        "MAX(updated_at)".to_string()
    } else {
        format!("'{}'", now())
    };

    let sql = format!(
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
            {source_kind_expr},
            SUM(prompt_tokens),
            SUM(completion_tokens),
            SUM(total_tokens),
            {cached_expr},
            {context_tokens_expr},
            {capacity_expr},
            {updated_at_expr}
        FROM dispatcher_session_token_usage_old
        GROUP BY workspace_id, model, {source_kind_expr};

        DROP TABLE dispatcher_session_token_usage_old;

        CREATE INDEX IF NOT EXISTS idx_dispatcher_session_token_usage_workspace_updated
        ON dispatcher_session_token_usage(workspace_id, updated_at DESC);
        "
    );
    conn.execute_batch(&sql)
        .context("migrate dispatcher session token usage primary key")
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
            params![column],
            |row| row.get(0),
        )
        .with_context(|| format!("read column info for {table}.{column}"))?;
    Ok(count > 0)
}

fn migrate_legacy_dispatcher_settings_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    if !table_exists(tx, "dispatcher_settings")? {
        return Ok(());
    }

    ensure_legacy_dispatcher_settings_columns_tx(tx)?;
    normalize_legacy_dispatcher_settings_tx(tx)?;

    let Some(row) = tx
        .query_row(
            "SELECT
                api_base, api_key, model,
                summary_model, vision_model,
                asr_api_key, asr_websocket_url,
                auto_approve_dispatch, context_debug,
                image_model_url, image_model_api_key, image_model, image_edit_model,
                chat_model_url, chat_model_api_key, chat_model_name, chat_model_configs_json,
                summary_model_url, summary_model_api_key, summary_model_name, summary_model_configs_json,
                vision_model_url, vision_model_api_key, vision_model_name, vision_model_configs_json,
                image_model_config_url, image_model_config_api_key, image_model_config_name, image_model_configs_json,
                image_edit_model_url, image_edit_model_api_key, image_edit_model_name, image_edit_model_configs_json,
                asr_model_url, asr_model_api_key, asr_model_name, asr_model_configs_json,
                tts_model_url, tts_model_api_key, tts_model_name, tts_model_configs_json,
                embedding_model_url, embedding_model_api_key, embedding_model_name, embedding_model_configs_json,
                allowed_tools_json
             FROM dispatcher_settings WHERE id = 'default'",
            [],
            LegacyDispatcherSettings::from_row,
        )
        .optional()
        .context("load legacy dispatcher settings")?
    else {
        tx.execute_batch("DROP TABLE dispatcher_settings;")
            .context("drop empty legacy dispatcher settings table")?;
        return Ok(());
    };

    let chat_configs = legacy_configs(
        &row.chat_model_configs_json,
        &row.chat_model_url,
        &row.chat_model_api_key,
        &row.chat_model_name,
        (&row.api_base, &row.api_key, &row.model),
    );
    let summary_configs = legacy_configs(
        &row.summary_model_configs_json,
        &row.summary_model_url,
        &row.summary_model_api_key,
        &row.summary_model_name,
        (&row.api_base, &row.api_key, &row.summary_model),
    );
    let vision_configs = legacy_configs(
        &row.vision_model_configs_json,
        &row.vision_model_url,
        &row.vision_model_api_key,
        &row.vision_model_name,
        (&row.api_base, &row.api_key, &row.vision_model),
    );
    let image_configs = legacy_configs(
        &row.image_model_configs_json,
        &row.image_model_config_url,
        &row.image_model_config_api_key,
        &row.image_model_config_name,
        (
            &row.image_model_url,
            &row.image_model_api_key,
            &row.image_model,
        ),
    );
    let image_edit_configs = legacy_configs(
        &row.image_edit_model_configs_json,
        &row.image_edit_model_url,
        &row.image_edit_model_api_key,
        &row.image_edit_model_name,
        (
            &row.image_model_url,
            &row.image_model_api_key,
            fallback_image_edit_model(&row.image_model, &row.image_edit_model),
        ),
    );
    let asr_configs = legacy_configs(
        &row.asr_model_configs_json,
        &row.asr_model_url,
        &row.asr_model_api_key,
        &row.asr_model_name,
        (&row.asr_websocket_url, &row.asr_api_key, "fun-asr-realtime"),
    );
    let tts_configs = legacy_configs(
        &row.tts_model_configs_json,
        &row.tts_model_url,
        &row.tts_model_api_key,
        &row.tts_model_name,
        ("", "", ""),
    );
    let embedding_configs = legacy_configs(
        &row.embedding_model_configs_json,
        &row.embedding_model_url,
        &row.embedding_model_api_key,
        &row.embedding_model_name,
        ("", "", ""),
    );

    tx.execute(
        "INSERT INTO dispatcher_settings_v2 (
            id,
            shared_vision_model_configs_json,
            shared_image_model_configs_json,
            shared_image_edit_model_configs_json,
            shared_asr_model_configs_json,
            shared_tts_model_configs_json,
            shared_embedding_model_configs_json,
            project_chat_model_configs_json,
            project_summary_model_configs_json,
            project_allowed_tools_json,
            chat_agent_chat_model_configs_json,
            chat_agent_summary_model_configs_json,
            chat_agent_allowed_tools_json,
            auto_approve_dispatch,
            context_debug
        ) VALUES (
            'default', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?7, ?8, ?9, ?10, ?11
        )
        ON CONFLICT(id) DO UPDATE SET
            shared_vision_model_configs_json = ?1,
            shared_image_model_configs_json = ?2,
            shared_image_edit_model_configs_json = ?3,
            shared_asr_model_configs_json = ?4,
            shared_tts_model_configs_json = ?5,
            shared_embedding_model_configs_json = ?6,
            project_chat_model_configs_json = ?7,
            project_summary_model_configs_json = ?8,
            project_allowed_tools_json = ?9,
            chat_agent_chat_model_configs_json = ?7,
            chat_agent_summary_model_configs_json = ?8,
            chat_agent_allowed_tools_json = ?9,
            auto_approve_dispatch = ?10,
            context_debug = ?11",
        params![
            serde_json::to_string(&vision_configs)?,
            serde_json::to_string(&image_configs)?,
            serde_json::to_string(&image_edit_configs)?,
            serde_json::to_string(&asr_configs)?,
            serde_json::to_string(&tts_configs)?,
            serde_json::to_string(&embedding_configs)?,
            serde_json::to_string(&chat_configs)?,
            serde_json::to_string(&summary_configs)?,
            normalize_json_array(&row.allowed_tools_json),
            row.auto_approve_dispatch,
            row.context_debug,
        ],
    )
    .context("migrate legacy dispatcher settings to v2")?;

    tx.execute_batch("DROP TABLE dispatcher_settings;")
        .context("drop legacy dispatcher settings table")?;
    Ok(())
}

#[derive(Default)]
struct LegacyDispatcherSettings {
    api_base: String,
    api_key: String,
    model: String,
    summary_model: String,
    vision_model: String,
    asr_api_key: String,
    asr_websocket_url: String,
    auto_approve_dispatch: i32,
    context_debug: i32,
    image_model_url: String,
    image_model_api_key: String,
    image_model: String,
    image_edit_model: String,
    chat_model_url: String,
    chat_model_api_key: String,
    chat_model_name: String,
    chat_model_configs_json: String,
    summary_model_url: String,
    summary_model_api_key: String,
    summary_model_name: String,
    summary_model_configs_json: String,
    vision_model_url: String,
    vision_model_api_key: String,
    vision_model_name: String,
    vision_model_configs_json: String,
    image_model_config_url: String,
    image_model_config_api_key: String,
    image_model_config_name: String,
    image_model_configs_json: String,
    image_edit_model_url: String,
    image_edit_model_api_key: String,
    image_edit_model_name: String,
    image_edit_model_configs_json: String,
    asr_model_url: String,
    asr_model_api_key: String,
    asr_model_name: String,
    asr_model_configs_json: String,
    tts_model_url: String,
    tts_model_api_key: String,
    tts_model_name: String,
    tts_model_configs_json: String,
    embedding_model_url: String,
    embedding_model_api_key: String,
    embedding_model_name: String,
    embedding_model_configs_json: String,
    allowed_tools_json: String,
}

impl LegacyDispatcherSettings {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            api_base: row.get(0)?,
            api_key: row.get(1)?,
            model: row.get(2)?,
            summary_model: row.get(3)?,
            vision_model: row.get(4)?,
            asr_api_key: row.get(5)?,
            asr_websocket_url: row.get(6)?,
            auto_approve_dispatch: row.get(7)?,
            context_debug: row.get(8)?,
            image_model_url: row.get(9)?,
            image_model_api_key: row.get(10)?,
            image_model: row.get(11)?,
            image_edit_model: row.get(12)?,
            chat_model_url: row.get(13)?,
            chat_model_api_key: row.get(14)?,
            chat_model_name: row.get(15)?,
            chat_model_configs_json: row.get(16)?,
            summary_model_url: row.get(17)?,
            summary_model_api_key: row.get(18)?,
            summary_model_name: row.get(19)?,
            summary_model_configs_json: row.get(20)?,
            vision_model_url: row.get(21)?,
            vision_model_api_key: row.get(22)?,
            vision_model_name: row.get(23)?,
            vision_model_configs_json: row.get(24)?,
            image_model_config_url: row.get(25)?,
            image_model_config_api_key: row.get(26)?,
            image_model_config_name: row.get(27)?,
            image_model_configs_json: row.get(28)?,
            image_edit_model_url: row.get(29)?,
            image_edit_model_api_key: row.get(30)?,
            image_edit_model_name: row.get(31)?,
            image_edit_model_configs_json: row.get(32)?,
            asr_model_url: row.get(33)?,
            asr_model_api_key: row.get(34)?,
            asr_model_name: row.get(35)?,
            asr_model_configs_json: row.get(36)?,
            tts_model_url: row.get(37)?,
            tts_model_api_key: row.get(38)?,
            tts_model_name: row.get(39)?,
            tts_model_configs_json: row.get(40)?,
            embedding_model_url: row.get(41)?,
            embedding_model_api_key: row.get(42)?,
            embedding_model_name: row.get(43)?,
            embedding_model_configs_json: row.get(44)?,
            allowed_tools_json: row.get(45)?,
        })
    }
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?1",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count > 0)
    .with_context(|| format!("check table exists: {table}"))
}

fn ensure_legacy_dispatcher_settings_columns_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    for (column, definition) in [
        ("summary_model", "TEXT NOT NULL DEFAULT 'deepseek-v4-flash'"),
        ("vision_model", "TEXT NOT NULL DEFAULT ''"),
        ("asr_api_key", "TEXT NOT NULL DEFAULT ''"),
        ("asr_websocket_url", "TEXT NOT NULL DEFAULT ''"),
        ("context_debug", "INTEGER NOT NULL DEFAULT 0"),
        (
            "image_model_url",
            "TEXT NOT NULL DEFAULT 'https://dashscope.aliyuncs.com/api/v1'",
        ),
        ("image_model_api_key", "TEXT NOT NULL DEFAULT ''"),
        ("image_model", "TEXT NOT NULL DEFAULT 'qwen-image-2.0-pro'"),
        ("image_edit_model", "TEXT NOT NULL DEFAULT ''"),
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
        ("allowed_tools_json", "TEXT NOT NULL DEFAULT '[]'"),
    ] {
        ensure_column_exists_tx(tx, "dispatcher_settings", column, definition)?;
    }
    Ok(())
}

fn normalize_legacy_dispatcher_settings_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "
        UPDATE dispatcher_settings SET
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
            asr_model_name = CASE WHEN trim(asr_model_name) = '' THEN 'fun-asr-realtime' ELSE asr_model_name END,
            chat_model_configs_json = CASE WHEN trim(chat_model_configs_json) = '' THEN '[]' ELSE chat_model_configs_json END,
            summary_model_configs_json = CASE WHEN trim(summary_model_configs_json) = '' THEN '[]' ELSE summary_model_configs_json END,
            vision_model_configs_json = CASE WHEN trim(vision_model_configs_json) = '' THEN '[]' ELSE vision_model_configs_json END,
            image_model_configs_json = CASE WHEN trim(image_model_configs_json) = '' THEN '[]' ELSE image_model_configs_json END,
            image_edit_model_configs_json = CASE WHEN trim(image_edit_model_configs_json) = '' THEN '[]' ELSE image_edit_model_configs_json END,
            asr_model_configs_json = CASE WHEN trim(asr_model_configs_json) = '' THEN '[]' ELSE asr_model_configs_json END,
            tts_model_configs_json = CASE WHEN trim(tts_model_configs_json) = '' THEN '[]' ELSE tts_model_configs_json END,
            embedding_model_configs_json = CASE WHEN trim(embedding_model_configs_json) = '' THEN '[]' ELSE embedding_model_configs_json END;
        ",
    )
    .context("normalize legacy dispatcher settings")
}

fn legacy_configs(
    raw_json: &str,
    url: &str,
    api_key: &str,
    model: &str,
    fallback: (&str, &str, &str),
) -> Vec<DispatcherModelConfig> {
    let parsed = serde_json::from_str::<Vec<DispatcherModelConfig>>(raw_json)
        .unwrap_or_default()
        .into_iter()
        .filter(|config| !legacy_model_config_is_empty(config))
        .collect::<Vec<_>>();
    if !parsed.is_empty() {
        return normalize_legacy_model_configs(parsed);
    }

    let config = DispatcherModelConfig {
        url: non_empty_or(url, fallback.0).to_string(),
        api_key: non_empty_or(api_key, fallback.1).to_string(),
        model: non_empty_or(model, fallback.2).to_string(),
        active: true,
        system_prompt: String::new(),
        library_id: String::new(),
    };
    if legacy_model_config_is_empty(&config) {
        Vec::new()
    } else {
        vec![config]
    }
}

fn normalize_legacy_model_configs(
    configs: Vec<DispatcherModelConfig>,
) -> Vec<DispatcherModelConfig> {
    let mut normalized = configs
        .into_iter()
        .filter(|config| !legacy_model_config_is_empty(config))
        .collect::<Vec<_>>();
    if let Some(active_index) = normalized.iter().position(|config| config.active) {
        for (index, config) in normalized.iter_mut().enumerate() {
            config.active = index == active_index;
        }
    } else if let Some(first) = normalized.first_mut() {
        first.active = true;
    }
    normalized
}

fn legacy_model_config_is_empty(config: &DispatcherModelConfig) -> bool {
    config.url.trim().is_empty()
        && config.api_key.trim().is_empty()
        && config.model.trim().is_empty()
}

fn non_empty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback.trim()
    } else {
        value.trim()
    }
}

fn fallback_image_edit_model<'a>(image_model: &'a str, image_edit_model: &'a str) -> &'a str {
    if image_edit_model.trim().is_empty() {
        image_model.trim()
    } else {
        image_edit_model.trim()
    }
}

fn normalize_json_array(raw: &str) -> String {
    serde_json::from_str::<Vec<String>>(raw)
        .map(|value| serde_json::to_string(&value).unwrap_or_else(|_| "[]".to_string()))
        .unwrap_or_else(|_| "[]".to_string())
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

/// 破坏性图迁移（v24/v25 的 DROP TABLE 重建）前的整库快照保险。
///
/// 四张图表中任一仍有数据时，用 `VACUUM INTO` 生成一份一致性快照（WAL 安全，
/// 不受未 checkpoint 的 -wal 数据影响），并把各表行数写入日志便于审计。
/// 任何失败（表不存在、磁盘满等）只告警不阻断迁移：备份是保险，不应让备份
/// 故障导致应用无法启动。但备份失败意味着旧图数据即将在无兜底副本的情况下
/// 被清空——这一事实必须在桌面应用的 stderr 之外留痕：失败时额外在数据库
/// 目录写入 `*.pre-graph-rebuild-failed.marker` 标记文件（不动存储 schema），
/// 供后续向用户提示与人工排查；备份成功时清理可能存在的过期失败标记。
fn snapshot_before_graph_rebuild(
    conn: &Connection,
    db_path: &std::path::Path,
    current_version: i32,
) {
    const GRAPH_TABLES: [&str; 4] = [
        "graph_plans",
        "graph_runs",
        "graph_node_runs",
        "graph_node_activities",
    ];
    let table_exists = |table: &str| -> bool {
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            params![table],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
            > 0
    };
    let mut counts: Vec<(String, i64)> = Vec::new();
    for table in GRAPH_TABLES {
        if !table_exists(table) {
            continue;
        }
        let count = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap_or(0);
        counts.push((table.to_string(), count));
    }
    let total: i64 = counts.iter().map(|(_, count)| *count).sum();
    if total == 0 {
        return;
    }
    let detail = counts
        .iter()
        .map(|(table, count)| format!("{table}={count}"))
        .collect::<Vec<_>>()
        .join(", ");
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let file_stem = db_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("database.sqlite3");
    let backup_name = format!("{file_stem}.pre-graph-rebuild.v{current_version}.{stamp}.backup");
    let Some(parent) = db_path.parent() else {
        eprintln!("[db] 图数据备份跳过：无法解析数据库目录（{detail}）");
        return;
    };
    let backup_path = parent.join(&backup_name);
    // 「备份失败」标记文件（固定名）：备份失败时写入、成功时清理，见函数注释。
    let failed_marker_path = parent.join(format!("{file_stem}.pre-graph-rebuild-failed.marker"));
    // SQLite 的 VACUUM INTO 只接受字符串字面量/标识符，不支持绑定参数
    // （`VACUUM INTO ?` 会在 prepare 阶段直接语法报错，导致快照从未生成）。
    // 这里把文件名以字面量拼入 SQL，单引号按 SQL 规则转义为两个单引号。
    let backup_sql = format!(
        "VACUUM INTO '{}'",
        backup_path.to_string_lossy().replace('\'', "''")
    );
    match conn.execute(&backup_sql, []) {
        Ok(_) => {
            eprintln!(
                "[db] 即将执行破坏性图迁移（user_version={current_version}），已备份数据库（{detail}）到 {}",
                backup_path.display()
            );
            // 只保留这份最新快照：清理同前缀的其他历史备份。迁移反复失败时
            // 每次启动都会新生成一份整库副本，不清理会无限累积占满磁盘。
            // 仅在新备份成功后清理，避免备份失败时落得新旧皆无。
            let backup_prefix = format!("{file_stem}.pre-graph-rebuild.");
            if let Ok(entries) = std::fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path == backup_path {
                        continue;
                    }
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with(&backup_prefix) && name.ends_with(".backup") {
                        if let Err(error) = std::fs::remove_file(&path) {
                            eprintln!("[db] 清理历史备份 {} 失败：{error}", path.display());
                        }
                    }
                }
            }
            // 本次备份成功：此前（如有）迁移重试期间留下的失败标记已过期。
            if failed_marker_path.exists() {
                if let Err(error) = std::fs::remove_file(&failed_marker_path) {
                    eprintln!(
                        "[db] 清理过期备份失败标记 {} 失败：{error}",
                        failed_marker_path.display()
                    );
                }
            }
        }
        Err(error) => {
            // 备份失败而迁移仍将继续：旧图数据即将在无兜底副本的情况下被清空。
            // 桌面应用中 eprintln 用户基本不可见，把失败事实持久化到标记文件，
            // 供后续向用户提示与人工排查（内容含行数明细、错误与备份目标）。
            let marker = format!(
                "time={}\nuser_version={current_version}\ntables={detail}\nbackup_target={}\nerror={error}\n",
                chrono::Utc::now().to_rfc3339(),
                backup_path.display()
            );
            if let Err(marker_error) = std::fs::write(&failed_marker_path, marker) {
                eprintln!(
                    "[db] 写入备份失败标记 {} 失败：{marker_error}",
                    failed_marker_path.display()
                );
            }
            eprintln!(
                "[db] 严重：图数据备份失败（{detail}），迁移仍将继续，旧图数据将在无备份情况下被清空：{error}；备份目标：{}；失败标记：{}",
                backup_path.display(),
                failed_marker_path.display()
            );
        }
    }
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

    /// 数据库版本高于当前代码版本（用户降级安装）时必须拒绝打开，而不是静默继续。
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
            message.contains("请升级应用"),
            "错误信息应提示升级：{message}"
        );
        cleanup_db_files(&path);
    }

    /// 旧库（无 kind 列、无 chat_sessions 表）升级时，缺列默认值必须为 'chat'：
    /// 会话在 v6 迁移后保留，而不是被当作 project 数据清空。
    #[test]
    fn legacy_db_without_kind_column_keeps_sessions() {
        let path = temp_db_path("legacy-kind");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE dispatcher_sessions (
                    id TEXT PRIMARY KEY,
                    project_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                INSERT INTO dispatcher_sessions (id, project_id, title, created_at, updated_at)
                    VALUES ('s1', 'p1', '旧会话', '2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z');
                CREATE TABLE dispatcher_messages (
                    id TEXT PRIMARY KEY,
                    workspace_id TEXT NOT NULL,
                    role TEXT NOT NULL,
                    context_cleared INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL
                );
                INSERT INTO dispatcher_messages (id, workspace_id, role, created_at)
                    VALUES ('m1', 's1', 'user', '2020-01-01T00:00:00Z');
                ",
            )
            .unwrap();
        }

        let db = DispatcherDb::new(path.clone()).unwrap();
        let conn = db.conn().expect("db conn");
        let chat_sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(chat_sessions, 1, "旧会话应保留为 chat 会话");
        let messages: i64 = conn
            .query_row("SELECT COUNT(*) FROM dispatcher_messages", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(messages, 1, "旧会话消息不应被清空");
        drop(conn);
        drop(db);
        cleanup_db_files(&path);
    }

    /// token 用量主键迁移：归一化后重复的 (workspace_id, model, source_kind)
    /// 旧行必须聚合去重（SUM），不能触发 UNIQUE 冲突让迁移失败。
    #[test]
    fn token_usage_primary_key_migration_merges_duplicates() {
        let path = temp_db_path("token-usage-pk");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            // 旧形态：弱主键（rowid 表）+ 可空串 source_kind，存在归一化后重复的行。
            conn.execute_batch(
                "
                CREATE TABLE dispatcher_session_token_usage (
                    workspace_id TEXT NOT NULL,
                    model TEXT NOT NULL,
                    source_kind TEXT NOT NULL DEFAULT '',
                    prompt_tokens INTEGER NOT NULL DEFAULT 0,
                    completion_tokens INTEGER NOT NULL DEFAULT 0,
                    total_tokens INTEGER NOT NULL DEFAULT 0,
                    cached_tokens INTEGER NOT NULL DEFAULT 0,
                    context_window_tokens INTEGER NOT NULL DEFAULT 0,
                    context_window_capacity INTEGER NOT NULL DEFAULT 1000000,
                    updated_at TEXT NOT NULL
                );
                INSERT INTO dispatcher_session_token_usage
                    (workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens, updated_at)
                    VALUES ('w1', 'gpt', 'primary', 10, 20, 30, '2026-01-01T00:00:00Z');
                INSERT INTO dispatcher_session_token_usage
                    (workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens, updated_at)
                    VALUES ('w1', 'gpt', '', 5, 7, 12, '2026-01-02T00:00:00Z');
                ",
            )
            .unwrap();
        }

        let db = DispatcherDb::new(path.clone()).unwrap();
        let conn = db.conn().expect("db conn");
        let (count, prompt, completion, total, source_kind, updated_at): (
            i64,
            i64,
            i64,
            i64,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT COUNT(*), SUM(prompt_tokens), SUM(completion_tokens), SUM(total_tokens),
                        source_kind, MAX(updated_at)
                 FROM dispatcher_session_token_usage
                 WHERE workspace_id = 'w1' AND model = 'gpt'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(count, 1, "归一化后重复行应合并为一行");
        assert_eq!(
            (prompt, completion, total),
            (15, 27, 42),
            "token 列应求和合并"
        );
        assert_eq!(source_kind, "primary", "空串 source_kind 应归一为 primary");
        assert_eq!(updated_at, "2026-01-02T00:00:00Z", "updated_at 应保留最新");
        drop(conn);
        drop(db);
        cleanup_db_files(&path);
    }

    /// v27 迁移：清理 v6 遗漏的悬空 dispatcher_tool_runs（workspace 已不存在），
    /// 并保留仍有效工作区的记录。
    #[test]
    fn v27_migration_removes_dangling_tool_runs() {
        let path = temp_db_path("v27-dangling");
        let db = DispatcherDb::new(path.clone()).unwrap();
        {
            let conn = db.conn().expect("db conn");
            conn.execute(
                "INSERT INTO dispatcher_sessions (id, project_id, kind, title, category, created_at, updated_at)
                 VALUES ('w-live', 'p1', 'chat', '活跃会话', 'tech', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
            let insert_run =
                "INSERT INTO dispatcher_tool_runs
                     (id, workspace_id, tool_call_id, tool_name, provider, category, status, created_at, updated_at)
                 VALUES (?1, ?2, 'call-1', 'local_zsh', 'dispatcher', 'shell', 'completed',
                         '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')";
            conn.execute(insert_run, rusqlite::params!["run-live", "w-live"])
                .unwrap();
            conn.execute(insert_run, rusqlite::params!["run-dangling", "w-gone"])
                .unwrap();
            // 回退版本号，让 init 重新执行 v27 迁移块。
            conn.pragma_update(None, "user_version", 26).unwrap();
        }
        db.init().expect("v27 migration should succeed");

        let conn = db.conn().expect("db conn");
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);
        let live: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatcher_tool_runs WHERE id = 'run-live'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(live, 1, "有效工作区的 tool run 必须保留");
        let dangling: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatcher_tool_runs WHERE id = 'run-dangling'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(dangling, 0, "悬空 tool run 必须被清理");
        drop(conn);
        drop(db);
        cleanup_db_files(&path);
    }

    /// v28 迁移：session_keywords.workspace_id 改名为 session_id——存量数据保留、
    /// 列名/主键/外键引用更新、悬空行清理、旧索引重建为新名、级联删除仍有效。
    #[test]
    fn v28_migration_renames_session_keywords_workspace_id() {
        let path = temp_db_path("v28-keywords-rename");
        let db = DispatcherDb::new(path.clone()).unwrap();
        {
            let conn = db.conn().expect("db conn");
            conn.execute(
                "INSERT INTO dispatcher_sessions (id, project_id, kind, title, category, created_at, updated_at)
                 VALUES ('s-live', 'p1', 'chat', '存活会话', 'tech', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
            // 还原 v27 时代的表形态：列名回退 workspace_id、旧索引重建回来，
            // 再塞入存量数据（含一条会话已不存在的悬空行）。悬空行只能在
            // foreign_keys=OFF 下写入，批处理结束后必须恢复 ON。
            conn.execute_batch(
                "
                PRAGMA foreign_keys = OFF;
                ALTER TABLE session_keywords RENAME COLUMN session_id TO workspace_id;
                DROP INDEX IF EXISTS idx_session_keywords_session;
                CREATE INDEX IF NOT EXISTS idx_session_keywords_workspace
                    ON session_keywords(workspace_id, weight DESC);
                INSERT INTO session_keywords (workspace_id, keyword, weight, created_at)
                    VALUES ('s-live', 'Rust', 8.0, '2026-01-01T00:00:00Z'),
                           ('s-live', 'Tauri', 6.0, '2026-01-02T00:00:00Z'),
                           ('s-gone', '悬空', 1.0, '2026-01-01T00:00:00Z');
                PRAGMA foreign_keys = ON;
                ",
            )
            .unwrap();
            // 回退版本号，让 init 重新执行 v28 迁移块。
            conn.pragma_update(None, "user_version", 27).unwrap();
        }
        db.init().expect("v28 migration should succeed");

        let conn = db.conn().expect("db conn");
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);

        let has_session_id: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('session_keywords') WHERE name = 'session_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_session_id, 1, "session_id 列必须存在");
        let has_legacy: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('session_keywords') WHERE name = 'workspace_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_legacy, 0, "旧 workspace_id 列不得残留");

        let kept: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_keywords WHERE session_id = 's-live'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(kept, 2, "存活会话的存量关键词必须保留");
        let (weight, created_at): (f64, String) = conn
            .query_row(
                "SELECT weight, created_at FROM session_keywords
                 WHERE session_id = 's-live' AND keyword = 'Rust'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(weight, 8.0, "权重必须保留");
        assert_eq!(created_at, "2026-01-01T00:00:00Z", "created_at 必须保留");
        let dangling: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_keywords WHERE session_id = 's-gone'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(dangling, 0, "悬空关键词行必须被清理");

        let fk_from: String = conn
            .query_row(
                "SELECT \"from\" FROM pragma_foreign_key_list('session_keywords')
                 WHERE \"table\" = 'dispatcher_sessions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fk_from, "session_id", "外键列引用必须随改名更新");

        let new_index: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_session_keywords_session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(new_index, 1, "会话维度索引应以新名重建");
        let legacy_index: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_session_keywords_workspace'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy_index, 0, "旧名索引应被删除");

        // ON DELETE CASCADE 在改名后仍然有效。
        conn.execute("DELETE FROM dispatcher_sessions WHERE id = 's-live'", [])
            .unwrap();
        let after_cascade: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_keywords", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(after_cascade, 0, "会话删除后关键词应级联清空");

        drop(conn);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn v29_migration_adds_tool_run_trace_columns_and_preserves_legacy_rows() {
        let path = temp_db_path("v29-tool-run-trace");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE dispatcher_sessions (
                    id TEXT PRIMARY KEY,
                    project_id TEXT NOT NULL,
                    kind TEXT NOT NULL DEFAULT 'project',
                    title TEXT NOT NULL,
                    category TEXT NOT NULL DEFAULT '',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                CREATE TABLE dispatcher_tool_runs (
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
                CREATE TABLE dispatcher_tool_artifacts (
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
                INSERT INTO dispatcher_sessions
                    (id, project_id, kind, title, category, created_at, updated_at)
                    VALUES ('ws', 'project', 'chat', 'legacy', 'tech',
                            '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
                INSERT INTO dispatcher_tool_runs
                    (id, workspace_id, tool_call_id, tool_name, provider, category, status,
                     created_at, updated_at)
                    VALUES ('root', 'ws', 'call-root', 'grep', 'builtin', 'search', 'succeeded',
                            '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
                INSERT INTO dispatcher_tool_artifacts
                    (id, workspace_id, tool_call_id, tool_name, title, kind, content, created_at)
                    VALUES ('legacy-artifact', 'ws', 'call-root', 'grep', 'legacy',
                            'tool_raw_output', 'content', '2026-01-01T00:00:00Z');
                PRAGMA user_version = 28;
                ",
            )
            .unwrap();
        }

        let db = DispatcherDb::new(path.clone()).expect("v29 migration should succeed");
        let conn = db.conn().expect("db conn");
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);

        let migrated: (Option<String>, String, Option<String>, i64) = conn
            .query_row(
                "SELECT parent_run_id, origin, step_id, sequence
                 FROM dispatcher_tool_runs WHERE id = 'root'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(migrated, (None, "model".to_string(), None, 0));
        let legacy_artifact_run: Option<String> = conn
            .query_row(
                "SELECT tool_run_id FROM dispatcher_tool_artifacts
                 WHERE id = 'legacy-artifact'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy_artifact_run, None);

        for index in [
            "idx_dispatcher_tool_runs_parent_sequence",
            "idx_dispatcher_tool_artifacts_run",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                    rusqlite::params![index],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "v29 index {index} must exist");
        }

        conn.execute(
            "INSERT INTO dispatcher_tool_runs
                (id, workspace_id, tool_call_id, parent_run_id, origin, step_id, sequence,
                 tool_name, provider, category, status, created_at, updated_at)
             VALUES ('child', 'ws', 'call-child', 'root', 'tool_program', 'read', 0,
                     'read_file', 'builtin', 'filesystem', 'planned',
                     '2026-01-01T00:00:01Z', '2026-01-01T00:00:01Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO dispatcher_tool_artifacts
                (id, workspace_id, tool_run_id, title, kind, content, created_at)
             VALUES ('child-artifact', 'ws', 'child', 'child output', 'tool_raw_output',
                     'content', '2026-01-01T00:00:01Z')",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM dispatcher_tool_runs WHERE id = 'root'", [])
            .unwrap();
        let cascaded: i64 = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM dispatcher_tool_runs WHERE id = 'child') +
                        (SELECT COUNT(*) FROM dispatcher_tool_artifacts WHERE id = 'child-artifact')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cascaded, 0, "child run and its artifact must cascade");

        drop(conn);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn fresh_v29_schema_contains_tool_run_trace_foreign_keys() {
        let path = temp_db_path("fresh-v29-tool-run-trace");
        let db = DispatcherDb::new(path.clone()).expect("create fresh v29 db");
        let conn = db.conn().expect("db conn");

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
            assert_eq!(exists, 1, "fresh schema must contain {table}.{column}");
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

    /// v24 时代的库（图表含真实数据）升级到当前版本时，v25 破坏性迁移会 DROP
    /// 重建四张图表：DROP 前必须生成保留原数据的整库快照备份。
    #[test]
    fn destructive_graph_migration_backs_up_existing_graph_data() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "aha-schema-backup-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        // 构造 v24 时代的库：四张图表含数据，user_version=24。
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE graph_plans (
                    id TEXT PRIMARY KEY,
                    workspace_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    summary TEXT NOT NULL DEFAULT '',
                    definition_json TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'draft',
                    state_json TEXT NOT NULL DEFAULT '{}',
                    latest_run_id TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE TABLE graph_runs (
                    id TEXT PRIMARY KEY,
                    plan_id TEXT NOT NULL,
                    attempt_no INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    started_at INTEGER NOT NULL,
                    finished_at INTEGER
                );
                CREATE TABLE graph_node_runs (
                    run_id TEXT NOT NULL,
                    plan_id TEXT NOT NULL,
                    node_id TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'pending',
                    PRIMARY KEY(run_id, node_id)
                );
                CREATE TABLE graph_node_activities (
                    id TEXT PRIMARY KEY,
                    run_id TEXT NOT NULL,
                    node_id TEXT NOT NULL,
                    sequence INTEGER NOT NULL,
                    kind TEXT NOT NULL,
                    status TEXT NOT NULL,
                    started_at INTEGER NOT NULL
                );
                INSERT INTO graph_plans
                    (id, workspace_id, title, definition_json, created_at, updated_at)
                    VALUES ('p1','w1','旧图 A','{}',1,1),('p2','w1','旧图 B','{}',1,1);
                INSERT INTO graph_runs (id, plan_id, attempt_no, status, started_at)
                    VALUES ('r1','p1',1,'completed',1);
                PRAGMA user_version = 24;
                ",
            )
            .unwrap();
        }

        let db = DispatcherDb::new(path.clone()).unwrap();
        drop(db);

        let file_name = path.file_name().unwrap().to_string_lossy().to_string();
        let backups: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| {
                name.starts_with(&format!("{file_name}.pre-graph-rebuild.v24."))
                    && name.ends_with(".backup")
            })
            .collect();
        assert_eq!(backups.len(), 1, "破坏性迁移前应恰好生成一份快照备份");

        // 备份保留清空前的图数据；当前库按产品决策已重建清空并升到最新版本。
        let backup_conn = rusqlite::Connection::open(dir.join(&backups[0])).unwrap();
        let backup_plans: i64 = backup_conn
            .query_row("SELECT COUNT(*) FROM graph_plans", [], |row| row.get(0))
            .unwrap();
        assert_eq!(backup_plans, 2);
        let backup_runs: i64 = backup_conn
            .query_row("SELECT COUNT(*) FROM graph_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(backup_runs, 1);
        drop(backup_conn);

        let current_conn = rusqlite::Connection::open(&path).unwrap();
        let version: i32 = current_conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);
        let current_plans: i64 = current_conn
            .query_row("SELECT COUNT(*) FROM graph_plans", [], |row| row.get(0))
            .unwrap();
        assert_eq!(current_plans, 0, "当前库按产品决策重建清空");
        drop(current_conn);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
        let _ = std::fs::remove_file(dir.join(&backups[0]));
    }
}
