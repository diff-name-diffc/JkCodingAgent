use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::llm::{ChatMessage, OutboundToolCall};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSessionRecord {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSettingsRecord {
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    pub auto_approve_dispatch: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherMessageRecord {
    pub id: String,
    pub workspace_id: String,
    pub role: String,
    pub content: String,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_calls_json: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct DispatcherDb {
    path: PathBuf,
}

impl DispatcherDb {
    pub fn new(path: PathBuf) -> Result<Self> {
        let db = Self { path };
        db.init()?;
        Ok(db)
    }

    // ── Settings ──────────────────────────────────────────────

    pub fn get_settings(&self) -> Result<Option<DispatcherSettingsRecord>> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT api_base, api_key, model, auto_approve_dispatch FROM dispatcher_settings WHERE id = 'default'",
            [],
            |row| {
                Ok(DispatcherSettingsRecord {
                    api_base: row.get(0)?,
                    api_key: row.get(1)?,
                    model: row.get(2)?,
                    auto_approve_dispatch: row.get::<_, i32>(3)? != 0,
                })
            },
        )
        .optional()
        .context("load dispatcher settings")
    }

    pub fn save_settings(
        &self,
        api_base: &str,
        api_key: &str,
        model: &str,
        auto_approve_dispatch: bool,
    ) -> Result<DispatcherSettingsRecord> {
        let conn = self.connect()?;
        let auto_approve_int = if auto_approve_dispatch { 1 } else { 0 };
        conn.execute(
            "INSERT INTO dispatcher_settings (id, api_base, api_key, model, auto_approve_dispatch)
             VALUES ('default', ?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET api_base = ?1, api_key = ?2, model = ?3, auto_approve_dispatch = ?4",
            params![api_base.trim(), api_key.trim(), model.trim(), auto_approve_int],
        )
        .context("save dispatcher settings")?;
        Ok(DispatcherSettingsRecord {
            api_base: api_base.trim().to_string(),
            api_key: api_key.trim().to_string(),
            model: model.trim().to_string(),
            auto_approve_dispatch,
        })
    }

    pub fn set_auto_approve_dispatch(
        &self,
        auto_approve_dispatch: bool,
    ) -> Result<DispatcherSettingsRecord> {
        let conn = self.connect()?;
        let auto_approve_int = if auto_approve_dispatch { 1 } else { 0 };
        conn.execute(
            "INSERT INTO dispatcher_settings (id, auto_approve_dispatch)
             VALUES ('default', ?1)
             ON CONFLICT(id) DO UPDATE SET auto_approve_dispatch = ?1",
            params![auto_approve_int],
        )
        .context("save dispatcher auto-approve setting")?;
        self.get_settings()?
            .context("load dispatcher settings after auto-approve update")
    }

    // ── Sessions ──────────────────────────────────────────────

    pub fn list_sessions(&self, project_id: &str) -> Result<Vec<DispatcherSessionRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, project_id, title, created_at, updated_at
             FROM dispatcher_sessions
             WHERE project_id = ?1
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(DispatcherSessionRecord {
                id: row.get(0)?,
                project_id: row.get(1)?,
                title: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list dispatcher sessions")
    }

    pub fn create_session(&self, project_id: &str, title: &str) -> Result<DispatcherSessionRecord> {
        let record = DispatcherSessionRecord {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            title: title.to_string(),
            created_at: now(),
            updated_at: now(),
        };

        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO dispatcher_sessions (id, project_id, title, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                record.id,
                record.project_id,
                record.title,
                record.created_at,
                record.updated_at
            ],
        )
        .context("insert dispatcher session")?;

        Ok(record)
    }

    pub fn delete_session(&self, session_id: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
            params![session_id],
        )?;
        conn.execute(
            "DELETE FROM dispatcher_sessions WHERE id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    // ── Messages ──────────────────────────────────────────────

    pub fn add_visible_message(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(workspace_id, role, content, None, None, None, true)
    }

    pub fn add_visible_message_with_tools(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(
            workspace_id,
            role,
            content,
            tool_call_id,
            tool_name,
            tool_calls,
            true,
        )
    }

    pub fn add_hidden_message(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(
            workspace_id,
            role,
            content,
            tool_call_id,
            tool_name,
            tool_calls,
            false,
        )
    }

    pub fn list_visible_messages(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<DispatcherMessageRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, role, content, tool_call_id, tool_name, tool_calls_json, created_at
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND visible = 1
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id], |row| {
            Ok(DispatcherMessageRecord {
                id: row.get(0)?,
                workspace_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                tool_call_id: row.get(4)?,
                tool_name: row.get(5)?,
                tool_calls_json: row.get(6)?,
                created_at: row.get(7)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("load visible dispatcher messages")
    }

    pub fn load_llm_history(&self, workspace_id: &str, limit: usize) -> Result<Vec<ChatMessage>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT role, content, tool_call_id, tool_name, tool_calls_json
             FROM dispatcher_messages
             WHERE workspace_id = ?1
             ORDER BY created_at DESC, rowid DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![workspace_id, limit as i64], |row| {
            let tool_calls_json: Option<String> = row.get(4)?;
            let tool_calls = tool_calls_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<Vec<OutboundToolCall>>(json).ok());
            Ok(ChatMessage {
                role: row.get(0)?,
                content: row.get(1)?,
                tool_call_id: row.get(2)?,
                name: row.get(3)?,
                tool_calls,
            })
        })?;

        let mut messages = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("load dispatcher llm history")?;
        messages.reverse();

        // 从第一条 user 消息开始
        if let Some(first_user_index) = messages.iter().position(|m| m.role == "user") {
            messages = messages.split_off(first_user_index);
        }

        // 移除开头的悬挂 tool 消息
        while matches!(messages.first().map(|m| m.role.as_str()), Some("tool")) {
            messages.remove(0);
        }

        Ok(messages)
    }

    pub fn clear_messages(&self, workspace_id: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher messages")?;
        Ok(())
    }

    fn add_message(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
        visible: bool,
    ) -> Result<DispatcherMessageRecord> {
        let tool_calls_json = tool_calls
            .map(serde_json::to_string)
            .transpose()
            .context("serialize tool calls")?;

        let record = DispatcherMessageRecord {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            tool_call_id: tool_call_id.map(|s| s.to_string()),
            tool_name: tool_name.map(|s| s.to_string()),
            tool_calls_json: tool_calls_json.clone(),
            created_at: now(),
        };

        let conn = self.connect()?;
        conn.execute(
            "UPDATE dispatcher_sessions SET updated_at = ?1 WHERE id = ?2",
            params![record.created_at, record.workspace_id],
        )?;

        conn.execute(
            "INSERT INTO dispatcher_messages (
                id, workspace_id, role, content, tool_call_id, tool_name, tool_calls_json, visible, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                record.id,
                record.workspace_id,
                record.role,
                record.content,
                record.tool_call_id,
                record.tool_name,
                record.tool_calls_json,
                if visible { 1 } else { 0 },
                record.created_at
            ],
        )
        .context("insert dispatcher message")?;

        Ok(record)
    }

    fn init(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create db directory {}", parent.display()))?;
        }
        let conn = self.connect()?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS dispatcher_sessions (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                title TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_sessions_project
            ON dispatcher_sessions(project_id, updated_at DESC);

            CREATE TABLE IF NOT EXISTS dispatcher_messages (
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                tool_call_id TEXT,
                tool_name TEXT,
                tool_calls_json TEXT,
                visible INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_messages_workspace_created
            ON dispatcher_messages(workspace_id, created_at);

            CREATE TABLE IF NOT EXISTS dispatcher_settings (
                id TEXT PRIMARY KEY DEFAULT 'default',
                api_base TEXT NOT NULL DEFAULT '',
                api_key TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL DEFAULT '',
                auto_approve_dispatch INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
        .context("initialize dispatcher sqlite schema")?;
        Ok(())
    }

    fn connect(&self) -> Result<Connection> {
        Connection::open(&self.path).with_context(|| format!("open {}", self.path.display()))
    }
}

fn now() -> String {
    Utc::now().to_rfc3339()
}
