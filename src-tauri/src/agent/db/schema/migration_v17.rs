//! v16 → v17：浏览器自动化迁入 browser-use，保留用户定制与历史轨迹。
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, Transaction};
use serde_json::Value;

use super::{categories::scenario_chat_category_agent_config, v16_prompts};
use crate::agent::sub_agent::config::{BROWSER_AGENT_ID, BROWSER_AGENT_SYSTEM_PROMPT};

const RETIRED_TOOLS: [&str; 7] = [
    "browser_click",
    "browser_type",
    "browser_press",
    "browser_wait_for",
    "browser_read_text",
    "browser_visual_analyze",
    "browser_close",
];

/// 旧开发链也占用过版本 17；检查当前领域形态，不能仅凭版本号放行。
pub(super) fn verify_shape(conn: &Connection) -> Result<()> {
    for sql in [
        "SELECT settings_json FROM dispatcher_settings LIMIT 0",
        "SELECT config_json, enabled FROM sub_agents LIMIT 0",
        "SELECT allowed_tools_json, system_prompt, sub_agent_ids_json FROM chat_category_agent_configs LIMIT 0",
        "SELECT definition_json FROM workflow_plans LIMIT 0",
    ] {
        conn.prepare(sql).context("数据库不符合 v16 及后续基线形态，无法迁移或打开；请使用匹配版本或恢复备份")?;
    }
    Ok(())
}

pub(super) fn backup(conn: &Connection, path: &Path) -> Result<()> {
    let target = path.with_extension(format!("pre-v17-{}.sqlite3", uuid::Uuid::new_v4()));
    let target_text = target.to_str().context("v17 backup path must be UTF-8")?;
    if let Err(error) = conn.execute("VACUUM INTO ?1", params![target_text]) {
        eprintln!(
            "数据库 v17 迁移备份失败，已停止迁移：{}：{error}",
            target.display()
        );
        return Err(error).context("backup database before v17 browser migration");
    }
    Ok(())
}

pub(super) fn migrate(tx: &Transaction<'_>) -> Result<Vec<String>> {
    let disabled = migrate_sub_agents(tx)?;
    migrate_categories(tx)?;
    migrate_settings(tx)?;
    Ok(disabled)
}

fn remove_retired(tools: &mut Vec<String>, delegate: bool) -> bool {
    let previous_len = tools.len();
    tools.retain(|tool| !RETIRED_TOOLS.contains(&tool.as_str()));
    let changed = previous_len != tools.len();
    if changed && delegate {
        for tool in ["list_sub_agents", "call_sub_agent"] {
            if !tools.iter().any(|value| value == tool) {
                tools.push(tool.to_string());
            }
        }
    }
    changed
}

fn migrate_sub_agents(tx: &Transaction<'_>) -> Result<Vec<String>> {
    let mut stmt = tx.prepare("SELECT id, config_json, enabled FROM sub_agents ORDER BY id")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    let mut disabled = Vec::new();
    for (id, raw, mut enabled) in rows {
        // 单行坏数据不得让迁移失败锁死应用启动（读路径对坏 config 一贯降级）；
        // 跳过该行保留原样，退役工具残留由读路径/工具面未知工具报错自愈。
        let mut config: Value = match serde_json::from_str(&raw) {
            Ok(config) => config,
            Err(error) => {
                eprintln!("v17 迁移：子智能体 {id} 的 config_json 无法解析，保留原样跳过：{error}");
                continue;
            }
        };
        if !config.is_object() {
            eprintln!("v17 迁移：子智能体 {id} 的 config_json 不是对象，保留原样跳过");
            continue;
        }
        let original = config.clone();
        let mut tools: Vec<String> = match serde_json::from_value(config["allowed_tools"].clone()) {
            Ok(tools) => tools,
            Err(error) => {
                eprintln!(
                    "v17 迁移：子智能体 {id} 的 allowed_tools 无法解析，保留原样跳过：{error}"
                );
                continue;
            }
        };
        if id == BROWSER_AGENT_ID {
            tools.clear();
            if config["system_prompt"].as_str() == Some(v16_prompts::BROWSER_AGENT_PROMPT) {
                config["system_prompt"] = BROWSER_AGENT_SYSTEM_PROMPT.into();
            }
        } else if remove_retired(&mut tools, false) && tools.is_empty() {
            config["enabled"] = false.into();
            enabled = 0;
            disabled.push(id.clone());
        }
        config["allowed_tools"] = serde_json::to_value(tools)?;
        if config != original {
            let timestamp = chrono::Utc::now().timestamp_millis();
            config["updated_at"] = timestamp.into();
            tx.execute(
                "UPDATE sub_agents SET config_json = ?1, enabled = ?2, updated_at = ?3 WHERE id = ?4",
                params![serde_json::to_string(&config)?, enabled, timestamp, id],
            ).context("update sub-agent for browser-use")?;
        }
    }
    Ok(disabled)
}

fn migrate_categories(tx: &Transaction<'_>) -> Result<()> {
    let mut stmt = tx.prepare("SELECT category_id, allowed_tools_json, system_prompt FROM chat_category_agent_configs ORDER BY category_id")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    for (id, raw, prompt) in rows {
        // 与读路径一致：坏 allowed_tools_json 按不可解析跳过，交给读路径的
        // 降级-自愈闭环处理，不阻塞迁移。
        let mut tools: Vec<String> = match serde_json::from_str(&raw) {
            Ok(tools) => tools,
            Err(error) => {
                eprintln!(
                    "v17 迁移：分类 {id} 的 allowed_tools_json 无法解析，保留原样跳过：{error}"
                );
                continue;
            }
        };
        let tools_changed = remove_retired(&mut tools, true);
        let next_prompt = if v16_prompts::category_prompt(&id) == Some(prompt.as_str()) {
            scenario_chat_category_agent_config(&id)
                .context("missing v17 category default")?
                .system_prompt
        } else if prompt == v16_prompts::PLAIN_CHAT_PROMPT {
            crate::agent::config::DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT
        } else {
            &prompt
        };
        if tools_changed || next_prompt != prompt {
            tx.execute(
                "UPDATE chat_category_agent_configs SET allowed_tools_json = ?1, system_prompt = ?2, updated_at = ?3 WHERE category_id = ?4",
                params![serde_json::to_string(&tools)?, next_prompt, super::super::util::now(), id],
            ).context("update category for browser-use")?;
        }
    }
    Ok(())
}

fn migrate_settings(tx: &Transaction<'_>) -> Result<()> {
    let mut stmt = tx.prepare("SELECT id, settings_json FROM dispatcher_settings ORDER BY id")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    for (id, raw) in rows {
        // 坏 settings 行保留原样跳过：读路径本就按错误报告该行，迁移不能更苛刻。
        let mut settings: Value = match serde_json::from_str(&raw) {
            Ok(settings) => settings,
            Err(error) => {
                eprintln!("v17 迁移：设置 {id} 的 settings_json 无法解析，保留原样跳过：{error}");
                continue;
            }
        };
        if !settings.is_object() {
            eprintln!("v17 迁移：设置 {id} 的 settings_json 不是对象，保留原样跳过");
            continue;
        }
        let mut changed = false;
        for context_name in ["chat", "project"] {
            let Some(context) = settings.get_mut(context_name) else {
                continue;
            };
            if !context.is_object() {
                eprintln!("v17 迁移：设置 {id} 的 {context_name} 不是对象，该用途保留原样跳过");
                continue;
            }
            let Some(value) = context.get_mut("allowedTools") else {
                continue;
            };
            let mut tools: Vec<String> = match serde_json::from_value(value.clone()) {
                Ok(tools) => tools,
                Err(error) => {
                    eprintln!(
                        "v17 迁移：设置 {id} 的 {context_name}.allowedTools 无法解析，该用途保留原样跳过：{error}"
                    );
                    continue;
                }
            };
            if remove_retired(&mut tools, true) {
                *value = serde_json::to_value(tools)?;
                changed = true;
            }
        }
        if changed {
            tx.execute(
                "UPDATE dispatcher_settings SET settings_json = ?1 WHERE id = ?2",
                params![serde_json::to_string(&settings)?, id],
            )
            .context("update settings for browser-use")?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "migration_v17_tests.rs"]
mod tests;
