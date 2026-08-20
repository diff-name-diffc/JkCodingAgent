//! 聊天分类（chat_categories 表）的 CRUD 与排序。

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
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
    #[serde(default)]
    pub sub_agent_ids: Vec<String>,
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
        // 常规路径：无锁只读，不再为读取开 Immediate 写事务。
        let conn = self.conn()?;
        let configs = read_chat_category_agent_configs(&conn)?;
        let category_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_categories", [], |row| row.get(0))
            .context("count chat categories")?;
        let complete = usize::try_from(category_count)
            .map(|count| configs.len() == count)
            .unwrap_or(false);
        if complete {
            return Ok(configs);
        }
        drop(configs);
        drop(conn);
        // 仅当配置行缺失（旧库升级 / 手工删除）时才走一次写路径按需补行并自愈坏行。
        let mut conn = self.conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin backfill chat category agent configs")?;
        backfill_chat_category_agent_configs_tx(&tx)?;
        repair_corrupt_chat_category_agent_configs_tx(&tx)?;
        tx.commit()
            .context("commit backfill chat category agent configs")?;
        read_chat_category_agent_configs(&conn)
    }

    pub fn save_chat_category_agent_configs(
        &self,
        configs: &[ChatCategoryAgentConfig],
    ) -> Result<Vec<ChatCategoryAgentConfig>> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin save chat category agent configs")?;
        backfill_chat_category_agent_configs_tx(&tx)?;
        // 保存前先自愈存量坏行，避免单行损坏 JSON 让整个保存/读取链路永久失败。
        repair_corrupt_chat_category_agent_configs_tx(&tx)?;
        let ts = now();
        for config in configs {
            let allowed_tools_json = serde_json::to_string(&config.allowed_tools)
                .with_context(|| format!("serialize tools for category {}", config.category_id))?;
            let sub_agent_ids_json =
                serde_json::to_string(&config.sub_agent_ids).with_context(|| {
                    format!(
                        "serialize sub_agent_ids for category {}",
                        config.category_id
                    )
                })?;
            let changed = tx
                .execute(
                    "UPDATE chat_category_agent_configs
                     SET allowed_tools_json = ?1, sub_agent_ids_json = ?2, system_prompt = ?3, updated_at = ?4
                     WHERE category_id = ?5",
                    params![
                        allowed_tools_json,
                        sub_agent_ids_json,
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
        // 常规路径：无锁只读。
        let conn = self.conn()?;
        let config = read_chat_session_category_agent_config(&conn, session_id)?;
        if config.is_some() {
            return Ok(config);
        }
        // 仅当“会话挂了分类但配置行缺失”时才补行；无分类会话保持纯读不拿写锁。
        let has_category: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM chat_sessions s
                     INNER JOIN chat_categories c ON c.id = s.category
                     WHERE s.id = ?1 AND s.category != ''
                 )",
                params![session_id],
                |row| row.get(0),
            )
            .context("check chat session category")?;
        if !has_category {
            return Ok(None);
        }
        drop(conn);
        let mut conn = self.conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin backfill chat session category agent config")?;
        backfill_chat_category_agent_configs_tx(&tx)?;
        repair_corrupt_chat_category_agent_configs_tx(&tx)?;
        tx.commit()
            .context("commit backfill chat session category agent config")?;
        read_chat_session_category_agent_config(&conn, session_id)
    }
}

fn read_chat_category_agent_configs(conn: &Connection) -> Result<Vec<ChatCategoryAgentConfig>> {
    let mut stmt = conn
        .prepare(
            "SELECT c.id, c.name, cfg.allowed_tools_json, cfg.sub_agent_ids_json, cfg.system_prompt, cfg.created_at, cfg.updated_at
             FROM chat_categories c
             INNER JOIN chat_category_agent_configs cfg ON cfg.category_id = c.id
             ORDER BY c.sort_order ASC, c.created_at ASC",
        )
        .context("prepare list chat category agent configs")?;
    let rows = stmt.query_map([], map_chat_category_agent_config)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("list chat category agent configs")
}

fn read_chat_session_category_agent_config(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<ChatCategoryAgentConfig>> {
    conn.query_row(
        "SELECT c.id, c.name, cfg.allowed_tools_json, cfg.sub_agent_ids_json, cfg.system_prompt, cfg.created_at, cfg.updated_at
         FROM chat_sessions s
         INNER JOIN chat_categories c ON c.id = s.category
         INNER JOIN chat_category_agent_configs cfg ON cfg.category_id = c.id
         WHERE s.id = ?1 AND s.category != ''",
        params![session_id],
        map_chat_category_agent_config,
    )
    .optional()
    .context("get chat session category agent config")
}

/// 写路径自愈：把 JSON 已损坏的配置行重置为默认值。
///
/// 与 `backfill_chat_category_agent_configs_tx`（只补缺失行）互补：
/// backfill 修不了已存在的坏行，这里按字段判断并只覆盖损坏的字段。
fn repair_corrupt_chat_category_agent_configs_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    let mut stmt = tx
        .prepare(
            "SELECT category_id, allowed_tools_json, sub_agent_ids_json
             FROM chat_category_agent_configs",
        )
        .context("prepare chat category agent config corruption scan")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut corrupt = Vec::new();
    for row in rows {
        let (category_id, allowed_tools_json, sub_agent_ids_json) =
            row.context("scan chat category agent config row")?;
        let tools_corrupt = serde_json::from_str::<Vec<String>>(&allowed_tools_json).is_err();
        let sub_agents_corrupt = serde_json::from_str::<Vec<String>>(&sub_agent_ids_json).is_err();
        if tools_corrupt || sub_agents_corrupt {
            corrupt.push((category_id, tools_corrupt, sub_agents_corrupt));
        }
    }
    drop(stmt);

    for (category_id, tools_corrupt, sub_agents_corrupt) in corrupt {
        let ts = now();
        if tools_corrupt {
            let (allowed_tools_json, _) = default_chat_category_agent_config_tx(tx, &category_id)?;
            tx.execute(
                "UPDATE chat_category_agent_configs
                 SET allowed_tools_json = ?1, updated_at = ?2
                 WHERE category_id = ?3",
                params![allowed_tools_json, ts, category_id],
            )
            .with_context(|| format!("repair corrupt allowed_tools for category {category_id}"))?;
        }
        if sub_agents_corrupt {
            tx.execute(
                "UPDATE chat_category_agent_configs
                 SET sub_agent_ids_json = '[]', updated_at = ?1
                 WHERE category_id = ?2",
                params![ts, category_id],
            )
            .with_context(|| format!("repair corrupt sub_agent_ids for category {category_id}"))?;
        }
        eprintln!(
            "警告：聊天分类 {category_id} 的智能体配置 JSON 损坏（allowed_tools_corrupt={tools_corrupt}, sub_agent_ids_corrupt={sub_agents_corrupt}），已重置为默认值"
        );
    }
    Ok(())
}

fn parse_category_json_list(category_id: &str, field: &str, raw: &str) -> Vec<String> {
    match serde_json::from_str::<Vec<String>>(raw) {
        Ok(value) => value,
        Err(error) => {
            // 单行坏数据不得拖垮全局：降级为空列表并记录日志，
            // 持久化的默认值由写路径的 repair 负责重写。
            eprintln!(
                "警告：聊天分类 {category_id} 的 {field} 解析失败（{error}），本次按空列表处理"
            );
            Vec::new()
        }
    }
}

fn map_chat_category_agent_config(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ChatCategoryAgentConfig> {
    let category_id: String = row.get(0)?;
    let allowed_tools_json: String = row.get(2)?;
    let sub_agent_ids_json: String = row.get(3)?;
    let allowed_tools =
        parse_category_json_list(&category_id, "allowed_tools_json", &allowed_tools_json);
    let sub_agent_ids =
        parse_category_json_list(&category_id, "sub_agent_ids_json", &sub_agent_ids_json);
    Ok(ChatCategoryAgentConfig {
        category_id,
        category_name: row.get(1)?,
        allowed_tools,
        sub_agent_ids,
        system_prompt: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
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
    fn corrupt_config_json_degrades_on_read_and_self_heals_on_save() {
        let db = test_db();
        db.conn()
            .expect("db conn")
            .execute(
                "UPDATE chat_category_agent_configs
                 SET allowed_tools_json = '{not valid json', sub_agent_ids_json = 'oops'
                 WHERE category_id = 'tech'",
                [],
            )
            .expect("corrupt one config row");

        // 读取不得因单行坏数据整体失败：降级为空列表。
        let configs = db
            .list_chat_category_agent_configs()
            .expect("list category configs despite corrupt row");
        let tech = configs
            .iter()
            .find(|config| config.category_id == "tech")
            .expect("tech config still readable");
        assert!(tech.allowed_tools.is_empty(), "坏 JSON 应降级为空列表");
        assert!(tech.sub_agent_ids.is_empty(), "坏 JSON 应降级为空列表");

        // 保存路径应自愈坏行：重写为默认值。
        let mut repaired = configs.clone();
        let tech_config = repaired
            .iter_mut()
            .find(|config| config.category_id == "tech")
            .expect("tech config to save");
        tech_config.system_prompt = "repaired prompt".to_string();
        let saved = db
            .save_chat_category_agent_configs(&repaired)
            .expect("save category configs after corruption");
        let tech_saved = saved
            .iter()
            .find(|config| config.category_id == "tech")
            .expect("tech config after save");
        assert_eq!(tech_saved.system_prompt, "repaired prompt");

        // 再次读取确认坏行已被修复为可解析的默认工具列表。
        let raw_tools: String = db
            .conn()
            .expect("db conn")
            .query_row(
                "SELECT allowed_tools_json FROM chat_category_agent_configs WHERE category_id = 'tech'",
                [],
                |row| row.get(0),
            )
            .expect("read repaired json");
        assert!(
            serde_json::from_str::<Vec<String>>(&raw_tools).is_ok(),
            "保存后坏行必须被修复为合法 JSON"
        );
    }

    #[test]
    fn deleting_missing_category_reports_error() {
        let db = test_db();
        let error = db
            .delete_chat_category("no-such-category")
            .expect_err("deleting a missing category must fail");
        assert!(error.to_string().contains("聊天分类不存在"));
    }

    #[test]
    fn deleting_existing_category_succeeds() {
        let db = test_db();
        let category = db
            .create_chat_category("待删除", "Folder", "#222222", None, None)
            .expect("create category");
        db.delete_chat_category(&category.id)
            .expect("delete existing category");
        let remaining = db.list_chat_categories().expect("list categories");
        assert!(remaining.iter().all(|item| item.id != category.id));
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
                sub_agent_ids: vec!["browser-agent".to_string()],
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
        assert_eq!(config.sub_agent_ids, vec!["browser-agent".to_string()]);
    }

    /// 回归测试：模拟旧库升级到新版本。
    /// 1) v14 旧库缺 `sub_agent_ids_json` 列 → 升级补列后 backfill/list/save 不报错；
    /// 2) v15 旧库残留 `context_sub_agents`/`session_sub_agents` → 升级后被 DROP。
    #[test]
    fn upgrade_legacy_db_drops_obsolete_tables_and_adds_column() {
        let path =
            std::env::temp_dir().join(format!("jkcodingagent-upgrade-{}.sqlite3", Uuid::new_v4()));
        let db = DispatcherDb::new(path.clone()).expect("create db");

        // 模拟升级前的旧库：回退版本，恢复无 sub_agent_ids_json 列的表结构，
        // 并补上两张已被废弃的关联表（模拟 v15 库的残留）。
        {
            let conn = db.conn().expect("legacy conn");
            conn.execute_batch("PRAGMA user_version = 14;")
                .expect("set legacy user_version");
            conn.execute(
                "CREATE TABLE chat_category_agent_configs_legacy AS
                 SELECT category_id, allowed_tools_json, system_prompt, created_at, updated_at
                 FROM chat_category_agent_configs",
                [],
            )
            .expect("snapshot legacy table");
            conn.execute("DROP TABLE chat_category_agent_configs", [])
                .expect("drop new table");
            conn.execute(
                "ALTER TABLE chat_category_agent_configs_legacy
                 RENAME TO chat_category_agent_configs",
                [],
            )
            .expect("restore legacy table without sub_agent_ids_json");
            // 模拟 v15 残留的废弃关联表
            conn.execute(
                "CREATE TABLE IF NOT EXISTS context_sub_agents (
                     context TEXT NOT NULL, sub_agent_id TEXT NOT NULL,
                     PRIMARY KEY (context, sub_agent_id))",
                [],
            )
            .expect("create legacy context_sub_agents");
            conn.execute(
                "CREATE TABLE IF NOT EXISTS session_sub_agents (
                     session_id TEXT NOT NULL, sub_agent_id TEXT NOT NULL,
                     PRIMARY KEY (session_id, sub_agent_id))",
                [],
            )
            .expect("create legacy session_sub_agents");
        }

        // 触发升级流程（模拟应用重启走 init）。
        db.init().expect("upgrade legacy db");

        // 升级后：sub_agent_ids 列存在，list/save 不再报错。
        let configs = db
            .list_chat_category_agent_configs()
            .expect("list after upgrade");
        assert!(
            configs
                .iter()
                .all(|config| serde_json::to_string(&config.sub_agent_ids).is_ok()),
            "sub_agent_ids 字段在升级后可读"
        );
        let saved = db
            .save_chat_category_agent_configs(&configs)
            .expect("save after upgrade");
        assert!(!saved.is_empty());

        // 升级后：废弃关联表必须已被删除。
        let conn = db.conn().expect("post-upgrade conn");
        let leftover: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name IN ('context_sub_agents', 'session_sub_agents')",
                [],
                |row| row.get(0),
            )
            .expect("count leftover tables");
        assert_eq!(leftover, 0, "废弃的关联表在升级后应被删除");
    }
}
