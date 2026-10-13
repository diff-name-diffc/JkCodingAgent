use super::*;
use crate::agent::db::DispatcherDb;
use crate::agent::sub_agent::config::SubAgentModelConfig;

fn setup() -> (DispatcherDb, SubAgentDb, crate::test_util::TempDirGuard) {
    let dir = crate::test_util::TempDirGuard::new("aha-subagent-category");
    let dispatcher =
        DispatcherDb::new(dir.path().join("jkbot.sqlite3")).expect("create dispatcher db");
    let sub_agent = SubAgentDb::new(dispatcher.pool());
    (dispatcher, sub_agent, dir)
}

fn seed_sub_agent(sub_agent: &SubAgentDb, id: &str) {
    let config = SubAgentConfig {
        agent_id: id.to_string(),
        agent_name: id.to_string(),
        description: "test".to_string(),
        system_prompt: "test".to_string(),
        user_prompt_template: "{{task}}".to_string(),
        allowed_tools: vec!["notify_user_progress".to_string()],
        browser_use_vision: false,
        model_config: SubAgentModelConfig::default(),
        max_iterations: 1,
        max_output_tokens: 256,
        temperature: 0.7,
        timeout_secs: 10,
        enabled: true,
        created_at: 0,
        updated_at: 0,
    };
    sub_agent.create(&config).expect("seed sub_agent");
}

fn set_category_sub_agents(dispatcher: &DispatcherDb, category: &str, ids: &[&str]) {
    let conn = dispatcher.conn().expect("dispatcher conn");
    let json = serde_json::to_string(ids).unwrap();
    conn.execute(
        "UPDATE chat_category_agent_configs SET sub_agent_ids_json = ?1 WHERE category_id = ?2",
        params![json, category],
    )
    .expect("update category sub_agent_ids");
}

#[test]
fn loads_sub_agents_per_chat_category() {
    let (dispatcher, sub_agent, _dir) = setup();
    // schema 初始化会自动 seed browser-agent 到 global，这里清空 global 以隔离分类级配置。
    sub_agent.set_global_enabled(&[]).expect("clear global");
    seed_sub_agent(&sub_agent, "tech-agent");
    seed_sub_agent(&sub_agent, "life-agent");

    set_category_sub_agents(&dispatcher, "tech", &["tech-agent"]);
    set_category_sub_agents(&dispatcher, "life", &["life-agent"]);

    let tech_session = dispatcher
        .create_chat_session("技术", Some("tech"))
        .expect("create tech session");
    let life_session = dispatcher
        .create_chat_session("生活", Some("life"))
        .expect("create life session");

    let tech_ids = sub_agent
        .get_enabled_agent_ids(&tech_session.id)
        .expect("enabled for tech");
    assert_eq!(tech_ids, vec!["tech-agent".to_string()]);

    let life_ids = sub_agent
        .get_enabled_agent_ids(&life_session.id)
        .expect("enabled for life");
    assert_eq!(life_ids, vec!["life-agent".to_string()]);
}

#[test]
fn invalid_category_sub_agent_json_does_not_break_other_sessions() {
    let (dispatcher, sub_agent, _dir) = setup();
    sub_agent.set_global_enabled(&[]).expect("clear global");
    seed_sub_agent(&sub_agent, "tech-agent");
    set_category_sub_agents(&dispatcher, "tech", &["tech-agent"]);

    // 手工把另一个分类的 JSON 损坏成非法值（模拟空串/手改数据）。
    let conn = dispatcher.conn().expect("dispatcher conn");
    conn.execute(
        "UPDATE chat_category_agent_configs SET sub_agent_ids_json = 'not-json'
             WHERE category_id = 'life'",
        [],
    )
    .expect("corrupt life category json");

    let session = dispatcher
        .create_chat_session("技术", Some("tech"))
        .expect("create session");

    // 其它分类的坏 JSON 不得拖垮当前会话的子智能体加载。
    let ids = sub_agent
        .get_enabled_agent_ids(&session.id)
        .expect("enabled ids despite corrupted category elsewhere");
    assert_eq!(ids, vec!["tech-agent".to_string()]);

    // 坏分类自己的会话应降级为空列表而非报错。
    let broken_session = dispatcher
        .create_chat_session("生活", Some("life"))
        .expect("create broken category session");
    let broken_ids = sub_agent
        .get_enabled_agent_ids(&broken_session.id)
        .expect("enabled ids for corrupted category session");
    assert!(
        broken_ids.is_empty(),
        "corrupted category json must degrade to empty list"
    );
}

#[test]
fn empty_category_sub_agents_yields_only_global() {
    let (dispatcher, sub_agent, _dir) = setup();
    sub_agent.set_global_enabled(&[]).expect("clear global");
    seed_sub_agent(&sub_agent, "tech-agent");

    set_category_sub_agents(&dispatcher, "tech", &[]);

    let session = dispatcher
        .create_chat_session("技术", Some("tech"))
        .expect("create session");

    let ids = sub_agent
        .get_enabled_agent_ids(&session.id)
        .expect("enabled ids");
    assert!(ids.is_empty(), "no sub-agents for empty category config");
}

#[test]
fn global_sub_agents_apply_to_all_sessions() {
    let (dispatcher, sub_agent, _dir) = setup();
    seed_sub_agent(&sub_agent, "global-agent");
    // 显式设置 global（覆盖 schema 自动 seed 的 browser-agent），以隔离断言。
    sub_agent
        .set_global_enabled(&["global-agent".to_string()])
        .expect("set global");

    set_category_sub_agents(&dispatcher, "tech", &[]);

    let session = dispatcher
        .create_chat_session("技术", Some("tech"))
        .expect("create session");

    let ids = sub_agent
        .get_enabled_agent_ids(&session.id)
        .expect("enabled ids");
    assert_eq!(ids, vec!["global-agent".to_string()]);
}

#[test]
fn run_traces_are_isolated_and_removed_with_session_resources() {
    let (dispatcher, sub_agent, _dir) = setup();
    let session = dispatcher
        .create_chat_session("轨迹测试", Some("tech"))
        .expect("create trace session");

    sub_agent
        .save_run_trace(
            &session.id,
            "tool-call-1",
            "browser-agent",
            "completed",
            r#"[{"event":"Started","data":{"agentId":"browser-agent"}}]"#,
            Some("test-model-x"),
        )
        .expect("save first trace");
    sub_agent
        .save_run_trace(
            &session.id,
            "tool-call-2",
            "browser-agent",
            "failed",
            r#"[{"event":"Failed","data":{"agentId":"browser-agent"}}]"#,
            None,
        )
        .expect("save second trace");

    let first = sub_agent
        .get_run_trace(&session.id, "tool-call-1")
        .expect("get first trace")
        .expect("first trace exists");
    let second = sub_agent
        .get_run_trace(&session.id, "tool-call-2")
        .expect("get second trace")
        .expect("second trace exists");
    assert_eq!(first.status, "completed");
    assert_eq!(second.status, "failed");
    // model 列 roundtrip：Some 存读一致，None 落 NULL（老轨迹语义）。
    assert_eq!(first.model.as_deref(), Some("test-model-x"));
    assert_eq!(second.model, None);

    dispatcher
        .clear_messages(&session.id)
        .expect("clear session resources");
    assert!(sub_agent
        .get_run_trace(&session.id, "tool-call-1")
        .expect("get cleared trace")
        .is_none());

    sub_agent
        .save_run_trace(
            &session.id,
            "tool-call-3",
            "browser-agent",
            "completed",
            "[]",
            None,
        )
        .expect("save trace before delete");
    dispatcher
        .delete_session(&session.id)
        .expect("delete trace session");
    assert!(sub_agent
        .get_run_trace(&session.id, "tool-call-3")
        .expect("get deleted trace")
        .is_none());
}

#[test]
fn run_trace_rejects_invalid_json() {
    let (dispatcher, sub_agent, _dir) = setup();
    let session = dispatcher
        .create_chat_session("无效轨迹", Some("tech"))
        .expect("create session");
    let error = sub_agent
        .save_run_trace(
            &session.id,
            "tool-call-invalid",
            "browser-agent",
            "failed",
            "not-json",
            None,
        )
        .expect_err("invalid trace json must fail");
    assert!(error.to_string().contains("validate sub-agent trace"));
}

#[test]
fn interrupted_checkpoint_is_closed_once_without_touching_completed_runs() {
    let (dispatcher, db, _dir) = setup();
    let session = dispatcher
        .create_chat_session("恢复", Some("tech"))
        .unwrap();
    let events = serde_json::json!([
        {"event":"Started","data":{"agentId":"browser-agent","model":"test"}},
        {"event":"ToolStarted","data":{"agentId":"browser-agent","taskId":"action-1","toolName":"navigate","arguments":{"url":"https://example.com"}}}
    ]).to_string();
    db.save_run_trace(
        &session.id,
        "interrupted",
        "browser-agent",
        "running",
        &events,
        Some("test"),
    )
    .unwrap();
    db.save_run_trace(
        &session.id,
        "finished",
        "browser-agent",
        "completed",
        "[]",
        Some("test"),
    )
    .unwrap();
    db.recover_interrupted_runs().unwrap();
    let recovered = db
        .get_run_trace(&session.id, "interrupted")
        .unwrap()
        .unwrap();
    assert_eq!(recovered.status, "failed");
    let events: serde_json::Value = serde_json::from_str(&recovered.events_json).unwrap();
    assert_eq!(events[2]["event"], "ToolFinished");
    assert_eq!(events[2]["data"]["taskId"], "action-1");
    assert!(events[2]["data"]["error"]
        .as_str()
        .unwrap()
        .contains("中断"));
    assert_eq!(events[3]["event"], "Failed");
    db.recover_interrupted_runs().unwrap();
    assert_eq!(
        db.get_run_trace(&session.id, "interrupted")
            .unwrap()
            .unwrap()
            .events_json,
        recovered.events_json
    );
    assert_eq!(
        db.get_run_trace(&session.id, "finished")
            .unwrap()
            .unwrap()
            .status,
        "completed"
    );
}

#[test]
fn corrupted_interrupted_trace_is_closed_with_synthetic_events_without_blocking_recovery() {
    let (dispatcher, db, _dir) = setup();
    let session = dispatcher
        .create_chat_session("恢复坏行", Some("tech"))
        .unwrap();
    // 崩溃半写导致 events_json 非法：该行不能让整个恢复（进而应用启动）失败。
    dispatcher
        .conn()
        .unwrap()
        .execute(
            "INSERT INTO sub_agent_run_traces (
                workspace_id, tool_call_id, agent_id, status, events_json, model, created_at, updated_at
             ) VALUES (?1, 'corrupted', 'browser-agent', 'running', '{not json', NULL, 0, 0)",
            params![session.id],
        )
        .unwrap();
    db.save_run_trace(
        &session.id,
        "interrupted",
        "browser-agent",
        "running",
        r#"[{"event":"Started","data":{"agentId":"browser-agent","model":"test"}}]"#,
        Some("test"),
    )
    .unwrap();
    db.recover_interrupted_runs().unwrap();
    let corrupted = db.get_run_trace(&session.id, "corrupted").unwrap().unwrap();
    assert_eq!(corrupted.status, "failed");
    let events: serde_json::Value = serde_json::from_str(&corrupted.events_json).unwrap();
    assert_eq!(events.as_array().unwrap().len(), 1);
    assert_eq!(events[0]["event"], "Failed");
    assert!(events[0]["data"]["error"]
        .as_str()
        .unwrap()
        .contains("中断"));
    assert_eq!(
        db.get_run_trace(&session.id, "interrupted")
            .unwrap()
            .unwrap()
            .status,
        "failed"
    );
}
