//! 会话记录与会话 CRUD：dispatcher_sessions（统一）、chat_sessions、project_sessions），
//! 以及上下文（AgentContext）、会话类型（DispatcherSessionKind）。

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::content::remove_chat_image_dir;
use super::util::now;
use super::DispatcherDb;

/// 应用内部分类的会话（架构设计助手等）：不混入未指定分类的默认聊天列表
/// 与会话搜索，由各自的专属界面按分类显式管理。前端持有同一字面量
/// （`src/types/architecture.ts` 的 `ARCH_DESIGN_CATEGORY`），两侧必须保持一致。
pub const INTERNAL_CHAT_CATEGORY: &str = "arch-design";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSessionRecord {
    pub id: String,
    pub project_id: String,
    pub kind: DispatcherSessionKind,
    pub title: String,
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
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSessionRecord {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPage<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub has_more: bool,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatSessionCursor {
    updated_at: String,
    id: String,
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
            "project" => Ok(Self::Project),
            "chat" => Ok(Self::Chat),
            other => anyhow::bail!("invalid agent context: {other}"),
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
    pub(super) fn from_sql_value(value: String) -> Self {
        match value.as_str() {
            "chat" => Self::Chat,
            _ => Self::Project,
        }
    }

    pub(super) fn as_sql_value(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Chat => "chat",
        }
    }
}

/// `create_session` 的返回载荷：按 kind 序列化为对应的子表记录形态
/// （chat → ChatSessionRecord / project → ProjectSessionRecord），
/// 前端两类消费方的既有类型不需要调整。
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum SessionCreatedRecord {
    Chat(ChatSessionRecord),
    Project(ProjectSessionRecord),
}

impl DispatcherDb {
    // ── Sessions ──────────────────────────────────────────────

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
                "SELECT id, project_id, kind, title, category, created_at, updated_at
             FROM dispatcher_sessions
             WHERE id = ?1",
                params![session_id],
                |row| {
                    Ok(DispatcherSessionRecord {
                        id: row.get(0)?,
                        project_id: row.get(1)?,
                        kind: DispatcherSessionKind::from_sql_value(row.get(2)?),
                        title: row.get(3)?,
                        category: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                },
            )
            .optional()
            .context("load dispatcher session after title update")?;
        tx.commit().context("commit update session title")?;
        Ok(record)
    }

    /// 按 id 读取会话记录（供图执行回执等场景广播会话更新）。
    pub fn get_dispatcher_session(
        &self,
        session_id: &str,
    ) -> Result<Option<DispatcherSessionRecord>> {
        self.conn()?
            .query_row(
                "SELECT id, project_id, kind, title, category, created_at, updated_at
                 FROM dispatcher_sessions
                 WHERE id = ?1",
                params![session_id],
                |row| {
                    Ok(DispatcherSessionRecord {
                        id: row.get(0)?,
                        project_id: row.get(1)?,
                        kind: DispatcherSessionKind::from_sql_value(row.get(2)?),
                        title: row.get(3)?,
                        category: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                },
            )
            .optional()
            .context("load dispatcher session by id")
    }

    pub async fn get_dispatcher_session_async(
        &self,
        session_id: &str,
    ) -> Result<Option<DispatcherSessionRecord>> {
        let db = self.clone();
        let session_id = session_id.to_string();
        tokio::task::spawn_blocking(move || db.get_dispatcher_session(&session_id))
            .await
            .context("get_dispatcher_session spawn_blocking")?
    }

    // ── Chat Sessions (v6) ────────────────────────────────────────

    pub fn list_chat_sessions_paginated(
        &self,
        category: Option<&str>,
        cursor: Option<&str>,
        page_size: i64,
    ) -> Result<SessionPage<ChatSessionRecord>> {
        let conn = self.conn()?;
        let cursor = cursor
            .map(|value| {
                serde_json::from_str::<ChatSessionCursor>(value)
                    .context("decode chat session cursor")
            })
            .transpose()?;
        // 内部分类（架构设计助手等）的会话不出现在未指定分类的默认列表中；
        // 显式按分类查询时仍可列出（其专属界面自行管理）。
        let total: i64 = if let Some(cat) = category {
            conn.query_row(
                "SELECT COUNT(*) FROM chat_sessions WHERE category = ?1",
                params![cat],
                |row| row.get(0),
            )?
        } else {
            conn.query_row(
                "SELECT COUNT(*) FROM chat_sessions WHERE category != ?1",
                params![INTERNAL_CHAT_CATEGORY],
                |row| row.get(0),
            )?
        };

        let (where_clause, bind): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
            match (category, cursor.as_ref()) {
                (Some(cat), Some(cur)) => (
                    "WHERE category = ?1
                     AND (updated_at < ?2 OR (updated_at = ?2 AND id < ?3))"
                        .into(),
                    vec![
                        Box::new(cat.to_string()),
                        Box::new(cur.updated_at.clone()),
                        Box::new(cur.id.clone()),
                    ],
                ),
                (Some(cat), None) => (
                    "WHERE category = ?1".into(),
                    vec![Box::new(cat.to_string())],
                ),
                (None, Some(cur)) => (
                    "WHERE category != ?1
                     AND (updated_at < ?2 OR (updated_at = ?2 AND id < ?3))"
                        .into(),
                    vec![
                        Box::new(INTERNAL_CHAT_CATEGORY.to_string()),
                        Box::new(cur.updated_at.clone()),
                        Box::new(cur.id.clone()),
                    ],
                ),
                (None, None) => (
                    "WHERE category != ?1".into(),
                    vec![Box::new(INTERNAL_CHAT_CATEGORY.to_string())],
                ),
            };

        let sql = format!(
            "SELECT id, title, category, created_at, updated_at
             FROM chat_sessions
             {}
             ORDER BY updated_at DESC, id DESC
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
                keywords: Vec::new(),
            })
        })?;
        let mut items: Vec<ChatSessionRecord> = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("list chat sessions paginated")?;

        let has_more = items.len() as i64 > page_size;
        if has_more {
            items.pop();
        }
        let session_ids = items
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        let mut keywords_by_session =
            super::keywords::load_keywords_by_session_ids(&conn, &session_ids)?;
        for session in &mut items {
            session.keywords = keywords_by_session.remove(&session.id).unwrap_or_default();
        }
        let next_cursor = items
            .last()
            .map(|session| {
                serde_json::to_string(&ChatSessionCursor {
                    updated_at: session.updated_at.clone(),
                    id: session.id.clone(),
                })
            })
            .transpose()
            .context("encode chat session cursor")?;

        Ok(SessionPage {
            items,
            total,
            has_more,
            next_cursor,
        })
    }

    /// 统一建会话入口（原 chat_create / project_create 两条命令合并后的
    /// DB 侧分流）。kind=chat 时 category 缺省由本层统一为 "tech"（消除
    /// 前后端双写默认值）；kind=project 时必须提供 project_id。
    pub fn create_session(
        &self,
        kind: DispatcherSessionKind,
        title: &str,
        category: Option<&str>,
        project_id: Option<&str>,
    ) -> Result<SessionCreatedRecord> {
        match kind {
            DispatcherSessionKind::Chat => Ok(SessionCreatedRecord::Chat(
                self.create_chat_session(title, category)?,
            )),
            DispatcherSessionKind::Project => {
                let project_id = project_id.ok_or_else(|| {
                    anyhow::anyhow!("project kind 会话必须提供 project_id")
                })?;
                Ok(SessionCreatedRecord::Project(
                    self.create_project_session(project_id, title)?,
                ))
            }
        }
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
            keywords: Vec::new(),
        };
        let mut conn = self.conn()?;
        // 子表与统一表两条 INSERT 同一事务包裹，第二条失败时整体回滚，不留孤儿记录。
        let tx = conn.transaction().context("begin create chat session")?;
        tx.execute(
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
        tx.execute(
            "INSERT INTO dispatcher_sessions (id, project_id, kind, title, category, created_at, updated_at)
             VALUES (?1, '__global_chat__', 'chat', ?2, ?3, ?4, ?5)",
            params![
                record.id,
                record.title,
                record.category,
                record.created_at,
                record.updated_at
            ],
        )
        .context("insert chat session into dispatcher_sessions")?;
        tx.commit().context("commit create chat session")?;
        Ok(record)
    }

    /// 统一删会话入口（原 delete_chat_session / delete_project_session 两条
    /// 逐行镜像的命令合并）：查统一表 kind 分流子表删除，级联资源清单走
    /// `purge_session_resources_tx` 单一出处。kind 异常值显式中止（保留原
    /// 「禁跨类误删」的数据完整性守卫：子表与统一表同事务成对删除）。
    pub fn delete_session(&self, session_id: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let kind: String = tx
            .query_row(
                "SELECT kind FROM dispatcher_sessions WHERE id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .optional()
            .context("load dispatcher session kind")?
            .ok_or_else(|| anyhow::anyhow!("session not found: {session_id}"))?;
        let image_dir = super::purge::purge_session_resources_tx(&tx, session_id)?;
        match kind.as_str() {
            "chat" => {
                tx.execute(
                    "DELETE FROM chat_sessions WHERE id = ?1",
                    params![session_id],
                )
                .context("delete chat session row")?;
            }
            "project" => {
                tx.execute(
                    "DELETE FROM project_sessions WHERE id = ?1",
                    params![session_id],
                )
                .context("delete project session row")?;
            }
            other => anyhow::bail!("未知会话类型 {other}（session {session_id}），中止删除"),
        }
        tx.execute(
            "DELETE FROM dispatcher_sessions WHERE id = ?1",
            params![session_id],
        )
        .context("delete dispatcher session row")?;
        tx.commit().context("commit delete session")?;
        // 数据库删除已提交，图片文件清理失败不应把删除误报为失败（否则调用方
        // 按 Err 重试时记录已不存在，孤儿文件将永远无法清理）。改为 best-effort。
        if let Some(dir) = image_dir {
            if let Err(error) = remove_chat_image_dir(&dir) {
                eprintln!("remove chat image dir failed (session {session_id}): {error:#}");
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn update_chat_session_updated_at(&self, session_id: &str) -> Result<()> {
        let conn = self.conn()?;
        let updated_at = now();
        conn.execute(
            "UPDATE chat_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&updated_at, session_id],
        )
        .context("update chat session updated_at")?;
        conn.execute(
            "UPDATE dispatcher_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&updated_at, session_id],
        )
        .context("reflect chat session updated_at in dispatcher_sessions")?;
        Ok(())
    }

    pub fn set_chat_session_category(&self, session_id: &str, category_id: &str) -> Result<()> {
        let conn = self.conn()?;
        let updated_at = now();
        conn.execute(
            "UPDATE chat_sessions SET category = ?1, updated_at = ?2 WHERE id = ?3",
            params![category_id, &updated_at, session_id],
        )
        .context("set chat session category")?;
        conn.execute(
            "UPDATE dispatcher_sessions SET category = ?1, updated_at = ?2 WHERE id = ?3",
            params![category_id, &updated_at, session_id],
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
            "SELECT id, project_id, title, created_at, updated_at
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
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                keywords: Vec::new(),
            })
        })?;
        let items: Vec<ProjectSessionRecord> = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("list project sessions paginated")?;
        let mut items = items;
        let session_ids = items
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        let mut keywords_by_session =
            super::keywords::load_keywords_by_session_ids(&conn, &session_ids)?;
        for session in &mut items {
            session.keywords = keywords_by_session.remove(&session.id).unwrap_or_default();
        }

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
    ) -> Result<ProjectSessionRecord> {
        let record = ProjectSessionRecord {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            title: title.to_string(),
            created_at: now(),
            updated_at: now(),
            keywords: Vec::new(),
        };
        let mut conn = self.conn()?;
        // 子表与统一表两条 INSERT 同一事务包裹，第二条失败时整体回滚，不留孤儿记录。
        let tx = conn.transaction().context("begin create project session")?;
        tx.execute(
            "INSERT INTO project_sessions (id, project_id, title, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                record.id,
                record.project_id,
                record.title,
                record.created_at,
                record.updated_at
            ],
        )
        .context("insert project session")?;
        tx.execute(
            "INSERT INTO dispatcher_sessions (id, project_id, kind, title, category, created_at, updated_at)
             VALUES (?1, ?2, 'project', ?3, '', ?4, ?5)",
            params![
                record.id,
                record.project_id,
                record.title,
                record.created_at,
                record.updated_at
            ],
        )
        .context("insert project session into dispatcher_sessions")?;
        tx.commit().context("commit create project session")?;
        Ok(record)
    }

    #[allow(dead_code)]
    pub fn update_project_session_updated_at(&self, session_id: &str) -> Result<()> {
        let conn = self.conn()?;
        let updated_at = now();
        conn.execute(
            "UPDATE project_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&updated_at, session_id],
        )
        .context("update project session updated_at")?;
        conn.execute(
            "UPDATE dispatcher_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&updated_at, session_id],
        )
        .context("reflect project session updated_at in dispatcher_sessions")?;
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

    /// 会话 → 项目 id（图运行器据此定位项目根路径）。
    pub fn get_session_project_id(&self, session_id: &str) -> Result<Option<String>> {
        let conn = self.conn()?;
        let project_id = conn
            .query_row(
                "SELECT project_id FROM dispatcher_sessions WHERE id = ?1",
                rusqlite::params![session_id],
                |row| row.get(0),
            )
            .optional()
            .context("load dispatcher session project id")?;
        Ok(project_id)
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

    pub async fn get_session_project_id_async(&self, workspace_id: &str) -> Result<Option<String>> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.get_session_project_id(&wid))
            .await
            .context("get_session_project_id spawn_blocking")?
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn test_db() -> DispatcherDb {
        let path = std::env::temp_dir().join(format!(
            "jkcodingagent-session-pagination-{}.sqlite3",
            Uuid::new_v4()
        ));
        DispatcherDb::new(path).expect("create test dispatcher db")
    }

    #[test]
    fn category_cursor_does_not_skip_sessions_with_equal_timestamps() {
        let db = test_db();
        for index in 0..41 {
            db.create_chat_session(&format!("session-{index}"), Some("tech"))
                .expect("create chat session");
        }
        db.conn()
            .expect("db conn")
            .execute(
                "UPDATE chat_sessions SET updated_at = '2026-01-01T00:00:00Z' WHERE category = 'tech'",
                [],
            )
            .expect("normalize timestamps");

        let mut cursor = None;
        let mut session_ids = Vec::new();
        loop {
            let page = db
                .list_chat_sessions_paginated(Some("tech"), cursor.as_deref(), 20)
                .expect("list category page");
            session_ids.extend(page.items.into_iter().map(|session| session.id));
            if !page.has_more {
                break;
            }
            cursor = page.next_cursor;
        }

        assert_eq!(session_ids.len(), 41);
        assert_eq!(session_ids.iter().collect::<HashSet<_>>().len(), 41);
    }

    #[test]
    fn internal_category_sessions_are_hidden_from_default_listing() {
        let db = test_db();
        db.create_chat_session("visible-chat", Some("tech"))
            .expect("create normal session");
        db.create_chat_session("arch-session", Some(INTERNAL_CHAT_CATEGORY))
            .expect("create internal session");

        // 默认列表（无分类过滤）不含内部分类会话，计数同样排除。
        let default_page = db
            .list_chat_sessions_paginated(None, None, 20)
            .expect("list default page");
        assert_eq!(default_page.total, 1);
        assert_eq!(default_page.items.len(), 1);
        assert_eq!(default_page.items[0].title, "visible-chat");

        // 显式按内部分类查询仍可列出（专属界面自行管理）。
        let arch_page = db
            .list_chat_sessions_paginated(Some(INTERNAL_CHAT_CATEGORY), None, 20)
            .expect("list internal category page");
        assert_eq!(arch_page.total, 1);
        assert_eq!(arch_page.items[0].title, "arch-session");
    }
}
