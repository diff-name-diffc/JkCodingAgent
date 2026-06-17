//! 聊天分类（chat_categories 表）的 CRUD 与排序。

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::util::now;
use super::DispatcherDb;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatCategory {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub color: String,
    pub sort_order: i32,
    pub session_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

impl DispatcherDb {
    pub fn list_chat_categories(&self) -> Result<Vec<ChatCategory>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT c.id, c.name, c.icon, c.color, c.sort_order, COUNT(s.id), c.created_at, c.updated_at
             FROM chat_categories c
             LEFT JOIN chat_sessions s ON s.category = c.id
             GROUP BY c.id, c.name, c.icon, c.color, c.sort_order, c.created_at, c.updated_at
             ORDER BY c.sort_order ASC, c.created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ChatCategory {
                id: row.get(0)?,
                name: row.get(1)?,
                icon: row.get(2)?,
                color: row.get(3)?,
                sort_order: row.get(4)?,
                session_count: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list chat categories")
    }

    pub fn create_chat_category(
        &self,
        name: &str,
        icon: &str,
        color: &str,
    ) -> Result<ChatCategory> {
        let now = now();
        let conn = self.conn()?;
        let max_order: i32 = conn
            .query_row(
                "SELECT COALESCE(MAX(sort_order), -1) FROM chat_categories",
                [],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        let next_order = max_order + 1;
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO chat_categories (id, name, icon, color, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, name.trim(), icon, color, next_order, now, now],
        )
        .context("insert chat category")?;
        Ok(ChatCategory {
            id,
            name: name.trim().to_string(),
            icon: icon.to_string(),
            color: color.to_string(),
            sort_order: next_order,
            session_count: 0,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub fn update_chat_category(
        &self,
        category_id: &str,
        name: Option<&str>,
        icon: Option<&str>,
        color: Option<&str>,
    ) -> Result<Option<ChatCategory>> {
        let updated = now();
        let conn = self.conn()?;
        let mut parts: Vec<String> = Vec::new();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(n) = name {
            parts.push(format!("name = ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(n.trim().to_string()));
        }
        if let Some(i) = icon {
            parts.push(format!("icon = ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(i.to_string()));
        }
        if let Some(c) = color {
            parts.push(format!("color = ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(c.to_string()));
        }
        if parts.is_empty() {
            return self.get_chat_category(category_id);
        }
        parts.push(format!("updated_at = ?{}", params_vec.len() + 1));
        params_vec.push(Box::new(updated.clone()));
        params_vec.push(Box::new(category_id.to_string()));

        let sql = format!(
            "UPDATE chat_categories SET {} WHERE id = ?{}",
            parts.join(", "),
            params_vec.len()
        );
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let changed = conn
            .execute(&sql, params_refs.as_slice())
            .context("update chat category")?;
        if changed == 0 {
            return Ok(None);
        }
        self.get_chat_category(category_id)
    }

    fn get_chat_category(&self, category_id: &str) -> Result<Option<ChatCategory>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT c.id, c.name, c.icon, c.color, c.sort_order, COUNT(s.id), c.created_at, c.updated_at
             FROM chat_categories c
             LEFT JOIN chat_sessions s ON s.category = c.id
             WHERE c.id = ?1
             GROUP BY c.id, c.name, c.icon, c.color, c.sort_order, c.created_at, c.updated_at",
            params![category_id],
            |row| {
                Ok(ChatCategory {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    icon: row.get(2)?,
                    color: row.get(3)?,
                    sort_order: row.get(4)?,
                    session_count: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            },
        )
        .optional()
        .context("get chat category")
    }

    pub fn delete_chat_category(&self, category_id: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .context("begin delete category transaction")?;
        let now_val = now();
        tx.execute(
            "UPDATE dispatcher_sessions SET category = '', updated_at = ?2 WHERE category = ?1",
            params![category_id, now_val],
        )
        .context("reassign uncategorized dispatcher sessions")?;
        tx.execute(
            "UPDATE chat_sessions SET category = '', updated_at = ?2 WHERE category = ?1",
            params![category_id, now_val],
        )
        .context("reassign uncategorized chat sessions")?;
        tx.execute(
            "DELETE FROM chat_categories WHERE id = ?1",
            params![category_id],
        )
        .context("delete chat category")?;
        tx.commit().context("commit delete category")?;
        Ok(())
    }

    pub fn set_session_category(&self, session_id: &str, category_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE dispatcher_sessions SET category = ?1, updated_at = ?2 WHERE id = ?3",
            params![category_id, now(), session_id],
        )
        .context("set session category")?;
        Ok(())
    }

    pub fn reorder_chat_categories(&self, ordered_ids: &[String]) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .context("begin reorder categories transaction")?;
        {
            let mut stmt = tx
                .prepare(
                    "UPDATE chat_categories SET sort_order = ?1, updated_at = ?2 WHERE id = ?3",
                )
                .context("prepare reorder statement")?;
            let now = now();
            for (order, id) in ordered_ids.iter().enumerate() {
                stmt.execute(params![order as i32, &now, id])
                    .with_context(|| format!("reorder category {id}"))?;
            }
        }
        tx.commit().context("commit reorder categories")?;
        Ok(())
    }
}
