//! sync_directory 工具测试。移植自旧 `tools/builtin/sync_directory_tests.rs`。
//!
//! 已随审查门禁移至 runtime 策略层（TODO T3）而删除的旧用例：
//! - `missing_review_config_blocks_even_when_server_review_is_disabled_and_audits`
//! - `configured_review_honors_explicit_server_exemption`
//! - `registered_for_chat_with_self_managed_review`（旧注册表形态，已不适用）
//! - `runtime_prompt_preserves_system_and_exposes_current_scope`（prompt 模块职责，非本组）

use super::*;
use crate::ssh_tool::SshDb;
use serde_json::json;

struct Fixture {
    root: PathBuf,
    db_path: PathBuf,
    manager: SshSessionManager,
    ssh_db: SshDb,
    workspace_id: String,
    db: DispatcherDb,
}

/// 审查上下文：配置存在但测试服务器显式关闭「执行前审查」
/// （fixture 的 `reviewEnabled:false`），因此审查按豁免通道放行、
/// 用例得以到达路径校验与连接阶段（不触发真实审查模型请求）。
fn review_context_with_config() -> crate::agent::rig_ext::review::RigReviewContext {
    crate::agent::rig_ext::review::RigReviewContext {
        config: Some(crate::agent::db::settings::SshReviewConfig::default()),
        session_title: "sync-test".to_string(),
        user_task: None,
        executor_task: None,
        review_conversation: None,
    }
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("sync-tool-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let pool = Arc::new(
            r2d2::Pool::builder()
                .max_size(1)
                .build(r2d2_sqlite::SqliteConnectionManager::memory())
                .unwrap(),
        );
        {
            let mut connection = pool.get().unwrap();
            let tx = connection.transaction().unwrap();
            crate::ssh_tool::db::ensure_ssh_tables_tx(&tx).unwrap();
            tx.commit().unwrap();
        }
        let ssh_db = SshDb::new(pool.clone());
        ssh_db
            .save_servers(&[serde_json::from_value(json!({
                "id":"customer-beijing-01", "host":"127.0.0.1", "username":"tester",
                "password":"never-disclose", "reviewEnabled":false, "defaultTimeoutSecs":2
            }))
            .unwrap()])
            .unwrap();
        let db_path = root.join("dispatcher.sqlite3");
        let db = DispatcherDb::new(db_path.clone()).unwrap();
        Self {
            root,
            db_path,
            manager: SshSessionManager::new(pool),
            ssh_db,
            workspace_id: uuid::Uuid::new_v4().to_string(),
            db,
        }
    }

    fn tool(&self) -> PortableDynamicTool {
        sync_directory_tool(
            self.manager.clone(),
            self.root.clone(),
            self.workspace_id.clone(),
            true,
            vec![],
            None,
            self.db.clone(),
            None,
            review_context_with_config(),
        )
    }

    fn args(&self) -> Value {
        json!({
            "ssh_profile":"customer-beijing-01", "source":self.root, "destination":"/opt/releases/v1"
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// 执行工具并取模型可见文本（含可恢复错误文本）。
async fn run(tool: &PortableDynamicTool, args: Value) -> String {
    match tool.execute(args).await {
        Ok(output) => output.as_text().unwrap_or("<非文本输出>").to_string(),
        Err(error) => error.model_feedback().unwrap_or("<无反馈>").to_string(),
    }
}

#[test]
fn tool_definition_preserves_name_and_schema() {
    let fixture = Fixture::new();
    let definition = fixture.tool().definition();
    assert_eq!(definition.name, "sync_directory");
    assert_eq!(
        definition.parameters["required"],
        json!(["ssh_profile", "source", "destination"])
    );
    assert!(definition.parameters["properties"].get("dry_run").is_some());
}

#[tokio::test]
async fn rejects_outside_workspace_before_review_or_connection() {
    let fixture = Fixture::new();
    let mut args = fixture.args();
    args["source"] = json!(std::env::temp_dir());
    let display = run(&fixture.tool(), args).await;
    assert!(display.contains("工作区之外"));
    assert!(display.contains("当前工作区："));
    assert!(display.contains(fixture.root.to_str().unwrap()));
    assert!(fixture.ssh_db.list_audit().unwrap().records.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn source_symlink_cannot_escape_workspace() {
    let fixture = Fixture::new();
    let link = fixture.root.join("outside");
    std::os::unix::fs::symlink(std::env::temp_dir(), &link).unwrap();
    let mut args = fixture.args();
    args["source"] = json!(link);
    let display = run(&fixture.tool(), args).await;
    assert!(display.contains("工作区之外"));
}

#[tokio::test]
async fn disabled_profile_is_not_executable() {
    let fixture = Fixture::new();
    let mut server = fixture
        .ssh_db
        .find_enabled_server("customer-beijing-01")
        .unwrap();
    server.enabled = false;
    fixture.ssh_db.save_servers(&[server]).unwrap();
    let display = run(&fixture.tool(), fixture.args()).await;
    assert!(display.starts_with("错误："), "{display}");
    assert!(fixture.ssh_db.list_audit().unwrap().records.is_empty());
}

#[tokio::test]
async fn session_workspace_and_shell_artifacts_are_valid_sync_sources() {
    let fixture = Fixture::new();
    let workspace = fixture
        .root
        .join(".jkcodingagent/plain-chat-browser/session-one");
    let artifacts = super::super::local_zsh::local_zsh_dir(&workspace)
        .unwrap()
        .join("release");
    std::fs::create_dir_all(&artifacts).unwrap();
    for path in [&workspace, &artifacts] {
        let mut args = fixture.args();
        args["source"] = json!(path);
        let tool = sync_directory_tool(
            fixture.manager.clone(),
            workspace.clone(),
            fixture.workspace_id.clone(),
            true,
            vec![],
            None,
            fixture.db.clone(),
            None,
            review_context_with_config(),
        );
        let display = run(&tool, args).await;
        // 必须通过路径校验到达连接阶段；连接 127.0.0.1 失败属预期（测试不连真实服务器）。
        assert!(
            !display.contains("工作区之外") && !display.contains("不能同步应用配置根目录"),
            "{display}"
        );
    }
    // 连接失败的执行同样写审计，且审计记录携带本次审查结论
    //（测试服务器显式关闭执行前审查 → 豁免放行）。
    let records = fixture.ssh_db.list_audit().unwrap().records;
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|record| {
        record
            .review
            .as_ref()
            .is_some_and(|review| review.allowed && review.reason.contains("显式关闭执行前审查"))
    }));
}

#[tokio::test]
async fn still_rejects_config_roots_ssh_secrets_and_git_metadata() {
    let fixture = Fixture::new();
    for suffix in [
        ".jkcodingagent",
        ".jkcodingagent/local_env",
        ".jkcodingagent/local_env/ssh",
        ".jkcodingagent/mcp.json",
        ".git",
        ".git/objects",
    ] {
        let source = fixture.root.join(suffix);
        std::fs::create_dir_all(&source).unwrap();
        let mut args = fixture.args();
        args["source"] = json!(source);
        let display = run(&fixture.tool(), args).await;
        assert!(display.starts_with("错误："), "{display}");
    }
    assert!(fixture.ssh_db.list_audit().unwrap().records.is_empty());
}
