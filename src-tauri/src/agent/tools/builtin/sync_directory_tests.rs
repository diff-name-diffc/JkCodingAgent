use super::*;
use crate::agent::tools::result::ToolStatus;
use crate::ssh_tool::SshDb;
use std::path::PathBuf;

struct Fixture {
    root: PathBuf,
    manager: SshSessionManager,
    db: SshDb,
    context: ToolContext,
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
        let db = SshDb::new(pool.clone());
        db.save_servers(&[serde_json::from_value(json!({
            "id":"customer-beijing-01", "host":"127.0.0.1", "username":"tester",
            "password":"never-disclose", "reviewEnabled":false
        }))
        .unwrap()])
            .unwrap();
        let context = ToolContext {
            workspace_id: uuid::Uuid::new_v4().to_string(),
            workspace: root.clone(),
            mcp_scope: crate::mcp::McpScope::Global,
            session_title: "sync test".into(),
            user_task: None,
            executor_task: None,
            review_conversation: None,
            ssh_review: None,
            exec_timeout_secs: 30,
            restrict_to_workspace: true,
            extra_allowed_dirs: vec![],
            app_handle: None,
            llm_provider: None,
            vision_model: String::new(),
            vision_provider: None,
            image_model_url: String::new(),
            image_model_api_key: String::new(),
            image_model: String::new(),
            image_edit_model: String::new(),
            sub_agent_tool_registry: None,
            current_sub_agent_id: None,
            current_sub_agent_name: None,
            current_tool_call_id: None,
            current_tool_spec_hash: None,
            cancel_rx: None,
            sub_agent_parent_tool_call_id: None,
            sub_agent_trace_events: None,
        };
        Self {
            root,
            manager: SshSessionManager::new(pool),
            db,
            context,
        }
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

#[tokio::test]
async fn missing_review_config_blocks_even_when_server_review_is_disabled_and_audits() {
    let fixture = Fixture::new();
    let result = sync_directory_tool(fixture.manager.clone())
        .execute(&fixture.args(), &fixture.context)
        .await;
    assert_eq!(result.status, ToolStatus::RecoverableError);
    assert!(result.display.contains("未配置安全审查模型"));
    assert!(!result.display.contains("never-disclose"));
    let audit = fixture.db.list_audit().unwrap();
    assert_eq!(audit.records.len(), 1);
    assert!(!audit.records[0].review.as_ref().unwrap().allowed);
    assert_eq!(audit.records[0].exit_code, None);
}

#[tokio::test]
async fn rejects_outside_workspace_before_review_or_connection() {
    let fixture = Fixture::new();
    let mut args = fixture.args();
    args["source"] = json!(std::env::temp_dir());
    let result = sync_directory_tool(fixture.manager.clone())
        .execute(&args, &fixture.context)
        .await;
    assert_eq!(result.status, ToolStatus::RecoverableError);
    assert!(result.display.contains("工作区之外"));
    assert!(result.display.contains("当前工作区："));
    assert!(result.display.contains(fixture.root.to_str().unwrap()));
    assert!(fixture.db.list_audit().unwrap().records.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn source_symlink_cannot_escape_workspace() {
    let fixture = Fixture::new();
    let link = fixture.root.join("outside");
    std::os::unix::fs::symlink(std::env::temp_dir(), &link).unwrap();
    let mut args = fixture.args();
    args["source"] = json!(link);
    let result = sync_directory_tool(fixture.manager.clone())
        .execute(&args, &fixture.context)
        .await;
    assert!(result.display.contains("工作区之外"));
}

#[tokio::test]
async fn disabled_profile_is_not_executable() {
    let fixture = Fixture::new();
    let mut server = fixture
        .db
        .find_enabled_server("customer-beijing-01")
        .unwrap();
    server.enabled = false;
    fixture.db.save_servers(&[server]).unwrap();
    let result = sync_directory_tool(fixture.manager.clone())
        .execute(&fixture.args(), &fixture.context)
        .await;
    assert_eq!(result.status, ToolStatus::RecoverableError);
    assert!(!result.display.contains("安全审查模型"));
    assert!(fixture.db.list_audit().unwrap().records.is_empty());
}

#[test]
fn registered_for_chat_with_self_managed_review() {
    let fixture = Fixture::new();
    let tools = super::super::plain_chat_tools(fixture.manager.clone());
    assert_eq!(
        tools
            .iter()
            .filter(|tool| tool.name() == "sync_directory")
            .count(),
        1
    );
}

#[tokio::test]
async fn configured_review_honors_explicit_server_exemption() {
    let mut fixture = Fixture::new();
    fixture.context.ssh_review = Some(serde_json::from_value(json!({})).unwrap());
    let server = fixture
        .db
        .find_enabled_server("customer-beijing-01")
        .unwrap();
    let verdict = review(&fixture.context, &fixture.args(), &server, "sync_directory").await;
    assert!(verdict.allowed);
    assert!(verdict.reason.contains("显式关闭"));
}

#[tokio::test]
async fn session_workspace_and_shell_artifacts_are_valid_sync_sources() {
    let mut fixture = Fixture::new();
    let workspace = fixture
        .root
        .join(".jkcodingagent/plain-chat-browser/session-one");
    let artifacts = crate::agent::tools::local_zsh_dir(&workspace)
        .unwrap()
        .join("release");
    std::fs::create_dir_all(&artifacts).unwrap();
    fixture.context.workspace = workspace.clone();
    for path in [workspace, artifacts] {
        let mut args = fixture.args();
        args["source"] = json!(path);
        let result = sync_directory_tool(fixture.manager.clone())
            .execute(&args, &fixture.context)
            .await;
        // 必须通过路径校验到达审查门禁；测试不配置审查，因而不连接真实服务器。
        assert!(
            result.display.contains("未配置安全审查模型"),
            "{}",
            result.display
        );
    }
    assert_eq!(fixture.db.list_audit().unwrap().records.len(), 2);
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
        let result = sync_directory_tool(fixture.manager.clone())
            .execute(&args, &fixture.context)
            .await;
        assert_eq!(result.status, ToolStatus::RecoverableError);
        assert!(
            !result.display.contains("安全审查模型"),
            "{}",
            result.display
        );
    }
    assert!(fixture.db.list_audit().unwrap().records.is_empty());
}

#[test]
fn runtime_prompt_preserves_system_and_exposes_current_scope() {
    use crate::agent::llm::ChatMessage;
    let fixture = Fixture::new();
    let mut messages = vec![ChatMessage::system("自定义角色".into())];
    crate::agent::prompt::runtime_workspace::append(&mut messages, &fixture.context, &[]).unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].content.starts_with("自定义角色"));
    assert!(messages[0]
        .content
        .contains(fixture.context.workspace.to_str().unwrap()));
    assert!(messages[0].content.contains("额外授权路径：无"));
}
