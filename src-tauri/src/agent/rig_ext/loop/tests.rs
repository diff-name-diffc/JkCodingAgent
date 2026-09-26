//! 运行时循环端到端测试：rig 官方 mock 模型（`test-utils` feature）+ 真实
//! `DispatcherDb`（临时目录）+ 真实事件通道（`Channel::new`），全程无网络。
//!
//! 覆盖：流式增量 → `AgentEvent` 序列、工具调用配对（Planned/Started/Finished）、
//! 消息落库（assistant 工具调用 / 工具结果 / 收口正文）、用量落库。

mod policy_cancellation;

use std::sync::Arc;

use parking_lot::Mutex;
use rig::completion::{CompletionError, CompletionRequest, CompletionResponse};
use rig::streaming::{RawStreamingChoice, StreamingCompletionResponse};
use rig::test_utils::{MockCompletionModel, MockStreamEvent};
use rig::tool::{PortableDynamicTool, ToolOutput};

use super::classify_tool_error;
use super::protocol::{ProtocolToolHandler, RigProtocolAction, RigProtocolResult};
use super::surface::{DirectToolExecution, RigToolSurface};
use super::*;
use crate::agent::db::{DispatcherDb, DispatcherSessionTokenUsageSource};

/// 事件通道捕获：把每个事件的 `event` 标签与正文 delta 收集起来。
#[derive(Default)]
struct CapturedEvents {
    tags: Vec<String>,
    text_deltas: Vec<String>,
    finished_message_count: Option<usize>,
}

fn capture_channel(captured: Arc<Mutex<CapturedEvents>>) -> Channel<AgentEvent> {
    Channel::new(move |body| {
        let tauri::ipc::InvokeResponseBody::Json(json) = body else {
            return Ok(());
        };
        let value: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
        let tag = value
            .get("event")
            .and_then(|tag| tag.as_str())
            .unwrap_or_default()
            .to_string();
        if tag == "assistantDelta" {
            if let Some(delta) = value
                .get("data")
                .and_then(|data| data.get("delta"))
                .and_then(|delta| delta.as_str())
            {
                captured.lock().text_deltas.push(delta.to_string());
            }
        }
        if tag == "finished" {
            captured.lock().finished_message_count = value
                .get("data")
                .and_then(|data| data.get("messageCount"))
                .and_then(|count| count.as_u64())
                .map(|count| count as usize);
        }
        captured.lock().tags.push(tag);
        Ok(())
    })
}

/// 夹具：临时库 + 聊天会话 + 一个 `echo` 工具面。
struct Fixture {
    db: DispatcherDb,
    workspace_id: String,
    temp_dir: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp_dir = std::env::temp_dir().join(format!("rig-loop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).expect("create temp dir");
        let db = DispatcherDb::new(temp_dir.join("jkbot.sqlite3")).expect("open temp db");
        let session = db
            .create_chat_session("循环测试", None)
            .expect("create chat session");
        Self {
            db,
            workspace_id: session.id,
            temp_dir,
        }
    }

    fn surface() -> RigToolSurface {
        let echo = PortableDynamicTool::new(
            "echo",
            "回显参数",
            serde_json::json!({
                "type": "object",
                "properties": { "value": { "type": "string" } },
                "required": ["value"]
            }),
            |args| {
                Box::pin(async move {
                    let value = args
                        .get("value")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default();
                    Ok(ToolOutput::text(format!("echo:{value}")))
                })
            },
        );
        RigToolSurface::new(vec![echo])
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.temp_dir);
    }
}

#[tokio::test]
async fn loop_streams_deltas_executes_tool_and_persists_messages() {
    let fixture = Fixture::new();
    let captured = Arc::new(Mutex::new(CapturedEvents::default()));
    let on_event = capture_channel(Arc::clone(&captured));

    // 第一轮：正文 + 工具调用；第二轮：收口正文。两轮都带 usage。
    let model = MockCompletionModel::from_stream_turns([
        vec![
            MockStreamEvent::message_id("msg-1"),
            MockStreamEvent::text("先查一下"),
            MockStreamEvent::tool_call("call-1", "echo", serde_json::json!({"value": "hi"})),
            MockStreamEvent::final_response_with_total_tokens(12),
        ],
        vec![
            MockStreamEvent::message_id("msg-2"),
            MockStreamEvent::text("完成"),
            MockStreamEvent::final_response_with_total_tokens(8),
        ],
    ]);

    let surface = Fixture::surface();
    let mut hooks = RigLoopHooks::from_chat_spec(&crate::agent::rig_ext::model::PurposeModelSpec {
        api_key: "test".to_string(),
        api_base: "http://127.0.0.1:1/v1".to_string(),
        model: "mock-chat".to_string(),
        max_tokens: None,
        context_window: None,
        temperature: 0.0,
        enable_thinking: true,
    });
    hooks.max_iterations = 8;
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let mut usage_tracker = crate::agent::common::UsageTracker::new();

    let reply = run_rig_loop(
        &fixture.db,
        &fixture.workspace_id,
        &model,
        vec![rig::completion::Message::user("帮我回显 hi")],
        Vec::new(),
        &surface,
        &DirectToolExecution,
        None::<&RigSummaryModel<'_, MockCompletionModel>>,
        &mut hooks,
        &on_event,
        cancel_rx,
        &mut usage_tracker,
    )
    .await
    .expect("循环应收口成功");

    // 模型被调用两次（工具轮 + 收口轮）。
    assert_eq!(model.request_count(), 2);
    // 收口正文落库。
    assert_eq!(reply.plain_text().trim(), "完成");

    let events = captured.lock();
    let tags = &events.tags;
    for expected in [
        "assistantStarted",
        "assistantDelta",
        "toolPlanned",
        "toolStarted",
        "toolFinished",
        "assistantMessage",
        "finished",
    ] {
        assert!(
            tags.iter().any(|tag| tag == expected),
            "缺少事件 {expected}：{tags:?}"
        );
    }
    // 两轮流式正文增量都到达前端（顺序保持）。
    assert!(events.text_deltas.join("").contains("先查一下"));
    assert!(events.text_deltas.join("").contains("完成"));
    // Finished 携带可见消息计数（工具调用消息 + 工具结果 + 收口正文 = 3）。
    assert_eq!(events.finished_message_count, Some(3));
    drop(events);

    // 落库形状：assistant（含工具调用）→ tool 结果 → assistant 收口。
    let messages = list_visible(&fixture);
    let roles = messages
        .iter()
        .map(|message| message.role.as_str())
        .collect::<Vec<_>>();
    assert_eq!(roles, vec!["assistant", "tool", "assistant"]);
    let tool_message = messages
        .iter()
        .find(|message| message.role == "tool")
        .expect("工具结果消息");
    assert!(
        tool_message.plain_text().contains("echo:hi"),
        "工具结果应回显：{}",
        tool_message.plain_text()
    );
}

#[tokio::test]
async fn loop_rejects_unknown_tool_without_panicking() {
    let fixture = Fixture::new();
    let captured = Arc::new(Mutex::new(CapturedEvents::default()));
    let on_event = capture_channel(Arc::clone(&captured));

    // 第一轮请求不存在的工具（模型幻觉），第二轮收口。
    let model = MockCompletionModel::from_stream_turns([
        vec![
            MockStreamEvent::tool_call("call-9", "ghost_tool", serde_json::json!({})),
            MockStreamEvent::final_response_with_total_tokens(4),
        ],
        vec![
            MockStreamEvent::text("已收口"),
            MockStreamEvent::final_response_with_total_tokens(3),
        ],
    ]);
    let surface = Fixture::surface();
    let mut hooks = RigLoopHooks::from_chat_spec(&crate::agent::rig_ext::model::PurposeModelSpec {
        api_key: "test".to_string(),
        api_base: "http://127.0.0.1:1/v1".to_string(),
        model: "mock-chat".to_string(),
        max_tokens: None,
        context_window: None,
        temperature: 0.0,
        enable_thinking: true,
    });
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let mut usage_tracker = crate::agent::common::UsageTracker::new();

    let reply = run_rig_loop(
        &fixture.db,
        &fixture.workspace_id,
        &model,
        vec![rig::completion::Message::user("调用幽灵工具")],
        Vec::new(),
        &surface,
        &DirectToolExecution,
        None::<&RigSummaryModel<'_, MockCompletionModel>>,
        &mut hooks,
        &on_event,
        cancel_rx,
        &mut usage_tracker,
    )
    .await
    .expect("未注册工具应以可恢复错误回灌模型，而非中断 run");
    assert_eq!(reply.plain_text().trim(), "已收口");

    let messages = list_visible(&fixture);
    let tool_message = messages
        .iter()
        .find(|message| message.role == "tool")
        .expect("工具结果消息");
    assert!(
        tool_message.plain_text().contains("未注册的工具"),
        "未注册工具应回灌可读错误：{}",
        tool_message.plain_text()
    );
}

#[tokio::test]
async fn loop_records_token_usage_for_the_session() {
    let fixture = Fixture::new();
    let captured = Arc::new(Mutex::new(CapturedEvents::default()));
    let on_event = capture_channel(Arc::clone(&captured));

    let model = MockCompletionModel::from_stream_turns([vec![
        MockStreamEvent::text("用量测试"),
        MockStreamEvent::final_response(rig::completion::Usage {
            input_tokens: 5,
            output_tokens: 7,
            total_tokens: 12,
            ..Default::default()
        }),
    ]]);
    let surface = Fixture::surface();
    let mut hooks = RigLoopHooks::from_chat_spec(&crate::agent::rig_ext::model::PurposeModelSpec {
        api_key: "test".to_string(),
        api_base: "http://127.0.0.1:1/v1".to_string(),
        model: "mock-chat".to_string(),
        max_tokens: None,
        context_window: None,
        temperature: 0.0,
        enable_thinking: true,
    });
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let mut usage_tracker = crate::agent::common::UsageTracker::new();

    run_rig_loop(
        &fixture.db,
        &fixture.workspace_id,
        &model,
        vec![rig::completion::Message::user("统计用量")],
        Vec::new(),
        &surface,
        &DirectToolExecution,
        None::<&RigSummaryModel<'_, MockCompletionModel>>,
        &mut hooks,
        &on_event,
        cancel_rx,
        &mut usage_tracker,
    )
    .await
    .expect("单轮收口");

    // 用量落库是 fire-and-forget（tokio::spawn）：等待写库完成后再断言。
    let usage = wait_for_usage(&fixture).await;
    let record = usage.expect("应写入会话用量");
    assert_eq!(record.model, "mock-chat");
    assert_eq!(record.prompt_tokens, 5);
    assert_eq!(record.completion_tokens, 7);
    assert_eq!(record.total_tokens, 12);
    assert_eq!(
        record.source_kind,
        DispatcherSessionTokenUsageSource::Primary,
        "聊天路径用量来源应为 primary"
    );
}

/// 可见消息（同步读取，测试内直接调用）。
fn list_visible(fixture: &Fixture) -> Vec<crate::agent::db::DispatcherMessageRecord> {
    fixture
        .db
        .list_visible_messages(&fixture.workspace_id)
        .expect("列出会话消息")
}

/// 轮询等待用量落库（最多 ~2s），返回该模型的行。
async fn wait_for_usage(
    fixture: &Fixture,
) -> Option<crate::agent::db::DispatcherSessionTokenUsageRecord> {
    for _ in 0..40 {
        if let Ok(rows) = fixture.db.list_session_token_usage(&fixture.workspace_id) {
            if let Some(row) = rows
                .into_iter()
                .find(|row| row.model == "mock-chat" && row.prompt_tokens > 0)
            {
                return Some(row);
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    None
}

// ─── 错误分类（对齐旧 ToolStatus 词表） ─────────────────────────────────────

#[test]
fn cancelled_errors_map_to_cancelled_status() {
    let (status, kind, fatal) =
        classify_tool_error(&rig::tool::ToolExecutionError::cancelled("已取消"));
    assert_eq!((status, kind, fatal), ("cancelled", "cancelled", false));
}

#[test]
fn fatal_code_marks_the_run_abort() {
    let error = rig::tool::ToolExecutionError::other("子智能体执行失败").with_code("fatal");
    let (status, kind, fatal) = classify_tool_error(&error);
    assert_eq!((status, kind, fatal), ("fatal_error", "fatal_error", true));
}

#[test]
fn ordinary_failures_stay_recoverable() {
    let error = rig::tool::ToolExecutionError::refused("错误：被安全审查拦截");
    let (status, kind, fatal) = classify_tool_error(&error);
    assert_eq!(
        (status, kind, fatal),
        ("recoverable_error", "recoverable_error", false)
    );
}

// ─── 协议工具拦截（编排器收口路径） ───────────────────────────────────────

/// 桩：`finish_tool` 触发协议收口（带动作 + 最终答复）、`reject_tool` 返回
/// 可重试错误（不收口，由模型自修复）。
struct StubProtocolHandler;

#[async_trait::async_trait]
impl ProtocolToolHandler for StubProtocolHandler {
    fn handles(&self, name: &str) -> bool {
        matches!(name, "finish_tool" | "reject_tool")
    }
    async fn handle(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Option<RigProtocolResult> {
        match tool_name {
            "finish_tool" => Some(RigProtocolResult {
                text: "壳工具回显".to_string(),
                retryable_error: false,
                actions: vec![RigProtocolAction::GraphSubmitted {
                    title: "测试图".to_string(),
                    node_count: 2,
                }],
                final_message: arguments
                    .get("content")
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
            }),
            "reject_tool" => Some(RigProtocolResult::retryable_error(
                "错误：定义不合法（可重试）",
            )),
            _ => None,
        }
    }

    async fn render_outcome(
        &self,
        actions: &[RigProtocolAction],
        final_message: Option<&str>,
    ) -> Option<String> {
        let mut sections = Vec::new();
        for action in actions {
            match action {
                RigProtocolAction::GraphSubmitted { title, node_count } => sections.push(format!(
                    "🗺️ 执行图《{title}》已生成并通过校验（{node_count} 个节点）。"
                )),
            }
        }
        if let Some(message) = final_message {
            if sections.is_empty() {
                sections.push(message.to_string());
            } else {
                sections.push(format!("补充说明：\n{message}"));
            }
        }
        (!sections.is_empty()).then(|| sections.join("\n\n"))
    }
}

fn protocol_surface() -> RigToolSurface {
    RigToolSurface::new(vec![
        PortableDynamicTool::new(
            "finish_tool",
            "协议收口壳工具",
            serde_json::json!({"type": "object", "properties": {}}),
            |_args| Box::pin(async { Err(rig::tool::ToolExecutionError::refused("不应执行")) }),
        ),
        PortableDynamicTool::new(
            "reject_tool",
            "协议拒绝壳工具",
            serde_json::json!({"type": "object", "properties": {}}),
            |_args| Box::pin(async { Err(rig::tool::ToolExecutionError::refused("不应执行")) }),
        ),
    ])
}

fn hook_with_protocol() -> RigLoopHooks {
    let mut hooks = RigLoopHooks::from_chat_spec(&spec());
    hooks.protocol_handler = Some(Arc::new(StubProtocolHandler));
    hooks
}

fn spec() -> crate::agent::rig_ext::model::PurposeModelSpec {
    crate::agent::rig_ext::model::PurposeModelSpec {
        api_key: "test".to_string(),
        api_base: "http://127.0.0.1:1/v1".to_string(),
        model: "mock-chat".to_string(),
        max_tokens: None,
        context_window: None,
        temperature: 0.0,
        enable_thinking: true,
    }
}

#[tokio::test]
async fn protocol_action_closes_the_turn_with_a_synthesized_reply() {
    let fixture = Fixture::new();
    let captured = Arc::new(Mutex::new(CapturedEvents::default()));
    let on_event = capture_channel(Arc::clone(&captured));

    // 单轮：模型调用协议壳工具 + 最终答复；协议动作优先收口，不再请求模型。
    let model = MockCompletionModel::from_stream_turns([vec![
        MockStreamEvent::text("提交图"),
        MockStreamEvent::tool_call(
            "call-p1",
            "finish_tool",
            serde_json::json!({"content": "补充说明文本"}),
        ),
        MockStreamEvent::final_response_with_total_tokens(9),
    ]]);
    let surface = protocol_surface();
    let mut hooks = hook_with_protocol();
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let mut usage_tracker = crate::agent::common::UsageTracker::new();

    let reply = run_rig_loop(
        &fixture.db,
        &fixture.workspace_id,
        &model,
        vec![rig::completion::Message::user("出图")],
        Vec::new(),
        &surface,
        &DirectToolExecution,
        None::<&RigSummaryModel<'_, MockCompletionModel>>,
        &mut hooks,
        &on_event,
        cancel_rx,
        &mut usage_tracker,
    )
    .await
    .expect("协议动作应完成收口");

    // 只请求模型一次：协议动作直接收口。
    assert_eq!(model.request_count(), 1);
    let text = reply.plain_text();
    assert!(
        text.contains("执行图《测试图》已生成并通过校验（2 个节点）"),
        "{text}"
    );
    assert!(text.contains("补充说明：\n补充说明文本"), "{text}");
    assert!(captured.lock().tags.iter().any(|tag| tag == "finished"));
}

#[tokio::test]
async fn protocol_retryable_error_keeps_the_loop_running() {
    let fixture = Fixture::new();
    let captured = Arc::new(Mutex::new(CapturedEvents::default()));
    let on_event = capture_channel(Arc::clone(&captured));

    // 第一轮协议拒绝（可重试）→ 不收口；第二轮模型给出最终答复收口。
    let model = MockCompletionModel::from_stream_turns([
        vec![
            MockStreamEvent::tool_call("call-r1", "reject_tool", serde_json::json!({})),
            MockStreamEvent::final_response_with_total_tokens(4),
        ],
        vec![
            MockStreamEvent::text("已修正，直接答复用户"),
            MockStreamEvent::final_response_with_total_tokens(3),
        ],
    ]);
    let surface = protocol_surface();
    let mut hooks = hook_with_protocol();
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let mut usage_tracker = crate::agent::common::UsageTracker::new();

    let reply = run_rig_loop(
        &fixture.db,
        &fixture.workspace_id,
        &model,
        vec![rig::completion::Message::user("试错")],
        Vec::new(),
        &surface,
        &DirectToolExecution,
        None::<&RigSummaryModel<'_, MockCompletionModel>>,
        &mut hooks,
        &on_event,
        cancel_rx,
        &mut usage_tracker,
    )
    .await
    .expect("可重试错误后应继续循环并收口");

    assert_eq!(model.request_count(), 2, "可重试错误不得收口");
    assert_eq!(reply.plain_text().trim(), "已修正，直接答复用户");

    // 协议拒绝以「错误：」文本落库（模型据此自修复）。
    let tool_message = list_visible(&fixture)
        .into_iter()
        .find(|message| message.role == "tool")
        .expect("协议拒绝结果应落库");
    assert!(tool_message.plain_text().contains("定义不合法"));
}

/// 批量执行中途取消时，本批未执行的剩余调用必须补占位结果：
/// assistant 消息已连同全部 tool_calls 落库，缺结果会让下一轮请求被服务端
/// 以 400 拒绝（assistant tool_calls 之后必须跟齐 tool 消息）。
#[tokio::test]
async fn cancelled_batch_persists_placeholder_results_for_remaining_calls() {
    let fixture = Fixture::new();
    let captured = Arc::new(Mutex::new(CapturedEvents::default()));
    let on_event = capture_channel(Arc::clone(&captured));

    let model = MockCompletionModel::from_stream_turns([vec![
        MockStreamEvent::message_id("msg-1"),
        MockStreamEvent::text("并行跑两条"),
        MockStreamEvent::tool_call("call-1", "echo", serde_json::json!({"value": "hi"})),
        MockStreamEvent::tool_call("call-2", "echo", serde_json::json!({"value": "bye"})),
        MockStreamEvent::final_response_with_total_tokens(10),
    ]]);

    // 第一个工具执行即触发取消（模拟用户按下停止）。
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let surface = RigToolSurface::new(vec![PortableDynamicTool::new(
        "echo",
        "回显参数",
        serde_json::json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"]
        }),
        move |_args| {
            let cancel_tx = cancel_tx.clone();
            Box::pin(async move {
                let _ = cancel_tx.send(true);
                Ok(ToolOutput::text("echo:done"))
            })
        },
    )]);

    let mut hooks = RigLoopHooks::from_chat_spec(&crate::agent::rig_ext::model::PurposeModelSpec {
        api_key: "test".to_string(),
        api_base: "http://127.0.0.1:1/v1".to_string(),
        model: "mock-chat".to_string(),
        max_tokens: None,
        context_window: None,
        temperature: 0.0,
        enable_thinking: true,
    });
    hooks.max_iterations = 8;
    let mut usage_tracker = crate::agent::common::UsageTracker::new();

    run_rig_loop(
        &fixture.db,
        &fixture.workspace_id,
        &model,
        vec![rig::completion::Message::user("帮我回显")],
        Vec::new(),
        &surface,
        &DirectToolExecution,
        None::<&RigSummaryModel<'_, MockCompletionModel>>,
        &mut hooks,
        &on_event,
        cancel_rx,
        &mut usage_tracker,
    )
    .await
    .expect("取消应收口成功");

    // 模型只被调用一次：取消后不再发起带悬空 tool_calls 的请求。
    assert_eq!(model.request_count(), 1);

    let messages = list_visible(&fixture);
    let shape = messages
        .iter()
        .map(|message| {
            (
                message.role.as_str(),
                message.tool_call_id.as_deref().unwrap_or("-"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        shape,
        vec![
            ("assistant", "-"),
            ("tool", "call-1"),
            ("tool", "call-2"),
            // 取消收口的助手消息（收口正文随 cancelled 模板落库）。
            ("assistant", "-"),
        ]
    );
    assert!(messages[1].plain_text().contains("echo:done"));
    assert!(
        messages[2].plain_text().contains("尚未执行"),
        "未执行的调用应补占位结果：{}",
        messages[2].plain_text()
    );
}

#[tokio::test]
async fn slow_tool_runs_across_model_turns_and_runtime_delivers_once() {
    let fixture = Fixture::new();
    let captured = Arc::new(Mutex::new(CapturedEvents::default()));
    let channel = capture_channel(captured);
    let release = Arc::new(tokio::sync::Notify::new());
    let slow_gate = release.clone();
    let slow = PortableDynamicTool::new(
        "read_file",
        "slow read",
        serde_json::json!({"type":"object"}),
        move |_| {
            let gate = slow_gate.clone();
            Box::pin(async move {
                gate.notified().await;
                Ok(ToolOutput::text("A completed"))
            })
        },
    );
    let quick = PortableDynamicTool::new(
        "list_dir",
        "independent read",
        serde_json::json!({"type":"object"}),
        move |_| {
            let gate = release.clone();
            Box::pin(async move {
                gate.notify_one();
                Ok(ToolOutput::text("B completed"))
            })
        },
    );
    let surface = RigToolSurface::new(vec![slow, quick]);
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
            MockStreamEvent::text("都已完成"),
            MockStreamEvent::final_response_with_total_tokens(1),
        ],
    ]);
    let mut hooks = RigLoopHooks::from_chat_spec(&spec());
    let (_cancel, rx) = watch::channel(false);
    let mut usage = UsageTracker::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_rig_loop(
            &fixture.db,
            &fixture.workspace_id,
            &model,
            vec![Message::user("执行 A 和 B")],
            vec![],
            &surface,
            &DirectToolExecution,
            None::<&RigSummaryModel<'_, MockCompletionModel>>,
            &mut hooks,
            &channel,
            rx,
            &mut usage,
        ),
    )
    .await
    .expect("B 必须在 A 未完成时执行，不能串行死锁")
    .unwrap();
    assert_eq!(result.plain_text(), "都已完成");
    assert_eq!(model.request_count(), 3);
    let messages = list_visible(&fixture);
    let replies = messages
        .iter()
        .filter(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some("a"))
        .collect::<Vec<_>>();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].tool_result_mode.as_deref(), Some("accepted"));
    let observations = messages
        .iter()
        .filter(|m| m.role == "runtime")
        .collect::<Vec<_>>();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].tool_task_id, replies[0].tool_task_id);
    assert!(observations[0]
        .context_payload
        .as_ref()
        .unwrap()
        .contains("A completed"));
}

#[tokio::test]
async fn explicit_wait_is_event_driven_and_completion_causes_next_decision() {
    let fixture = Fixture::new();
    let waiting = Arc::new(tokio::sync::Notify::new());
    let observed_wait = waiting.clone();
    let channel = Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(json) = body {
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();
            if value["event"] == "runPhaseChanged" && value["data"]["phase"] == "waiting" {
                observed_wait.notify_one();
            }
        }
        Ok(())
    });
    let gate = Arc::new(tokio::sync::Notify::new());
    let worker_gate = gate.clone();
    let surface = RigToolSurface::new(vec![PortableDynamicTool::new(
        "read_file",
        "blocked read",
        serde_json::json!({"type":"object"}),
        move |_| {
            let gate = worker_gate.clone();
            Box::pin(async move {
                gate.notified().await;
                Ok(ToolOutput::text("ready"))
            })
        },
    )]);
    let model = MockCompletionModel::from_stream_turns([
        vec![
            MockStreamEvent::tool_call("a", "read_file", serde_json::json!({})),
            MockStreamEvent::final_response_with_total_tokens(1),
        ],
        vec![
            MockStreamEvent::tool_call("wait", "wait_for_tools", serde_json::json!({})),
            MockStreamEvent::final_response_with_total_tokens(1),
        ],
        vec![
            MockStreamEvent::text("观察结果后完成"),
            MockStreamEvent::final_response_with_total_tokens(1),
        ],
    ]);
    let (_cancel, rx) = watch::channel(false);
    let mut usage = UsageTracker::new();
    let mut hooks = RigLoopHooks::from_chat_spec(&spec());
    let run = run_rig_loop(
        &fixture.db,
        &fixture.workspace_id,
        &model,
        vec![Message::user("wait")],
        vec![],
        &surface,
        &DirectToolExecution,
        None::<&RigSummaryModel<'_, MockCompletionModel>>,
        &mut hooks,
        &channel,
        rx,
        &mut usage,
    );
    let signal = async {
        waiting.notified().await;
        assert_eq!(model.request_count(), 2, "等待时不能请求模型轮询");
        gate.notify_one();
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(run, signal)
    })
    .await
    .expect("完成事件应唤醒等待");
    assert_eq!(result.unwrap().plain_text(), "观察结果后完成");
    assert_eq!(model.request_count(), 3);
    let messages = list_visible(&fixture);
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.tool_call_id.as_deref() == Some("wait"))
            .count(),
        1
    );
    assert_eq!(messages.iter().filter(|m| m.role == "runtime").count(), 1);
}

// ─── 取消路径回归（R26/R27/R28） ────────────────────────────────────────────

/// 闸门流模型：流出一个正文增量后挂起（sender 由模型持有，流不自然终结），
/// 复现「流式输出中途取消」时 drive 与 consume_stream 的取消竞态。
#[derive(Default)]
struct PendingStreamModel {
    held:
        Mutex<Option<futures::channel::mpsc::Sender<Result<RawStreamingChoice, CompletionError>>>>,
}

impl CompletionModel for PendingStreamModel {
    async fn completion(
        &self,
        _request: CompletionRequest,
    ) -> Result<CompletionResponse, CompletionError> {
        Err(CompletionError::ProviderError(
            "PendingStreamModel 不支持非流式调用".into(),
        ))
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
    ) -> Result<StreamingCompletionResponse, CompletionError> {
        let (mut sender, receiver) = futures::channel::mpsc::channel(1);
        sender
            .try_send(Ok(RawStreamingChoice::Message("前半句".to_string())))
            .expect("预置首个正文增量");
        *self.held.lock() = Some(sender);
        Ok(StreamingCompletionResponse::stream(
            "mock",
            Box::pin(receiver),
        ))
    }
}

/// R26：流式输出中途取消——无论 drive 的取消分支还是 consume_stream 自带的
/// 取消检查赢掉竞态，已流出的部分正文都必须随取消收口落库，不落空 stub。
#[tokio::test]
async fn cancel_mid_stream_persists_partial_text() {
    let fixture = Fixture::new();
    let delta_seen = Arc::new(tokio::sync::Notify::new());
    let delta_watcher = delta_seen.clone();
    let channel = Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(json) = body {
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();
            if value["event"] == "assistantDelta" {
                delta_watcher.notify_one();
            }
        }
        Ok(())
    });
    let model = PendingStreamModel::default();
    let surface = Fixture::surface();
    let mut hooks = RigLoopHooks::from_chat_spec(&spec());
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let mut usage = UsageTracker::new();
    let run = run_rig_loop(
        &fixture.db,
        &fixture.workspace_id,
        &model,
        vec![Message::user("说点什么")],
        vec![],
        &surface,
        &DirectToolExecution,
        None::<&RigSummaryModel<'_, MockCompletionModel>>,
        &mut hooks,
        &channel,
        cancel_rx,
        &mut usage,
    );
    let signal = async {
        delta_seen.notified().await;
        let _ = cancel_tx.send(true);
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(run, signal)
    })
    .await
    .expect("取消收口不应挂起");
    let reply = result.expect("取消应收口成功");
    assert!(
        reply.plain_text().contains("前半句"),
        "已流出的部分正文应随取消落库：{}",
        reply.plain_text()
    );
    let messages = list_visible(&fixture);
    assert_eq!(messages.len(), 1, "取消收口只落一条消息");
    assert!(messages[0].plain_text().contains("前半句"));
}

/// R27：正式答复已落库+发事件后，等待未决工具期间收到取消——答复本身就是
/// 收口，外层取消路径不得再落一条「已停止」stub（幂等）。
#[tokio::test]
async fn cancel_during_wait_after_reply_keeps_the_persisted_reply() {
    let fixture = Fixture::new();
    let waiting = Arc::new(tokio::sync::Notify::new());
    let observed_wait = waiting.clone();
    let channel = Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(json) = body {
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();
            if value["event"] == "runPhaseChanged" && value["data"]["phase"] == "waiting" {
                observed_wait.notify_one();
            }
        }
        Ok(())
    });
    let gate = Arc::new(tokio::sync::Notify::new());
    let worker_gate = gate.clone();
    let surface = RigToolSurface::new(vec![PortableDynamicTool::new(
        "read_file",
        "slow read",
        serde_json::json!({"type":"object"}),
        move |_| {
            let gate = worker_gate.clone();
            Box::pin(async move {
                gate.notified().await;
                Ok(ToolOutput::text("迟到结果"))
            })
        },
    )]);
    // 第一轮：调用慢工具；第二轮：工具未决时模型直接给出正式答复。
    let model = MockCompletionModel::from_stream_turns([
        vec![
            MockStreamEvent::tool_call("slow", "read_file", serde_json::json!({})),
            MockStreamEvent::final_response_with_total_tokens(1),
        ],
        vec![
            MockStreamEvent::text("正式答复"),
            MockStreamEvent::final_response_with_total_tokens(1),
        ],
    ]);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let mut usage = UsageTracker::new();
    let mut hooks = RigLoopHooks::from_chat_spec(&spec());
    let run = run_rig_loop(
        &fixture.db,
        &fixture.workspace_id,
        &model,
        vec![Message::user("先跑工具再答复")],
        vec![],
        &surface,
        &DirectToolExecution,
        None::<&RigSummaryModel<'_, MockCompletionModel>>,
        &mut hooks,
        &channel,
        cancel_rx,
        &mut usage,
    );
    let signal = async {
        waiting.notified().await;
        let _ = cancel_tx.send(true);
        // 放行慢工具 worker，保证 shutdown 的 drain 能收尾。
        gate.notify_one();
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(run, signal)
    })
    .await
    .expect("取消收口不应挂起");
    let reply = result.expect("答复已持久化，取消应幂等收口");
    assert_eq!(reply.plain_text().trim(), "正式答复");
    assert_eq!(model.request_count(), 2);
    // assistant（工具调用）→ tool（accepted）→ assistant（正式答复）；无重复取消 stub。
    let messages = list_visible(&fixture);
    let roles = messages
        .iter()
        .map(|message| message.role.as_str())
        .collect::<Vec<_>>();
    assert_eq!(roles, vec!["assistant", "tool", "assistant"]);
    assert!(
        !messages
            .iter()
            .any(|message| message.plain_text().contains("本轮聊天已停止")),
        "答复已落库后不得再写「已停止」stub"
    );
}
