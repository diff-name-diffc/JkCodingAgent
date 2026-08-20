//! 应用级键值配置：app_config 表（key → value_json）。
//!
//! 承载不属于智能体领域但同属应用生命周期配置的条目：外观设置
//! （app_settings）、全局浏览器选项（browser）、RAG 知识库配置（rag）。
//! v33 迁移把对应的散落 JSON 文件一次性导入本表后删除原文件。

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension, Transaction};

use super::DispatcherDb;
use crate::browser::BrowserConfig;

pub(crate) const APP_SETTINGS_KEY: &str = "app_settings";
pub(crate) const BROWSER_KEY: &str = "browser";
pub(crate) const RAG_KEY: &str = "rag";

const APP_CONFIG_DDL: &str = "CREATE TABLE IF NOT EXISTS app_config (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL
)";

/// 建表（幂等）。基线 DDL 与 v33 迁移共用，保持同文。
pub(crate) fn ensure_app_config_table_tx(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(APP_CONFIG_DDL).context("create app_config table")
}

/// v33 迁移：导入散落的 JSON 配置文件。逐项幂等（已有键跳过）。
/// 返回迁移后需要清理的文件列表（commit 成功后 best-effort 删除）。
pub(crate) fn import_legacy_app_config_files(
    tx: &Transaction<'_>,
    root_dir: &Path,
) -> Result<Vec<std::path::PathBuf>> {
    let mut cleanup = Vec::new();

    // 1. 外观设置：settings.json → app_settings
    let settings_json = root_dir.join("settings.json");
    if let Ok(raw) = std::fs::read_to_string(&settings_json) {
        if serde_json::from_str::<serde_json::Value>(&raw).is_ok() {
            insert_if_absent(tx, APP_SETTINGS_KEY, &raw)?;
            cleanup.push(settings_json);
        }
    }

    // 2. RAG 配置：rag/config.json → rag
    let rag_json = root_dir.join("rag").join("config.json");
    if let Ok(raw) = std::fs::read_to_string(&rag_json) {
        if serde_json::from_str::<serde_json::Value>(&raw).is_ok() {
            insert_if_absent(tx, RAG_KEY, &raw)?;
            cleanup.push(rag_json);
        }
    }

    // 3. 浏览器选项：迁移前为项目级 config.toml [browser]，按注册顺序取
    //    第一个「非默认」配置作为全局默认；全部默认则不写键（读取端用默认）。
    if tx
        .query_row(
            "SELECT COUNT(*) FROM app_config WHERE key = ?1",
            params![BROWSER_KEY],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        == 0
    {
        if let Some(browser) = first_non_default_project_browser(tx)? {
            let raw = serde_json::to_string(&browser).context("serialize browser config")?;
            insert_if_absent(tx, BROWSER_KEY, &raw)?;
        }
    }

    Ok(cleanup)
}

fn insert_if_absent(tx: &Transaction<'_>, key: &str, value_json: &str) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO app_config (key, value_json) VALUES (?1, ?2)",
        params![key, value_json],
    )
    .with_context(|| format!("insert app_config {key}"))?;
    Ok(())
}

/// 遍历注册项目（v31 已入库），取第一个非默认的 [browser] 配置。
fn first_non_default_project_browser(tx: &Transaction<'_>) -> Result<Option<BrowserConfig>> {
    let mut stmt = tx
        .prepare("SELECT path FROM projects ORDER BY sort_order, rowid")
        .context("prepare list projects for browser migration")?;
    let paths = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("load project paths for browser migration")?;
    for path in paths {
        let config_path = Path::new(&path).join(".jkcodingagent").join("config.toml");
        let Ok(raw) = std::fs::read_to_string(&config_path) else {
            continue;
        };
        #[derive(serde::Deserialize, Default)]
        struct BrowserSection {
            #[serde(default)]
            browser: BrowserConfig,
        }
        let Ok(section) = toml::from_str::<BrowserSection>(&raw) else {
            continue;
        };
        if section.browser != BrowserConfig::default() {
            return Ok(Some(section.browser));
        }
    }
    Ok(None)
}

/// 迁移提交成功后清理旧文件（best-effort）。
pub(crate) fn cleanup_legacy_files(files: &[std::path::PathBuf]) {
    for file in files {
        if let Err(error) = std::fs::remove_file(file) {
            eprintln!(
                "[app-config] 迁移后清理旧配置文件失败（{}）：{error}；建议手动删除。",
                file.display()
            );
        }
    }
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
        assert!(db.get_app_config_json(APP_SETTINGS_KEY).unwrap().is_none());
        db.set_app_config_json(APP_SETTINGS_KEY, r#"{"theme":"dark"}"#)
            .unwrap();
        db.set_app_config_json(APP_SETTINGS_KEY, r#"{"theme":"light"}"#)
            .unwrap();
        assert_eq!(
            db.get_app_config_json(APP_SETTINGS_KEY).unwrap().as_deref(),
            Some(r#"{"theme":"light"}"#)
        );
    }

    #[test]
    fn migrates_settings_and_rag_files_and_project_browser() {
        let (db, root) = test_db();
        assert!(db.get_app_config_json(APP_SETTINGS_KEY).unwrap().is_none());
        drop(db);

        // 重建旧布局：settings.json / rag/config.json / 项目 config.toml + projects 表数据。
        std::fs::remove_file(root.join("jkbot.sqlite3")).unwrap();
        std::fs::write(root.join("settings.json"), r#"{"theme":"dark"}"#).unwrap();
        std::fs::create_dir_all(root.join("rag")).unwrap();
        std::fs::write(root.join("rag").join("config.json"), r#"{"log_level":"INFO"}"#).unwrap();
        let project_dir = root.join("proj");
        let jk_dir = project_dir.join(".jkcodingagent");
        std::fs::create_dir_all(&jk_dir).unwrap();
        std::fs::write(
            jk_dir.join("config.toml"),
            "[git]\ncommit_prompt = \"x\"\n\n[browser]\nenabled = true\nproxy = \"http://127.0.0.1:7890\"\n",
        )
        .unwrap();

        let db = DispatcherDb::new(root.join("jkbot.sqlite3")).unwrap();
        // 模拟 v31 已把项目入库（新装库直接跑全量迁移时 projects.json 缺失，
        // 项目浏览器配置无从谈起，属预期）。
        db.save_projects_all(&[crate::project::storage::Project {
            id: "p1".to_string(),
            name: "proj".to_string(),
            path: project_dir.to_string_lossy().into_owned(),
            branch: None,
            last_opened_at: 0,
        }])
        .unwrap();

        // 手动重跑 v33 导入逻辑（init 已在 new 时跑过一次，幂等验证）。
        let mut conn = db.conn().unwrap();
        let tx = conn.transaction().unwrap();
        let cleanup = import_legacy_app_config_files(&tx, &root).unwrap();
        tx.commit().unwrap();
        drop(conn);
        cleanup_legacy_files(&cleanup);

        let settings = db.get_app_config_json(APP_SETTINGS_KEY).unwrap().unwrap();
        assert!(settings.contains("dark"));
        let rag = db.get_app_config_json(RAG_KEY).unwrap().unwrap();
        assert!(rag.contains("INFO"));
        let browser_raw = db.get_app_config_json(BROWSER_KEY).unwrap().unwrap();
        let browser: BrowserConfig = serde_json::from_str(&browser_raw).unwrap();
        assert_eq!(browser.proxy, "http://127.0.0.1:7890");
        assert!(!root.join("settings.json").exists());
        assert!(!root.join("rag").join("config.json").exists());
    }
}
