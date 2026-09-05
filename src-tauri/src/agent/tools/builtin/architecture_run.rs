//! `architecture_run`：架构设计视觉 Agent 的画布操作工具。
//!
//! 载荷为类型化画布程序（见 `agents/architecture/program.rs`），由前端画布
//! 解释器执行（绝不 eval）。工具登记 oneshot 后 emit 执行事件并等待前端经
//! `architecture_run_complete` 命令回传执行报告；超时/取消一律返回可恢复
//! 错误文本并显式清槽（fail-closed）。
//!
//! 只注册进架构专用注册表（`ToolRegistry::architecture_tools`），不进
//! `builtin_tools` / `plain_chat_tools`，避免污染聊天上下文与设置页工具清单
//! （同 submit_graph 先例）。

use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use tauri::{Emitter, Manager};
use tokio::sync::watch;

use super::super::context::ToolContext;
use super::super::registry::AgentTool;
use super::super::ToolResult;
use crate::agent::agents::architecture::program::{validate_program, ArchProgram};
use crate::agent::agents::architecture::program_schema::architecture_run_parameters_schema;
use crate::agent::DispatcherState;

/// 等待画布前端执行与回传的总时限（含截图耗时）；策略表超时 40s 留出余量。
const ARCH_RUN_WAIT_TIMEOUT: Duration = Duration::from_secs(20);

pub(super) fn architecture_run_tool() -> Box<dyn AgentTool> {
    Box::new(ArchitectureRunTool)
}

struct ArchitectureRunTool;

/// 事件载荷：前端画布监听器收到后在 editor 上执行程序并经
/// `architecture_run_complete` 回传报告。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchRunRequestPayload<'a> {
    run_id: &'a str,
    workspace_id: &'a str,
    program: &'a ArchProgram,
}

#[async_trait]
impl AgentTool for ArchitectureRunTool {
    fn name(&self) -> &'static str {
        "architecture_run"
    }

    fn description(&self) -> &'static str {
        "向架构设计画布提交类型化画布程序：创建/更新/删除/移动形状、创建与修改箭头（update_arrow：label/labelPosition/kind/箭头/样式）、声明式布局（grid/row/column）、frame 容器（create_shape.into 直接建在容器内；reparent 把已有形状移入容器或移回页面根）、选中与相机导航（select_shapes 圈出形状让用户看到、可选缩放；camera 缩放全图或居中到坐标）。画布解释器整体执行，all-or-nothing——任一指令失败整个程序回滚、画布无变化。程序内用 ref 别名引用本次新建的形状，用画布快照提供的 shapeId 引用已有形状，严禁编造形状 id；形状用 update_shape、箭头用 update_arrow，两者不通用；无需手算坐标，成组排布用 layout，省略 x/y 的 create_shape 会自动放置，move_shape 可只给一个轴。校验错误会一次列出全部问题，修正后重新提交完整程序即可。执行报告给出 ref→shapeId 映射与受影响区域截图（下一轮自动可见）。"
    }

    fn parameters(&self) -> Value {
        architecture_run_parameters_schema()
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        let Some(program_value) = args.get("program") else {
            return ToolResult::recoverable_error("错误：缺少 program 参数（类型化画布程序）。");
        };
        let program: ArchProgram = match serde_json::from_value(program_value.clone()) {
            Ok(program) => program,
            Err(error) => {
                return ToolResult::recoverable_error(format!(
                    "错误：程序不符合画布程序 DSL：{error}"
                ));
            }
        };
        if let Err(error) = validate_program(&program) {
            return ToolResult::recoverable_error(error);
        }

        let Some(app_handle) = context.app_handle.clone() else {
            return ToolResult::recoverable_error("错误：应用句柄不可用，无法操作画布。");
        };
        let state = app_handle.state::<DispatcherState>();
        let (run_id, report_rx) = state.begin_arch_run();
        let payload = ArchRunRequestPayload {
            run_id: &run_id,
            workspace_id: &context.workspace_id,
            program: &program,
        };
        let _ = app_handle.emit("architecture-run-request", payload);

        tokio::select! {
            biased;
            () = wait_for_cancellation(context.cancel_rx.clone()) => {
                state.remove_arch_run(&run_id);
                ToolResult::cancelled("本轮已停止，画布程序未执行。")
            }
            received = report_rx => {
                let report = received
                    .unwrap_or_else(|_| "错误：画布响应通道已关闭。".to_string());
                ToolResult::from_text(report)
            }
            () = tokio::time::sleep(ARCH_RUN_WAIT_TIMEOUT) => {
                state.remove_arch_run(&run_id);
                ToolResult::recoverable_error(format!(
                    "错误：画布 {} 秒未响应（请确认已打开架构设计视图后重试）。",
                    ARCH_RUN_WAIT_TIMEOUT.as_secs()
                ))
            }
        }
    }
}

/// 等待取消信号；无取消通道时永不唤醒。通道关闭按已取消处理
///（与 `common::cancellation_requested` 的语义一致）。
async fn wait_for_cancellation(cancel_rx: Option<watch::Receiver<bool>>) {
    let Some(mut cancel_rx) = cancel_rx else {
        std::future::pending::<()>().await;
        return;
    };
    if *cancel_rx.borrow() {
        return;
    }
    loop {
        if cancel_rx.changed().await.is_err() {
            return;
        }
        if *cancel_rx.borrow() {
            return;
        }
    }
}
