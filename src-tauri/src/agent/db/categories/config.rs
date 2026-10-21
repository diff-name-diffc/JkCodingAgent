//! 聊天分类的智能体配置读写。

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use super::ChatCategoryAgentConfig;
use crate::agent::chat_directories::normalize_project_directories;
use crate::agent::db::schema::{
    backfill_chat_category_agent_configs_tx, default_chat_category_agent_config_tx,
};
use crate::agent::db::util::now;
use crate::agent::db::DispatcherDb;

impl DispatcherDb {
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
        // 路径检查涉及文件系统 I/O，先校验完整批次，再取得数据库写事务。
        let project_directories_json = configs
            .iter()
            .map(|config| {
                let directories = normalize_project_directories(&config.project_directories)
                    .with_context(|| {
                        format!("聊天分类 {} 的共享工程目录无效", config.category_id)
                    })?;
                let paths = directories
                    .iter()
                    .map(|path| path.to_str().context("共享工程目录路径必须是 UTF-8"))
                    .collect::<Result<Vec<_>>>()?;
                serde_json::to_string(&paths).context("serialize category project directories")
            })
            .collect::<Result<Vec<_>>>()?;
        let mut conn = self.conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin save chat category agent configs")?;
        backfill_chat_category_agent_configs_tx(&tx)?;
        // 保存前先自愈存量坏行，避免单行损坏 JSON 让整个保存/读取链路永久失败。
        repair_corrupt_chat_category_agent_configs_tx(&tx)?;
        let ts = now();
        for (config, directories_json) in configs.iter().zip(project_directories_json) {
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
                     SET allowed_tools_json = ?1, sub_agent_ids_json = ?2, system_prompt = ?3, updated_at = ?4,
                         project_directories_json = ?6
                     WHERE category_id = ?5",
                    params![
                        allowed_tools_json,
                        sub_agent_ids_json,
                        config.system_prompt,
                        ts,
                        config.category_id,
                        directories_json
                    ],
                )
                .with_context(|| format!("update category agent config {}", config.category_id))?;
            if changed == 0 {
                anyhow::bail!("聊天分类不存在或缺少配置：{}", config.category_id);
            }
        }
        // 新目录字段损坏必须使整次保存失败；在提交前读取，避免“已提交却报错”。
        let saved = read_chat_category_agent_configs(&tx)?;
        tx.commit()
            .context("commit save chat category agent configs")?;
        Ok(saved)
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
                     FROM dispatcher_sessions s
                     INNER JOIN chat_categories c ON c.id = s.category
                     WHERE s.id = ?1 AND s.kind = 'chat' AND s.category != ''
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
            "SELECT c.id, c.name, cfg.allowed_tools_json, cfg.sub_agent_ids_json, cfg.system_prompt, cfg.created_at, cfg.updated_at, cfg.project_directories_json
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
        "SELECT c.id, c.name, cfg.allowed_tools_json, cfg.sub_agent_ids_json, cfg.system_prompt, cfg.created_at, cfg.updated_at, cfg.project_directories_json
         FROM dispatcher_sessions s
         INNER JOIN chat_categories c ON c.id = s.category
         INNER JOIN chat_category_agent_configs cfg ON cfg.category_id = c.id
         WHERE s.id = ?1 AND s.kind = 'chat' AND s.category != ''",
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
    let project_directories_json: String = row.get(7)?;
    let project_directories = serde_json::from_str::<Vec<String>>(&project_directories_json)
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                7,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("聊天分类 {category_id} 的共享工程目录配置 JSON 损坏：{error}"),
                )),
            )
        })?;
    Ok(ChatCategoryAgentConfig {
        category_id,
        category_name: row.get(1)?,
        allowed_tools,
        sub_agent_ids,
        project_directories,
        system_prompt: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}
