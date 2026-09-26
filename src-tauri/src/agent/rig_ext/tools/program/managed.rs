//! 程序叶子的受管执行：叶子以 `parent_run_id = 程序自身 run` 登记为内部工具运行，
//! 并保留原始 ToolOutput 供 IR 数据依赖使用。
//!
//! 两处刻意的取舍（审查记录 R63/R64）：
//! - 每个叶子各建一个 `TaskScheduler`。共享一个调度器需要给 `drain`/`ready` 加一层
//!   按 task_id 的结算路由（现有接口是「等全部 job 结束、ready 取任意一条」），
//!   收益只是省下每次叶子的调度器分配，风险却落在程序执行的关键路径上，故暂不合并。
//!   预算/租约本身按 run id 共享（`RunBudgets::shared`），不受影响。
//! - 叶子事件走空接收端：叶子是程序内部子台账（不产生可见聊天卡片，与旧实现的
//!   Broker 审计树一致），且当前没有可从工具回调复用的外层事件通道。
use super::{DataPlane, RigToolDeps};
use crate::agent::rig_ext::{
    r#loop::{
        invocation::ToolInvocationContext, scheduler::TaskScheduler, AppToolExecutionPolicy,
        AppToolPolicyConfig,
    },
    tool_result::{prepare::raw_preparer, RigToolResultPolicy},
};
use parking_lot::Mutex;
use rig::{
    message::{ToolCall, ToolFunction},
    tool::{PortableDynamicTool, ToolExecutionError, ToolOutput},
};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 执行一个程序叶子。
///
/// `sequence` 是叶子在程序内的声明序号（`collect_call_sequences`），它同时充当
/// 登记用的 round，使 `dispatcher_tool_runs.sequence = sequence * 32` 在同一父 run
/// 内唯一——否则同一程序的两个叶子会撞 `idx_dispatcher_tool_runs_parent_sequence`
/// 唯一索引，第二个叶子以 fatal 收场、整个程序中止。
pub(super) async fn execute(
    plane: &DataPlane,
    tool: &PortableDynamicTool,
    step: &str,
    sequence: u64,
    arguments: Value,
) -> Result<ToolOutput, ToolExecutionError> {
    let Some(deps) = plane.runtime.as_ref() else {
        // runtime=None 是 `DataPlane::new` 的默认形态，预期调用方只有两类：
        // 单元测试（直接 `build_program_tool` + `DataPlane::new` 走裸路径）与
        // 尚未接管 runtime 的外部调用方。没有 runtime 就没有库、审查上下文与取消
        // 信号，叶子只能裸执行——登记/结算/命令门禁全部跳过，故不走受管路径。
        // 生产路径由 `program_tool` 经 `with_runtime` 注入 deps（R60）。
        warn_bare_execution_once(step);
        return tool.execute(arguments).await;
    };
    let parent = ToolInvocationContext::current().ok_or_else(|| {
        ToolExecutionError::other("ToolProgram 缺少受管调用上下文，拒绝执行叶子工具")
            .with_code("fatal")
    })?;
    execute_managed(deps, tool, step, sequence, arguments, &parent)
        .await
        .map_err(|error| ToolExecutionError::other(error.to_string()).with_code("fatal"))?
}

/// runtime=None 的裸执行在真实运行里至少留一条可搜痕迹。
///
/// 用 `AtomicBool` 去重：测试会反复走这条路径，不刷屏。
fn warn_bare_execution_once(step: &str) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if WARNED.swap(true, Ordering::AcqRel) {
        return;
    }
    eprintln!(
        "[agent] 警告：ToolProgram 在未注入运行时依赖（runtime=None）下裸执行叶子，跳过登记与结算：{step}"
    );
}

async fn execute_managed(
    deps: &RigToolDeps,
    tool: &PortableDynamicTool,
    step: &str,
    sequence: u64,
    arguments: Value,
    parent: &ToolInvocationContext,
) -> anyhow::Result<Result<ToolOutput, ToolExecutionError>> {
    let output = Arc::new(Mutex::new(None));
    let captured = output.clone();
    let original = tool.clone();
    let definition = tool.definition();
    let wrapped = PortableDynamicTool::new(
        tool.name(),
        definition.description,
        definition.parameters,
        move |args| {
            let (original, captured) = (original.clone(), captured.clone());
            Box::pin(async move {
                let result = original.execute(args).await;
                if let Ok(output) = &result {
                    *captured.lock() = Some(output.clone());
                }
                result
            })
        },
    );
    // 叶子事件 sink：内部子台账不产生可见卡片（见模块注释）。
    let events = tauri::ipc::Channel::new(|_| Ok(()));
    let policy = AppToolExecutionPolicy::new(
        &deps.db,
        &events,
        AppToolPolicyConfig {
            workspace_id: parent.workspace_id.clone(),
            workspace: deps.workspace.clone(),
            review: deps.review.clone(),
            cancel_rx: Some(parent.cancel_rx.clone()),
            trace: crate::agent::db::ToolRunTraceContext {
                parent_run_id: Some(parent.task_id.clone()),
                origin: "tool_program".into(),
                step_id: Some(step.into()),
                // 登记时 dispatch 以 `round * 32 + index` 覆写该字段（叶子批次的
                // index 恒为 0）：这里给同一口径的值，避免两处读起来不一致。
                sequence: sequence * 32,
            },
        },
    );
    let mut scheduler = TaskScheduler::new(
        deps.db.clone(),
        parent.workspace_id.clone(),
        parent.cancel_rx.clone(),
        raw_preparer(),
        events,
    );
    let call = ToolCall::from_wire(
        format!("{}:{step}", parent.tool_call_id),
        ToolFunction {
            name: tool.name().into(),
            arguments,
        },
    );
    let ids = scheduler
        .enqueue(
            &[call],
            &[wrapped],
            &policy,
            &[RigToolResultPolicy::default()],
            // 叶子在自己的调度器里是唯一一个批次：把声明序号当 round 用，
            // 登记 sequence = sequence * 32，同一程序内两两不同。
            sequence,
            &parent.root_request_message_id,
        )
        .await?;
    scheduler.drain().await?;
    let completion = scheduler
        .ready
        .values()
        .next()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("程序叶子缺少结算事件"))?;
    let (db, run, scope, event) = (
        deps.db.clone(),
        scheduler.run_id.clone(),
        scheduler.scope_id.clone(),
        completion.event_id,
    );
    tokio::task::spawn_blocking(move || db.observe_internal_completions(&run, &scope, 0, &[event]))
        .await??;
    scheduler.delivered(&ids[0]);
    scheduler.ready.clear();
    if completion.status == "succeeded" {
        Ok(Ok(output.lock().take().ok_or_else(|| {
            anyhow::anyhow!("成功叶子缺少原始结构化输出")
        })?))
    } else {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::db::NewToolRun;
    use crate::agent::rig_ext::{review::RigReviewContext, tools::deps::ImageToolConfig};
    use serde_json::json;
    use tokio::sync::watch;
    use uuid::Uuid;

    /// 夹具：临时目录 + 真实库 + 只构造不执行的最小工具依赖。
    fn test_deps(dir: &std::path::Path) -> RigToolDeps {
        let db =
            crate::agent::db::DispatcherDb::new(dir.join("jkbot.sqlite3")).expect("open temp db");
        RigToolDeps {
            workspace_id: "managed-leaf-test".to_string(),
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

    fn parent_run(deps: &RigToolDeps) -> crate::agent::db::DispatcherToolRunRecord {
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

    /// R61 回归：同一程序的两个叶子必须都登记成功并结算。
    ///
    /// 修复前两个叶子的登记 sequence 同为 `父 dispatch_round * 32`，第二个会撞
    /// `(parent_run_id, sequence)` 唯一索引，登记失败被标 fatal 中止整个程序。
    #[tokio::test]
    async fn sibling_leaves_register_distinct_sequences_and_settle() {
        let dir = std::env::temp_dir().join(format!("rig-program-managed-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let deps = test_deps(&dir);
        let parent = parent_run(&deps);
        let plane = DataPlane::new(vec![read_file_tool()]).with_runtime(deps.clone());
        let tool = plane.get("read_file").expect("tool registered").clone();
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let context = ToolInvocationContext {
            workspace_id: deps.workspace_id.clone(),
            agent_run_id: parent.id.clone(),
            task_id: parent.id.clone(),
            tool_call_id: "program-call".to_string(),
            root_request_message_id: "anchor".to_string(),
            cancel_rx,
        };

        let (first, second) = context
            .clone()
            .scope(async move {
                let first = execute(&plane, &tool, "first", 1, json!({ "path": "a.txt" })).await;
                let second = execute(&plane, &tool, "second", 2, json!({ "path": "b.txt" })).await;
                (first, second)
            })
            .await;

        let first = first.expect("第一个叶子登记并执行成功");
        assert_eq!(first.as_text(), Some("contents of a.txt"));
        let second = second.expect("第二个叶子登记并执行成功（唯一索引回归）");
        assert_eq!(second.as_text(), Some("contents of b.txt"));

        let tree = deps
            .db
            .list_tool_run_tree(&deps.workspace_id, &parent.id)
            .expect("load program run tree");
        let mut leaves = tree
            .iter()
            .filter(|run| run.parent_run_id.as_deref() == Some(parent.id.as_str()))
            .collect::<Vec<_>>();
        leaves.sort_by_key(|run| run.sequence);
        assert_eq!(leaves.len(), 2, "两个叶子都应登记在同一父 run 下");
        assert_eq!(leaves[0].sequence, 32);
        assert_eq!(leaves[1].sequence, 64);
        assert!(leaves.iter().all(|run| run.status == "succeeded"));
        assert_eq!(leaves[0].origin, "tool_program");
        assert_eq!(leaves[0].step_id.as_deref(), Some("first"));
        assert_eq!(leaves[1].step_id.as_deref(), Some("second"));
    }
}
