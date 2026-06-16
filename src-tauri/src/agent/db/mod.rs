pub(crate) mod artifacts;
pub(crate) mod categories;
pub(crate) mod content;
pub(crate) mod keywords;
pub(crate) mod messages;
pub(crate) mod python_runs;
pub(crate) mod runtime_state;
pub(crate) mod schema;
pub(crate) mod sessions;
pub(crate) mod settings;
pub(crate) mod token_usage;
pub(crate) mod util;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Connection};

// 对外引用的共享常量需显式重新导出（glob `pub use` 会丢弃 pub(crate) 项）。
pub use artifacts::{DispatcherToolArtifactRecord, DispatcherToolArtifactRef, ToolArtifactDraft};
pub use categories::ChatCategory;
pub use keywords::{KeywordAction, SessionKeywordRecord, SessionSearchResult};
pub use messages::{DispatcherMessageRecord, DispatcherMessageUsageStats};
pub use python_runs::PythonCodeRunRecord;
pub use runtime_state::{
    ChecklistPlanItem, ChecklistPlanState, ChecklistStepStatus, DispatcherSessionRuntimeState,
    PlanInteraction, PlanQuestionOption,
};
pub use sessions::{
    AgentContext, ChatSessionRecord, DispatcherMode, DispatcherSessionKind,
    DispatcherSessionRecord, ProjectSessionRecord, SessionPage,
};
pub use settings::{
    AhaContextConfig, AhaSettingsV2, AhaSharedModels, DispatcherModelConfig,
    DispatcherSettingsModelConfigs, DispatcherSettingsRecord,
};
pub use token_usage::{DispatcherSessionTokenUsageRecord, DispatcherSessionTokenUsageSource};
use util::MAX_DIALOGUE_QUERY_LIMIT;
pub(crate) use util::{DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS, TOOL_RETRY_CONTEXT_PREFIX};

#[derive(Debug, Clone)]
pub struct DispatcherDb {
    pub(super) pool: Arc<Pool<SqliteConnectionManager>>,
    pub(super) path: PathBuf,
}

impl DispatcherDb {
    pub fn new(path: PathBuf) -> Result<Self> {
        let manager = SqliteConnectionManager::file(&path).with_init(|conn| {
            conn.execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;",
            )?;
            Ok(())
        });
        let pool = Pool::builder()
            .max_size(4)
            .build(manager)
            .with_context(|| format!("创建数据库连接池失败：{}", path.display()))?;
        let db = Self {
            pool: Arc::new(pool),
            path,
        };
        db.init()?;
        Ok(db)
    }

    pub fn pool(&self) -> Arc<Pool<SqliteConnectionManager>> {
        Arc::clone(&self.pool)
    }

    pub(super) fn conn(&self) -> Result<r2d2::PooledConnection<SqliteConnectionManager>> {
        self.pool.get().with_context(|| "获取数据库连接")
    }

    pub(super) fn find_dialogue_cutoff_rowid(
        &self,
        conn: &Connection,
        workspace_id: &str,
        max_dialogues: usize,
    ) -> Result<i64> {
        let max_dialogues = max_dialogues.clamp(1, MAX_DIALOGUE_QUERY_LIMIT);
        let mut stmt = conn.prepare(
            "SELECT rowid
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND role = 'user' AND context_cleared = 0
             ORDER BY rowid DESC
             LIMIT ?2",
        )?;
        let rowids = stmt
            .query_map(params![workspace_id, max_dialogues as i64], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("load dispatcher dialogue boundaries")?;

        Ok(rowids.into_iter().min().unwrap_or(0))
    }

    /// Rough token estimate for context budget management.
    /// Uses a ~4 chars/token heuristic suitable for mixed CJK/Latin content.
    pub(crate) fn estimate_context_tokens(messages: &[crate::agent::llm::ChatMessage]) -> u64 {
        let total_chars: usize = messages
            .iter()
            .map(|m| {
                m.content.len()
                    + m.reasoning_content.as_ref().map_or(0, |s| s.len())
                    + m.tool_calls
                        .as_ref()
                        .map_or(0, |tc| serde_json::to_string(tc).map_or(0, |s| s.len()))
            })
            .sum();
        (total_chars as u64) / 4
    }
}
