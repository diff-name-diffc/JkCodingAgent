//! 聊天分类（chat_categories 表）的 CRUD 与排序。

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::schema::default_chat_category_agent_config_tx;
use super::util::now;
use super::DispatcherDb;

mod config;
#[cfg(test)]
mod tests;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatCategoryAgentConfig {
    pub category_id: String,
    pub category_name: String,
    pub allowed_tools: Vec<String>,
    pub system_prompt: String,
    #[serde(default)]
    pub sub_agent_ids: Vec<String>,
    #[serde(default)]
    pub project_directories: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl DispatcherDb {
    pub fn list_chat_categories(&self) -> Result<Vec<ChatCategory>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT c.id, c.name, c.icon, c.color, c.sort_order, COUNT(s.id), c.created_at, c.updated_at
             FROM chat_categories c
             LEFT JOIN dispatcher_sessions s
                    ON s.category = c.id AND s.kind = 'chat'
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
        allowed_tools: Option<Vec<String>>,
        system_prompt: Option<&str>,
    ) -> Result<ChatCategory> {
        let now = now();
        let mut conn = self.conn()?;
        // IMMEDIATE：事务内先 SELECT（sort_order / backfill 读分类）再写入，
        // 延迟事务在 WAL 下读后再升级写锁时，若期间有其他连接提交写入会立刻
        // 报 SQLITE_BUSY（database is locked），busy_timeout 无法兜底。
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin create chat category transaction")?;
        // 该聚合查询恒有返回行，出错只可能是真实的数据库故障（磁盘 I/O、损坏等），
        // 必须向上传播而不是吞成 -1（会造成重复/错误的 sort_order）。
        let max_order: i32 = tx
            .query_row(
                "SELECT COALESCE(MAX(sort_order), -1) FROM chat_categories",
                [],
                |row| row.get(0),
            )
            .context("query max chat category sort_order")?;
        let next_order = max_order + 1;
        let id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO chat_categories (id, name, icon, color, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, name.trim(), icon, color, next_order, now, now],
        )
        .context("insert chat category")?;
        let (default_allowed_tools_json, default_system_prompt) =
            default_chat_category_agent_config_tx(&tx, &id)?;
        let allowed_tools_json = allowed_tools
            .map(|tools| serde_json::to_string(&tools))
            .transpose()
            .context("serialize new category tools")?
            .unwrap_or(default_allowed_tools_json);
        let system_prompt = system_prompt
            .map(str::trim)
            .filter(|prompt| !prompt.is_empty())
            .map(str::to_string)
            .unwrap_or(default_system_prompt);
        tx.execute(
            "INSERT INTO chat_category_agent_configs
             (category_id, allowed_tools_json, system_prompt, sub_agent_ids_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, allowed_tools_json, system_prompt, "[]", now, now],
        )
        .context("insert chat category agent config")?;
        tx.commit().context("commit create chat category")?;
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
             LEFT JOIN dispatcher_sessions s
                    ON s.category = c.id AND s.kind = 'chat'
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
        // 不存在的分类必须与“删除成功”区分开（与 update 返回 Option 的语义对齐），
        // 先校验存在性，避免前端拿到无差别的 Ok。
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_categories WHERE id = ?1)",
                params![category_id],
                |row| row.get(0),
            )
            .context("check chat category exists")?;
        if !exists {
            anyhow::bail!("聊天分类不存在：{}", category_id);
        }
        let now_val = now();
        tx.execute(
            "UPDATE dispatcher_sessions SET category = '', updated_at = ?2 WHERE category = ?1",
            params![category_id, now_val],
        )
        .context("reassign uncategorized dispatcher sessions")?;
        tx.execute(
            "DELETE FROM chat_categories WHERE id = ?1",
            params![category_id],
        )
        .context("delete chat category")?;
        tx.commit().context("commit delete category")?;
        Ok(())
    }
}
