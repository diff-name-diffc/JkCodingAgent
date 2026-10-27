use super::*;
use crate::agent::rig_ext::chat_workspace::ChatWorkspace;
use crate::agent::rig_ext::tools::spec::{registered_tool_names, ToolSpec};
use serde_json::json;

const TOOL_NAMES: [&str; 6] = [
    "ssh_log_discover",
    "ssh_log_snapshot_update",
    "ssh_log_snapshot_status",
    "ssh_log_snapshot_cancel",
    "ssh_log_search",
    "ssh_log_read",
];

struct Fixture {
    deps: RigToolDeps,
    source: std::path::PathBuf,
    _guard: crate::test_util::TempDirGuard,
}

impl Fixture {
    fn new() -> Self {
        let guard = crate::test_util::TempDirGuard::new("log-tools");
        let session_root = guard.path().join("session");
        std::fs::create_dir_all(&session_root).unwrap();
        let source = session_root.join("app.log");
        std::fs::write(&source, "INFO boot\nERROR timeout trace=abc-123\n").unwrap();
        let db = crate::agent::db::DispatcherDb::new(guard.path().join("db.sqlite3")).unwrap();
        {
            let mut connection = db.pool().get().unwrap();
            let tx = connection.transaction().unwrap();
            crate::ssh_tool::db::ensure_ssh_tables_tx(&tx).unwrap();
            tx.commit().unwrap();
        }
        let chat_workspace = ChatWorkspace {
            session_root: session_root.clone(),
            project_roots: Vec::new(),
        };
        let deps = RigToolDeps {
            workspace_id: "log-tool-session".into(),
            workspace: session_root,
            chat_workspace: Some(chat_workspace),
            mcp_scope: crate::mcp::McpScope::Global,
            exec_timeout_secs: 30,
            restrict_to_workspace: true,
            extra_allowed_dirs: Vec::new(),
            app_handle: None,
            db: db.clone(),
            ssh_manager: crate::ssh_tool::SshSessionManager::new(db.pool()),
            log_snapshots: Some(LogSnapshotManager::new()),
            term_registry: crate::ssh_tool::term::TermSessionRegistry::new(),
            mcp_registry: crate::mcp::McpRegistry::new(db),
            sub_agent_manager: None,
            cancel_rx: None,
            vision_spec: None,
            image: super::super::deps::ImageToolConfig {
                url: String::new(),
                api_key: String::new(),
                model: String::new(),
                edit_model: String::new(),
            },
            tool_timeouts: Default::default(),
            review: crate::agent::rig_ext::review::RigReviewContext::unconfigured(),
        };
        Self {
            deps,
            source,
            _guard: guard,
        }
    }

    fn tools(&self) -> Vec<PortableDynamicTool> {
        log_tools(&self.deps)
    }

    fn source(&self) -> Value {
        json!({"type":"local_file","path":self.source})
    }
}

async fn run(tools: &[PortableDynamicTool], name: &str, args: Value) -> String {
    let tool = tools.iter().find(|tool| tool.name() == name).unwrap();
    match tool.execute(args).await {
        Ok(output) => output.as_text().unwrap_or_default().to_string(),
        Err(error) => error.model_feedback().unwrap_or_default().to_string(),
    }
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|error| panic!("invalid tool JSON: {error}\n{text}"))
}

#[test]
fn tools_and_policies_are_registered() {
    let fixture = Fixture::new();
    let tools = fixture.tools();
    let names = tools.iter().map(|tool| tool.name()).collect::<Vec<_>>();
    for name in TOOL_NAMES {
        assert!(names.contains(&name), "missing tool {name}");
        assert!(registered_tool_names().contains(&name));
        let spec = ToolSpec::new(name, "", json!({}));
        if matches!(name, "ssh_log_search" | "ssh_log_read") {
            assert!(spec.execution.parallelizable);
            assert!(spec.access.readonly);
        }
    }
    assert!(ToolSpec::new(TOOL_NAMES[0], "", json!({})).review_self_managed);
    assert!(ToolSpec::new(TOOL_NAMES[1], "", json!({})).review_self_managed);
}

#[tokio::test]
async fn local_file_works_through_real_agent_tools_and_repeated_update() {
    let fixture = Fixture::new();
    let tools = fixture.tools();
    let discovered = run(
        &tools,
        "ssh_log_discover",
        json!({"include_docker":false,"local_files":[fixture.source]}),
    )
    .await;
    assert!(discovered.contains("local_file"), "{discovered}");
    assert!(discovered.contains("size_bytes"), "{discovered}");

    let created = run(
        &tools,
        "ssh_log_snapshot_update",
        json!({
            "snapshot_id":"tool-e2e",
            "sources":[fixture.source()],
            "max_total_bytes":8388608u64,
            "timeout_secs":30
        }),
    )
    .await;
    assert_eq!(json(&created)["state"], "queued", "{created}");
    let status = json(
        &run(
            &tools,
            "ssh_log_snapshot_status",
            json!({"snapshot_id":"tool-e2e","wait_secs":30}),
        )
        .await,
    );
    assert_eq!(status["update"]["state"], "ready", "{status}");
    assert_eq!(status["current"]["generation"], 1);

    let found = json(
        &run(
            &tools,
            "ssh_log_search",
            json!({"snapshot_id":"tool-e2e","pattern":"c-123","context_before":1}),
        )
        .await,
    );
    assert_eq!(found["engine"], "fts5-trigram", "{found}");
    assert_eq!(found["matches"][0]["lineNumber"], 2);
    let read = run(
        &tools,
        "ssh_log_read",
        json!({"snapshot_id":"tool-e2e","file_id":1,"line_number":2,"before":1,"after":0}),
    )
    .await;
    assert!(read.contains("ERROR timeout"), "{read}");

    std::fs::write(&fixture.source, "INFO recovered\n").unwrap();
    run(
        &tools,
        "ssh_log_snapshot_update",
        json!({"snapshot_id":"tool-e2e","timeout_secs":30}),
    )
    .await;
    let second = json(
        &run(
            &tools,
            "ssh_log_snapshot_status",
            json!({"snapshot_id":"tool-e2e","wait_secs":30}),
        )
        .await,
    );
    assert_eq!(second["current"]["generation"], 2, "{second}");
    let old = json(
        &run(
            &tools,
            "ssh_log_search",
            json!({"snapshot_id":"tool-e2e","pattern":"timeout","engine":"scan"}),
        )
        .await,
    );
    assert_eq!(old["scanComplete"], true);
    assert_eq!(old["matches"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn remote_source_is_blocked_before_connection_without_review_config() {
    let fixture = Fixture::new();
    let tools = fixture.tools();
    run(
        &tools,
        "ssh_log_snapshot_update",
        json!({
            "snapshot_id":"remote-blocked",
            "sources":[{"type":"docker","server_id":"missing-server","container":"api"}],
            "timeout_secs":30
        }),
    )
    .await;
    let status = json(
        &run(
            &tools,
            "ssh_log_snapshot_status",
            json!({"snapshot_id":"remote-blocked","wait_secs":30}),
        )
        .await,
    );
    assert_eq!(status["update"]["state"], "failed", "{status}");
    assert!(status["update"]["error"]
        .as_str()
        .unwrap()
        .contains("未配置安全审查"));
    assert!(status["current"].is_null());
    let audit = fixture.deps.ssh_manager.load_audit_async().await.unwrap();
    assert!(audit.records.iter().any(|record| {
        record.server_id == "missing-server" && record.command.contains("docker logs")
    }));
}
