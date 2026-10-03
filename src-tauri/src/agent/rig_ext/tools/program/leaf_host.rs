//! 程序叶子的共享宿主：整个 ToolProgram 执行期只建一个 `TaskScheduler`，
//! 各叶子按自己的 task_id 取回自己的结算（R63）。
//!
//! 前端可见性（R64）：叶子台账的两条 `ToolRunUpdated`（started / 终态）走
//! `run_events`（当前 run 的真实事件通道），由前端按 `parentRunId` 挂进
//! `run_tool_program` 卡片。叶子的工具级事件（`ToolStarted` / `ToolFinished` /
//! 摘要）**仍然**走空接收端——它们带的是形如 `{父调用}:ptc:1` 的 wire call id，
//! 前端 `toolStarted` 分支会据此新建顶层工具卡片，正是要避免的噪声。
//!
//! 构造纪律：`LeafHost::new` 必须在工具回调的 task-local 作用域内调用。`TaskScheduler::new`
//! 用 `ToolInvocationContext::current()` 决定 run_id，而 `tokio::spawn` 不继承
//! task-local（见 `loop/invocation.rs` 的测试），放进 spawn 会登记到错误的 run。

use std::time::Duration;

use rig::{
    message::ToolCall,
    tool::{PortableDynamicTool, ToolExecutionError, ToolOutput},
};
use tauri::ipc::Channel;
use tokio::sync::Mutex;

use crate::agent::db::DispatcherDb;
use crate::agent::rig_ext::events::AgentEvent;
use crate::agent::rig_ext::r#loop::{
    invocation::ToolInvocationContext, scheduler::TaskScheduler, AppToolExecutionPolicy,
};
use crate::agent::rig_ext::tool_result::{prepare::raw_preparer, RigToolResultPolicy};
use crate::agent::rig_ext::tools::deps::RigToolDeps;

/// 单次等待窗口：每轮要么窗口走完、要么在窗口内吸收到自己（或兄弟）的结算，
/// 不做无锁自旋。
const SETTLE_WINDOW: Duration = Duration::from_millis(10);

/// 一次程序执行期共享的叶子宿主。
pub(super) struct LeafHost {
    /// 共享调度器（`enqueue`/`window` 都需要 `&mut`）。
    inner: Mutex<TaskScheduler>,
    db: DispatcherDb,
    /// 真实事件通道；`None` 时只登记台账，不发叶子 `ToolRunUpdated`。
    run_events: Option<Channel<AgentEvent>>,
}

impl LeafHost {
    pub(super) fn new(
        deps: &RigToolDeps,
        parent: &ToolInvocationContext,
        run_events: Option<Channel<AgentEvent>>,
    ) -> Self {
        // 叶子工具级事件的 sink（见模块注释）。
        let events = Channel::new(|_| Ok(()));
        let scheduler = TaskScheduler::new(
            deps.db.clone(),
            parent.workspace_id.clone(),
            parent.cancel_rx.clone(),
            raw_preparer(),
            events,
            run_events.clone(),
        );
        Self {
            inner: Mutex::new(scheduler),
            db: deps.db.clone(),
            run_events,
        }
    }

    /// 执行一个叶子并取回它自己的结算。
    ///
    /// `captured` 是 `wrapped` 捕获的原始结构化输出（成功时由调用方取用）。
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_leaf(
        &self,
        tool: &PortableDynamicTool,
        call: &ToolCall,
        policy: &AppToolExecutionPolicy,
        result_policy: &RigToolResultPolicy,
        sequence: u64,
        anchor: &str,
        captured: &std::sync::Arc<parking_lot::Mutex<Option<ToolOutput>>>,
    ) -> anyhow::Result<Result<ToolOutput, ToolExecutionError>> {
        let ids = {
            let mut scheduler = self.inner.lock().await;
            scheduler
                .enqueue(
                    std::slice::from_ref(call),
                    std::slice::from_ref(tool),
                    policy,
                    std::slice::from_ref(result_policy),
                    sequence,
                    anchor,
                )
                .await?
        };
        let task = ids
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("程序叶子未登记任何任务"))?;

        // 只等自己的那条：`ready` 与在途 job 都可能属于兄弟叶子。
        let completion = loop {
            let mut scheduler = self.inner.lock().await;
            scheduler
                .window(tokio::time::Instant::now() + SETTLE_WINDOW)
                .await?;
            let taken = scheduler.take_completion(&task);
            let idle = !scheduler.has_jobs();
            drop(scheduler);
            if let Some(completion) = taken {
                break completion;
            }
            // job 已空却仍没有自己的结算：登记与结算不一致，属异常。
            anyhow::ensure!(!idle, "程序叶子缺少结算事件");
        };

        let (run, scope) = {
            let scheduler = self.inner.lock().await;
            (scheduler.run_id.clone(), scheduler.scope_id.clone())
        };
        let (db, event_id) = (self.db.clone(), completion.event_id);
        tokio::task::spawn_blocking(move || {
            db.observe_internal_completions(&run, &scope, 0, &[event_id])
        })
        .await??;
        {
            let mut scheduler = self.inner.lock().await;
            scheduler.delivered(&task);
        }

        // 结算已写库，此刻取回终态行（status / duration_ms 已由 settle 填好）。
        let loaded = {
            let (db, task) = (self.db.clone(), task);
            tokio::task::spawn_blocking(move || db.load_tool_run(&task)).await??
        };
        if let Some(run_events) = &self.run_events {
            crate::agent::common::emit(
                run_events,
                AgentEvent::ToolRunUpdated {
                    run: Box::new(loaded),
                },
            );
        }

        leaf_result(completion, captured)
    }

    /// 程序结束时收一次尾（残留 job 收敛）。调用方不因失败改变程序结果。
    pub(super) async fn shutdown(&self) -> anyhow::Result<()> {
        let mut scheduler = self.inner.lock().await;
        scheduler.drain().await
    }
}

/// 结算 → 叶子返回值（语义与合并前的 `managed::execute_managed` 逐字一致：
/// 成功取捕获的原始结构化输出，失败按 status/fatal/error_kind/retryable 组合）。
fn leaf_result(
    completion: crate::agent::db::tool_completions::ToolCompletion,
    captured: &std::sync::Arc<parking_lot::Mutex<Option<ToolOutput>>>,
) -> anyhow::Result<Result<ToolOutput, ToolExecutionError>> {
    if completion.status == "succeeded" {
        let output = captured
            .lock()
            .take()
            .ok_or_else(|| anyhow::anyhow!("成功叶子缺少原始结构化输出"))?;
        return Ok(Ok(output));
    }
    let mut error = if completion.status == "cancelled" {
        ToolExecutionError::cancelled(completion.context_payload)
    } else {
        ToolExecutionError::other(completion.context_payload)
    };
    if completion.fatal {
        error = error.with_code("fatal");
    } else if let Some(kind) = completion.error_kind {
        error = error.with_code(kind);
    }
    Ok(Err(error.with_retryable(completion.retryable)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::db::{DispatcherToolRunRecord, NewToolRun};
    use crate::agent::rig_ext::r#loop::invocation::ToolInvocationContext;
    use crate::agent::rig_ext::review::RigReviewContext;
    use crate::agent::rig_ext::tools::deps::ImageToolConfig;
    use crate::agent::rig_ext::tools::program::{managed, DataPlane};
    use parking_lot::Mutex as ParkingMutex;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use uuid::Uuid;

    /// 夹具：临时目录 + 真实库 + 只构造不执行的最小工具依赖。
    fn test_deps(dir: &std::path::Path) -> RigToolDeps {
        let db =
            crate::agent::db::DispatcherDb::new(dir.join("jkbot.sqlite3")).expect("open temp db");
        RigToolDeps {
            workspace_id: "leaf-host-test".to_string(),
            workspace: dir.to_path_buf(),
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
            image: ImageToolConfig {
                url: String::new(),
                api_key: String::new(),
                model: String::new(),
                edit_model: String::new(),
            },
            review: RigReviewContext::unconfigured(),
        }
    }

    /// 名字取自策略表的只读安全工具，避免未配置审查时的 fail-closed 门禁。
    fn read_file_tool() -> PortableDynamicTool {
        PortableDynamicTool::new(
            "read_file",
            "读取文件（测试替身）",
            json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
            |args| {
                Box::pin(async move {
                    let path = args
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    Ok(ToolOutput::text(format!("contents of {path}")))
                })
            },
        )
    }

    fn parent_run(deps: &RigToolDeps) -> DispatcherToolRunRecord {
        deps.db
            .create_tool_run(NewToolRun {
                workspace_id: deps.workspace_id.clone(),
                tool_call_id: "program-call".to_string(),
                tool_name: "run_tool_program".to_string(),
                provider: "builtin".to_string(),
                category: "program".to_string(),
                arguments_json: "{}".to_string(),
                effective_arguments_json: "{}".to_string(),
                metadata_json: "{}".to_string(),
            })
            .expect("create parent program run")
    }

    /// 返回调用上下文与 run 级取消信号的发送端：发送端必须比叶子活得久
    /// （掉线会被调度器当作 run 取消，叶子直接以 cancelled 收场）。
    fn parent_context(
        deps: &RigToolDeps,
        parent: &DispatcherToolRunRecord,
    ) -> (ToolInvocationContext, tokio::sync::watch::Sender<bool>) {
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let context = ToolInvocationContext {
            workspace_id: deps.workspace_id.clone(),
            agent_run_id: parent.id.clone(),
            task_id: parent.id.clone(),
            tool_call_id: "program-call".to_string(),
            root_request_message_id: "anchor".to_string(),
            cancel_rx,
            prepared_arguments: None,
        };
        (context, cancel_tx)
    }

    fn read_file_program() -> Value {
        json!({
            "code": "const a = await tools.read_file({ path: \"a.txt\" }); const b = await tools.read_file({ path: \"b.txt\" }); return { a, b };",
            "description": "读取两个文件"
        })
    }

    /// `toolRunUpdated` 的 `data.run` 载荷按到达顺序收集（走真实通道的真实线格式）。
    fn capture_runs() -> (Channel<AgentEvent>, Arc<ParkingMutex<Vec<Value>>>) {
        let runs = Arc::new(ParkingMutex::new(Vec::new()));
        let sink = runs.clone();
        let events = Channel::new(move |body| {
            let tauri::ipc::InvokeResponseBody::Json(json) = body else {
                return Ok(());
            };
            let value: Value = serde_json::from_str(&json).unwrap_or_default();
            if value.get("event").and_then(Value::as_str) != Some("toolRunUpdated") {
                return Ok(());
            }
            if let Some(run) = value.get("data").and_then(|data| data.get("run")) {
                sink.lock().push(run.clone());
            }
            Ok(())
        });
        (events, runs)
    }

    /// R63 回归：同一程序的两个叶子共享一个调度器 —— 两次登记的行 `scope_id`
    /// 相同（scope id 是调度器级的），且各自登记成功、结算成功。
    #[tokio::test]
    async fn leaves_through_one_host_share_the_scheduler_scope() {
        let dir = std::env::temp_dir().join(format!("rig-leaf-host-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let deps = test_deps(&dir);
        let parent = parent_run(&deps);
        let plane = DataPlane::new(vec![read_file_tool()]).with_runtime(deps.clone());
        let tool = plane.get("read_file").expect("tool registered").clone();
        let (context, _cancel_tx) = parent_context(&deps, &parent);

        let (first, second) = context
            .scope(async {
                let host = LeafHost::new(
                    &deps,
                    &ToolInvocationContext::current().expect("上下文"),
                    None,
                );
                let first = managed::execute(
                    &plane,
                    Some(&host),
                    &tool,
                    "first",
                    1,
                    json!({ "path": "a.txt" }),
                )
                .await;
                let second = managed::execute(
                    &plane,
                    Some(&host),
                    &tool,
                    "second",
                    2,
                    json!({ "path": "b.txt" }),
                )
                .await;
                (first, second)
            })
            .await;

        assert_eq!(
            first.expect("第一个叶子成功").as_text(),
            Some("contents of a.txt")
        );
        assert_eq!(
            second.expect("第二个叶子成功").as_text(),
            Some("contents of b.txt")
        );

        let tree = deps
            .db
            .list_tool_run_tree(&deps.workspace_id, &parent.id)
            .expect("load program run tree");
        let mut leaves = tree
            .iter()
            .filter(|run| run.parent_run_id.as_deref() == Some(parent.id.as_str()))
            .collect::<Vec<_>>();
        leaves.sort_by_key(|run| run.sequence);
        assert_eq!(leaves.len(), 2);
        assert_eq!(leaves[0].sequence, 32);
        assert_eq!(leaves[1].sequence, 64);
        assert!(leaves.iter().all(|run| run.status == "succeeded"));
        assert_eq!(
            leaves[0].scope_id, leaves[1].scope_id,
            "共享调度器：两个叶子必须落在同一个 scope"
        );
        assert!(leaves[0].scope_id.is_some(), "受管路径必须登记 scope_id");
    }

    /// R64 回归：受管路径下两个叶子的生命周期事件发到真实通道，且带程序 run 的
    /// `parentRunId` 与 `tool_program` origin —— 前端据此把两条运行挂进
    /// `run_tool_program` 卡片的 `toolRuns`（不生成顶层卡片）。
    #[tokio::test]
    async fn program_leaves_publish_tool_run_updates_on_the_real_channel() {
        let dir = std::env::temp_dir().join(format!("rig-leaf-events-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let deps = test_deps(&dir);
        let parent = parent_run(&deps);
        let (events, captured) = capture_runs();
        let plane = DataPlane::new(vec![read_file_tool()])
            .with_runtime(deps.clone())
            .with_run_events(events);
        let tool = super::super::build_program_tool(None, plane);
        let (context, _cancel_tx) = parent_context(&deps, &parent);

        let output = context
            .scope(tool.execute(read_file_program()))
            .await
            .expect("受管程序执行成功");
        assert_eq!(
            output.as_text(),
            Some("{\"a\":\"contents of a.txt\",\"b\":\"contents of b.txt\"}")
        );

        let runs = captured.lock().clone();
        // 修复契约：程序启动前先把父 run 行广播一次（无 parentRunId），前端卡片
        // 由 toolStarted 只拿到 toolCallId，必须先经这条事件补上 runId，后续叶子的
        // parentRunId 才能挂进 run_tool_program 卡片——根事件必须先于全部叶子。
        assert_eq!(
            runs.len(),
            5,
            "父 run 行 1 条 + 两叶子各 started + 终态：{runs:?}"
        );
        assert_eq!(runs[0]["id"], json!(parent.id));
        assert_eq!(runs[0]["parentRunId"], Value::Null);
        assert_eq!(runs[0]["toolCallId"], json!("program-call"));
        let terminal = runs
            .iter()
            .filter(|run| run.get("finishedAt").is_some_and(Value::is_string))
            .collect::<Vec<_>>();
        assert_eq!(terminal.len(), 2, "两个叶子各发一条终态 ToolRunUpdated");
        for run in &terminal {
            assert_eq!(run["parentRunId"], json!(parent.id));
            assert_eq!(run["origin"], json!("tool_program"));
            assert_eq!(run["workspaceId"], json!(deps.workspace_id));
            assert_eq!(run["status"], json!("succeeded"));
            assert!(run["durationMs"].is_number(), "终态带耗时：{run}");
        }
        assert_eq!(terminal[0]["stepId"], json!("ptc:1"));
        assert_eq!(terminal[1]["stepId"], json!("ptc:2"));
        // 每个叶子另有一条 started：前端先落卡片再补终态。
        assert_eq!(
            runs.iter()
                .filter(|run| run["status"] == json!("running"))
                .count(),
            2
        );
    }
}
