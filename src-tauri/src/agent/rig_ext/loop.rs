//! rig 多轮决策入口：共享协调器交付完成事件，工具跨模型轮次执行。
//! `decision` 维护模型决策，`coordinator` 保证唯一应答与观察确认，
//! `scheduler` 管理 worker、资源和取消收敛；子 Agent 使用同一调度底座。
//! 成功/取消只在全部实际操作收尾后发布 Finished，基础设施失败发布 Failed。

use anyhow::Result;
use rig::completion::{CompletionModel, Message};
use tauri::ipc::Channel;
use tokio::sync::watch;

use super::message::attach_turn_tool_images;
use super::model::build_completion_request;
use super::tool_result::RigSummaryModel;
use crate::agent::common::{
    cancellation_requested, emit, persist_assistant_message, persist_tool_calls_message,
    UsageTracker,
};
use crate::agent::db::{DispatcherDb, DispatcherMessageRecord};
use crate::agent::rig_ext::events::AgentEvent;
use crate::shared::error::format_anyhow_error;

mod app_policy;
mod batch;
mod decision;
use decision::run_loop_inner;
mod budgets;
mod profile;
#[cfg(test)]
use batch::classify_tool_error;
use batch::execute_tool_calls;
pub use profile::{RigLoopHooks, RigTurnDiagnostics};
pub(crate) mod coordinator;
pub(crate) mod host;
pub(crate) mod invocation;
mod protocol;
pub(crate) mod resources;
pub(crate) mod runtime;
pub(crate) mod scheduler;
mod stream;
mod support;
mod surface;
#[cfg(test)]
mod tests;

/// 上下文超限（400）收缩重试上限：每次预算减半，重试耗尽后错误照常上抛。
const MAX_CONTEXT_OVERFLOW_RETRIES: usize = 2;
/// 收缩重试的预算下限（字符）：低于此值继续减半已无意义（头+尾都放不下），
/// 避免除二把预算压成 0。
const MIN_CONTEXT_BUDGET_CHARS: usize = 16_000;

pub use app_policy::{AppToolExecutionPolicy, AppToolPolicyConfig};
pub use protocol::{ProtocolToolHandler, RigProtocolAction, RigProtocolResult};
pub use surface::{RigToolSurface, ToolCallOutcome, ToolExecutionPolicy};

use stream::{consume_stream, split_choice, StreamProgress};
use support::{
    build_assistant_message, current_model_name, emit_finished, finalize_cancelled,
    format_finish_reason, maybe_emit_model_switched, outbound_tool_call, record_rig_usage,
};

// ─── 主循环 ───────────────────────────────────────────────────────────────────

/// 运行一轮多轮工具循环，返回收口的 assistant 落库消息。
///
/// 终止事件：成功/取消发 `Finished`，失败发 `Failed`（自包含收口）。
#[allow(clippy::too_many_arguments)]
pub async fn run_rig_loop<M, S, P>(
    db: &DispatcherDb,
    workspace_id: &str,
    model: &M,
    messages: Vec<Message>,
    message_ids: Vec<Option<String>>,
    surface: &RigToolSurface,
    tool_policy: &P,
    summary: Option<&RigSummaryModel<'_, S>>,
    hooks: &mut RigLoopHooks,
    on_event: &Channel<AgentEvent>,
    cancel_rx: watch::Receiver<bool>,
    usage_tracker: &mut UsageTracker,
) -> Result<DispatcherMessageRecord>
where
    M: CompletionModel,
    S: CompletionModel + Clone + 'static,
    P: ToolExecutionPolicy + Clone + 'static,
{
    let question = db
        .get_latest_user_message_content_async(workspace_id)
        .await?;
    let preparer = super::tool_result::prepare::owned_preparer(summary, question);
    let mut coordinator = coordinator::Coordinator::new(scheduler::TaskScheduler::new(
        db.clone(),
        workspace_id.into(),
        cancel_rx.clone(),
        preparer,
        on_event.clone(),
    ));
    coordinator.tasks.recover_completions().await?;
    let result = run_loop_inner(
        db,
        workspace_id,
        model,
        messages,
        message_ids,
        surface,
        tool_policy,
        summary,
        hooks,
        on_event,
        cancel_rx,
        usage_tracker,
        &mut coordinator,
    )
    .await;
    let cleanup = coordinator.tasks.shutdown().await;
    let result = match (result, cleanup) {
        (Err(error), Ok(())) if error.is::<scheduler::RuntimeCancelled>() => {
            finalize_cancelled(db, workspace_id, on_event, hooks, usage_tracker, "", None).await
        }
        (Err(error), Err(cleanup)) => Err(error.context(format!("工具收尾同时失败：{cleanup:#}"))),
        (result, cleanup) => result.and_then(|reply| cleanup.map(|()| reply)),
    };
    if result.is_ok() {
        emit_finished(db, workspace_id, on_event).await?;
    }

    if let Err(error) = &result {
        emit(
            on_event,
            AgentEvent::Failed {
                workspace_id: workspace_id.to_string(),
                // format_anyhow_error 保留完整错误链（HTTP 状态、响应体、连接失败等根因），
                // 对齐旧 run_agent_turn 的 Failed 收口。
                message: format_anyhow_error(error),
            },
        );
    }
    result
}
