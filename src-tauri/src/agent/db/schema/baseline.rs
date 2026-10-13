use anyhow::{Context, Result};
use rusqlite::Connection;

use super::{categories::*, runtime, SCHEMA_VERSION};

/// 全新建库：单事务内执行基线 DDL + 领域建表助手 + 内置种子数据，
/// 并把 user_version 推进到 SCHEMA_VERSION（与建表同事务，失败整体回滚）。
pub(super) fn create_baseline(conn: &mut Connection) -> Result<()> {
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
    super::super::projects::ensure_projects_table_tx(&tx)?;
    super::super::mcp_servers::ensure_mcp_servers_table_tx(&tx)?;
    super::super::app_config::ensure_app_config_table_tx(&tx)?;

    tx.pragma_update(None, "user_version", SCHEMA_VERSION)
        .context("set baseline user_version")?;
    tx.commit().context("commit baseline schema")
}

/// 基线 DDL：核心表按「当前最终形态」一次性建齐。领域模块自管的表
/// （sub_agent / ssh / projects / mcp_servers / app_config）由各自的
/// `ensure_*_tx` 助手在 `create_baseline` 中补齐，DDL 保持单一出处。
/// `dispatcher_session_summaries` 的 DDL 在 `SESSION_SUMMARIES_DDL`（基线内建）。
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

-- 单行 JSON 形态——整对象序列化 AhaSettingsV2（落库剥离库引用凭据，
-- 读取由库条目回填）。
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

-- 工作流（PI workflow v3：run/attempt 模型 + 验收回执 + 继承快照）。
CREATE TABLE IF NOT EXISTS workflow_plans (
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
CREATE INDEX IF NOT EXISTS idx_workflow_plans_ws
ON workflow_plans(workspace_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS workflow_runs (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    attempt_no INTEGER NOT NULL,
    status TEXT NOT NULL,
    mode TEXT NOT NULL DEFAULT 'full',
    verdict_status TEXT NOT NULL DEFAULT '',
    verdict_reason TEXT NOT NULL DEFAULT '',
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    -- 执行结果：结论节点 id 与其输出 md 快照（结论节点未成功为 NULL）、
    -- review|edit|unknown 结果类型、修改文件并集（JSON 数组，BTreeSet 排序）。
    conclusion_node_id TEXT,
    conclusion_md TEXT,
    result_kind TEXT NOT NULL DEFAULT 'unknown',
    modified_files_json TEXT NOT NULL DEFAULT '[]',
    UNIQUE(plan_id, attempt_no),
    FOREIGN KEY(plan_id) REFERENCES workflow_plans(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_workflow_runs_plan
ON workflow_runs(plan_id, attempt_no DESC);

CREATE TABLE IF NOT EXISTS workflow_node_runs (
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
    FOREIGN KEY(run_id) REFERENCES workflow_runs(id) ON DELETE CASCADE,
    FOREIGN KEY(plan_id) REFERENCES workflow_plans(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_workflow_node_runs_plan
ON workflow_node_runs(plan_id);

CREATE TABLE IF NOT EXISTS workflow_node_activities (
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
    FOREIGN KEY(run_id) REFERENCES workflow_runs(id) ON DELETE CASCADE
);
";
