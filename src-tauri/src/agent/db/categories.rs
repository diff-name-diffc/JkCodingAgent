//! 聊天分类（chat_categories 表）的 CRUD 与排序。

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::schema::{
    backfill_chat_category_agent_configs_tx, default_chat_category_agent_config_tx,
};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatCategoryAgentConfig {
    pub category_id: String,
    pub category_name: String,
    pub allowed_tools: Vec<String>,
    pub system_prompt: String,
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
        allowed_tools: Option<Vec<String>>,
        system_prompt: Option<&str>,
    ) -> Result<ChatCategory> {
        let now = now();
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .context("begin create chat category transaction")?;
        let max_order: i32 = tx
            .query_row(
                "SELECT COALESCE(MAX(sort_order), -1) FROM chat_categories",
                [],
                |row| row.get(0),
            )
            .unwrap_or(-1);
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
             (category_id, allowed_tools_json, system_prompt, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, allowed_tools_json, system_prompt, now, now],
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

    pub fn list_chat_category_agent_configs(&self) -> Result<Vec<ChatCategoryAgentConfig>> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .context("begin list chat category agent configs")?;
        backfill_chat_category_agent_configs_tx(&tx)?;
        let configs = {
            let mut stmt = tx
                .prepare(
                    "SELECT c.id, c.name, cfg.allowed_tools_json, cfg.system_prompt, cfg.created_at, cfg.updated_at
                     FROM chat_categories c
                     INNER JOIN chat_category_agent_configs cfg ON cfg.category_id = c.id
                     ORDER BY c.sort_order ASC, c.created_at ASC",
                )
                .context("prepare list chat category agent configs")?;
            let rows = stmt.query_map([], map_chat_category_agent_config)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .context("list chat category agent configs")?
        };
        tx.commit()
            .context("commit list chat category agent configs")?;
        Ok(configs)
    }

    pub fn save_chat_category_agent_configs(
        &self,
        configs: &[ChatCategoryAgentConfig],
    ) -> Result<Vec<ChatCategoryAgentConfig>> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .context("begin save chat category agent configs")?;
        backfill_chat_category_agent_configs_tx(&tx)?;
        let ts = now();
        for config in configs {
            let allowed_tools_json = serde_json::to_string(&config.allowed_tools)
                .with_context(|| format!("serialize tools for category {}", config.category_id))?;
            let changed = tx
                .execute(
                    "UPDATE chat_category_agent_configs
                     SET allowed_tools_json = ?1, system_prompt = ?2, updated_at = ?3
                     WHERE category_id = ?4",
                    params![
                        allowed_tools_json,
                        config.system_prompt,
                        ts,
                        config.category_id
                    ],
                )
                .with_context(|| format!("update category agent config {}", config.category_id))?;
            if changed == 0 {
                anyhow::bail!("聊天分类不存在或缺少配置：{}", config.category_id);
            }
        }
        tx.commit()
            .context("commit save chat category agent configs")?;
        self.list_chat_category_agent_configs()
    }

    pub fn get_chat_session_category_agent_config(
        &self,
        session_id: &str,
    ) -> Result<Option<ChatCategoryAgentConfig>> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .context("begin get chat session category agent config")?;
        backfill_chat_category_agent_configs_tx(&tx)?;
        let config = tx
            .query_row(
                "SELECT c.id, c.name, cfg.allowed_tools_json, cfg.system_prompt, cfg.created_at, cfg.updated_at
                 FROM chat_sessions s
                 INNER JOIN chat_categories c ON c.id = s.category
                 INNER JOIN chat_category_agent_configs cfg ON cfg.category_id = c.id
                 WHERE s.id = ?1 AND s.category != ''",
                params![session_id],
                map_chat_category_agent_config,
            )
            .optional()
            .context("get chat session category agent config")?;
        tx.commit()
            .context("commit get chat session category agent config")?;
        Ok(config)
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

fn map_chat_category_agent_config(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ChatCategoryAgentConfig> {
    let category_id: String = row.get(0)?;
    let allowed_tools_json: String = row.get(2)?;
    let allowed_tools =
        serde_json::from_str::<Vec<String>>(&allowed_tools_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
    Ok(ChatCategoryAgentConfig {
        category_id,
        category_name: row.get(1)?,
        allowed_tools,
        system_prompt: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> DispatcherDb {
        let path = std::env::temp_dir().join(format!(
            "jkcodingagent-category-config-{}.sqlite3",
            Uuid::new_v4()
        ));
        DispatcherDb::new(path).expect("create test dispatcher db")
    }

    #[test]
    fn init_backfills_default_category_configs() {
        let db = test_db();

        let categories = db.list_chat_categories().expect("list categories");
        let configs = db
            .list_chat_category_agent_configs()
            .expect("list category configs");

        assert!(!categories.is_empty());
        assert_eq!(categories.len(), configs.len());
        assert!(configs
            .iter()
            .all(|config| !config.system_prompt.trim().is_empty()));
        let tech = configs
            .iter()
            .find(|config| config.category_id == "tech")
            .expect("tech config exists");
        let life = configs
            .iter()
            .find(|config| config.category_id == "life")
            .expect("life config exists");
        assert_ne!(tech.system_prompt, life.system_prompt);
        assert!(tech.allowed_tools.contains(&"local_zsh".to_string()));
        assert!(!life.allowed_tools.contains(&"local_zsh".to_string()));
    }

    #[test]
    fn listing_configs_repairs_missing_rows() {
        let db = test_db();
        db.conn()
            .expect("db conn")
            .execute(
                "DELETE FROM chat_category_agent_configs WHERE category_id = 'tech'",
                [],
            )
            .expect("delete one config");

        let configs = db
            .list_chat_category_agent_configs()
            .expect("list category configs");

        assert!(configs.iter().any(|config| config.category_id == "tech"));
    }

    #[test]
    fn creating_category_creates_agent_config() {
        let db = test_db();
        let category = db
            .create_chat_category("测试", "Folder", "#111111", None, None)
            .expect("create category");

        let configs = db
            .list_chat_category_agent_configs()
            .expect("list category configs");

        assert!(configs
            .iter()
            .any(|config| config.category_id == category.id));
    }

    #[test]
    fn creating_category_can_override_agent_config() {
        let db = test_db();
        let category = db
            .create_chat_category(
                "自定义",
                "Folder",
                "#111111",
                Some(vec!["browser_read_text".to_string()]),
                Some("custom prompt"),
            )
            .expect("create category");

        let configs = db
            .list_chat_category_agent_configs()
            .expect("list category configs");
        let config = configs
            .iter()
            .find(|config| config.category_id == category.id)
            .expect("new category config");

        assert_eq!(config.system_prompt, "custom prompt");
        assert_eq!(config.allowed_tools, vec!["browser_read_text".to_string()]);
    }

    #[test]
    fn session_resolves_its_category_agent_config() {
        let db = test_db();
        let configs = db
            .save_chat_category_agent_configs(&[ChatCategoryAgentConfig {
                category_id: "tech".to_string(),
                category_name: "技术".to_string(),
                allowed_tools: vec!["browser_read_text".to_string()],
                system_prompt: "tech prompt".to_string(),
                created_at: String::new(),
                updated_at: String::new(),
            }])
            .expect("save category configs");
        assert!(configs.iter().any(|config| config.category_id == "tech"));

        let session = db
            .create_chat_session("技术会话", Some("tech"))
            .expect("create chat session");
        let config = db
            .get_chat_session_category_agent_config(&session.id)
            .expect("get session category config")
            .expect("session has category config");

        assert_eq!(config.category_id, "tech");
        assert_eq!(config.system_prompt, "tech prompt");
        assert_eq!(config.allowed_tools, vec!["browser_read_text".to_string()]);
    }
}
