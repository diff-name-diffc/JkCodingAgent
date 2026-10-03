//! acp_exec 映射层与结算逻辑的单元测试。

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, PermissionOption, PermissionOptionKind, Plan, PlanEntry,
    PlanEntryPriority, PlanEntryStatus, PromptResponse, SessionUpdate, TextContent, ToolCall,
    ToolCallLocation, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, UsageUpdate,
};
use serde_json::json;

use super::client::extract_usage_json;
use super::mapping::{
    decide_static, redact, Mapper, MapperAction, PermissionDecision, MAX_NODE_OUTPUT_BYTES,
};
use super::settle_stop_reason;
use super::NodeExecOutcome;
use super::StopReason;
use crate::agent::workflow::types::{AgentActivity, WorkflowRunEvent};

fn mapper() -> Mapper {
    Mapper::new("r1", "n1", std::path::Path::new("/ws"))
}

fn text_chunk(text: &str) -> ContentChunk {
    ContentChunk::new(ContentBlock::Text(TextContent::new(text)))
}

fn activities(actions: Vec<MapperAction>) -> Vec<AgentActivity> {
    actions
        .into_iter()
        .filter_map(|action| match action {
            MapperAction::Activity(activity) => Some(activity),
            _ => None,
        })
        .collect()
}

fn events(actions: &[MapperAction]) -> Vec<&WorkflowRunEvent> {
    actions
        .iter()
        .filter_map(|action| match action {
            MapperAction::Emit(event) => Some(event),
            _ => None,
        })
        .collect()
}

#[test]
fn message_chunks_accumulate_output_and_emit_deltas() {
    let mut mapper = mapper();
    let actions = mapper.feed(&SessionUpdate::AgentMessageChunk(text_chunk("你好")));
    let emitted = events(&actions);
    assert_eq!(emitted.len(), 2, "首次输出应带阶段切换 + 输出增量两个事件");
    assert!(matches!(
        emitted[0],
        WorkflowRunEvent::NodePhaseChanged { phase, .. } if phase == "responding"
    ));
    assert!(matches!(
        &emitted[1],
        WorkflowRunEvent::NodeOutputDelta { delta, .. } if delta == "你好"
    ));
    mapper.feed(&SessionUpdate::AgentMessageChunk(text_chunk("，世界")));
    // 阶段不重复广播：第二次 chunk 只有增量事件。
    let actions = mapper.feed(&SessionUpdate::AgentMessageChunk(text_chunk("！")));
    assert_eq!(events(&actions).len(), 1);
    let (_, output, _, _) = mapper.finish();
    assert_eq!(output, "你好，世界！");
}

#[test]
fn node_output_is_capped_with_one_truncation_marker() {
    let mut mapper = mapper();
    let big = "x".repeat(MAX_NODE_OUTPUT_BYTES);
    mapper.feed(&SessionUpdate::AgentMessageChunk(text_chunk(&big)));
    // 超限后：不再累积、不再转发增量，只补记一次截断标记。
    let actions = mapper.feed(&SessionUpdate::AgentMessageChunk(text_chunk("more")));
    let deltas: Vec<_> = events(&actions)
        .into_iter()
        .filter(|event| matches!(event, WorkflowRunEvent::NodeOutputDelta { .. }))
        .collect();
    assert_eq!(deltas.len(), 1, "超限后只发一次截断标记增量");
    let actions = mapper.feed(&SessionUpdate::AgentMessageChunk(text_chunk("again")));
    assert!(
        events(&actions)
            .into_iter()
            .all(|event| !matches!(event, WorkflowRunEvent::NodeOutputDelta { .. })),
        "截断后不再转发任何输出增量"
    );
    let (_, output, _, _) = mapper.finish();
    assert!(output.contains("后续内容已丢弃"));
    assert!(!output.contains("more"));
    assert!(!output.contains("again"));
}

#[test]
fn thought_chunks_buffer_and_flush_as_thinking_activity() {
    let mut mapper = mapper();
    mapper.feed(&SessionUpdate::AgentThoughtChunk(text_chunk("先分析")));
    mapper.feed(&SessionUpdate::AgentThoughtChunk(text_chunk("再动手")));
    // 思考缓冲在非思考更新到来时落地。
    let actions = mapper.feed(&SessionUpdate::AgentMessageChunk(text_chunk("结论")));
    let thinking = activities(actions);
    assert_eq!(thinking.len(), 1);
    assert_eq!(thinking[0].kind, "thinking");
    assert_eq!(thinking[0].status, "finished");
    assert_eq!(thinking[0].content, "先分析再动手");
}

#[test]
fn finish_flushes_pending_thinking() {
    let mut mapper = mapper();
    mapper.feed(&SessionUpdate::AgentThoughtChunk(text_chunk("只有思考")));
    let (actions, _, _, _) = mapper.finish();
    let thinking = activities(actions);
    assert_eq!(thinking.len(), 1);
    assert_eq!(thinking[0].content, "只有思考");
}

#[test]
fn tool_call_maps_to_started_activity_with_locations() {
    let mut mapper = mapper();
    let tool_call = ToolCall::new("call-1", "读取文件")
        .status(ToolCallStatus::InProgress)
        .raw_input(json!({ "path": "src/a.rs" }))
        .locations(vec![
            ToolCallLocation::new("/ws/src/a.rs"),
            ToolCallLocation::new("/etc/passwd"),
        ]);
    let actions = mapper.feed(&SessionUpdate::ToolCall(tool_call));
    let tool_activities = activities(actions);
    assert_eq!(tool_activities.len(), 1);
    let activity = &tool_activities[0];
    assert_eq!(activity.kind, "tool_call");
    assert_eq!(activity.status, "started");
    assert_eq!(activity.title, "读取文件");
    assert_eq!(activity.id, "r1:n1:tool:call-1");
    let payload: serde_json::Value = serde_json::from_str(&activity.payload_json).unwrap();
    assert_eq!(payload["args"]["path"], "src/a.rs");
    assert_eq!(
        payload["locations"],
        json!(["src/a.rs", "[工作区外] /etc/passwd"]),
        "工作区外路径必须带标记保留在审计记录里"
    );
    let (_, _, tool_call_count, affected) = mapper.finish();
    assert_eq!(tool_call_count, 1);
    assert_eq!(affected, vec!["[工作区外] /etc/passwd", "src/a.rs"]);
}

#[test]
fn tool_call_update_reuses_activity_and_settles_terminal() {
    let mut mapper = mapper();
    let tool_call = ToolCall::new("call-1", "执行命令").status(ToolCallStatus::Pending);
    let first = activities(mapper.feed(&SessionUpdate::ToolCall(tool_call)));
    let update = ToolCallUpdate::new(
        "call-1",
        ToolCallUpdateFields::new()
            .status(ToolCallStatus::Completed)
            .raw_output(json!("ok\n")),
    );
    let second = activities(mapper.feed(&SessionUpdate::ToolCallUpdate(update)));
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].id, first[0].id, "同一调用复用同一 activity id");
    assert_eq!(
        second[0].sequence, first[0].sequence,
        "同一调用复用同一 sequence（DB upsert 键）"
    );
    assert_eq!(second[0].status, "finished");
    assert!(second[0].finished_at.is_some());
    assert_eq!(second[0].content, "ok\n");
    // 终态后的乱序更新被忽略，不再产出 activity。
    let late = ToolCallUpdate::new(
        "call-1",
        ToolCallUpdateFields::new().status(ToolCallStatus::InProgress),
    );
    assert!(activities(mapper.feed(&SessionUpdate::ToolCallUpdate(late))).is_empty());
    let (_, _, tool_call_count, _) = mapper.finish();
    assert_eq!(tool_call_count, 1, "update 不重复计数");
}

#[test]
fn unknown_tool_call_update_registers_new_call() {
    let mut mapper = mapper();
    let update = ToolCallUpdate::new(
        "call-9",
        ToolCallUpdateFields::new()
            .title("写入文件")
            .status(ToolCallStatus::Completed),
    );
    let tool_activities = activities(mapper.feed(&SessionUpdate::ToolCallUpdate(update)));
    assert_eq!(tool_activities.len(), 1);
    assert_eq!(tool_activities[0].status, "finished");
    let (_, _, tool_call_count, _) = mapper.finish();
    assert_eq!(tool_call_count, 1);
}

#[test]
fn plan_maps_to_plan_activity() {
    let mut mapper = mapper();
    let plan = Plan::new(vec![
        PlanEntry::new(
            "调研代码",
            PlanEntryPriority::High,
            PlanEntryStatus::Completed,
        ),
        PlanEntry::new(
            "实施改造",
            PlanEntryPriority::Medium,
            PlanEntryStatus::Pending,
        ),
    ]);
    let plan_activities = activities(mapper.feed(&SessionUpdate::Plan(plan)));
    assert_eq!(plan_activities.len(), 1);
    assert_eq!(plan_activities[0].kind, "plan");
    assert_eq!(plan_activities[0].title, "任务计划");
    let payload: serde_json::Value =
        serde_json::from_str(&plan_activities[0].payload_json).unwrap();
    assert_eq!(payload["entries"].as_array().unwrap().len(), 2);
    assert!(plan_activities[0].content.contains("调研代码"));
}

#[test]
fn usage_maps_to_context_usage_with_frontend_keys() {
    let mut mapper = mapper();
    let usage = UsageUpdate::new(55_000, 128_000);
    let usage_activities = activities(mapper.feed(&SessionUpdate::UsageUpdate(usage)));
    assert_eq!(usage_activities.len(), 1);
    assert_eq!(usage_activities[0].kind, "context_usage");
    let payload: serde_json::Value =
        serde_json::from_str(&usage_activities[0].payload_json).unwrap();
    assert_eq!(payload["tokens"], 55_000);
    assert_eq!(payload["contextWindow"], 128_000);
    let percent = payload["percent"].as_f64().unwrap();
    assert!((percent - 42.97).abs() < 0.01);
}

#[test]
fn enter_plan_mode_is_always_allowed() {
    // 子智能体自发进入计划模式是允许的工作方式：allow_once 优先。
    let options = vec![
        PermissionOption::new(
            "always",
            "Yes, always plan",
            PermissionOptionKind::AllowAlways,
        ),
        PermissionOption::new(
            "once",
            "Yes, enter plan mode",
            PermissionOptionKind::AllowOnce,
        ),
        PermissionOption::new(
            "reject",
            "No, start implementing now",
            PermissionOptionKind::RejectOnce,
        ),
    ];
    let decision = decide_static("EnterPlanMode", &options, false).expect("快路径决策");
    assert!(
        matches!(decision, PermissionDecision::Select(id) if id.0.as_ref() == "once"),
        "EnterPlanMode 放行且不放大为 allow_always"
    );
    // 无一次性允许选项时取消，不回退到 allow_always。
    let allow_always_only = vec![PermissionOption::new(
        "always",
        "Yes, always plan",
        PermissionOptionKind::AllowAlways,
    )];
    assert_eq!(
        decide_static("EnterPlanMode", &allow_always_only, false),
        Some(PermissionDecision::Cancel)
    );
}

#[test]
fn out_of_workspace_permission_is_rejected_before_review() {
    // 路径越界是硬边界：先于审查 AI 静态拒绝；无 reject_once 则取消。
    let options = vec![
        PermissionOption::new("once", "允许一次", PermissionOptionKind::AllowOnce),
        PermissionOption::new("reject", "拒绝一次", PermissionOptionKind::RejectOnce),
    ];
    let decision = decide_static("Edit", &options, true).expect("越界走静态快路径，不进审查");
    assert!(
        matches!(decision, PermissionDecision::Select(id) if id.0.as_ref() == "reject"),
        "越界路径必须拒绝"
    );
    let allow_only = vec![PermissionOption::new(
        "once",
        "允许一次",
        PermissionOptionKind::AllowOnce,
    )];
    assert_eq!(
        decide_static("Edit", &allow_only, true),
        Some(PermissionDecision::Cancel)
    );
}

#[test]
fn ordinary_requests_defer_to_review() {
    // 常规工具请求（无论工具组）交给审查 AI 裁决：静态层返回 None。
    for tool in ["Edit", "Write", "Bash", "WebFetch", "mcp__server__tool"] {
        let options = vec![PermissionOption::new(
            "once",
            "允许一次",
            PermissionOptionKind::AllowOnce,
        )];
        assert_eq!(
            decide_static(tool, &options, false),
            None,
            "{tool} 应交给审查 AI"
        );
    }
}

/// claude-agent-acp 0.79.0 的 ExitPlanMode（"Ready to code?"）选项表。
fn plan_approval_options() -> Vec<PermissionOption> {
    vec![
        PermissionOption::new(
            "clear_auto",
            "Yes, clear context (73% used) and use auto mode",
            PermissionOptionKind::AllowAlways,
        ),
        PermissionOption::new(
            "auto",
            "Yes, and use auto mode",
            PermissionOptionKind::AllowAlways,
        ),
        PermissionOption::new(
            "accept_edits",
            "Yes, and auto-accept edits",
            PermissionOptionKind::AllowAlways,
        ),
        PermissionOption::new(
            "manual",
            "Yes, manually approve edits",
            PermissionOptionKind::AllowOnce,
        ),
        PermissionOption::new(
            "reject",
            "No, keep planning",
            PermissionOptionKind::RejectOnce,
        ),
    ]
}

#[test]
fn plan_approval_prefers_elevated_option_and_skips_clear_context() {
    let decision = decide_static("ExitPlanMode", &plan_approval_options(), false)
        .expect("ExitPlanMode 走静态快路径");
    // 0.79.0 的选项表没有 bypass 变体（elevated 档恒为 auto）：选任意
    // allow_always 批准载体，目标级别由 client.rs 批准后 set_mode 保证。
    assert!(
        matches!(decision, PermissionDecision::Select(id) if id.0.as_ref() == "auto"),
        "选中非 clear-context 的 elevated 批准选项"
    );

    // 未来 adapter 提供 bypass 变体时优先选它（无需跟进 set_mode）。
    let mut with_bypass = plan_approval_options();
    with_bypass.insert(
        0,
        PermissionOption::new(
            "bypass",
            "Yes, and bypass permissions",
            PermissionOptionKind::AllowAlways,
        ),
    );
    let decision = decide_static("ExitPlanMode", &with_bypass, false).unwrap();
    assert!(
        matches!(decision, PermissionDecision::Select(id) if id.0.as_ref() == "bypass"),
        "bypass 命名选项优先"
    );

    // 连 allow_always 升级项都没有时，退到「manually approve edits」（allow_once）。
    let options: Vec<_> = plan_approval_options()
        .into_iter()
        .filter(|option| {
            option.kind == PermissionOptionKind::AllowOnce
                || option.kind == PermissionOptionKind::RejectOnce
        })
        .collect();
    let decision = decide_static("ExitPlanMode", &options, false).unwrap();
    assert!(
        matches!(decision, PermissionDecision::Select(id) if id.0.as_ref() == "manual"),
        "无升级选项时批准为逐次审批模式"
    );

    // 完全无可选项时取消。
    assert_eq!(
        decide_static("ExitPlanMode", &[], false),
        Some(PermissionDecision::Cancel)
    );
}

#[test]
fn stop_reason_settlement() {
    let usage = r#"{"prompt_tokens":120,"completion_tokens":30}"#;
    match settle_stop_reason(
        StopReason::EndTurn,
        "产出".into(),
        2,
        vec!["a.rs".into()],
        usage.into(),
    ) {
        NodeExecOutcome::Succeeded {
            output,
            affected_files,
            tool_call_count,
            usage_json,
        } => {
            assert_eq!(output, "产出");
            assert_eq!(affected_files, vec!["a.rs"]);
            assert_eq!(tool_call_count, 2);
            assert_eq!(usage_json, usage, "成功结算透传执行器上报的真实用量");
        }
        _ => panic!("end_turn 应结算为成功"),
    }
    match settle_stop_reason(StopReason::MaxTokens, "产出".into(), 0, vec![], "{}".into()) {
        NodeExecOutcome::Succeeded { output, .. } => {
            assert!(output.contains("产出可能不完整"), "max_tokens 成功但附注");
        }
        _ => panic!("max_tokens 应结算为成功（附注）"),
    }
    assert!(matches!(
        settle_stop_reason(StopReason::Refusal, String::new(), 0, vec![], "{}".into()),
        NodeExecOutcome::Failed { .. }
    ));
    assert!(matches!(
        settle_stop_reason(StopReason::Cancelled, String::new(), 0, vec![], "{}".into()),
        NodeExecOutcome::Cancelled
    ));
}

#[test]
fn recursively_redacts_secrets() {
    let value = json!({
        "apiKey": "one",
        "nested": { "refresh_token": "two", "safe": "visible" },
        "items": [{ "authorization": "Bearer three" }]
    });
    let redacted = redact(value);
    assert_eq!(redacted["apiKey"], "***");
    assert_eq!(redacted["nested"]["refresh_token"], "***");
    assert_eq!(redacted["nested"]["safe"], "visible");
    assert_eq!(redacted["items"][0]["authorization"], "***");
}

#[test]
fn redacts_secret_key_token_variants() {
    let value = json!({
        "X-Api-Key": "one",
        "client_secret": "two",
        "access_key": "three",
        "auth_token": "four",
        "db_password": "five",
        "nested": { "SIGNING_KEY": "six" },
        "name": "visible",
        "description": "also visible"
    });
    let redacted = redact(value);
    assert_eq!(redacted["X-Api-Key"], "***");
    assert_eq!(redacted["client_secret"], "***");
    assert_eq!(redacted["access_key"], "***");
    assert_eq!(redacted["auth_token"], "***");
    assert_eq!(redacted["db_password"], "***");
    assert_eq!(redacted["nested"]["SIGNING_KEY"], "***");
    assert_eq!(redacted["name"], "visible");
    assert_eq!(redacted["description"], "also visible");
}

#[test]
fn pinned_settings_meta_pins_tool_search_off() {
    // 路径必须与 claude-agent-acp 0.79.0 的读取点一致：
    // params._meta.claudeCode.options.settings → SDK programmatic settings
    // （env 优先级高于用户 settings.json）。
    let meta = super::client::pinned_settings_meta();
    assert_eq!(
        meta["claudeCode"]["options"]["settings"]["env"]["ENABLE_TOOL_SEARCH"],
        json!("false")
    );
}

#[test]
fn extract_usage_reads_quota_token_count_from_meta() {
    // claude-agent-acp 0.79.0 的 _meta.quota.token_count 形态（计费口径：
    // inputTokens 不含缓存，缓存读/写单列）。
    let mut meta = serde_json::Map::new();
    meta.insert(
        "quota".into(),
        json!({
            "token_count": {
                "totalTokens": 35_450,
                "inputTokens": 1_200,
                "cachedInputTokens": 30_000,
                "cachedWriteTokens": 4_000,
                "outputTokens": 250,
                "reasoningOutputTokens": 0,
            }
        }),
    );
    let response = PromptResponse::new(StopReason::EndTurn).meta(meta);
    let usage = extract_usage_json(&response);
    let parsed: serde_json::Value = serde_json::from_str(&usage).unwrap();
    // prompt_tokens 归一为输入侧合计（input + 缓存读 + 缓存写），
    // 与 OpenAI prompt_tokens 含缓存的口径一致，回执可直接聚合。
    assert_eq!(parsed["prompt_tokens"], 35_200);
    assert_eq!(parsed["completion_tokens"], 250);
    assert_eq!(parsed["cached_read_tokens"], 30_000);
    assert_eq!(parsed["cached_write_tokens"], 4_000);
}

#[test]
fn extract_usage_tolerates_missing_or_zero_quota() {
    // 自定义执行器不带 _meta.quota 时按未上报处理（"{}"）。
    let response = PromptResponse::new(StopReason::EndTurn);
    assert_eq!(extract_usage_json(&response), "{}");
    // 全零（响应经过 quota 通道但未实际产生计费）同样按未上报处理。
    let mut meta = serde_json::Map::new();
    meta.insert(
        "quota".into(),
        json!({ "token_count": { "inputTokens": 0, "outputTokens": 0 } }),
    );
    let response = PromptResponse::new(StopReason::EndTurn).meta(meta);
    assert_eq!(extract_usage_json(&response), "{}");
}
