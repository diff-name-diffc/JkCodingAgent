use super::*;

/// 夹具：临时库 + 最小工具依赖（只构造不执行）。
fn test_deps(temp_dir: &std::path::Path) -> RigToolDeps {
    let db =
        crate::agent::db::DispatcherDb::new(temp_dir.join("jkbot.sqlite3")).expect("open temp db");
    let mut deps = RigToolDeps {
        workspace_id: "sub-agent-test".to_string(),
        workspace: temp_dir.to_path_buf(),
        mcp_scope: crate::mcp::McpScope::Global,
        exec_timeout_secs: 30,
        restrict_to_workspace: true,
        extra_allowed_dirs: Vec::new(),
        app_handle: None,
        db: db.clone(),
        ssh_manager: crate::ssh_tool::SshSessionManager::new(db.pool()),
        mcp_registry: crate::mcp::McpRegistry::new(db),
        sub_agent_manager: None,
        cancel_rx: None,
        vision_spec: None,
        image: crate::agent::rig_ext::tools::deps::ImageToolConfig {
            url: String::new(),
            api_key: String::new(),
            model: String::new(),
            edit_model: String::new(),
        },
        review: crate::agent::rig_ext::review::RigReviewContext::unconfigured(),
    };
    deps.review.executor_task = None;
    deps
}

fn config(allowed_tools: Vec<&str>) -> crate::agent::sub_agent::config::SubAgentConfig {
    crate::agent::sub_agent::config::SubAgentConfig {
        agent_id: "a1".to_string(),
        agent_name: "测试子智能体".to_string(),
        description: "测试".to_string(),
        system_prompt: "你是测试子智能体。".to_string(),
        user_prompt_template: "任务：{{task}}".to_string(),
        allowed_tools: allowed_tools.into_iter().map(str::to_string).collect(),
        model_config: Default::default(),
        max_iterations: 2,
        max_output_tokens: 128,
        temperature: 0.0,
        timeout_secs: 30,
        enabled: true,
        created_at: 0,
        updated_at: 0,
    }
}

#[test]
fn surface_offers_progress_tool_and_rejects_nested_sub_agents() {
    let temp_dir = std::env::temp_dir().join(format!("rig-sub-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");
    let deps = test_deps(&temp_dir);
    let parent_spec = PurposeModelSpec {
        api_key: "test".to_string(),
        api_base: "http://127.0.0.1:1/v1".to_string(),
        model: "mock".to_string(),
        max_tokens: None,
        context_window: None,
        temperature: 0.0,
        enable_thinking: true,
    };
    let cfg = config(vec!["local_zsh", "notify_user_progress"]);
    let runtime = RigSubAgentRuntime::build(&RigSubAgentRequest {
        config: &cfg,
        parent_spec: &parent_spec,
        deps: &deps,
        task: "跑一下",
        parent_tool_call_id: "call-1",
        app_handle: None,
        session_id: "ws-1",
        cancel_rx: None,
    })
    .expect("构建应成功");
    let mut names = runtime
        .surface
        .iter()
        .map(|tool| tool.name().to_string())
        .collect::<Vec<_>>();
    names.sort();
    // 允许列表精确生效：只有显式列出的两个工具。
    assert_eq!(names, vec!["local_zsh", "notify_user_progress"]);

    // 嵌套子智能体工具一律拒绝（防递归派生）。
    let nested = config(vec!["call_sub_agent"]);
    let error = RigSubAgentRuntime::build(&RigSubAgentRequest {
        config: &nested,
        parent_spec: &parent_spec,
        deps: &deps,
        task: "跑一下",
        parent_tool_call_id: "call-1",
        app_handle: None,
        session_id: "ws-1",
        cancel_rx: None,
    })
    .err()
    .expect("嵌套子智能体工具必须被拒绝");
    assert!(error.contains("不允许递归调用子智能体工具"), "{error}");

    // 不可用工具名（编排器专属）在构建期报错，而不是运行期静默缺失。
    let unavailable = config(vec!["submit_graph"]);
    let error = RigSubAgentRuntime::build(&RigSubAgentRequest {
        config: &unavailable,
        parent_spec: &parent_spec,
        deps: &deps,
        task: "跑一下",
        parent_tool_call_id: "call-1",
        app_handle: None,
        session_id: "ws-1",
        cancel_rx: None,
    })
    .err()
    .expect("编排器工具对子智能体不可用");
    assert!(error.contains("不可用的工具"), "{error}");

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn truncation_keeps_head_and_tail() {
    let long = "a".repeat(SUB_AGENT_RESULT_MAX_CHARS + 100);
    let truncated = truncate_tool_result(&long);
    assert!(truncated.starts_with(&"a".repeat(64)));
    assert!(truncated.contains("已截断 100 字符"));
}

#[test]
fn tagged_thinking_is_split_into_reasoning() {
    let (visible, thinking) = split_tagged_thinking("前<think>推理</think>后");
    assert_eq!(visible, "前后");
    assert_eq!(thinking, "推理");
}

#[tokio::test]
async fn child_tools_cross_decision_rounds_without_writing_parent_messages() {
    use crate::agent::db::NewToolRun;
    use crate::agent::rig_ext::r#loop::{
        coordinator::Coordinator, host::LoopHost, invocation::ToolInvocationContext,
        scheduler::TaskScheduler,
    };
    use rig::{
        test_utils::{MockCompletionModel, MockStreamEvent},
        tool::ToolOutput,
    };
    let temp = std::env::temp_dir().join(format!("child-async-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp).unwrap();
    let mut deps = test_deps(&temp);
    let session = deps.db.create_chat_session("child async", None).unwrap();
    deps.workspace_id = session.id.clone();
    let parent = deps
        .db
        .create_tool_run(NewToolRun {
            workspace_id: session.id.clone(),
            tool_call_id: "parent-call".into(),
            tool_name: "call_sub_agent".into(),
            provider: "builtin".into(),
            category: "agent".into(),
            arguments_json: "{}".into(),
            effective_arguments_json: "{}".into(),
            metadata_json: "{}".into(),
        })
        .unwrap();
    let spec = PurposeModelSpec {
        api_key: "test".into(),
        api_base: "http://127.0.0.1:1/v1".into(),
        model: "mock".into(),
        max_tokens: None,
        context_window: None,
        temperature: 0.0,
        enable_thinking: false,
    };
    let mut cfg = config(vec!["notify_user_progress"]);
    cfg.max_iterations = 4;
    let mut runtime = RigSubAgentRuntime::build(&RigSubAgentRequest {
        config: &cfg,
        parent_spec: &spec,
        deps: &deps,
        task: "A then independent B",
        parent_tool_call_id: "parent-call",
        app_handle: None,
        session_id: &session.id,
        cancel_rx: None,
    })
    .unwrap();
    let gate = Arc::new(tokio::sync::Notify::new());
    let slow_gate = gate.clone();
    runtime.surface = vec![
        PortableDynamicTool::new(
            "read_file",
            "A",
            serde_json::json!({"type":"object"}),
            move |_| {
                let gate = slow_gate.clone();
                Box::pin(async move {
                    gate.notified().await;
                    Ok(ToolOutput::text("A done"))
                })
            },
        ),
        PortableDynamicTool::new(
            "list_dir",
            "B",
            serde_json::json!({"type":"object"}),
            move |_| {
                let gate = gate.clone();
                Box::pin(async move {
                    gate.notify_one();
                    Ok(ToolOutput::text("B done"))
                })
            },
        ),
    ];
    let model = MockCompletionModel::from_stream_turns([
        vec![
            MockStreamEvent::tool_call("a", "read_file", serde_json::json!({})),
            MockStreamEvent::final_response_with_total_tokens(1),
        ],
        vec![
            MockStreamEvent::tool_call("b", "list_dir", serde_json::json!({})),
            MockStreamEvent::final_response_with_total_tokens(1),
        ],
        vec![
            MockStreamEvent::text("child finished"),
            MockStreamEvent::final_response_with_total_tokens(1),
        ],
    ]);
    let (_cancel, rx) = watch::channel(false);
    let invocation = ToolInvocationContext {
        workspace_id: session.id.clone(),
        agent_run_id: "root".into(),
        task_id: parent.id,
        tool_call_id: "parent-call".into(),
        root_request_message_id: "anchor".into(),
        cancel_rx: rx.clone(),
    };
    invocation
        .scope(async {
            let mut coordinator = Coordinator::new(TaskScheduler::new(
                deps.db.clone(),
                session.id.clone(),
                rx,
                crate::agent::rig_ext::tool_result::prepare::raw_preparer(),
                runtime.loop_events.clone(),
            ));
            coordinator.host = LoopHost::Memory;
            let mut messages = vec![Message::system("test"), Message::user("run A and B")];
            let mut usage = SubAgentUsage::default();
            let mut forced = false;
            let mut iteration = 0;
            let start = Instant::now();
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                runtime.run_loop(
                    &model,
                    &mut messages,
                    &mut usage,
                    start,
                    start + Duration::from_secs(5),
                    &mut forced,
                    &mut iteration,
                    &mut coordinator,
                ),
            )
            .await
            .expect("子 Agent 不得串行等待 A")
            .unwrap();
            assert_eq!(result, "child finished");
            assert_eq!(model.request_count(), 3);
            assert!(!coordinator.unsettled());
            assert_eq!(deps.db.count_visible_messages(&session.id).unwrap(), 0);
            let encoded = serde_json::to_string(&messages).unwrap();
            assert!(encoded.contains("tool_completion"));
            assert!(deps
                .db
                .pending_tool_completions("root", &coordinator.tasks.scope_id, i64::MAX)
                .unwrap()
                .is_empty());
            coordinator.tasks.shutdown().await.unwrap();
        })
        .await;
}
