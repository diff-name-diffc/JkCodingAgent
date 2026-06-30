//! 会话记录与会话 CRUD：dispatcher_sessions（统一）、chat_sessions、project_sessions，
//! 以及会话模式（DispatcherMode）、上下文（AgentContext）、会话类型（DispatcherSessionKind）。

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::content::{delete_chat_image_resources, delete_plan_file_resources};
use super::util::now;
use super::DispatcherDb;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSessionRecord {
    pub id: String,
    pub project_id: String,
    pub kind: DispatcherSessionKind,
    pub title: String,
    pub mode: DispatcherMode,
    pub active_plan_path: Option<String>,
    pub category: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSessionRecord {
    pub id: String,
    pub title: String,
    pub category: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSessionRecord {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub mode: DispatcherMode,
    pub active_plan_path: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPage<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub has_more: bool,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DispatcherMode {
    Default,
    Plan,
}

impl DispatcherMode {
    pub fn from_wire(value: &str) -> Result<Self> {
        match value.trim() {
            "" | "default" => Ok(Self::Default),
            "plan" => Ok(Self::Plan),
            other => anyhow::bail!("invalid dispatcher mode: {other}"),
        }
    }

    pub fn as_sql_value(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Plan => "plan",
        }
    }

    pub(super) fn from_sql_value(value: String) -> Self {
        match value.as_str() {
            "plan" => Self::Plan,
            _ => Self::Default,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum AgentContext {
    Project,
    Chat,
}

impl AgentContext {
    pub fn from_wire(value: &str) -> Result<Self> {
        match value.trim() {
            "project" | "" => Ok(Self::Project),
            "chat" => Ok(Self::Chat),
            other => anyhow::bail!("invalid agent context: {other}"),
        }
    }

    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Chat => "chat",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DispatcherSessionKind {
    Project,
    Chat,
}

impl DispatcherSessionKind {
    pub fn from_wire(value: &str) -> Result<Self> {
        match value.trim() {
            "" | "project" => Ok(Self::Project),
            "chat" => Ok(Self::Chat),
            other => anyhow::bail!("invalid dispatcher session kind: {other}"),
        }
    }

    pub fn as_sql_value(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Chat => "chat",
        }
    }

    pub(super) fn from_sql_value(value: String) -> Self {
        match value.as_str() {
            "chat" => Self::Chat,
            _ => Self::Project,
        }
    }
}

impl DispatcherDb {
    // ── Sessions ──────────────────────────────────────────────

    pub fn list_sessions(
        &self,
        project_id: &str,
        kind: DispatcherSessionKind,
    ) -> Result<Vec<DispatcherSessionRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, project_id, kind, title, mode, active_plan_path, category, created_at, updated_at
             FROM dispatcher_sessions
             WHERE project_id = ?1 AND kind = ?2
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![project_id, kind.as_sql_value()], |row| {
            Ok(DispatcherSessionRecord {
                id: row.get(0)?,
                project_id: row.get(1)?,
                kind: DispatcherSessionKind::from_sql_value(row.get(2)?),
                title: row.get(3)?,
                mode: DispatcherMode::from_sql_value(row.get(4)?),
                active_plan_path: row.get(5)?,
                category: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list dispatcher sessions")
    }

    pub fn create_session(
        &self,
        project_id: &str,
        title: &str,
        kind: DispatcherSessionKind,
        mode: DispatcherMode,
        active_plan_path: Option<&str>,
        category: Option<&str>,
    ) -> Result<DispatcherSessionRecord> {
        let category = category.unwrap_or("").to_string();
        let record = DispatcherSessionRecord {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            kind,
            title: title.to_string(),
            mode,
            active_plan_path: active_plan_path.map(str::to_string),
            category,
            created_at: now(),
            updated_at: now(),
        };

        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO dispatcher_sessions (id, project_id, kind, title, mode, active_plan_path, category, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                record.id,
                record.project_id,
                record.kind.as_sql_value(),
                record.title,
                record.mode.as_sql_value(),
                record.active_plan_path,
                record.category,
                record.created_at,
                record.updated_at
            ],
        )
        .context("insert dispatcher session")?;

        Ok(record)
    }

    pub fn update_session_title(
        &self,
        session_id: &str,
        title: &str,
    ) -> Result<Option<DispatcherSessionRecord>> {
        let updated_at = now();
        let mut conn = self.conn()?;
        let tx = conn.transaction().context("begin update session title")?;
        let changed = tx
            .execute(
                "UPDATE dispatcher_sessions
                 SET title = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![title.trim(), &updated_at, session_id],
            )
            .context("update dispatcher session title")?;

        if changed == 0 {
            return Ok(None);
        }

        tx.execute(
            "UPDATE chat_sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
            params![title.trim(), &updated_at, session_id],
        )
        .context("reflect dispatcher title in chat session")?;
        tx.execute(
            "UPDATE project_sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
            params![title.trim(), &updated_at, session_id],
        )
        .context("reflect dispatcher title in project session")?;

        let record = tx
            .query_row(
            "SELECT id, project_id, kind, title, mode, active_plan_path, category, created_at, updated_at
             FROM dispatcher_sessions
             WHERE id = ?1",
            params![session_id],
            |row| {
                Ok(DispatcherSessionRecord {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    kind: DispatcherSessionKind::from_sql_value(row.get(2)?),
                    title: row.get(3)?,
                    mode: DispatcherMode::from_sql_value(row.get(4)?),
                    active_plan_path: row.get(5)?,
                    category: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            },
        )
        .optional()
        .context("load dispatcher session after title update")?;
        tx.commit().context("commit update session title")?;
        Ok(record)
    }

    pub fn delete_session(&self, session_id: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM dispatcher_tool_artifacts WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_tool_runs WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_session_token_usage WHERE workspace_id = ?1",
            params![session_id],
        )?;
        delete_chat_image_resources(&tx, session_id)?;
        delete_plan_file_resources(&tx, session_id)?;
        tx.execute(
            "DELETE FROM session_keywords WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM chat_sessions WHERE id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM project_sessions WHERE id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_sessions WHERE id = ?1",
            params![session_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    // ── Chat Sessions (v6) ────────────────────────────────────────

    pub fn list_chat_sessions_paginated(
        &self,
        category: Option<&str>,
        cursor: Option<&str>,
        page_size: i64,
    ) -> Result<SessionPage<ChatSessionRecord>> {
        let conn = self.conn()?;
        let total: i64 = if let Some(cat) = category {
            conn.query_row(
                "SELECT COUNT(*) FROM chat_sessions WHERE category = ?1",
                params![cat],
                |row| row.get(0),
            )?
        } else {
            conn.query_row("SELECT COUNT(*) FROM chat_sessions", [], |row| row.get(0))?
        };

        let (where_clause, bind): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
            match (category, cursor) {
                (Some(cat), Some(cur)) => (
                    "WHERE category = ?1 AND updated_at < ?2".into(),
                    vec![Box::new(cat.to_string()), Box::new(cur.to_string())],
                ),
                (Some(cat), None) => (
                    "WHERE category = ?1".into(),
                    vec![Box::new(cat.to_string())],
                ),
                (None, Some(cur)) => (
                    "WHERE updated_at < ?1".into(),
                    vec![Box::new(cur.to_string())],
                ),
                (None, None) => (String::new(), vec![]),
            };

        let sql = format!(
            "SELECT id, title, category, created_at, updated_at
             FROM chat_sessions
             {}
             ORDER BY updated_at DESC
             LIMIT ?{}",
            where_clause,
            bind.len() + 1
        );
        let mut stmt = conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            bind.iter().map(|b| b.as_ref()).collect();
        let limit_param: i64 = page_size + 1;
        let mut all_params: Vec<&dyn rusqlite::types::ToSql> = params_refs;
        all_params.push(&limit_param);

        let rows = stmt.query_map(all_params.as_slice(), |row| {
            Ok(ChatSessionRecord {
                id: row.get(0)?,
                title: row.get(1)?,
                category: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        let mut items: Vec<ChatSessionRecord> = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("list chat sessions paginated")?;

        let has_more = items.len() as i64 > page_size;
        if has_more {
            items.pop();
        }
        let next_cursor = items.last().map(|s| s.updated_at.clone());

        Ok(SessionPage {
            items,
            total,
            has_more,
            next_cursor,
        })
    }

    pub fn create_chat_session(
        &self,
        title: &str,
        category: Option<&str>,
    ) -> Result<ChatSessionRecord> {
        let record = ChatSessionRecord {
            id: Uuid::new_v4().to_string(),
            title: title.to_string(),
            category: category.unwrap_or("tech").to_string(),
            created_at: now(),
            updated_at: now(),
        };
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO chat_sessions (id, title, category, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                record.id,
                record.title,
                record.category,
                record.created_at,
                record.updated_at
            ],
        )
        .context("insert chat session")?;
        conn.execute(
            "INSERT INTO dispatcher_sessions (id, project_id, kind, title, mode, category, created_at, updated_at)
             VALUES (?1, '__global_chat__', 'chat', ?2, 'default', ?3, ?4, ?5)",
            params![
                record.id,
                record.title,
                record.category,
                record.created_at,
                record.updated_at
            ],
        )
        .context("insert chat session into dispatcher_sessions")?;
        Ok(record)
    }

    pub fn delete_chat_session(&self, session_id: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM dispatcher_tool_artifacts WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_tool_runs WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_session_token_usage WHERE workspace_id = ?1",
            params![session_id],
        )?;
        delete_chat_image_resources(&tx, session_id)?;
        delete_plan_file_resources(&tx, session_id)?;
        tx.execute(
            "DELETE FROM session_keywords WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM chat_sessions WHERE id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_sessions WHERE id = ?1",
            params![session_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn update_chat_session_updated_at(&self, session_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE chat_sessions SET updated_at = ?1 WHERE id = ?2",
            params![now(), session_id],
        )
        .context("update chat session updated_at")?;
        Ok(())
    }

    pub fn update_chat_session_title(
        &self,
        session_id: &str,
        title: &str,
    ) -> Result<Option<ChatSessionRecord>> {
        let updated_at = now();
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "UPDATE chat_sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
                params![title.trim(), &updated_at, session_id],
            )
            .context("update chat session title")?;
        if changed == 0 {
            return Ok(None);
        }
        conn.execute(
            "UPDATE dispatcher_sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
            params![title.trim(), &updated_at, session_id],
        )
        .context("reflect chat title in dispatcher_sessions")?;
        conn.query_row(
            "SELECT id, title, category, created_at, updated_at
             FROM chat_sessions WHERE id = ?1",
            params![session_id],
            |row| {
                Ok(ChatSessionRecord {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    category: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        )
        .optional()
        .context("load chat session after title update")
    }

    pub fn set_chat_session_category(&self, session_id: &str, category_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE chat_sessions SET category = ?1, updated_at = ?2 WHERE id = ?3",
            params![category_id, now(), session_id],
        )
        .context("set chat session category")?;
        conn.execute(
            "UPDATE dispatcher_sessions SET category = ?1 WHERE id = ?2",
            params![category_id, session_id],
        )
        .context("reflect category in dispatcher_sessions")?;
        Ok(())
    }

    // ── Project Sessions (v6) ─────────────────────────────────────

    pub fn list_project_sessions_paginated(
        &self,
        project_id: &str,
        offset: i64,
        page_size: i64,
    ) -> Result<SessionPage<ProjectSessionRecord>> {
        let conn = self.conn()?;
        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM project_sessions WHERE project_id = ?1",
            params![project_id],
            |row| row.get(0),
        )?;

        let mut stmt = conn.prepare(
            "SELECT id, project_id, title, mode, active_plan_path, created_at, updated_at
             FROM project_sessions
             WHERE project_id = ?1
             ORDER BY updated_at DESC
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![project_id, page_size, offset], |row| {
            Ok(ProjectSessionRecord {
                id: row.get(0)?,
                project_id: row.get(1)?,
                title: row.get(2)?,
                mode: DispatcherMode::from_sql_value(row.get(3)?),
                active_plan_path: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        let items: Vec<ProjectSessionRecord> = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("list project sessions paginated")?;

        let has_more = (offset + items.len() as i64) < total;
        let next_cursor = items.last().map(|s| s.updated_at.clone());

        Ok(SessionPage {
            items,
            total,
            has_more,
            next_cursor,
        })
    }

    pub fn create_project_session(
        &self,
        project_id: &str,
        title: &str,
        mode: DispatcherMode,
        active_plan_path: Option<&str>,
    ) -> Result<ProjectSessionRecord> {
        let record = ProjectSessionRecord {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            title: title.to_string(),
            mode,
            active_plan_path: active_plan_path.map(str::to_string),
            created_at: now(),
            updated_at: now(),
        };
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO project_sessions (id, project_id, title, mode, active_plan_path, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.id,
                record.project_id,
                record.title,
                record.mode.as_sql_value(),
                record.active_plan_path,
                record.created_at,
                record.updated_at
            ],
        )
        .context("insert project session")?;
        conn.execute(
            "INSERT INTO dispatcher_sessions (id, project_id, kind, title, mode, active_plan_path, category, created_at, updated_at)
             VALUES (?1, ?2, 'project', ?3, ?4, ?5, '', ?6, ?7)",
            params![
                record.id,
                record.project_id,
                record.title,
                record.mode.as_sql_value(),
                record.active_plan_path,
                record.created_at,
                record.updated_at
            ],
        )
        .context("insert project session into dispatcher_sessions")?;
        Ok(record)
    }

    pub fn delete_project_session(&self, session_id: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM dispatcher_tool_artifacts WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_tool_runs WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_session_token_usage WHERE workspace_id = ?1",
            params![session_id],
        )?;
        delete_chat_image_resources(&tx, session_id)?;
        delete_plan_file_resources(&tx, session_id)?;
        tx.execute(
            "DELETE FROM session_keywords WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM project_sessions WHERE id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_sessions WHERE id = ?1",
            params![session_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn update_project_session_updated_at(&self, session_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE project_sessions SET updated_at = ?1 WHERE id = ?2",
            params![now(), session_id],
        )
        .context("update project session updated_at")?;
        Ok(())
    }

    pub fn get_session_title(&self, session_id: &str) -> Result<String> {
        let conn = self.conn()?;
        let title: String = conn
            .query_row(
                "SELECT title FROM dispatcher_sessions WHERE id = ?1",
                rusqlite::params![session_id],
                |row| row.get(0),
            )
            .optional()
            .context("load dispatcher session title")?
            .unwrap_or_else(|| "untitled".to_string());
        Ok(title)
    }
}

impl DispatcherDb {
    pub async fn get_session_title_async(&self, workspace_id: &str) -> Result<String> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.get_session_title(&wid))
            .await
            .context("get_session_title spawn_blocking")?
    }
}
