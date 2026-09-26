//! `executor.rs` 的单元测试。迁移自旧自实现工具层（已随迁移删除）的工具程序 executor_tests 模块：
//! mock 由旧 `ProgramBroker`/`CapabilityBroker` 改为构造 `PortableDynamicTool`
//! 注入 `DataPlane`。rig 工具回调只接收 arguments（无 step id），mock 按
//! `arguments.value` 键控回复；调用序列/审计 id 的断言随旧 Broker 接缝退役。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant as StdInstant};

use serde_json::{json, Value};
use tokio::sync::watch;

use rig::tool::{PortableDynamicTool, ToolErrorKind, ToolExecutionError, ToolOutput};

use super::super::error::ProgramErrorKind;
use super::super::validate::{validate_program_value, CapabilityPolicy, ProgramLimits};
use super::super::DataPlane;
use super::{execute_program, execute_program_inner};

#[derive(Clone)]
struct MockReply {
    delay: Duration,
    result: Result<ToolOutput, ToolExecutionError>,
}

/// 按 `arguments.value` 键控回复的 mock 数据面工具状态。
#[derive(Default)]
struct MockTool {
    replies: BTreeMap<String, MockReply>,
    invocations: Mutex<Vec<Value>>,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl MockTool {
    fn with_reply(
        self,
        value_key: &str,
        delay_ms: u64,
        result: Result<ToolOutput, ToolExecutionError>,
    ) -> Self {
        let mut this = self;
        this.replies.insert(
            value_key.to_string(),
            MockReply {
                delay: Duration::from_millis(delay_ms),
                result,
            },
        );
        this
    }

    fn invocations(&self) -> Vec<Value> {
        self.invocations.lock().unwrap().clone()
    }
}

/// 默认回复：原样回显 arguments（对齐旧 MockBroker 的 echo 语义）。
fn mock_tool(name: &str, mock: Arc<MockTool>) -> PortableDynamicTool {
    PortableDynamicTool::new(
        name.to_string(),
        "mock",
        json!({ "type": "object" }),
        move |args| {
            let mock = mock.clone();
            Box::pin(async move {
                mock.invocations.lock().unwrap().push(args.clone());
                let key = args
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let reply = mock.replies.get(&key).cloned().unwrap_or(MockReply {
                    delay: Duration::ZERO,
                    result: Ok(ToolOutput::json(args)),
                });
                let active = mock.active.fetch_add(1, Ordering::AcqRel) + 1;
                mock.max_active.fetch_max(active, Ordering::AcqRel);
                tokio::time::sleep(reply.delay).await;
                mock.active.fetch_sub(1, Ordering::AcqRel);
                reply.result
            })
        },
    )
}

fn data_plane(mock: Arc<MockTool>) -> DataPlane {
    DataPlane::new(vec![mock_tool("echo", mock)])
}

fn validate(program: Value, policy: CapabilityPolicy) -> super::super::validate::ValidatedProgram {
    validate_program_value(
        &program,
        &|name: &str| (name == "echo").then_some(policy),
        &ProgramLimits::default(),
    )
    .expect("program should validate")
}

fn sequential_program() -> Value {
    json!({
        "version": 1,
        "root": {
            "op": "sequence",
            "steps": [
                {
                    "op": "call",
                    "id": "first",
                    "tool": "echo",
                    "arguments": { "value": "hello" }
                },
                {
                    "op": "call",
                    "id": "second",
                    "tool": "echo",
                    "arguments": {
                        "value": { "$ref": { "step": "first", "pointer": "/data/value" } }
                    }
                },
                {
                    "op": "return",
                    "value": {
                        "value": { "$ref": { "step": "second", "pointer": "/data/value" } }
                    }
                }
            ]
        }
    })
}

fn parallel_program(step_ids: &[&str]) -> Value {
    let branches = step_ids
        .iter()
        .map(|step_id| {
            json!({
                "op": "call",
                "id": step_id,
                "tool": "echo",
                "arguments": { "value": step_id }
            })
        })
        .collect::<Vec<_>>();
    let returned = step_ids
        .iter()
        .map(|step_id| json!({ "$ref": { "step": step_id, "pointer": "/data/value" } }))
        .collect::<Vec<_>>();
    json!({
        "version": 1,
        "root": {
            "op": "sequence",
            "steps": [
                { "op": "parallel", "branches": branches },
                { "op": "return", "value": returned }
            ]
        }
    })
}

#[tokio::test]
async fn echo_chain_executes_and_merges_step_results() {
    let plane = data_plane(Arc::new(MockTool::default()));
    let program = validate(sequential_program(), CapabilityPolicy::sequential());

    let success = execute_program_inner(&program, &plane, &ProgramLimits::default(), None)
        .await
        .expect("program succeeds");

    assert_eq!(success.value, json!({ "value": "hello" }));
    assert_eq!(success.completed_steps, vec!["first", "second"]);

    // 外层入口：成功时模型可见输出为渲染后的 return 值文本。
    let output = execute_program(&program, &plane, &ProgramLimits::default(), None)
        .await
        .expect("program succeeds");
    assert_eq!(output.as_text(), Some("{\"value\":\"hello\"}"));
}

/// 回归：数据面工具只返回文本时（rig 迁移后 read_file / list_dir / glob /
/// grep 全部如此），`/data` 必须解析为这段文本，而不是 `null`。
///
/// 旧实现把 `data` 直接取自 `ToolResult::data`，文本工具缺该字段即回落
/// `null`：程序会「成功」返回 `{"dir":null,"gos":null}`——模型拿不到任何
/// 证据，只能重试。该 envelope 由 `success_envelope` 统一构造，此处从
/// 程序入口逐层验证（envelope → `$ref` 解析 → 模型可见输出）。
#[tokio::test]
async fn text_only_tool_results_resolve_through_data_pointer() {
    let mock = Arc::new(MockTool::default().with_reply(
        "listing",
        0,
        Ok(ToolOutput::text("src\n  main.rs\n  lib.rs")),
    ));
    let plane = data_plane(mock);
    let program = validate(
        json!({
            "version": 1,
            "root": { "op": "sequence", "steps": [
                { "op": "call", "id": "dir", "tool": "echo", "arguments": { "value": "listing" } },
                { "op": "return", "value": {
                    "dir": { "$ref": { "step": "dir", "pointer": "/data" } },
                    "text": { "$ref": { "step": "dir", "pointer": "/output" } }
                } }
            ] }
        }),
        CapabilityPolicy::sequential(),
    );

    let success = execute_program_inner(&program, &plane, &ProgramLimits::default(), None)
        .await
        .expect("program succeeds");
    assert_eq!(
        success.value,
        json!({ "dir": "src\n  main.rs\n  lib.rs", "text": "src\n  main.rs\n  lib.rs" })
    );

    let output = execute_program(&program, &plane, &ProgramLimits::default(), None)
        .await
        .expect("program succeeds");
    assert_eq!(
        output.as_text(),
        Some("{\"dir\":\"src\\n  main.rs\\n  lib.rs\",\"text\":\"src\\n  main.rs\\n  lib.rs\"}")
    );
}

/// `/data` 的 JSON 分支不受文本回落影响：工具返回 JSON 块时仍是结构本身。
#[tokio::test]
async fn json_tool_results_keep_their_structure_under_data_pointer() {
    let mock = Arc::new(MockTool::default().with_reply(
        "payload",
        0,
        Ok(ToolOutput::json(json!({ "files": ["a.rs"] }))),
    ));
    let plane = data_plane(mock);
    let program = validate(
        json!({
            "version": 1,
            "root": { "op": "sequence", "steps": [
                { "op": "call", "id": "find", "tool": "echo", "arguments": { "value": "payload" } },
                { "op": "return", "value": {
                    "files": { "$ref": { "step": "find", "pointer": "/data/files" } }
                } }
            ] }
        }),
        CapabilityPolicy::sequential(),
    );

    let success = execute_program_inner(&program, &plane, &ProgramLimits::default(), None)
        .await
        .expect("program succeeds");
    assert_eq!(success.value, json!({ "files": ["a.rs"] }));
}

#[tokio::test]
async fn parallel_calls_are_bounded_and_merged_in_declaration_order() {
    let mock = Arc::new(
        MockTool::default()
            .with_reply("a", 50, Ok(ToolOutput::json(json!({ "value": "a" }))))
            .with_reply("b", 5, Ok(ToolOutput::json(json!({ "value": "b" }))))
            .with_reply("c", 10, Ok(ToolOutput::json(json!({ "value": "c" })))),
    );
    let plane = data_plane(mock.clone());
    let program = validate(
        parallel_program(&["a", "b", "c"]),
        CapabilityPolicy::parallel_readonly(),
    );
    let limits = ProgramLimits {
        max_concurrency: 2,
        ..ProgramLimits::default()
    };

    let success = execute_program_inner(&program, &plane, &limits, None)
        .await
        .expect("program succeeds");

    assert_eq!(success.value, json!(["a", "b", "c"]));
    assert_eq!(success.completed_steps, vec!["a", "b", "c"]);
    assert_eq!(mock.max_active.load(Ordering::Acquire), 2);
    let invoked = mock
        .invocations()
        .into_iter()
        .map(|args| args["value"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(invoked, vec!["a", "b", "c"]);
}

#[tokio::test]
async fn parallel_failure_stops_unstarted_calls_and_awaits_in_flight_call() {
    let mock = Arc::new(
        MockTool::default()
            .with_reply("a", 80, Ok(ToolOutput::json(json!({ "value": "a" }))))
            .with_reply("b", 5, Err(ToolExecutionError::other("boom"))),
    );
    let plane = data_plane(mock.clone());
    let program = validate(
        parallel_program(&["a", "b", "c", "d"]),
        CapabilityPolicy::parallel_readonly(),
    );
    let limits = ProgramLimits {
        max_concurrency: 2,
        ..ProgramLimits::default()
    };
    let started_at = StdInstant::now();

    let error = execute_program_inner(&program, &plane, &limits, None)
        .await
        .expect_err("program fails");

    assert_eq!(error.kind, ProgramErrorKind::ChildRecoverable);
    assert!(started_at.elapsed() >= Duration::from_millis(70));
    let invoked = mock
        .invocations()
        .into_iter()
        .map(|args| args["value"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(invoked, vec!["a", "b"]);
    assert_eq!(
        error.completed_steps.into_vec(),
        vec!["a".to_string(), "b".to_string()]
    );
}

#[tokio::test]
async fn enforces_resolved_arguments_envelope_environment_and_return_budgets() {
    let large = "x".repeat(256);
    let program_value = json!({
        "version": 1,
        "root": { "op": "sequence", "steps": [
            { "op": "call", "id": "first", "tool": "echo", "arguments": {} },
            { "op": "call", "id": "second", "tool": "echo", "arguments": {
                "value": { "$ref": { "step": "first", "pointer": "/data/blob" } }
            } },
            { "op": "return", "value": {
                "$ref": { "step": "first", "pointer": "/data/blob" }
            } }
        ] }
    });
    let program = validate(program_value, CapabilityPolicy::sequential());
    let large_reply = || Ok(ToolOutput::json(json!({ "blob": large })));

    // 解析后参数预算：第一步产物经 $ref 注入第二步参数后超限。
    let mock = Arc::new(MockTool::default().with_reply("", 0, large_reply()));
    let limits = ProgramLimits {
        max_resolved_arguments_bytes: 128,
        ..ProgramLimits::default()
    };
    let error = execute_program_inner(&program, &data_plane(mock.clone()), &limits, None)
        .await
        .expect_err("arguments budget exceeded");
    assert_eq!(error.kind, ProgramErrorKind::LimitExceeded);
    assert_eq!(mock.invocations().len(), 1);

    // 步骤 envelope 预算：第一步结果 envelope 超限。
    let mock = Arc::new(MockTool::default().with_reply("", 0, large_reply()));
    let limits = ProgramLimits {
        max_step_envelope_bytes: 128,
        ..ProgramLimits::default()
    };
    let error = execute_program_inner(&program, &data_plane(mock.clone()), &limits, None)
        .await
        .expect_err("envelope budget exceeded");
    assert_eq!(error.kind, ProgramErrorKind::LimitExceeded);
    assert_eq!(mock.invocations().len(), 1);

    // 环境预算：第一步 envelope 入环境后超限。
    let mock = Arc::new(MockTool::default().with_reply("", 0, large_reply()));
    let limits = ProgramLimits {
        max_environment_bytes: 128,
        ..ProgramLimits::default()
    };
    let error = execute_program_inner(&program, &data_plane(mock.clone()), &limits, None)
        .await
        .expect_err("environment budget exceeded");
    assert_eq!(error.kind, ProgramErrorKind::LimitExceeded);
    assert_eq!(mock.invocations().len(), 1);

    // return 预算：放宽参数预算让第二步成功，最终由解析后的大 return 触发。
    let mock = Arc::new(MockTool::default().with_reply("", 0, large_reply()));
    let limits = ProgramLimits {
        max_return_bytes: 128,
        max_resolved_arguments_bytes: 1024,
        ..ProgramLimits::default()
    };
    let error = execute_program_inner(&program, &data_plane(mock.clone()), &limits, None)
        .await
        .expect_err("return budget exceeded");
    assert_eq!(error.kind, ProgramErrorKind::LimitExceeded);
    assert_eq!(mock.invocations().len(), 2);
}

#[tokio::test]
async fn wall_time_stops_new_work_and_bounds_in_flight_drain() {
    let mock = Arc::new(MockTool::default().with_reply(
        "hello",
        3_000,
        Ok(ToolOutput::json(json!({ "value": "late" }))),
    ));
    let plane = data_plane(mock.clone());
    let program = validate(sequential_program(), CapabilityPolicy::sequential());
    let limits = ProgramLimits {
        max_wall_time_secs: 1,
        max_drain_time_ms: 50,
        ..ProgramLimits::default()
    };
    let started_at = StdInstant::now();

    let error = execute_program_inner(&program, &plane, &limits, None)
        .await
        .expect_err("deadline exceeded");

    assert_eq!(error.kind, ProgramErrorKind::DeadlineExceeded);
    assert!(started_at.elapsed() >= Duration::from_millis(1_000));
    assert!(started_at.elapsed() < Duration::from_millis(1_500));
    assert_eq!(mock.invocations().len(), 1);

    // 外层入口映射为 rig Timeout 错误（分类 code 保留 deadline_exceeded）。
    let error = execute_program(&program, &plane, &limits, None)
        .await
        .expect_err("deadline exceeded");
    assert_eq!(error.kind(), ToolErrorKind::Timeout);
    assert_eq!(error.code(), Some("deadline_exceeded"));
}

#[tokio::test]
async fn maps_cancelled_refused_and_failed_child_results() {
    for (reply, expected_kind) in [
        (
            ToolExecutionError::cancelled("cancelled"),
            ProgramErrorKind::Cancelled,
        ),
        (
            ToolExecutionError::refused("denied"),
            ProgramErrorKind::ChildRecoverable,
        ),
        (
            ToolExecutionError::other("boom"),
            ProgramErrorKind::ChildRecoverable,
        ),
    ] {
        let mock = Arc::new(MockTool::default().with_reply("hello", 0, Err(reply)));
        let plane = data_plane(mock.clone());
        let program = validate(sequential_program(), CapabilityPolicy::sequential());

        let error = execute_program_inner(&program, &plane, &ProgramLimits::default(), None)
            .await
            .expect_err("child error aborts program");

        assert_eq!(error.kind, expected_kind);
        assert_eq!(mock.invocations().len(), 1);
    }

    // 子调用取消经外层入口映射为 rig Cancelled 错误。
    let mock = Arc::new(MockTool::default().with_reply(
        "hello",
        0,
        Err(ToolExecutionError::cancelled("cancelled")),
    ));
    let plane = data_plane(mock);
    let program = validate(sequential_program(), CapabilityPolicy::sequential());
    let error = execute_program(&program, &plane, &ProgramLimits::default(), None)
        .await
        .expect_err("cancelled child maps to cancelled");
    assert_eq!(error.kind(), ToolErrorKind::Cancelled);
    assert_eq!(error.code(), Some("cancelled"));
}

#[tokio::test]
async fn pre_cancelled_program_starts_no_calls() {
    let mock = Arc::new(MockTool::default());
    let plane = data_plane(mock.clone());
    let program = validate(sequential_program(), CapabilityPolicy::sequential());
    let (cancel_tx, cancel_rx) = watch::channel(true);
    drop(cancel_tx);

    let error = execute_program_inner(&program, &plane, &ProgramLimits::default(), Some(cancel_rx))
        .await
        .expect_err("pre-cancelled program aborts");

    assert_eq!(error.kind, ProgramErrorKind::Cancelled);
    assert!(mock.invocations().is_empty());
}

#[tokio::test]
async fn cancel_signal_stops_scheduling_and_drains_in_flight_call() {
    let mock = Arc::new(MockTool::default().with_reply(
        "hello",
        3_000,
        Ok(ToolOutput::json(json!({ "value": "late" }))),
    ));
    let plane = data_plane(mock.clone());
    let program = validate(sequential_program(), CapabilityPolicy::sequential());
    let limits = ProgramLimits {
        max_drain_time_ms: 50,
        ..ProgramLimits::default()
    };
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let started_at = StdInstant::now();

    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = cancel_tx.send(true);
    });
    let error = execute_program_inner(&program, &plane, &limits, Some(cancel_rx))
        .await
        .expect_err("cancelled program aborts");
    canceller.await.expect("cancellers joins");

    assert_eq!(error.kind, ProgramErrorKind::Cancelled);
    assert!(started_at.elapsed() >= Duration::from_millis(100));
    assert!(started_at.elapsed() < Duration::from_millis(1_000));
    assert_eq!(mock.invocations().len(), 1);

    // 外层入口映射为 rig Cancelled 错误。
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let _ = cancel_tx.send(true);
    let error = execute_program(&program, &plane, &limits, Some(cancel_rx))
        .await
        .expect_err("cancelled program aborts");
    assert_eq!(error.kind(), ToolErrorKind::Cancelled);
    assert_eq!(error.code(), Some("cancelled"));
}
