use super::*;
use rig::message::ToolFunction;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// 在途执行的 drop 计数：上限到达后必须仍为 0（未丢弃）。
struct DropFlag(Arc<AtomicUsize>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn policy_with(
    tool_timeouts: ToolTimeoutDefaults,
) -> (AppToolExecutionPolicy, crate::test_util::TempDirGuard) {
    let dir = crate::test_util::TempDirGuard::new("rig-app-policy");
    let db = DispatcherDb::new(dir.path().join("jkbot.sqlite3")).expect("open temp db");
    let events = Channel::new(|_| Ok(()));
    let policy = AppToolExecutionPolicy::new(
        &db,
        &events,
        AppToolPolicyConfig {
            workspace_id: "workspace".into(),
            workspace: dir.path().to_path_buf(),
            filesystem_roots: Vec::new(),
            review: RigReviewContext::unconfigured(),
            cancel_rx: None,
            trace: Default::default(),
            tool_timeouts,
            app_handle: None,
        },
    );
    (policy, dir)
}

fn policy() -> (AppToolExecutionPolicy, crate::test_util::TempDirGuard) {
    policy_with(ToolTimeoutDefaults::default())
}

/// 带用户配置超时默认的策略（deadline 解析测试用）。
fn policy_with_timeouts(
    tool_timeouts: ToolTimeoutDefaults,
) -> (AppToolExecutionPolicy, crate::test_util::TempDirGuard) {
    policy_with(tool_timeouts)
}

/// 永不返回的桩工具：deadline 触发后不收敛，错误文案携带实际 deadline
/// 秒数（「执行超时（N秒）」），据此断言解析结果。
fn stuck_tool(name: &'static str) -> PortableDynamicTool {
    PortableDynamicTool::new(name, "卡死工具", json!({"type":"object"}), move |_| {
        Box::pin(
            async move { std::future::pending::<Result<ToolOutput, ToolExecutionError>>().await },
        )
    })
}

/// 白名单工具的调用声明超时优先于策略表默认：声明 40 秒的 generate_image
/// 必须在 40 秒（而非表默认 120 秒）触发统一超时。
#[tokio::test(start_paused = true)]
async fn declared_call_timeout_overrides_table_default() {
    let (policy, _dir) = policy();
    let tool = stuck_tool("generate_image");
    let call = ToolCall::from_wire(
        "call",
        ToolFunction {
            name: "generate_image".into(),
            arguments: json!({ "timeout_secs": 40 }),
        },
    );
    let task = tokio::spawn(async move { policy.execute(&tool, &call).await });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(40)).await;
    tokio::time::advance(SETTLE_CEILING + Duration::from_secs(1)).await;
    let error = task.await.expect("join").expect_err("必须返回错误");
    assert!(
        error.message().contains("执行超时（40秒）"),
        "声明 40 秒必须生效，实际错误：{}",
        error.message()
    );
}

/// 未声明时用户配置默认优先于表默认：配置 90 秒后 deadline 为 90 秒。
#[tokio::test(start_paused = true)]
async fn user_default_timeout_applies_without_declaration() {
    let (policy, _dir) = policy_with_timeouts(ToolTimeoutDefaults {
        generate_image: Some(90),
        ..Default::default()
    });
    let tool = stuck_tool("generate_image");
    let call = ToolCall::from_wire(
        "call",
        ToolFunction {
            name: "generate_image".into(),
            arguments: json!({}),
        },
    );
    let task = tokio::spawn(async move { policy.execute(&tool, &call).await });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(90)).await;
    tokio::time::advance(SETTLE_CEILING + Duration::from_secs(1)).await;
    let error = task.await.expect("join").expect_err("必须返回错误");
    assert!(
        error.message().contains("执行超时（90秒）"),
        "用户默认 90 秒必须生效，实际错误：{}",
        error.message()
    );
}

/// 上限只兜住真正卡死的工具：发出取消信号后仍不收敛时，不丢弃在途执行
/// （移交后台继续收敛），调用方拿到明确的「结算未确认」错误而不是无限等待。
#[tokio::test(start_paused = true)]
async fn settle_ceiling_hands_off_stuck_execution_without_dropping_it() {
    let (policy, _dir) = policy();
    let dropped = Arc::new(AtomicUsize::new(0));
    let tool_dropped = dropped.clone();
    // 忽略取消信号且永不返回：模拟取消后仍不收敛的工具。
    let tool = PortableDynamicTool::new(
        "read_file",
        "卡死工具",
        json!({"type":"object"}),
        move |_| {
            let tool_dropped = tool_dropped.clone();
            Box::pin(async move {
                let _guard = DropFlag(tool_dropped);
                std::future::pending::<Result<ToolOutput, ToolExecutionError>>().await
            })
        },
    );
    let call = ToolCall::from_wire(
        "call",
        ToolFunction {
            name: "read_file".into(),
            arguments: json!({}),
        },
    );
    let task = tokio::spawn(async move { policy.execute(&tool, &call).await });
    tokio::task::yield_now().await;
    // read_file 统一超时 30 秒 → 发取消信号 → 再等满 SETTLE_CEILING 仍不收敛。
    tokio::time::advance(Duration::from_secs(30)).await;
    tokio::time::advance(SETTLE_CEILING + Duration::from_secs(1)).await;
    let error = task.await.expect("join").expect_err("必须返回错误");
    assert_eq!(error.retryable(), Some(false));
    assert!(
        error.message().contains("结算未确认"),
        "错误应说明结算未确认：{}",
        error.message()
    );
    tokio::task::yield_now().await;
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        0,
        "上限到达不得丢弃仍在收敛的在途执行"
    );
}

/// 自管工具（unified_timeout=false）自带预算，但仍受兜底上限约束：
/// 到上限 → 按 `cancellable` 发取消 → 宽限收敛 → 仍不收则交接后台并按「结算未确认」收口。
#[tokio::test(start_paused = true)]
async fn self_managed_tool_hits_settle_ceiling_and_hands_off() {
    let (policy, _dir) = policy();
    let dropped = Arc::new(AtomicUsize::new(0));
    let tool_dropped = dropped.clone();
    // local_zsh：策略表里的自管工具（声明预算 60 秒、兜底上限 600 秒）。
    let tool = PortableDynamicTool::new(
        "local_zsh",
        "卡死的自管工具",
        json!({"type":"object"}),
        move |_| {
            let tool_dropped = tool_dropped.clone();
            Box::pin(async move {
                let _guard = DropFlag(tool_dropped);
                std::future::pending::<Result<ToolOutput, ToolExecutionError>>().await
            })
        },
    );
    let call = ToolCall::from_wire(
        "call",
        ToolFunction {
            name: "local_zsh".into(),
            arguments: json!({}),
        },
    );
    let task = tokio::spawn(async move { policy.execute(&tool, &call).await });
    tokio::task::yield_now().await;
    // 兜底上限 600 秒：未到点前不得被判失败。
    tokio::time::advance(Duration::from_secs(599)).await;
    assert!(!task.is_finished(), "自管工具在兜底上限前不应被判定失败");
    tokio::time::advance(Duration::from_secs(2) + SETTLE_CEILING).await;
    let error = task.await.expect("join").expect_err("必须返回错误");
    assert_eq!(error.retryable(), Some(false));
    assert!(
        error.message().contains("自管预算上限"),
        "错误应说明自管预算上限：{}",
        error.message()
    );
    tokio::task::yield_now().await;
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        0,
        "上限到达不得丢弃仍在收敛的在途执行"
    );
}

/// 调度器路径：enqueue 准入产出的 effective 参数随 `ToolInvocationContext`
/// 流入，execute 直接消费——`prepare_arguments` 不重算，且工具闭包收到的
/// 正是这份 effective 值。
#[tokio::test]
async fn execute_reuses_prepared_arguments_without_recompute() {
    use super::super::invocation::ToolInvocationContext;
    use crate::agent::rig_ext::tools::run_record::PREPARE_ARGUMENTS_CALLS;

    let (policy, _dir) = policy();
    let received = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));
    let captured = received.clone();
    let tool = PortableDynamicTool::new(
        "read_file",
        "捕获参数",
        json!({"type":"object"}),
        move |arguments| {
            let captured = captured.clone();
            Box::pin(async move {
                *captured.lock().unwrap() = Some(arguments);
                Ok(ToolOutput::text("ok"))
            })
        },
    );
    let call = ToolCall::from_wire(
        "call",
        ToolFunction {
            name: "read_file".into(),
            arguments: json!({ "paths": ["a.txt"] }),
        },
    );
    let prepared = json!({ "paths": ["a.txt"], "offset": 1 });
    let (_cancel, cancel_rx) = watch::channel(false);
    let context = ToolInvocationContext {
        workspace_id: "workspace".into(),
        agent_run_id: "run".into(),
        task_id: "task".into(),
        tool_call_id: "call".into(),
        root_request_message_id: "request".into(),
        cancel_rx,
        prepared_arguments: Some(prepared.clone()),
    };
    PREPARE_ARGUMENTS_CALLS.with(|count| count.set(0));
    context
        .scope(policy.execute(&tool, &call))
        .await
        .expect("执行成功");
    assert_eq!(
        PREPARE_ARGUMENTS_CALLS.with(std::cell::Cell::get),
        0,
        "execute 应复用 context 携带的 prepared 参数，不得重算"
    );
    assert_eq!(
        received.lock().unwrap().clone(),
        Some(prepared),
        "工具闭包收到的必须是 enqueue 产出的 effective 参数"
    );
}

/// 调度器路径：before_call 复用 enqueue 准入结果——校验门不再重查
/// （准入已用同一纯函数对同一不可变输入校验）。
#[tokio::test]
async fn before_call_reuses_prepared_arguments_without_recompute() {
    use super::super::invocation::ToolInvocationContext;
    use crate::agent::rig_ext::tools::run_record::PREPARE_ARGUMENTS_CALLS;

    let (policy, _dir) = policy();
    let tool = PortableDynamicTool::new(
        "read_file",
        "只读工具",
        json!({"type":"object"}),
        move |_| Box::pin(async move { Ok(ToolOutput::text("ok")) }),
    );
    let call = ToolCall::from_wire(
        "call",
        ToolFunction {
            name: "read_file".into(),
            arguments: json!({ "paths": ["a.txt"] }),
        },
    );
    let (_cancel, cancel_rx) = watch::channel(false);
    let context = ToolInvocationContext {
        workspace_id: "workspace".into(),
        agent_run_id: "run".into(),
        task_id: "task".into(),
        tool_call_id: "call".into(),
        root_request_message_id: "request".into(),
        cancel_rx,
        prepared_arguments: Some(json!({ "paths": ["a.txt"], "offset": 1 })),
    };
    PREPARE_ARGUMENTS_CALLS.with(|count| count.set(0));
    let guard = context.scope(policy.before_call(&tool, &call)).await;
    assert!(guard.rejection.is_none(), "已准入参数不应被门禁拒绝");
    assert_eq!(
        PREPARE_ARGUMENTS_CALLS.with(std::cell::Cell::get),
        0,
        "before_call 应复用 context 携带的 prepared 参数，不得重算"
    );
}

/// 无上下文调用：before_call 的校验门只做一次参数准备（原实现各算一次）。
#[tokio::test]
async fn bare_before_call_prepares_arguments_exactly_once() {
    use crate::agent::rig_ext::tools::run_record::PREPARE_ARGUMENTS_CALLS;

    let (policy, _dir) = policy();
    let tool = PortableDynamicTool::new(
        "read_file",
        "只读工具",
        json!({"type":"object"}),
        move |_| Box::pin(async move { Ok(ToolOutput::text("ok")) }),
    );
    let call = ToolCall::from_wire(
        "call",
        ToolFunction {
            name: "read_file".into(),
            arguments: json!({ "paths": ["a.txt"] }),
        },
    );
    PREPARE_ARGUMENTS_CALLS.with(|count| count.set(0));
    let guard = policy.before_call(&tool, &call).await;
    assert!(guard.rejection.is_none());
    assert_eq!(
        PREPARE_ARGUMENTS_CALLS.with(std::cell::Cell::get),
        1,
        "裸路径 before_call 的台账与校验门应共用一次参数准备"
    );
}
