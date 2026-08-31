//! 应用级键值配置：app_config 表（key → value_json）。
//!
//! 承载不属于智能体领域但同属应用生命周期配置的条目：全局浏览器选项
//! （browser）、RAG 知识库配置（rag）。外观主题已并入 `AhaSettingsV2`
//! （dispatcher_settings.theme），不再占用本表。

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension, Transaction};

use super::DispatcherDb;

pub(crate) const BROWSER_KEY: &str = "browser";
pub(crate) const RAG_KEY: &str = "rag";

const APP_CONFIG_DDL: &str = "CREATE TABLE IF NOT EXISTS app_config (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL
)";

/// 建表（幂等）。schema.rs 基线建库时调用，DDL 单一出处。
pub(crate) fn ensure_app_config_table_tx(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(APP_CONFIG_DDL)
        .context("create app_config table")
}

impl DispatcherDb {
    pub fn get_app_config_json(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT value_json FROM app_config WHERE key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .context("load app_config")
    }

    pub fn set_app_config_json(&self, key: &str, value_json: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO app_config (key, value_json) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value_json = ?2",
            params![key, value_json],
        )
        .with_context(|| format!("save app_config {key}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> (DispatcherDb, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "aha-app-config-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        (DispatcherDb::new(root.join("jkbot.sqlite3")).unwrap(), root)
    }

    #[test]
    fn roundtrips_json_values() {
        let (db, _root) = test_db();
        const KEY: &str = "roundtrip-test";
        assert!(db.get_app_config_json(KEY).unwrap().is_none());
        db.set_app_config_json(KEY, r#"{"flag":true}"#).unwrap();
        db.set_app_config_json(KEY, r#"{"flag":false}"#).unwrap();
        assert_eq!(
            db.get_app_config_json(KEY).unwrap().as_deref(),
            Some(r#"{"flag":false}"#)
        );
    }
}
