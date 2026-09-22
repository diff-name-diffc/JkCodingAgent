//! acp_exec 映射层与结算逻辑的单元测试。

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, PermissionOption, PermissionOptionKind, Plan, PlanEntry,
    PlanEntryPriority, PlanEntryStatus, SessionUpdate, TextContent, ToolCall, ToolCallLocation,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, UsageUpdate,
};
use serde_json::json;

use super::mapping::{
    decide_permission, redact, Mapper, MapperAction, PermissionDecision, MAX_NODE_OUTPUT_BYTES,
};
use super::settle_stop_reason;
use super::NodeExecOutcome;
use super::PermissionMode;
use super::StopReason;
use crate::agent::graph::types::{AgentActivity, GraphRunEvent};

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

fn events(actions: &[MapperAction]) -> Vec<&GraphRunEvent> {
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
        GraphRunEvent::NodePhaseChanged { phase, .. } if phase == "responding"
    ));
    assert!(matches!(
        &emitted[1],
        GraphRunEvent::NodeOutputDelta { delta, .. } if delta == "你好"
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
        .filter(|event| matches!(event, GraphRunEvent::NodeOutputDelta { .. }))
        .collect();
    assert_eq!(deltas.len(), 1, "超限后只发一次截断标记增量");
    let actions = mapper.feed(&SessionUpdate::AgentMessageChunk(text_chunk("again")));
    assert!(
        events(&actions)
            .into_iter()
            .all(|event| !matches!(event, GraphRunEvent::NodeOutputDelta { .. })),
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
        PlanEntry::new("调研代码", PlanEntryPriority::High, PlanEntryStatus::Completed),
        PlanEntry::new("实施改造", PlanEntryPriority::Medium, PlanEntryStatus::Pending),
    ]);
    let plan_activities = activities(mapper.feed(&SessionUpdate::Plan(plan)));
    assert_eq!(plan_activities.len(), 1);
    assert_eq!(plan_activities[0].kind, "plan");
    assert_eq!(plan_activities[0].title, "任务计划");
    let payload: serde_json::Value = serde_json::from_str(&plan_activities[0].payload_json).unwrap();
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
fn coding_permission_prefers_allow_once() {
    let options = vec![
        PermissionOption::new("always", "总是允许", PermissionOptionKind::AllowAlways),
        PermissionOption::new("once", "允许一次", PermissionOptionKind::AllowOnce),
    ];
    let decision = decide_permission(PermissionMode::AcceptEdits, &options, false);
    assert!(
        matches!(decision, PermissionDecision::Select(id) if id.0.as_ref() == "once"),
        "不放大为 allow_always，优先 allow_once"
    );
}

#[test]
fn coding_permission_cancels_when_only_allow_always() {
    // 无 allow_once 时绝不回退到选项列表第一项（可能是 allow_always）：
    // 自动应答的作用域仅限单次调用，宁可取消也不放大为常驻授权。
    let options = vec![PermissionOption::new(
        "always",
        "总是允许",
        PermissionOptionKind::AllowAlways,
    )];
    assert_eq!(
        decide_permission(PermissionMode::AcceptEdits, &options, false),
        PermissionDecision::Cancel
    );
}

#[test]
fn out_of_workspace_permission_is_rejected_in_both_modes() {
    // 路径越界：两种模式一律 reject_once，无 reject_once 则取消。
    let options = vec![
        PermissionOption::new("once", "允许一次", PermissionOptionKind::AllowOnce),
        PermissionOption::new("reject", "拒绝一次", PermissionOptionKind::RejectOnce),
    ];
    for mode in [PermissionMode::AcceptEdits, PermissionMode::Plan] {
        assert!(
            matches!(decide_permission(mode, &options, true), PermissionDecision::Select(id) if id.0.as_ref() == "reject"),
            "越界路径在 {mode:?} 下必须拒绝"
        );
    }
    let allow_only = vec![PermissionOption::new(
        "once",
        "允许一次",
        PermissionOptionKind::AllowOnce,
    )];
    assert_eq!(
        decide_permission(PermissionMode::AcceptEdits, &allow_only, true),
        PermissionDecision::Cancel
    );
}

#[test]
fn read_only_permission_selects_reject_once_or_cancels() {
    let options = vec![
        PermissionOption::new("allow", "允许一次", PermissionOptionKind::AllowOnce),
        PermissionOption::new("reject", "拒绝一次", PermissionOptionKind::RejectOnce),
    ];
    let decision = decide_permission(PermissionMode::Plan, &options, false);
    assert!(matches!(decision, PermissionDecision::Select(id) if id.0.as_ref() == "reject"));
    let allow_only = vec![PermissionOption::new(
        "allow",
        "允许一次",
        PermissionOptionKind::AllowOnce,
    )];
    assert_eq!(
        decide_permission(PermissionMode::Plan, &allow_only, false),
        PermissionDecision::Cancel
    );
    assert_eq!(
        decide_permission(PermissionMode::Plan, &[], false),
        PermissionDecision::Cancel
    );
}

#[test]
fn stop_reason_settlement() {
    match settle_stop_reason(StopReason::EndTurn, "产出".into(), 2, vec!["a.rs".into()]) {
        NodeExecOutcome::Succeeded {
            output,
            affected_files,
            tool_call_count,
            usage_json,
        } => {
            assert_eq!(output, "产出");
            assert_eq!(affected_files, vec!["a.rs"]);
            assert_eq!(tool_call_count, 2);
            assert_eq!(usage_json, "{}");
        }
        _ => panic!("end_turn 应结算为成功"),
    }
    match settle_stop_reason(StopReason::MaxTokens, "产出".into(), 0, vec![]) {
        NodeExecOutcome::Succeeded { output, .. } => {
            assert!(output.contains("产出可能不完整"), "max_tokens 成功但附注");
        }
        _ => panic!("max_tokens 应结算为成功（附注）"),
    }
    assert!(matches!(
        settle_stop_reason(StopReason::Refusal, String::new(), 0, vec![]),
        NodeExecOutcome::Failed { .. }
    ));
    assert!(matches!(
        settle_stop_reason(StopReason::Cancelled, String::new(), 0, vec![]),
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
