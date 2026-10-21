use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::*;
use crate::agent::{db::DispatcherDb, sub_agent::config::SubAgentConfig};

fn legacy_db(tag: &str) -> (std::path::PathBuf, crate::test_util::TempDirGuard) {
    let dir = crate::test_util::TempDirGuard::new(&format!("browser-v17-{tag}"));
    let path = dir.path().join("test.sqlite3");
    drop(DispatcherDb::new(path.clone()).unwrap());
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "user_version", 16).unwrap();
    let mut browser = serde_json::to_value(SubAgentConfig::browser_agent_default()).unwrap();
    browser["system_prompt"] = v16_prompts::BROWSER_AGENT_PROMPT.into();
    browser["allowed_tools"] = json!(["browser_open_url", "browser_click", "browser_read_text"]);
    conn.execute(
        "UPDATE sub_agents SET config_json = ?1 WHERE id = ?2",
        params![browser.to_string(), BROWSER_AGENT_ID],
    )
    .unwrap();
    for id in ["general", "life", "work", "tech", "learning"] {
        conn.execute(
            "UPDATE chat_category_agent_configs SET allowed_tools_json = ?1, system_prompt = ?2 WHERE category_id = ?3",
            params![r#"["browser_open_url","browser_read_text"]"#, v16_prompts::category_prompt(id).unwrap(), id],
        ).unwrap();
    }
    conn.execute(
        "INSERT INTO dispatcher_settings (id, settings_json) VALUES ('default', ?1)",
        params![json!({
            "chat": { "allowedTools": ["browser_read_text", "list_sub_agents", "custom_tool"] },
            "project": { "allowedTools": ["browser_click", "browser_open_url"] },
            "modelLibrary": [{ "id": "keep-model", "apiKey": "test-key" }],
            "unknownSetting": { "preserve": true }
        })
        .to_string()],
    )
    .unwrap();
    (path, dir)
}

fn browser_config(conn: &Connection) -> Value {
    let raw: String = conn
        .query_row(
            "SELECT config_json FROM sub_agents WHERE id = ?1",
            params![BROWSER_AGENT_ID],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn snapshot(conn: &Connection) -> Vec<String> {
    let mut values = Vec::new();
    for sql in [
        "SELECT id || config_json || enabled || updated_at FROM sub_agents ORDER BY id",
        "SELECT category_id || allowed_tools_json || system_prompt || sub_agent_ids_json || updated_at FROM chat_category_agent_configs ORDER BY category_id",
        "SELECT id || settings_json FROM dispatcher_settings ORDER BY id",
    ] {
        let mut stmt = conn.prepare(sql).unwrap();
        values.extend(stmt.query_map([], |row| row.get::<_, String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap());
    }
    values
}

fn insert_custom_agent(conn: &Connection, id: &str, tools: &[&str]) {
    let mut config = SubAgentConfig::browser_agent_default();
    config.agent_id = id.to_string();
    config.system_prompt = "保留自定义任务要求".to_string();
    config.allowed_tools = tools.iter().map(|name| name.to_string()).collect();
    conn.execute(
        "INSERT INTO sub_agents (id, name, description, config_json, enabled, created_at, updated_at) VALUES (?1, ?1, 'custom', ?2, 1, 1, 1)",
        params![id, serde_json::to_string(&config).unwrap()],
    ).unwrap();
}

#[test]
fn upgrade_preserves_models_custom_fields_and_disabled_state() {
    let (path, dir) = legacy_db("preserve");
    let conn = Connection::open(&path).unwrap();
    let mut browser = browser_config(&conn);
    browser["model_config"] = json!({"inherit_from_parent": false, "model_name": "custom-model", "api_key": "test-key", "api_base": "http://localhost:8000/v1"});
    browser["max_iterations"] = 73.into();
    browser["timeout_secs"] = 1200.into();
    browser["enabled"] = false.into();
    browser["agent_name"] = "定制浏览器".into();
    browser["unknown_field"] = json!({"keep": true});
    conn.execute(
        "UPDATE sub_agents SET config_json = ?1, name = '定制浏览器', enabled = 0 WHERE id = ?2",
        params![browser.to_string(), BROWSER_AGENT_ID],
    )
    .unwrap();
    insert_custom_agent(
        &conn,
        "browser-only",
        &["browser_read_text", "browser_click"],
    );
    insert_custom_agent(&conn, "mixed-agent", &["browser_read_text", "local_zsh"]);
    drop(conn);

    let db = DispatcherDb::new(path.clone()).unwrap();
    let conn = db.conn().unwrap();
    let migrated = browser_config(&conn);
    assert_eq!(migrated["allowed_tools"], json!([]));
    assert_eq!(migrated["system_prompt"], BROWSER_AGENT_SYSTEM_PROMPT);
    for key in [
        "model_config",
        "max_iterations",
        "timeout_secs",
        "enabled",
        "agent_name",
        "unknown_field",
    ] {
        assert_eq!(migrated[key], browser[key], "must preserve {key}");
    }
    let disabled: (String, i64) = conn
        .query_row(
            "SELECT config_json, enabled FROM sub_agents WHERE id = 'browser-only'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let disabled_config: Value = serde_json::from_str(&disabled.0).unwrap();
    assert_eq!(disabled.1, 0);
    assert_eq!(disabled_config["enabled"], false);
    assert_eq!(disabled_config["allowed_tools"], json!([]));
    assert_eq!(disabled_config["system_prompt"], "保留自定义任务要求");
    let mixed: String = conn
        .query_row(
            "SELECT config_json FROM sub_agents WHERE id = 'mixed-agent'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mixed: Value = serde_json::from_str(&mixed).unwrap();
    assert_eq!(mixed["allowed_tools"], json!(["local_zsh"]));
    assert_eq!(mixed["enabled"], true);
    let settings: String = conn
        .query_row("SELECT settings_json FROM dispatcher_settings", [], |row| {
            row.get(0)
        })
        .unwrap();
    let settings: Value = serde_json::from_str(&settings).unwrap();
    assert_eq!(
        settings["chat"]["allowedTools"],
        json!(["list_sub_agents", "custom_tool", "call_sub_agent"])
    );
    assert_eq!(
        settings["project"]["allowedTools"],
        json!(["browser_open_url", "list_sub_agents", "call_sub_agent"])
    );
    assert_eq!(settings["modelLibrary"][0]["apiKey"], "test-key");
    assert_eq!(settings["unknownSetting"], json!({"preserve": true}));
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
            .unwrap(),
        super::super::SCHEMA_VERSION
    );
    assert!(std::fs::read_dir(dir.path()).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains("pre-v17-")));
}

#[test]
fn custom_prompts_survive_and_default_category_prompts_migrate() {
    let (path, _dir) = legacy_db("prompts");
    let conn = Connection::open(&path).unwrap();
    let mut browser = browser_config(&conn);
    browser["system_prompt"] = "我定制的 browser_read_text 使用要求".into();
    conn.execute(
        "UPDATE sub_agents SET config_json = ?1 WHERE id = ?2",
        params![browser.to_string(), BROWSER_AGENT_ID],
    )
    .unwrap();
    conn.execute("UPDATE chat_category_agent_configs SET system_prompt = '自定义场景', sub_agent_ids_json = '[\"custom-agent\"]' WHERE category_id = 'life'", []).unwrap();
    drop(conn);
    let db = DispatcherDb::new(path).unwrap();
    let conn = db.conn().unwrap();
    assert_eq!(
        browser_config(&conn)["system_prompt"],
        browser["system_prompt"]
    );
    let life: (String, String, String) = conn.query_row("SELECT system_prompt, sub_agent_ids_json, allowed_tools_json FROM chat_category_agent_configs WHERE category_id = 'life'", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
    assert_eq!(life.0, "自定义场景");
    assert_eq!(life.1, r#"["custom-agent"]"#);
    assert_eq!(
        serde_json::from_str::<Value>(&life.2).unwrap(),
        json!(["browser_open_url", "list_sub_agents", "call_sub_agent"])
    );
    let work: String = conn
        .query_row(
            "SELECT system_prompt FROM chat_category_agent_configs WHERE category_id = 'work'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(work.contains("browser-agent"));
    assert!(!work.contains("browser_read_text"));
}

#[test]
fn migration_is_idempotent_without_timestamp_or_data_churn() {
    let (path, _dir) = legacy_db("idempotent");
    let db = DispatcherDb::new(path.clone()).unwrap();
    let mut conn = db.conn().unwrap();
    let before = snapshot(&conn);
    let tx = conn.transaction().unwrap();
    assert!(migrate(&tx).unwrap().is_empty());
    tx.commit().unwrap();
    assert_eq!(snapshot(&conn), before);
    drop(conn);
    drop(db);
    let reopened = DispatcherDb::new(path).unwrap();
    assert_eq!(snapshot(&reopened.conn().unwrap()), before);
}

#[test]
fn failed_migration_rolls_back_all_changes_and_can_retry() {
    let (path, _dir) = legacy_db("rollback");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_v17 BEFORE UPDATE ON dispatcher_settings BEGIN SELECT RAISE(ABORT, 'forced migration failure'); END;").unwrap();
    let before = snapshot(&conn);
    drop(conn);
    let error = DispatcherDb::new(path.clone()).unwrap_err();
    assert!(format!("{error:#}").contains("forced migration failure"));
    let conn = Connection::open(&path).unwrap();
    assert_eq!(snapshot(&conn), before);
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
            .unwrap(),
        16
    );
    conn.execute_batch("DROP TRIGGER fail_v17").unwrap();
    drop(conn);
    DispatcherDb::new(path).expect("retry after removing failure must succeed");
}

#[test]
fn malformed_config_is_skipped_not_replaced_and_does_not_block_startup() {
    let (path, _dir) = legacy_db("corrupt");
    let conn = Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE dispatcher_settings SET settings_json = '{broken'",
        [],
    )
    .unwrap();
    drop(conn);
    // 单行坏数据不得让迁移失败锁死应用启动（读路径对坏 settings 一贯按
    // 错误降级报告，而非拒绝打开整库）：该行保留原样跳过，其余数据照常迁移。
    let db = DispatcherDb::new(path).expect("malformed single row must not block startup");
    let conn = db.conn().unwrap();
    let raw: String = conn
        .query_row(
            "SELECT settings_json FROM dispatcher_settings LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(raw, "{broken", "corrupt row must be preserved verbatim");
    let prompt: String = conn
        .query_row(
            "SELECT config_json FROM sub_agents WHERE id = ?1",
            params![BROWSER_AGENT_ID],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        !prompt.contains(v16_prompts::BROWSER_AGENT_PROMPT),
        "other rows must still migrate normally"
    );
}

#[test]
fn reused_legacy_v17_version_is_rejected_by_shape() {
    let dir = crate::test_util::TempDirGuard::new("legacy-version-17");
    let path = dir.path().join("test.sqlite3");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE graph_plans (id TEXT PRIMARY KEY); PRAGMA user_version = 17;")
        .unwrap();
    drop(conn);
    assert!(DispatcherDb::new(path).is_err());
}

#[test]
fn fresh_defaults_delegate_to_browser_agent() {
    let dir = crate::test_util::TempDirGuard::new("browser-v17-fresh");
    let db = DispatcherDb::new(dir.path().join("test.sqlite3")).unwrap();
    let conn = db.conn().unwrap();
    let browser = browser_config(&conn);
    assert_eq!(browser["allowed_tools"], json!([]));
    assert_eq!(browser["system_prompt"], BROWSER_AGENT_SYSTEM_PROMPT);
    for category in db.list_chat_category_agent_configs().unwrap() {
        assert!(category
            .allowed_tools
            .iter()
            .all(|name| !RETIRED_TOOLS.contains(&name.as_str())));
        assert!(category
            .allowed_tools
            .contains(&"list_sub_agents".to_string()));
        assert!(category
            .allowed_tools
            .contains(&"call_sub_agent".to_string()));
        assert!(category.system_prompt.contains("browser-agent"));
    }
}
