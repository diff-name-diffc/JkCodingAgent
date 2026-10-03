//! 数据库 schema 初始化与版本管理（PRAGMA user_version 方案）。
//!
//! 当前基线为 **v14**（历史 v0→v33 迁移链已按产品决策清除）。`init()` 的
//! 路径：同版本库直接复用；低于基线但存在迁移块的版本逐级前向迁移
//! （当前为 v1→v2、…、v13→v14）；再早的旧开发库一律拒绝打开
//! （提示运行 `scripts/reset-dev-data.sh`）；user_version=0 且无表则按
//! 基线全新建库。
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
/// v4：删除 projects 表的死列 `branch`（前端从未写入、唯一消费点恒渲染
/// 兜底文案，详见 docs/tauri-commands.md D 域分析）。
/// v5：sub_agent_run_traces 新增可空 `model` 列（子智能体运行轨迹记录
/// 真实模型，UI-14 遗留；老轨迹为 NULL，前端「未记录」兜底）。
/// v6：新增 `dispatcher_session_summaries` 表（历史级滚动压缩的跨 run
/// 持久化：滚动摘要 + 覆盖锚点消息 id，详见
/// docs/context-management-2026-09-25/00-progress.md 阶段 2）。
/// v7：删除 dispatcher_messages 死列 `context_cleared`（从未有写方、恒 0，
/// 读侧过滤与索引一并移除）。
/// v8：异步工具调用身份、消息 task_id 和事务型完成事件 outbox。
/// v9：为 `dispatcher_tool_completions.delivery_message_id` 补索引（消息删除
/// 触发器按该列反查投递消息，无索引时对事件表全表扫描）。
/// v10：dispatcher_settings 新增 `project_verifier_model_configs_json` 列
/// （执行图验收模型的独立用途槽位；未配置时回退项目摘要槽位）。
/// v11：graph_runs 新增执行结果列（conclusion_node_id / conclusion_md /
/// result_kind / modified_files_json）——run 收尾把「结论节点输出 + 修改文件
/// 并集 + 结果类型」结构化落库，供图面板结果视图与列表轻量摘要读取。
/// v12：dispatcher_settings 由 20 列宽表收敛为 `(id, settings_json)`——
/// 整对象 JSON 序列化 AhaSettingsV2（落库形态沿用既有「剥离库引用凭据」
/// 约定），消除 19 参 upsert 与列序索引交错的静默错位面。
/// v13：删除 chat_sessions / project_sessions 两张列表读模型子表——写入
/// 路径双写收敛为单写 dispatcher_sessions（先做防孤儿回填再 DROP），
/// 两类列表改查统一表（kind 区分），并补 kind+category+updated_at 索引。
/// v14：删除三个无读写方的死列（dispatcher_messages.visible——写侧恒 1、
/// dispatcher_tool_runs.action_kind、graph_node_runs.special_tools_json），
/// 读侧 `visible = 1` 谓词随列一并移除。
pub(crate) const SCHEMA_VERSION: i32 = 14;

mod runtime;

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
        // 链尾（最后一块）返回，尾部 bail 保留为「升级 SCHEMA_VERSION 却漏挂
        // 迁移块」的兜底：落在最后一块与基线之间的版本号会命中它。
        if current_version < 2 {
            self.migrate_v1_to_v2(&mut conn)?;
        }
        if current_version < 3 {
            self.migrate_v2_to_v3(&mut conn)?;
        }
        if current_version < 4 {
            self.migrate_v3_to_v4(&mut conn)?;
        }
        if current_version < 5 {
            self.migrate_v4_to_v5(&mut conn)?;
        }
        if current_version < 6 {
            self.migrate_v5_to_v6(&mut conn)?;
        }
        if current_version < 7 {
            self.migrate_v6_to_v7(&mut conn)?;
        }
        if current_version < 8 {
            runtime::migrate(self, &mut conn)?;
        }
        if current_version < 9 {
            self.migrate_v8_to_v9(&mut conn)?;
        }
        if current_version < 10 {
            self.migrate_v9_to_v10(&mut conn)?;
        }
        if current_version < 11 {
            self.migrate_v10_to_v11(&mut conn)?;
        }
        if current_version < 12 {
            self.migrate_v11_to_v12(&mut conn)?;
        }
        if current_version < 13 {
            self.migrate_v12_to_v13(&mut conn)?;
        }
        if current_version < 14 {
            self.migrate_v13_to_v14(&mut conn)?;
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

    /// v3 → v4：删除 projects 表的死列 `branch`——前端任何路径都不写入
    /// （仅测试写过），唯一消费点 WelcomePage 的分支 pill 因此恒显示兜底
    /// 文案「本地」。列删除属破坏性 DDL，按规范先做整库快照备份
    /// （VACUUM INTO，不能在事务内执行）；备份失败只留痕不阻断——
    /// 被删列本就无有效数据。`DROP COLUMN` 要求 SQLite ≥ 3.35，
    /// rusqlite bundled 版本满足。
    fn migrate_v3_to_v4(&self, conn: &mut Connection) -> Result<()> {
        let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
        let file_stem = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("jkbot.sqlite3");
        let backup_path = self
            .path
            .with_file_name(format!("{file_stem}.pre-v4-backup-{stamp}"));
        if let Err(error) = conn.execute(
            "VACUUM INTO ?1",
            params![backup_path.to_string_lossy().to_string()],
        ) {
            eprintln!("v3→v4 迁移前整库快照失败（被删列 branch 为死数据，继续）：{error}");
        }

        let tx = conn
            .transaction()
            .context("begin v3→v4 migration transaction")?;
        // 幂等容错：列不存在（极简/异常库形态）时跳过 DROP，仅推进版本号。
        let branch_column_exists: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('projects') WHERE name = 'branch'",
                [],
                |row| row.get(0),
            )
            .context("inspect projects.branch column")?;
        if branch_column_exists > 0 {
            tx.execute_batch("ALTER TABLE projects DROP COLUMN branch;")
                .context("drop projects.branch dead column")?;
        }
        tx.pragma_update(None, "user_version", 4)
            .context("advance user_version to 4")?;
        tx.commit().context("commit v3→v4 migration")
    }

    /// v4 → v5：sub_agent_run_traces 新增可空 `model` 列——子智能体运行
    /// 轨迹记录真实模型（UI-14 遗留，展示端「未记录」占位改为真实值）。
    /// ADD COLUMN 非破坏性、老行 NULL 即「历史轨迹未记录」语义；按规范
    /// 仍先做整库快照备份（VACUUM INTO，不能在事务内执行），备份失败只
    /// 留痕不阻断。幂等可重试：缺表先按新基线补建（ensure 助手
    /// CREATE TABLE IF NOT EXISTS），列已存在则跳过 ADD COLUMN。
    fn migrate_v4_to_v5(&self, conn: &mut Connection) -> Result<()> {
        let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
        let file_stem = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("jkbot.sqlite3");
        let backup_path = self
            .path
            .with_file_name(format!("{file_stem}.pre-v5-backup-{stamp}"));
        if let Err(error) = conn.execute(
            "VACUUM INTO ?1",
            params![backup_path.to_string_lossy().to_string()],
        ) {
            eprintln!("v4→v5 迁移前整库快照失败（ADD COLUMN 非破坏性，继续）：{error}");
        }

        let tx = conn
            .transaction()
            .context("begin v4→v5 migration transaction")?;
        // 缺表（极简/异常库形态）先按新基线建表（含 model 列，幂等）。
        crate::agent::sub_agent::db::ensure_sub_agent_trace_table_tx(&tx)?;
        let model_column_exists: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sub_agent_run_traces') WHERE name = 'model'",
                [],
                |row| row.get(0),
            )
            .context("inspect sub_agent_run_traces.model column")?;
        if model_column_exists == 0 {
            tx.execute_batch("ALTER TABLE sub_agent_run_traces ADD COLUMN model TEXT;")
                .context("add sub_agent_run_traces.model column")?;
        }
        tx.pragma_update(None, "user_version", 5)
            .context("advance user_version to 5")?;
        tx.commit().context("commit v4→v5 migration")
    }

    /// v5 → v6：新增 `dispatcher_session_summaries` 表（历史级滚动压缩的跨
    /// run 持久化）。纯增量建表、零数据迁移；按规范仍先做整库快照备份
    /// （VACUUM INTO，不能在事务内执行），备份失败只留痕不阻断。
    /// CREATE TABLE IF NOT EXISTS 幂等可重试。
    fn migrate_v5_to_v6(&self, conn: &mut Connection) -> Result<()> {
        let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
        let file_stem = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("jkbot.sqlite3");
        let backup_path = self
            .path
            .with_file_name(format!("{file_stem}.pre-v6-backup-{stamp}"));
        if let Err(error) = conn.execute(
            "VACUUM INTO ?1",
            params![backup_path.to_string_lossy().to_string()],
        ) {
            eprintln!("v5→v6 迁移前整库快照失败（纯增量建表，继续）：{error}");
        }

        let tx = conn
            .transaction()
            .context("begin v5→v6 migration transaction")?;
        tx.execute_batch(SESSION_SUMMARIES_DDL)
            .context("create dispatcher_session_summaries table")?;
        tx.pragma_update(None, "user_version", 6)
            .context("advance user_version to 6")?;
        tx.commit().context("commit v5→v6 migration")
    }

    /// v6 → v7：删除 dispatcher_messages 死列 `context_cleared`（从未有写方、
    /// 恒 0；读侧过滤与索引随列一并移除）。事务内重建表 + INSERT SELECT 全量
    /// 保留数据（显式 ORDER BY rowid 保持物理顺序）。**关键陷阱**：DROP 父表
    /// 在外键开启时会执行隐式 DELETE，触发 chat_images / python_code_runs 等
    /// 子表的 ON DELETE CASCADE 误删行——必须先在事务外关闭 foreign_keys，
    /// 提交后无论成败都恢复（连接来自连接池，状态不得外泄）。幂等可重试：
    /// 列已不存在且新表已就位则只推进版本号。迁移前按规范 VACUUM INTO 快照。
    fn migrate_v6_to_v7(&self, conn: &mut Connection) -> Result<()> {
        let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
        let file_stem = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("jkbot.sqlite3");
        let backup_path = self
            .path
            .with_file_name(format!("{file_stem}.pre-v7-backup-{stamp}"));
        if let Err(error) = conn.execute(
            "VACUUM INTO ?1",
            params![backup_path.to_string_lossy().to_string()],
        ) {
            eprintln!("v6→v7 迁移前整库快照失败（INSERT SELECT 保留全部数据，继续）：{error}");
        }

        conn.pragma_update(None, "foreign_keys", "OFF")
            .context("disable foreign_keys for v6→v7 rebuild")?;
        let result = self.migrate_v6_to_v7_tx(conn);
        let restore = conn
            .pragma_update(None, "foreign_keys", "ON")
            .context("restore foreign_keys after v6→v7 rebuild");
        restore?;
        result
    }

    fn migrate_v6_to_v7_tx(&self, conn: &mut Connection) -> Result<()> {
        // 幂等检查：列已不存在说明表已是新形态（或前次重试已完成重建）。
        let legacy_column_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('dispatcher_messages') WHERE name = 'context_cleared'",
                [],
                |row| row.get(0),
            )
            .context("inspect dispatcher_messages.context_cleared column")?;

        let tx = conn
            .transaction()
            .context("begin v6→v7 migration transaction")?;
        if legacy_column_exists > 0 {
            tx.execute_batch(
                "CREATE TABLE dispatcher_messages_v7 (
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
                    created_at TEXT NOT NULL
                );
                INSERT INTO dispatcher_messages_v7 (
                    id, workspace_id, role, segments_json, thinking_content,
                    thinking_elapsed_ms, context_payload, tool_call_id, tool_name,
                    tool_result_mode, tool_artifacts_json, tool_calls_json,
                    usage_stats_json, visible, created_at
                )
                SELECT
                    id, workspace_id, role, segments_json, thinking_content,
                    thinking_elapsed_ms, context_payload, tool_call_id, tool_name,
                    tool_result_mode, tool_artifacts_json, tool_calls_json,
                    usage_stats_json, visible, created_at
                FROM dispatcher_messages ORDER BY rowid;
                DROP TABLE dispatcher_messages;
                ALTER TABLE dispatcher_messages_v7 RENAME TO dispatcher_messages;
                CREATE INDEX idx_dispatcher_messages_workspace_created
                ON dispatcher_messages(workspace_id, created_at);
                CREATE INDEX idx_dispatcher_messages_workspace_role_created
                ON dispatcher_messages(workspace_id, role, created_at);",
            )
            .context("rebuild dispatcher_messages without context_cleared")?;
        }
        tx.pragma_update(None, "user_version", 7)
            .context("advance user_version to 7")?;
        tx.commit().context("commit v6→v7 migration")
    }

    /// v8 → v9：为 `dispatcher_tool_completions.delivery_message_id` 补索引
    /// ——消息删除触发器按该列反查投递消息（`schema/runtime.rs` 的
    /// `reset_tool_completion_observation`），无索引时每次删除都对该表全表
    /// 扫描。纯增量建索引、零数据迁移；DDL 与基线同源（复用
    /// `runtime::extend_schema`，避免索引定义两处漂移），`IF NOT EXISTS`
    /// 幂等可重试。按规范仍先做整库快照备份（VACUUM INTO，不能在事务内
    /// 执行），备份失败只留痕不阻断。
    fn migrate_v8_to_v9(&self, conn: &mut Connection) -> Result<()> {
        let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
        let file_stem = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("jkbot.sqlite3");
        let backup_path = self
            .path
            .with_file_name(format!("{file_stem}.pre-v9-backup-{stamp}"));
        if let Err(error) = conn.execute(
            "VACUUM INTO ?1",
            params![backup_path.to_string_lossy().to_string()],
        ) {
            eprintln!("v8→v9 迁移前整库快照失败（纯增量建索引，继续）：{error}");
        }

        let tx = conn
            .transaction()
            .context("begin v8→v9 migration transaction")?;
        runtime::extend_schema(&tx)?;
        tx.pragma_update(None, "user_version", 9)
            .context("advance user_version to 9")?;
        tx.commit().context("commit v8→v9 migration")
    }

    /// v9 → v10：dispatcher_settings 新增 `project_verifier_model_configs_json`
    /// 列——执行图验收模型的独立用途槽位（此前验收复用摘要槽位，模型网关
    /// 故障时无法单独替换）。纯增量加列（带默认 '[]'）、零数据迁移；
    /// `ALTER TABLE ... ADD COLUMN` 在事务内失败整体回滚（user_version 不
    /// 推进），重试安全幂等。按规范仍先做整库快照备份（VACUUM INTO 不能在
    /// 事务内执行），备份失败只留痕不阻断。
    fn migrate_v9_to_v10(&self, conn: &mut Connection) -> Result<()> {
        let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
        let file_stem = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("jkbot.sqlite3");
        let backup_path = self
            .path
            .with_file_name(format!("{file_stem}.pre-v10-backup-{stamp}"));
        if let Err(error) = conn.execute(
            "VACUUM INTO ?1",
            params![backup_path.to_string_lossy().to_string()],
        ) {
            eprintln!("v9→v10 迁移前整库快照失败（纯增量加列，继续）：{error}");
        }

        let tx = conn
            .transaction()
            .context("begin v9→v10 migration transaction")?;
        tx.execute(
            "ALTER TABLE dispatcher_settings
             ADD COLUMN project_verifier_model_configs_json TEXT NOT NULL DEFAULT '[]'",
            [],
        )
        .context("add dispatcher_settings.project_verifier_model_configs_json column")?;
        tx.pragma_update(None, "user_version", 10)
            .context("advance user_version to 10")?;
        tx.commit().context("commit v9→v10 migration")
    }

    /// v10 → v11：graph_runs 新增执行结果 4 列（结论节点 id / 结论 md 快照 /
    /// 结果类型 / 修改文件并集 JSON）。纯增量加列（带默认值）、零数据迁移；
    /// 历史 run 无结果记录，读取层按列默认值呈现 unknown/空清单。事务内失败
    /// 整体回滚（user_version 不推进），重试安全幂等。按规范仍先做整库快照
    /// 备份（VACUUM INTO 不能在事务内执行），备份失败只留痕不阻断。
    fn migrate_v10_to_v11(&self, conn: &mut Connection) -> Result<()> {
        let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
        let file_stem = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("jkbot.sqlite3");
        let backup_path = self
            .path
            .with_file_name(format!("{file_stem}.pre-v11-backup-{stamp}"));
        if let Err(error) = conn.execute(
            "VACUUM INTO ?1",
            params![backup_path.to_string_lossy().to_string()],
        ) {
            eprintln!("v10→v11 迁移前整库快照失败（纯增量加列，继续）：{error}");
        }

        let tx = conn
            .transaction()
            .context("begin v10→v11 migration transaction")?;
        tx.execute(
            "ALTER TABLE graph_runs ADD COLUMN conclusion_node_id TEXT",
            [],
        )
        .context("add graph_runs.conclusion_node_id column")?;
        tx.execute("ALTER TABLE graph_runs ADD COLUMN conclusion_md TEXT", [])
            .context("add graph_runs.conclusion_md column")?;
        tx.execute(
            "ALTER TABLE graph_runs
             ADD COLUMN result_kind TEXT NOT NULL DEFAULT 'unknown'",
            [],
        )
        .context("add graph_runs.result_kind column")?;
        tx.execute(
            "ALTER TABLE graph_runs
             ADD COLUMN modified_files_json TEXT NOT NULL DEFAULT '[]'",
            [],
        )
        .context("add graph_runs.modified_files_json column")?;
        tx.pragma_update(None, "user_version", 11)
            .context("advance user_version to 11")?;
        tx.commit().context("commit v10→v11 migration")
    }

    /// v11 → v12：dispatcher_settings 由 20 列宽表收敛为 `(id, settings_json)`
    /// 单行 JSON——整对象序列化 stored-form `AhaSettingsV2`（列→字段映射与
    /// 「剥离库引用凭据」的既有落库约定一致，读取路径照旧由库条目回填）。
    /// 消除 19 参 upsert 与「列序调整即 row.get(N) 静默错位」的脆弱面。
    ///
    /// 旧行按列名容错读取（v3 前无 theme、v10 前无 verifier 列的更旧库沿
    /// 迁移链到达本块时已是 20 列，但防御性按名兜底不依赖该前提）。
    /// DROP 属破坏性变更：迁移前按规范 VACUUM INTO 整库快照（不能在事务内
    /// 执行），备份失败只留痕不阻断。幂等可重试：settings_json 列已存在
    /// （或表本就不存在/已是新形态）则只建表/推进版本号。
    fn migrate_v11_to_v12(&self, conn: &mut Connection) -> Result<()> {
        let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
        let file_stem = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("jkbot.sqlite3");
        let backup_path = self
            .path
            .with_file_name(format!("{file_stem}.pre-v12-backup-{stamp}"));
        if let Err(error) = conn.execute(
            "VACUUM INTO ?1",
            params![backup_path.to_string_lossy().to_string()],
        ) {
            eprintln!("v11→v12 迁移前整库快照失败（数据经映射全量保留，继续）：{error}");
        }

        let tx = conn
            .transaction()
            .context("begin v11→v12 migration transaction")?;
        let settings_table_exists: i64 = tx.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'dispatcher_settings'",
            [],
            |row| row.get(0),
        )?;
        if settings_table_exists == 0 {
            tx.execute_batch(
                "CREATE TABLE dispatcher_settings (
                    id TEXT PRIMARY KEY DEFAULT 'default',
                    settings_json TEXT NOT NULL
                );",
            )
            .context("create dispatcher_settings json table")?;
        } else {
            let json_shape: i64 = tx.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('dispatcher_settings')
                 WHERE name = 'settings_json'",
                [],
                |row| row.get(0),
            )?;
            if json_shape == 0 {
                // 旧行 → stored-form AhaSettingsV2 → 整对象 JSON。
                let legacy_row: Option<super::settings::AhaSettingsV2> = tx
                    .query_row(
                        "SELECT * FROM dispatcher_settings WHERE id = 'default'",
                        [],
                        super::settings::legacy_settings_from_row,
                    )
                    .optional()
                    .context("load legacy dispatcher settings for v12 migration")?;
                tx.execute_batch(
                    "CREATE TABLE dispatcher_settings_v12 (
                        id TEXT PRIMARY KEY DEFAULT 'default',
                        settings_json TEXT NOT NULL
                    );",
                )
                .context("create dispatcher_settings_v12 table")?;
                if let Some(settings) = legacy_row {
                    let json = serde_json::to_string(&settings)
                        .context("serialize legacy dispatcher settings to json")?;
                    tx.execute(
                        "INSERT INTO dispatcher_settings_v12 (id, settings_json)
                         VALUES ('default', ?1)",
                        params![&json],
                    )
                    .context("insert migrated dispatcher settings json")?;
                }
                tx.execute_batch(
                    "DROP TABLE dispatcher_settings;
                     ALTER TABLE dispatcher_settings_v12 RENAME TO dispatcher_settings;",
                )
                .context("replace legacy dispatcher_settings with json table")?;
            }
        }
        tx.pragma_update(None, "user_version", 12)
            .context("advance user_version to 12")?;
        tx.commit().context("commit v11→v12 migration")
    }

    /// v12 → v13：删除 chat_sessions / project_sessions 两张列表读模型子表，
    /// 会话写入路径的双写收敛为单写 dispatcher_sessions。DROP 前对统一表做
    /// 防孤儿回填（INSERT OR IGNORE：id 已存在的行保留统一表版本），子表独有
    /// 的孤儿行得以保留而非随表丢弃。破坏性变更，迁移前 VACUUM INTO 整库
    /// 快照，备份失败只留痕不阻断。幂等可重试：表已不存在则跳过该表。
    fn migrate_v12_to_v13(&self, conn: &mut Connection) -> Result<()> {
        let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
        let file_stem = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("jkbot.sqlite3");
        let backup_path = self
            .path
            .with_file_name(format!("{file_stem}.pre-v13-backup-{stamp}"));
        if let Err(error) = conn.execute(
            "VACUUM INTO ?1",
            params![backup_path.to_string_lossy().to_string()],
        ) {
            eprintln!("v12→v13 迁移前整库快照失败（子表数据经回填保留，继续）：{error}");
        }

        let tx = conn
            .transaction()
            .context("begin v12→v13 migration transaction")?;
        let chat_table_exists: i64 = tx.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'chat_sessions'",
            [],
            |row| row.get(0),
        )?;
        if chat_table_exists > 0 {
            tx.execute(
                "INSERT OR IGNORE INTO dispatcher_sessions
                     (id, project_id, kind, title, category, created_at, updated_at)
                 SELECT id, '__global_chat__', 'chat', title, category, created_at, updated_at
                 FROM chat_sessions",
                [],
            )
            .context("backfill chat sessions into dispatcher_sessions")?;
            tx.execute_batch("DROP TABLE chat_sessions;")
                .context("drop chat_sessions read model")?;
        }
        let project_table_exists: i64 = tx.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'project_sessions'",
            [],
            |row| row.get(0),
        )?;
        if project_table_exists > 0 {
            tx.execute(
                "INSERT OR IGNORE INTO dispatcher_sessions
                     (id, project_id, kind, title, category, created_at, updated_at)
                 SELECT id, project_id, 'project', title, '', created_at, updated_at
                 FROM project_sessions",
                [],
            )
            .context("backfill project sessions into dispatcher_sessions")?;
            tx.execute_batch("DROP TABLE project_sessions;")
                .context("drop project_sessions read model")?;
        }
        // 极简/异常库形态（如测试夹具手工建的最小会话表）可能缺 kind 列：
        // 按列存在性守卫，缺列时跳过建索引——真实 v12 库的 dispatcher_sessions
        // 必有 kind（v1 基线起就存在）。
        let kind_column_exists: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('dispatcher_sessions')
                 WHERE name = 'kind'",
                [],
                |row| row.get(0),
            )
            .context("inspect dispatcher_sessions.kind column")?;
        if kind_column_exists > 0 {
            tx.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_dispatcher_sessions_kind_category_updated
                 ON dispatcher_sessions(kind, category, updated_at DESC, id DESC);",
            )
            .context("create dispatcher sessions listing index")?;
        }
        tx.pragma_update(None, "user_version", 13)
            .context("advance user_version to 13")?;
        tx.commit().context("commit v12→v13 migration")
    }

    /// v13 → v14：删除三个无读写方的死列——dispatcher_messages.visible
    /// （写侧恒 1、读侧谓词恒真）、dispatcher_tool_runs.action_kind、
    /// graph_node_runs.special_tools_json（后两者 v8/v4 起零引用）。
    /// 三列均无索引/触发器/CHECK/FK 牵连，ALTER TABLE DROP COLUMN（SQLite
    /// ≥ 3.35）即可，无需 v6→v7 式表重建。破坏性变更，迁移前 VACUUM INTO
    /// 整库快照，备份失败只留痕不阻断。幂等可重试：列已不存在则跳过。
    fn migrate_v13_to_v14(&self, conn: &mut Connection) -> Result<()> {
        let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
        let file_stem = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("jkbot.sqlite3");
        let backup_path = self
            .path
            .with_file_name(format!("{file_stem}.pre-v14-backup-{stamp}"));
        if let Err(error) = conn.execute(
            "VACUUM INTO ?1",
            params![backup_path.to_string_lossy().to_string()],
        ) {
            eprintln!("v13→v14 迁移前整库快照失败（纯删死列，继续）：{error}");
        }

        let tx = conn
            .transaction()
            .context("begin v13→v14 migration transaction")?;
        for (table, column) in [
            ("dispatcher_messages", "visible"),
            ("dispatcher_tool_runs", "action_kind"),
            ("graph_node_runs", "special_tools_json"),
        ] {
            let column_exists: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
                    params![table, column],
                    |row| row.get(0),
                )
                .with_context(|| format!("inspect {table}.{column} before drop"))?;
            if column_exists > 0 {
                tx.execute_batch(&format!("ALTER TABLE {table} DROP COLUMN {column};"))
                    .with_context(|| format!("drop dead column {table}.{column}"))?;
            }
        }
        tx.pragma_update(None, "user_version", 14)
            .context("advance user_version to 14")?;
        tx.commit().context("commit v13→v14 migration")
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
    runtime::extend_schema(&tx)?;
    tx.execute_batch(SESSION_SUMMARIES_DDL)
        .context("initialize session summaries table")?;

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
/// `dispatcher_session_summaries`（v6 新增）的 DDL 在
/// `SESSION_SUMMARIES_DDL`，基线与 v5→v6 迁移共用同一出处。
const SESSION_SUMMARIES_DDL: &str = "
-- 历史级滚动压缩的跨 run 持久化（rig_ext::context::compact_history）：
-- 同会话只保留最新一条滚动摘要；锚点为覆盖范围内最近一条已知消息 id，
-- 锚点被截断/删除即摘要失效（读取路径即读即删，purge/truncate 级联清理）。
CREATE TABLE IF NOT EXISTS dispatcher_session_summaries (
    workspace_id TEXT PRIMARY KEY,
    summary TEXT NOT NULL,
    covered_through_message_id TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
";

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
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_dispatcher_messages_workspace_created
ON dispatcher_messages(workspace_id, created_at);
CREATE INDEX IF NOT EXISTS idx_dispatcher_messages_workspace_role_created
ON dispatcher_messages(workspace_id, role, created_at);

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

-- v12 起：单行 JSON 形态——整对象序列化 AhaSettingsV2（落库剥离库引用凭据，
-- 读取由库条目回填）。列形态变更见 migrate_v11_to_v12。
CREATE TABLE IF NOT EXISTS dispatcher_settings (
    id TEXT PRIMARY KEY DEFAULT 'default',
    settings_json TEXT NOT NULL
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

-- 会话统一锚点（v13 起单表）：消息/关键词/轨迹的外键目标，两类会话列表
-- 直接按 kind 过滤读取（chat 列表走下方 kind+category 索引做 keyset 分页）。
CREATE INDEX IF NOT EXISTS idx_dispatcher_sessions_kind_category_updated
ON dispatcher_sessions(kind, category, updated_at DESC, id DESC);

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
    -- 执行结果（v11）：结论节点 id 与其输出 md 快照（结论节点未成功为 NULL）、
    -- review|edit|unknown 结果类型、修改文件并集（JSON 数组，BTreeSet 排序）。
    conclusion_node_id TEXT,
    conclusion_md TEXT,
    result_kind TEXT NOT NULL DEFAULT 'unknown',
    modified_files_json TEXT NOT NULL DEFAULT '[]',
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
    // v12 起 dispatcher_settings 为整对象 JSON：默认聊天工具面从
    // chat.allowed_tools 读取（无设置行/解析失败回落空清单——与旧列形态
    // 「无行回落 '[]'」语义一致）。
    let raw: Option<String> = tx
        .query_row(
            "SELECT settings_json FROM dispatcher_settings WHERE id = 'default'",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("load default chat agent config")?;
    let allowed_tools_json = raw
        .and_then(|json| serde_json::from_str::<super::settings::AhaSettingsV2>(&json).ok())
        .map(|settings| {
            serde_json::to_string(&settings.chat.allowed_tools).unwrap_or_else(|_| "[]".to_string())
        })
        .unwrap_or_else(|| "[]".to_string());
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
                "ssh_memo_read",
                "ssh_memo_upsert",
                "ssh_memo_delete",
                "list_sub_agents",
                "call_sub_agent",
            ],
            system_prompt: r#"# 技术助手

你是面向工程问题的技术助手，适合排查错误、解释代码、验证命令、阅读文档和推进技术方案。

工作方式：
- 事实优先，必要时用浏览器查看官方文档或公开资料；不要编造 API、参数或版本信息。
- 本地验证优先使用 local_zsh，并保持命令小步、可复现、可审计。
- 远程命令必须先确认目标服务器；涉及写入、删除、部署、重启等操作前必须说明影响并等待用户确认。
- 对不熟悉的服务器，先读运维备忘录（ssh_memo_read）获取部署路径、特殊命令方式与已知问题；解决新问题或发现新路径/特殊命令后，克制地更新备忘录（先读后写、整段重写，不记录凭据）。
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
    /// v9 形态的 dispatcher_settings 宽表（19 列，含 theme、无 v10 验收槽位
    /// 列）：版本号 ≤ v9 的夹具沿链需真实走到 v9→v10 加列与 v11→v12 的
    /// JSON 化迁移。基线 DDL 已是 (id, settings_json) 形态，不能再从基线提取。
    const LEGACY_SETTINGS_DDL_V9: &str = "CREATE TABLE IF NOT EXISTS dispatcher_settings (
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
    );";

    // 旧迁移夹具仅建被测表；补齐迁移链依赖的既有表，不覆盖旧表形态。
    fn complete_runtime_fixture(path: &std::path::Path) {
        let conn = rusqlite::Connection::open(path).unwrap();
        for table in [
            "dispatcher_sessions",
            "dispatcher_messages",
            "dispatcher_tool_runs",
            "graph_runs",
        ] {
            let prefix = format!("CREATE TABLE IF NOT EXISTS {table} (");
            let start = super::BASELINE_DDL.find(&prefix).unwrap();
            let tail = &super::BASELINE_DDL[start..];
            let end = tail.find("\n);").unwrap() + 3;
            conn.execute_batch(&tail[..end]).unwrap();
        }
        // dispatcher_settings 建成 v9 形态宽表：这些 fixture 的版本号 ≤ v9，
        // 打开时需能走到 v9→v10 加列迁移与 v11→v12 JSON 化迁移。旧版本
        // fixture 手写的 dispatcher_settings（v1/v2 形态）IF NOT EXISTS 跳过。
        conn.execute_batch(LEGACY_SETTINGS_DDL_V9).unwrap();
        // graph_runs 建成 v10 形态（删掉 v11 新增的执行结果列）：迁移链现在
        // 触及该表，fixture 版本号 ≤ v10，打开时需能走到 v10→v11 的加列迁移。
        revert_v11_graph_run_columns(&conn);
    }

    /// 把 graph_runs 回退到 v10 形态：按列存在性删除 v11 新增的执行结果列
    /// （基线 DDL 建出的是当前形态；graph_runs 表不存在的 fixture 查询
    /// pragma_table_info 返回空集，天然跳过）。与上方 v10 列回退同一约定。
    fn revert_v11_graph_run_columns(conn: &rusqlite::Connection) {
        for column in [
            "conclusion_node_id",
            "conclusion_md",
            "result_kind",
            "modified_files_json",
        ] {
            let exists: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('graph_runs')
                         WHERE name = '{column}'"
                    ),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            if exists > 0 {
                conn.execute_batch(&format!("ALTER TABLE graph_runs DROP COLUMN {column};"))
                    .unwrap();
            }
        }
    }

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
                CREATE TABLE projects (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    path TEXT NOT NULL UNIQUE,
                    branch TEXT,
                    last_opened_at INTEGER NOT NULL DEFAULT 0,
                    sort_order INTEGER NOT NULL DEFAULT 0
                );
                INSERT INTO projects (id, name, path, branch, last_opened_at)
                VALUES ('p-1', '项目一', '/tmp/p1', 'main', 1);
                PRAGMA user_version = 1;",
            )
            .unwrap();
        }

        complete_runtime_fixture(&path);
        let db = DispatcherDb::new(path.clone()).unwrap();
        {
            let conn = db.conn().unwrap();
            let version: i32 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, super::SCHEMA_VERSION);

            // v3→v4：projects 死列 branch 已删，行数据保留。
            let branch_columns: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('projects') WHERE name = 'branch'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(branch_columns, 0, "branch 死列应随迁移删除");
            let (project_id, project_name): (String, String) = conn
                .query_row(
                    "SELECT id, name FROM projects WHERE id = 'p-1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(
                (project_id.as_str(), project_name.as_str()),
                ("p-1", "项目一")
            );

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

            // v2→v3：旧主题搬移进设置（v12 起整对象存 settings_json），
            // 旧键删除。
            let theme = db.get_settings_v2().unwrap().theme;
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
                && (name.contains("pre-v2-backup")
                    || name.contains("pre-v3-backup")
                    || name.contains("pre-v4-backup")
                    || name.contains("pre-v5-backup")
                    || name.contains("pre-v6-backup")
                    || name.contains("pre-v7-backup")
                    || name.contains("pre-v8-backup")
                    || name.contains("pre-v9-backup")
                    || name.contains("pre-v10-backup")
                    || name.contains("pre-v11-backup")
                    || name.contains("pre-v12-backup")
                    || name.contains("pre-v13-backup")
                    || name.contains("pre-v14-backup"))
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// v3 库打开时迁移到 v4：projects 死列 `branch` 删除、行数据全量保留、
    /// 快照备份生成、重复打开幂等。
    #[test]
    fn v3_database_drops_projects_branch_dead_column() {
        let path = temp_db_path("v3-to-v4");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE projects (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    path TEXT NOT NULL UNIQUE,
                    branch TEXT,
                    last_opened_at INTEGER NOT NULL DEFAULT 0,
                    sort_order INTEGER NOT NULL DEFAULT 0
                );
                INSERT INTO projects (id, name, path, branch, last_opened_at)
                VALUES ('p-1', '项目一', '/tmp/p1', 'main', 1);
                PRAGMA user_version = 3;",
            )
            .unwrap();
        }

        complete_runtime_fixture(&path);
        let db = DispatcherDb::new(path.clone()).unwrap();
        {
            let conn = db.conn().unwrap();
            let version: i32 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, super::SCHEMA_VERSION);

            let branch_columns: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('projects') WHERE name = 'branch'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(branch_columns, 0, "branch 死列应随迁移删除");
            let project_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
                .unwrap();
            assert_eq!(project_count, 1, "projects 行数据应全量保留");
        }
        // 重开幂等：已是 v4 的库直接打开，不再触发任何迁移。
        drop(db);
        let _ = DispatcherDb::new(path.clone());
        cleanup_db_files(&path);
        let dir = path.parent().unwrap();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.contains("v3-to-v4-")
                && (name.contains("pre-v4-backup")
                    || name.contains("pre-v5-backup")
                    || name.contains("pre-v6-backup")
                    || name.contains("pre-v7-backup")
                    || name.contains("pre-v8-backup")
                    || name.contains("pre-v9-backup")
                    || name.contains("pre-v10-backup")
                    || name.contains("pre-v11-backup")
                    || name.contains("pre-v12-backup")
                    || name.contains("pre-v13-backup")
                    || name.contains("pre-v14-backup"))
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// v4 库打开时迁移到 v5：sub_agent_run_traces 补可空 `model` 列、
    /// 既有行数据全量保留（model 为 NULL）、快照备份生成、重复打开幂等。
    #[test]
    fn v4_database_adds_sub_agent_trace_model_column() {
        let path = temp_db_path("v4-to-v5");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE dispatcher_sessions (id TEXT PRIMARY KEY);
                INSERT INTO dispatcher_sessions (id) VALUES ('ws-1');
                CREATE TABLE sub_agent_run_traces (
                    workspace_id TEXT NOT NULL,
                    tool_call_id TEXT NOT NULL,
                    agent_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    events_json TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY (workspace_id, tool_call_id),
                    FOREIGN KEY (workspace_id) REFERENCES dispatcher_sessions(id) ON DELETE CASCADE
                );
                INSERT INTO sub_agent_run_traces (
                    workspace_id, tool_call_id, agent_id, status,
                    events_json, created_at, updated_at
                ) VALUES (
                    'ws-1', 'tc-1', 'browser-agent', 'completed',
                    '[]', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'
                );
                PRAGMA user_version = 4;",
            )
            .unwrap();
        }

        complete_runtime_fixture(&path);
        let db = DispatcherDb::new(path.clone()).unwrap();
        {
            let conn = db.conn().unwrap();
            let version: i32 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, super::SCHEMA_VERSION);

            let model_columns: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('sub_agent_run_traces') WHERE name = 'model'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(model_columns, 1, "model 列应随迁移补齐");
            let (status, model): (String, Option<String>) = conn
                .query_row(
                    "SELECT status, model FROM sub_agent_run_traces WHERE tool_call_id = 'tc-1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(status, "completed", "既有行数据应全量保留");
            assert_eq!(
                model, None,
                "老轨迹行 model 应为 NULL（前端「未记录」兜底）"
            );
        }
        // 重开幂等：已是 v5 的库直接打开，不再触发任何迁移。
        drop(db);
        let _ = DispatcherDb::new(path.clone());
        cleanup_db_files(&path);
        let dir = path.parent().unwrap();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.contains("v4-to-v5-")
                && (name.contains("pre-v5-backup")
                    || name.contains("pre-v6-backup")
                    || name.contains("pre-v7-backup")
                    || name.contains("pre-v8-backup")
                    || name.contains("pre-v9-backup")
                    || name.contains("pre-v10-backup")
                    || name.contains("pre-v11-backup")
                    || name.contains("pre-v12-backup")
                    || name.contains("pre-v13-backup")
                    || name.contains("pre-v14-backup"))
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// v5 库打开时迁移到 v6：新增 dispatcher_session_summaries 表、
    /// 快照备份生成、重复打开幂等；摘要读路径的锚点校验（失效即删）。
    #[test]
    fn v5_database_adds_session_summaries_table() {
        let path = temp_db_path("v5-to-v6");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE dispatcher_sessions (id TEXT PRIMARY KEY);
                CREATE TABLE dispatcher_messages (
                    id TEXT PRIMARY KEY,
                    workspace_id TEXT NOT NULL
                );
                PRAGMA user_version = 5;",
            )
            .unwrap();
        }

        complete_runtime_fixture(&path);
        let db = DispatcherDb::new(path.clone()).unwrap();
        {
            let conn = db.conn().unwrap();
            let version: i32 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, super::SCHEMA_VERSION);
        }
        // 锚点消息不存在 → 摘要失效，即读即删。
        db.upsert_session_summary("ws-1", "摘要正文", "msg-1")
            .unwrap();
        assert!(db.valid_session_summary("ws-1").unwrap().is_none());
        // 锚点存在 → 正常读回（upsert 冲突更新同一会话行）。
        {
            let conn = db.conn().unwrap();
            conn.execute(
                "INSERT INTO dispatcher_messages (id, workspace_id) VALUES ('msg-1', 'ws-1')",
                [],
            )
            .unwrap();
        }
        db.upsert_session_summary("ws-1", "摘要正文", "msg-1")
            .unwrap();
        let record = db
            .valid_session_summary("ws-1")
            .unwrap()
            .expect("锚点存在应读回摘要");
        assert_eq!(record.summary, "摘要正文");
        assert_eq!(record.covered_through_message_id, "msg-1");

        // 重开幂等：已是 v6 的库直接打开，不再触发任何迁移。
        drop(db);
        let _ = DispatcherDb::new(path.clone());
        cleanup_db_files(&path);
        let dir = path.parent().unwrap();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.contains("v5-to-v6-")
                && (name.contains("pre-v6-backup")
                    || name.contains("pre-v7-backup")
                    || name.contains("pre-v8-backup")
                    || name.contains("pre-v9-backup")
                    || name.contains("pre-v10-backup")
                    || name.contains("pre-v11-backup")
                    || name.contains("pre-v12-backup")
                    || name.contains("pre-v13-backup")
                    || name.contains("pre-v14-backup"))
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// v6 库打开时迁移到 v7：dispatcher_messages 死列 `context_cleared` 删除、
    /// 数据全量保留、引用父表的子表行不被级联误删（DROP 前关闭 foreign_keys）、
    /// 快照备份生成、重复打开幂等。
    #[test]
    fn v6_database_drops_context_cleared_column() {
        let path = temp_db_path("v6-to-v7");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "PRAGMA foreign_keys = ON;
                CREATE TABLE dispatcher_sessions (id TEXT PRIMARY KEY);
                INSERT INTO dispatcher_sessions (id) VALUES ('ws-1');
                CREATE TABLE dispatcher_messages (
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
                INSERT INTO dispatcher_messages (id, workspace_id, role, created_at)
                VALUES ('m-1', 'ws-1', 'user', '2026-01-01T00:00:00Z');
                INSERT INTO dispatcher_messages (id, workspace_id, role, created_at)
                VALUES ('m-2', 'ws-1', 'assistant', '2026-01-01T00:00:01Z');
                CREATE TABLE chat_images (
                    id TEXT PRIMARY KEY,
                    image_id TEXT NOT NULL UNIQUE,
                    workspace_id TEXT NOT NULL,
                    message_id TEXT,
                    segment_index INTEGER NOT NULL DEFAULT 0,
                    path TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY (message_id) REFERENCES dispatcher_messages(id) ON DELETE CASCADE
                );
                INSERT INTO chat_images (id, image_id, workspace_id, message_id, path, created_at)
                VALUES ('ci-1', 'img-1', 'ws-1', 'm-1', '/tmp/x.png', '2026-01-01T00:00:00Z');
                PRAGMA user_version = 6;",
            )
            .unwrap();
        }

        complete_runtime_fixture(&path);
        let db = DispatcherDb::new(path.clone()).unwrap();
        {
            let conn = db.conn().unwrap();
            let version: i32 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, super::SCHEMA_VERSION);

            let legacy_columns: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('dispatcher_messages') WHERE name = 'context_cleared'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(legacy_columns, 0, "context_cleared 列应被删除");

            let message_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM dispatcher_messages", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(message_count, 2, "消息行应全量保留");
            let image_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM chat_images", [], |row| row.get(0))
                .unwrap();
            assert_eq!(
                image_count, 1,
                "子表行不得被 DROP 父表的隐式 DELETE 级联误删"
            );

            // 时间顺序保持（rowid 相对顺序不变）：m-1 先于 m-2。
            let first_id: String = conn
                .query_row(
                    "SELECT id FROM dispatcher_messages ORDER BY rowid ASC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(first_id, "m-1");
        }
        // 重开幂等：已是 v7 的库直接打开，不再触发任何迁移。
        drop(db);
        let _ = DispatcherDb::new(path.clone());
        cleanup_db_files(&path);
        let dir = path.parent().unwrap();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.contains("v6-to-v7-")
                && (name.contains("pre-v7-backup")
                    || name.contains("pre-v8-backup")
                    || name.contains("pre-v9-backup")
                    || name.contains("pre-v10-backup")
                    || name.contains("pre-v11-backup")
                    || name.contains("pre-v12-backup")
                    || name.contains("pre-v13-backup")
                    || name.contains("pre-v14-backup"))
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// v11 库（settings 宽表 + chat/project 双读模型子表 + 三个死列就位）沿链
    /// 迁移到 v14：settings 整对象 JSON 化、两类会话并入统一表（含子表孤儿行
    /// 回填）、死列删除；列表读取与新建写入路径照常工作。
    #[test]
    fn v11_database_migrates_settings_sessions_and_dead_columns() {
        let path = temp_db_path("v11-to-v14");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.pragma_update(None, "foreign_keys", "ON").unwrap();
            conn.execute_batch(
                "CREATE TABLE dispatcher_sessions (
                    id TEXT PRIMARY KEY,
                    project_id TEXT NOT NULL,
                    kind TEXT NOT NULL DEFAULT 'project',
                    title TEXT NOT NULL,
                    category TEXT NOT NULL DEFAULT '',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                INSERT INTO dispatcher_sessions
                    (id, project_id, kind, title, category, created_at, updated_at)
                VALUES ('chat-1', '__global_chat__', 'chat', '同步过的会话', 'tech',
                        '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
                CREATE TABLE chat_sessions (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL,
                    category TEXT NOT NULL DEFAULT 'tech',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                INSERT INTO chat_sessions (id, title, category, created_at, updated_at)
                VALUES ('chat-1', '同步过的会话', 'tech',
                        '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
                -- 子表孤儿行：统一表缺失该 id，v13 回填必须保留。
                INSERT INTO chat_sessions (id, title, category, created_at, updated_at)
                VALUES ('chat-orphan', '孤儿会话', 'tech',
                        '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z');
                CREATE TABLE project_sessions (
                    id TEXT PRIMARY KEY,
                    project_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                INSERT INTO project_sessions (id, project_id, title, created_at, updated_at)
                VALUES ('proj-1', 'p-1', '项目会话',
                        '2026-01-01T00:00:00Z', '2026-01-03T00:00:00Z');
                CREATE TABLE dispatcher_messages (
                    id TEXT PRIMARY KEY,
                    workspace_id TEXT NOT NULL,
                    role TEXT NOT NULL,
                    segments_json TEXT NOT NULL DEFAULT '[]',
                    created_at TEXT NOT NULL,
                    visible INTEGER NOT NULL DEFAULT 1
                );
                INSERT INTO dispatcher_messages (id, workspace_id, role, segments_json, created_at)
                VALUES ('m-1', 'chat-1', 'user', '[]', '2026-01-01T00:00:00Z');
                CREATE TABLE dispatcher_tool_runs (
                    id TEXT PRIMARY KEY,
                    workspace_id TEXT NOT NULL,
                    tool_call_id TEXT NOT NULL,
                    tool_name TEXT NOT NULL,
                    provider TEXT NOT NULL,
                    category TEXT NOT NULL,
                    status TEXT NOT NULL,
                    action_kind TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                INSERT INTO dispatcher_tool_runs
                    (id, workspace_id, tool_call_id, tool_name, provider, category, status,
                     created_at, updated_at)
                VALUES ('run-1', 'chat-1', 'call-1', 'read_file', 'builtin', 'fs', 'succeeded',
                        '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
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
                    PRIMARY KEY(run_id, node_id)
                );
                -- 列表读取路径（list_*_sessions_paginated）会按会话批量查关键字，
                -- 夹具补上最小形态的 session_keywords 表。
                CREATE TABLE session_keywords (
                    session_id TEXT NOT NULL,
                    keyword TEXT NOT NULL,
                    weight REAL NOT NULL DEFAULT 1.0,
                    created_at TEXT NOT NULL,
                    PRIMARY KEY (session_id, keyword)
                );
                PRAGMA user_version = 11;",
            )
            .unwrap();
            // v11 形态 settings = v9 宽表 + v10 验收槽位列。
            conn.execute_batch(LEGACY_SETTINGS_DDL_V9).unwrap();
            conn.execute_batch(
                "ALTER TABLE dispatcher_settings
                   ADD COLUMN project_verifier_model_configs_json TEXT NOT NULL DEFAULT '[]';
                 INSERT INTO dispatcher_settings (id, theme, context_debug, project_chat_model_configs_json)
                 VALUES ('default', 'dark', 1,
                         '[{\"url\":\"https://api.example.com/v1\",\"apiKey\":\"k\",\"model\":\"m\",\"active\":true}]');",
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

            // 两张读模型子表删除；统一表 = 双写行 + 子表孤儿回填。
            for table in ["chat_sessions", "project_sessions"] {
                let exists: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?1",
                        [table],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(exists, 0, "{table} 应随 v13 迁移删除");
            }
            let sessions: i64 = conn
                .query_row("SELECT COUNT(*) FROM dispatcher_sessions", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(sessions, 3, "双写行 + 子表孤儿回填共 3 行");

            // 三个死列全部删除。
            for (table, column) in [
                ("dispatcher_messages", "visible"),
                ("dispatcher_tool_runs", "action_kind"),
                ("graph_node_runs", "special_tools_json"),
            ] {
                let left: i64 = conn
                    .query_row(
                        &format!(
                            "SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = '{column}'"
                        ),
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(left, 0, "{table}.{column} 死列应随 v14 删除");
            }

            // settings：宽表行已收敛为 settings_json 单列。
            let json_columns: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('dispatcher_settings')
                     WHERE name = 'settings_json'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(json_columns, 1, "v12 后应只剩 settings_json 数据列");
        }
        let settings = db.get_settings_v2().unwrap();
        assert_eq!(settings.theme, "dark", "宽表 theme 应随 JSON 化保留");
        assert!(settings.context_debug);
        assert_eq!(settings.project.chat_model_configs.len(), 1);
        assert_eq!(settings.project.chat_model_configs[0].model, "m");

        // 列表读取走统一表：默认列表按分类过滤、包含回填的孤儿行。
        let page = db
            .list_chat_sessions_paginated(Some("tech"), None, 20)
            .unwrap();
        assert_eq!(page.total, 2);
        let titles: Vec<&str> = page.items.iter().map(|s| s.title.as_str()).collect();
        assert!(titles.contains(&"同步过的会话"));
        assert!(titles.contains(&"孤儿会话"));
        let project_page = db.list_project_sessions_paginated("p-1", 0, 20).unwrap();
        assert_eq!(project_page.total, 1);
        assert_eq!(project_page.items[0].id, "proj-1");

        // 新建会话走单写统一表路径。
        let created = db.create_chat_session("新建会话", Some("tech")).unwrap();
        let conn = db.conn().unwrap();
        let rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatcher_sessions WHERE id = ?1",
                [&created.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rows, 1);
        drop(conn);
        drop(db);
        cleanup_db_files(&path);
        let dir = path.parent().unwrap();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.contains("v11-to-v14-")
                && (name.contains("pre-v12-backup")
                    || name.contains("pre-v13-backup")
                    || name.contains("pre-v14-backup"))
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// v2 库打开时沿链迁移：v2→v3 dispatcher_settings 补 theme 列（旧
    /// app_config `app_settings` 键存在时主题搬移进既有行），再 v3→v4 删除
    /// projects.branch 死列。fixture 含 projects 表以覆盖 DROP 路径。
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
                CREATE TABLE projects (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    path TEXT NOT NULL UNIQUE,
                    branch TEXT,
                    last_opened_at INTEGER NOT NULL DEFAULT 0,
                    sort_order INTEGER NOT NULL DEFAULT 0
                );
                PRAGMA user_version = 2;",
            )
            .unwrap();
        }

        complete_runtime_fixture(&path);
        let db = DispatcherDb::new(path.clone()).unwrap();
        {
            let conn = db.conn().unwrap();
            let version: i32 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, super::SCHEMA_VERSION);

            // 既有行保留原值，只追加 theme（v12 起整对象存 settings_json）；
            // 旧键删除。
            let settings = db.get_settings_v2().unwrap();
            assert_eq!(settings.theme, "light", "旧主题应搬移进既有设置行");
            assert!(settings.context_debug, "迁移不应改动其它设置");
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
            if name.contains("v2-to-v3-")
                && (name.contains("pre-v3-backup")
                    || name.contains("pre-v4-backup")
                    || name.contains("pre-v5-backup")
                    || name.contains("pre-v6-backup")
                    || name.contains("pre-v7-backup")
                    || name.contains("pre-v8-backup")
                    || name.contains("pre-v9-backup")
                    || name.contains("pre-v10-backup")
                    || name.contains("pre-v11-backup")
                    || name.contains("pre-v12-backup")
                    || name.contains("pre-v13-backup")
                    || name.contains("pre-v14-backup"))
            {
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

        complete_runtime_fixture(&path);
        let db = DispatcherDb::new(path.clone()).unwrap();
        let conn = db.conn().unwrap();
        // v12 起表为 (id, settings_json) 单行 JSON 形态。
        let json_columns: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('dispatcher_settings')
                 WHERE name = 'settings_json'",
                [],
                |row| row.get(0),
            )
            .expect("settings_json 列应存在");
        assert_eq!(json_columns, 1);
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM dispatcher_settings", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0, "无旧主题时迁移不应创建设置行");
        drop(conn);
        assert_eq!(
            db.get_settings_v2().unwrap().theme,
            "system",
            "无设置行时读取回落默认主题"
        );
        drop(db);
        cleanup_db_files(&path);
        let dir = path.parent().unwrap();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.contains("v2-to-v3-no-legacy-")
                && (name.contains("pre-v3-backup")
                    || name.contains("pre-v4-backup")
                    || name.contains("pre-v5-backup")
                    || name.contains("pre-v6-backup")
                    || name.contains("pre-v7-backup")
                    || name.contains("pre-v8-backup")
                    || name.contains("pre-v9-backup")
                    || name.contains("pre-v10-backup")
                    || name.contains("pre-v11-backup")
                    || name.contains("pre-v12-backup")
                    || name.contains("pre-v13-backup")
                    || name.contains("pre-v14-backup"))
            {
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

    /// 造一个 v8 形态的库：基线表 + 工具完成事件表（`runtime::extend_schema`
    /// 的 DDL），随后删掉 v9 新增的索引与 v10 新增的验收槽位列、版本号置 8。
    /// 事件表结构在 v8/v9 之间无差异，索引是两者唯一区别。
    fn write_v8_runtime_fixture(path: &std::path::Path) {
        let mut conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(super::BASELINE_DDL).unwrap();
        let tx = conn.transaction().unwrap();
        super::runtime::extend_schema(&tx).unwrap();
        tx.commit().unwrap();
        conn.execute_batch(
            "DROP INDEX idx_tool_completions_delivery;
             DROP TABLE dispatcher_settings;
             PRAGMA user_version = 8;",
        )
        .unwrap();
        // v8 形态 settings = 19 列宽表（含 theme、无 v10 验收槽位列）：
        // 基线已是 (id, settings_json) JSON 形态，重建旧宽表供迁移链走
        // v9→v10 加列与 v11→v12 JSON 化。
        conn.execute_batch(LEGACY_SETTINGS_DDL_V9).unwrap();
        revert_v11_graph_run_columns(&conn);
    }

    /// 目标库的迁移前快照文件（与库文件同目录，`{库名}.{marker}-{stamp}`）；
    /// 按库名前缀过滤，避免与并行测试的快照互相干扰。
    fn backup_files(path: &std::path::Path, marker: &str) -> Vec<std::path::PathBuf> {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|entry| {
                let file = entry.file_name().unwrap().to_string_lossy().to_string();
                file.starts_with(&name) && file.contains(marker)
            })
            .collect()
    }

    /// v8 库（工具完成事件表尚无 `delivery_message_id` 索引）打开时前向迁移
    /// 到 v9：索引补齐且被删除触发器的反查用上、既有行全量保留、生成迁移前
    /// 快照。
    #[test]
    fn v8_database_gains_delivery_message_index() {
        let path = temp_db_path("v8-to-v9");
        write_v8_runtime_fixture(&path);
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "INSERT INTO dispatcher_tool_runs
                    (id, workspace_id, tool_call_id, tool_name, provider, category,
                     status, created_at, updated_at)
                 VALUES ('task', 'workspace', 'call', 'echo', 'builtin', 'general',
                    'succeeded', '2026-09-27T00:00:00Z', '2026-09-27T00:00:00Z');
                 INSERT INTO dispatcher_messages (id, workspace_id, role, created_at)
                 VALUES ('delivery', 'workspace', 'tool', '2026-09-27T00:00:00Z');
                 INSERT INTO dispatcher_tool_completions
                    (tool_run_id, agent_run_id, scope_id, status, display_content,
                     context_payload, result_mode, delivery_message_id,
                     observed_request_step, created_at)
                 VALUES ('task', 'run', 'scope', 'succeeded', 'out', '{}', 'inline',
                    'delivery', 3, '2026-09-27T00:00:00Z');",
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
            let indexes: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index'
                       AND name = 'idx_tool_completions_delivery'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(indexes, 1, "v9 迁移应补出投递消息索引");
            let (step, delivery): (Option<i64>, Option<String>) = conn
                .query_row(
                    "SELECT observed_request_step, delivery_message_id
                     FROM dispatcher_tool_completions",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(step, Some(3), "事件行应全量保留");
            assert_eq!(delivery.as_deref(), Some("delivery"));

            // 索引确实服务于删除触发器按 delivery_message_id 的反查。
            let mut plan = conn
                .prepare(
                    "EXPLAIN QUERY PLAN UPDATE dispatcher_tool_completions
                     SET observed_request_step = NULL WHERE delivery_message_id = 'delivery'",
                )
                .unwrap();
            let details = plan
                .query_map([], |row| row.get::<_, String>(3))
                .unwrap()
                .map(|row| row.unwrap())
                .collect::<Vec<_>>()
                .join(" ");
            assert!(
                details.contains("idx_tool_completions_delivery"),
                "反查应走投递消息索引，实际计划：{details}"
            );
        }
        drop(db);

        let backups = backup_files(&path, "pre-v9-backup");
        assert_eq!(backups.len(), 1, "迁移前应生成整库快照");
        let snapshot = rusqlite::Connection::open(&backups[0]).unwrap();
        assert_eq!(
            snapshot
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
                .unwrap(),
            8
        );
        drop(snapshot);
        for backup in backups {
            let _ = std::fs::remove_file(backup);
        }
        // v8 fixture 打开会继续走 v9→v10 迁移，其快照一并清理。
        for backup in backup_files(&path, "pre-v10-backup") {
            let _ = std::fs::remove_file(backup);
        }
        cleanup_db_files(&path);
    }

    /// 已迁到 v9 的库再次打开走同版本 fast path：版本号不变、不重跑迁移、
    /// 不再生成迁移前快照。
    #[test]
    fn reopened_v9_database_skips_migration() {
        let path = temp_db_path("v9-reopen");
        write_v8_runtime_fixture(&path);
        {
            let db = DispatcherDb::new(path.clone()).unwrap();
            drop(db);
        }
        assert_eq!(
            backup_files(&path, "pre-v9-backup").len(),
            1,
            "首次打开应完成 v8→v9 迁移"
        );

        let db = DispatcherDb::new(path.clone()).unwrap();
        let conn = db.conn().expect("db conn");
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);
        drop(conn);
        drop(db);

        let backups = backup_files(&path, "pre-v9-backup");
        assert_eq!(backups.len(), 1, "同版本直开不得重跑迁移或再生成快照");
        for backup in backups {
            let _ = std::fs::remove_file(backup);
        }
        // v8 fixture 首次打开会继续走 v9→v10→v11 迁移，其快照一并清理。
        for backup in backup_files(&path, "pre-v10-backup") {
            let _ = std::fs::remove_file(backup);
        }
        for backup in backup_files(&path, "pre-v11-backup") {
            let _ = std::fs::remove_file(backup);
        }
        cleanup_db_files(&path);
    }

    /// 造一个 v9 形态的库：settings 换回 19 列宽表（基线已是 JSON 单列形态）
    /// 、graph_runs 摘掉 v11 新列、版本号置 9，并预置一行带既有设置的
    /// dispatcher_settings（验证迁移零数据丢失）。
    fn write_v9_settings_fixture(path: &std::path::Path) {
        let mut conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(super::BASELINE_DDL).unwrap();
        let tx = conn.transaction().unwrap();
        super::runtime::extend_schema(&tx).unwrap();
        tx.commit().unwrap();
        conn.execute_batch("DROP TABLE dispatcher_settings;")
            .unwrap();
        conn.execute_batch(LEGACY_SETTINGS_DDL_V9).unwrap();
        conn.execute_batch(
            "INSERT INTO dispatcher_settings (id, project_summary_model_configs_json)
               VALUES ('default', '[{\"url\":\"http://u\",\"apiKey\":\"k\",\"model\":\"m\",\"active\":true}]');
             PRAGMA user_version = 9;",
        )
        .unwrap();
        revert_v11_graph_run_columns(&conn);
    }

    /// v9 库（dispatcher_settings 尚无验收槽位列）打开时前向迁移到 v10：
    /// 列补齐且默认 '[]'、既有设置行全量保留、版本推进、生成迁移前快照。
    #[test]
    fn v9_database_gains_verifier_slot_column() {
        let path = temp_db_path("v9-to-v10");
        write_v9_settings_fixture(&path);

        let db = DispatcherDb::new(path.clone()).unwrap();
        {
            let conn = db.conn().unwrap();
            let version: i32 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, super::SCHEMA_VERSION);
        }
        // v9→v10 补验收槽位（默认空）；v11→v12 JSON 化后经读取面验证既有
        // 摘要槽位设置全量保留。
        let settings = db.get_settings_v2().unwrap();
        assert!(
            settings.project.verifier_model_configs.is_empty(),
            "新验收槽位默认为空"
        );
        assert!(
            settings
                .project
                .summary_model_configs
                .iter()
                .any(|config| config.url == "http://u" && config.model == "m"),
            "既有摘要槽位设置应全量保留"
        );
        drop(db);

        let backups = backup_files(&path, "pre-v10-backup");
        assert_eq!(backups.len(), 1, "迁移前应生成整库快照");
        let snapshot = rusqlite::Connection::open(&backups[0]).unwrap();
        assert_eq!(
            snapshot
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
                .unwrap(),
            9
        );
        drop(snapshot);
        for backup in backups {
            let _ = std::fs::remove_file(backup);
        }
        cleanup_db_files(&path);
    }

    /// 已迁到 v10 的库再次打开走同版本 fast path：不重跑迁移、不再生成快照。
    #[test]
    fn reopened_v10_database_skips_migration() {
        let path = temp_db_path("v10-reopen");
        write_v9_settings_fixture(&path);
        {
            let db = DispatcherDb::new(path.clone()).unwrap();
            drop(db);
        }
        assert_eq!(
            backup_files(&path, "pre-v10-backup").len(),
            1,
            "首次打开应完成 v9→v10 迁移"
        );

        let db = DispatcherDb::new(path.clone()).unwrap();
        let conn = db.conn().expect("db conn");
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);
        drop(conn);
        drop(db);

        let backups = backup_files(&path, "pre-v10-backup");
        assert_eq!(backups.len(), 1, "同版本直开不得重跑迁移或再生成快照");
        for backup in backups {
            let _ = std::fs::remove_file(backup);
        }
        // v9 fixture 首次打开会继续走 v10→v11 迁移，其快照一并清理。
        for backup in backup_files(&path, "pre-v11-backup") {
            let _ = std::fs::remove_file(backup);
        }
        cleanup_db_files(&path);
    }

    /// v10 库（graph_runs 尚无执行结果列）打开时前向迁移到 v11：4 列补齐
    /// （结果类型默认 unknown、文件清单默认 '[]'）、既有行全量保留、版本推进、
    /// 生成迁移前快照。
    #[test]
    fn v10_database_gains_run_result_columns() {
        let path = temp_db_path("v10-to-v11");
        {
            let mut conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(super::BASELINE_DDL).unwrap();
            let tx = conn.transaction().unwrap();
            super::runtime::extend_schema(&tx).unwrap();
            tx.commit().unwrap();
            // v10 形态：仅摘掉 v11 新列（dispatcher_settings 已含 v10 列），
            // 并预置一帧既有 run 行验证迁移零数据丢失。
            revert_v11_graph_run_columns(&conn);
            conn.execute_batch(
                "INSERT INTO graph_plans (id,workspace_id,title,definition_json,status,created_at,updated_at)
                 VALUES ('p1','ws','历史图','{}','completed',1,1);
                 INSERT INTO graph_runs (id,plan_id,attempt_no,status,mode,verdict_status,started_at,finished_at)
                 VALUES ('r1','p1',1,'completed','full','pass',1,2);",
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 10).unwrap();
        }

        let db = DispatcherDb::new(path.clone()).unwrap();
        {
            let conn = db.conn().unwrap();
            let version: i32 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, super::SCHEMA_VERSION);
            let (kind, files, conclusion, node_id, attempt, verdict): (
                String,
                String,
                Option<String>,
                Option<String>,
                i64,
                String,
            ) = conn
                .query_row(
                    "SELECT result_kind, modified_files_json, conclusion_md,
                            conclusion_node_id, attempt_no, verdict_status
                     FROM graph_runs WHERE id = 'r1'",
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
            assert_eq!(kind, "unknown", "历史行结果类型兜底 unknown");
            assert_eq!(files, "[]", "历史行文件清单兜底空数组");
            assert_eq!(conclusion, None, "历史行无结论");
            assert_eq!(node_id, None, "历史行无结论节点");
            assert_eq!(attempt, 1, "既有 run 行全量保留");
            assert_eq!(verdict, "pass", "既有验收结论保留");
        }
        drop(db);

        let backups = backup_files(&path, "pre-v11-backup");
        assert_eq!(backups.len(), 1, "迁移前应生成整库快照");
        let snapshot = rusqlite::Connection::open(&backups[0]).unwrap();
        assert_eq!(
            snapshot
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
                .unwrap(),
            10
        );
        drop(snapshot);
        for backup in backups {
            let _ = std::fs::remove_file(backup);
        }
        cleanup_db_files(&path);
    }
}
