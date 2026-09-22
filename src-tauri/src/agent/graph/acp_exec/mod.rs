//! 单节点执行器（ACP / claude-agent-acp）。
//!
//! 每个节点 spawn 一个 claude-agent-acp 子进程（stdio JSON-RPC，经
//! agent-client-protocol crate 的 ByteStreams 传输）：`launcher` 解析启动
//! 计划（托管安装 / 自定义命令 + env 白名单），`process` 自有 spawn
//! （env_clear + 进程组守卫 + stdout 单行上限），initialize →
//! session/new(cwd=workspace) → session/prompt(节点输入)；session/update
//! 通知经 `mapping` 映射为 graph-run-event / AgentActivity 词汇，整轮结束
//! 按 stopReason 结算。子进程回收由 `process::ChildGuard` 负责。

mod client;
mod launcher;
mod mapping;
mod process;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    SelectedPermissionOutcome, SessionUpdate, StopReason,
};
use parking_lot::Mutex;
use tauri::AppHandle;
use tokio::sync::{mpsc, watch};

use super::harness::{PermissionMode, ResolvedNodeHarness};
use super::runner::emit_run_event;
use super::store::GraphStore;
use super::types::{AgentActivity, GraphNode, GraphRunEvent};
use crate::agent::db::settings::AcpAgentConfig as AcpSettings;
use client::AcpSessionError;
use mapping::{decide_permission, Mapper, MapperAction, PermissionDecision};

/// 节点执行整体超时（含进程启动、握手与整轮提示）。
const NODE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Debug)]
pub(crate) enum NodeExecOutcome {
    Succeeded {
        output: String,
        affected_files: Vec<String>,
        tool_call_count: i64,
        usage_json: String,
    },
    /// 失败结算携带已消耗的 usage（执行器若在失败时上报则透传；否则为 "{}"）。
    /// 记录层不得丢弃已发生的 token 消耗——尤其重试场景的首次尝试。
    Failed {
        error: String,
        usage_json: String,
    },
    Cancelled,
}

pub(crate) struct NodeExecContext {
    pub app: AppHandle,
    pub plan_id: String,
    pub run_id: String,
    pub workspace_id: String,
    pub workspace_root: PathBuf,
    pub node: GraphNode,
    pub input: String,
    pub harness: ResolvedNodeHarness,
    /// ACP 执行器启动与凭据配置（设置中心「执行图」页）。
    pub acp: AcpSettings,
    pub store: GraphStore,
    pub cancel_rx: watch::Receiver<bool>,
}

/// session/update 与权限请求处理器共享的上下文。
/// 处理器运行在 crate 连接事件循环上：只做同步映射 + 轻量分发（emit /
/// channel send）；activity 落库由 execute_node 的 saver 任务串行执行，
/// 避免阻塞事件循环导致后续消息停摆（crate 文档明确警告）。
#[derive(Clone)]
struct HandlerContext {
    app: AppHandle,
    plan_id: String,
    run_id: String,
    workspace_id: String,
    node_id: String,
    workspace_root: PathBuf,
    mapper: Arc<Mutex<Mapper>>,
    activity_tx: mpsc::UnboundedSender<AgentActivity>,
}

impl HandlerContext {
    fn handle_update(&self, update: &SessionUpdate) {
        let actions = self.mapper.lock().feed(update);
        self.apply(actions);
    }

    fn apply(&self, actions: Vec<MapperAction>) {
        for action in actions {
            match action {
                MapperAction::Emit(event) => emit_run_event(
                    &self.app,
                    &self.plan_id,
                    &self.run_id,
                    &self.workspace_id,
                    event,
                ),
                MapperAction::Activity(activity) => {
                    emit_run_event(
                        &self.app,
                        &self.plan_id,
                        &self.run_id,
                        &self.workspace_id,
                        GraphRunEvent::NodeActivity {
                            node_id: self.node_id.clone(),
                            activity: activity.clone(),
                        },
                    );
                    let _ = self.activity_tx.send(activity);
                }
            }
        }
    }

    /// 权限自动应答：coding 节点允许（allow_once），read_only 节点与路径越界
    /// 的调用拒绝（reject_once，无则取消）。每次应答落一条 lifecycle 审计活动。
    fn answer_permission(
        &self,
        mode: PermissionMode,
        request: &RequestPermissionRequest,
    ) -> RequestPermissionOutcome {
        let locations = request.tool_call.fields.locations.as_deref().unwrap_or(&[]);
        // 越界判定：任一声明位置越出工作区即拒绝。无 locations 的调用（如
        // Bash）无法按路径约束，仍走模式默认策略。
        let out_of_workspace = locations
            .iter()
            .any(|location| !location.path.starts_with(&self.workspace_root));
        let decision = decide_permission(mode, &request.options, out_of_workspace);
        let tool_title = request
            .tool_call
            .fields
            .title
            .clone()
            .or_else(|| request.tool_call.fields.name.clone())
            .unwrap_or_else(|| "工具调用".into());
        let (audit, outcome) = match &decision {
            PermissionDecision::Select(option_id) => {
                let allowed = request
                    .options
                    .iter()
                    .find(|option| &option.option_id == option_id)
                    .map(|option| {
                        matches!(
                            option.kind,
                            PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways
                        )
                    })
                    .unwrap_or(false);
                let reason = if allowed {
                    ""
                } else if out_of_workspace {
                    "（路径越出工作区）"
                } else {
                    "（只读节点）"
                };
                (
                    format!(
                        "权限自动应答：{} {tool_title}{reason}",
                        if allowed { "允许" } else { "拒绝" }
                    ),
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                        option_id.clone(),
                    )),
                )
            }
            PermissionDecision::Cancel => {
                let reason = if out_of_workspace {
                    "路径越出工作区，且执行器未提供拒绝选项"
                } else {
                    match mode {
                        PermissionMode::AcceptEdits => {
                            "执行器未提供一次性允许选项（不升级为常驻授权）"
                        }
                        PermissionMode::Plan => "执行器未提供拒绝选项",
                    }
                };
                (
                    format!("权限自动应答：取消 {tool_title}（{reason}）"),
                    RequestPermissionOutcome::Cancelled,
                )
            }
        };
        let action = self.mapper.lock().lifecycle_activity(&audit, "");
        self.apply(vec![action]);
        outcome
    }
}

pub(crate) async fn execute_node(ctx: &NodeExecContext) -> NodeExecOutcome {
    if *ctx.cancel_rx.borrow() {
        return NodeExecOutcome::Cancelled;
    }
    let plan = match launcher::prepare(&ctx.acp).await {
        Ok(plan) => plan,
        Err(error) => {
            return NodeExecOutcome::Failed {
                error,
                usage_json: "{}".into(),
            }
        }
    };
    let mapper = Arc::new(Mutex::new(Mapper::new(
        &ctx.run_id,
        &ctx.node.id,
        &ctx.workspace_root,
    )));
    let (activity_tx, activity_rx) = mpsc::unbounded_channel();
    let handler = HandlerContext {
        app: ctx.app.clone(),
        plan_id: ctx.plan_id.clone(),
        run_id: ctx.run_id.clone(),
        workspace_id: ctx.workspace_id.clone(),
        node_id: ctx.node.id.clone(),
        workspace_root: ctx.workspace_root.clone(),
        mapper: Arc::clone(&mapper),
        activity_tx,
    };
    // activity 落库任务：与连接事件循环解耦，串行写库保证 upsert 顺序。
    let saver = spawn_activity_saver(ctx.store.clone(), activity_rx);

    let run = tokio::time::timeout(
        NODE_TIMEOUT,
        client::run_prompt_turn(
            plan,
            ctx.workspace_root.clone(),
            ctx.input.clone(),
            &ctx.harness,
            handler.clone(),
            ctx.cancel_rx.clone(),
        ),
    )
    .await;

    // 会话诊断：落一条 lifecycle 活动便于排障（saver 关闭前）。
    if let Ok(Ok(turn)) = &run {
        if !turn.diagnostics.is_empty() {
            for line in &turn.diagnostics {
                eprintln!("[graph] ACP 会话诊断（{}）：{line}", ctx.node.id);
            }
            let action = mapper
                .lock()
                .lifecycle_activity("ACP 会话诊断", &turn.diagnostics.join("\n"));
            handler.apply(vec![action]);
        }
    }
    // 收尾：flush 未落地的思考缓冲并取出产出，随后关闭落库通道。
    let (final_actions, output, tool_call_count, affected_files) = mapper.lock().finish();
    handler.apply(final_actions);
    drop(handler);
    if let Err(error) = saver.await {
        eprintln!("[graph] 节点活动落库任务异常（{}）：{error}", ctx.node.id);
    }

    match run {
        Err(_elapsed) => NodeExecOutcome::Failed {
            // 超时 drop 了连接 future，进程组已被 ChildGuard 回收。
            error: format!("节点执行超时（{} 分钟）", NODE_TIMEOUT.as_secs() / 60),
            usage_json: "{}".into(),
        },
        Ok(Err(AcpSessionError::Cancelled)) => NodeExecOutcome::Cancelled,
        Ok(Err(AcpSessionError::Failed(error))) => NodeExecOutcome::Failed {
            error,
            usage_json: "{}".into(),
        },
        Ok(Ok(turn)) => {
            settle_stop_reason(turn.stop_reason, output, tool_call_count, affected_files)
        }
    }
}

/// activity 串行落库任务：channel 关闭（所有 sender drop）后自然退出。
fn spawn_activity_saver(
    store: GraphStore,
    mut rx: mpsc::UnboundedReceiver<AgentActivity>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(activity) = rx.recv().await {
            if let Err(error) = store.save_activity_async(&activity).await {
                eprintln!("[graph] 保存节点活动失败（{}）：{error:#}", activity.id);
            }
        }
    })
}

/// stopReason 结算：end_turn 成功；max_tokens / max_turn_requests 成功但
/// 输出附注（产出可能不完整）；refusal 失败；cancelled 取消。
fn settle_stop_reason(
    stop_reason: StopReason,
    output: String,
    tool_call_count: i64,
    affected_files: Vec<String>,
) -> NodeExecOutcome {
    let succeeded = |output: String| NodeExecOutcome::Succeeded {
        output,
        affected_files,
        tool_call_count,
        // ACP 的逐轮 token 用量仍是 unstable 特性（未开启）；上下文占用经
        // usage_update 以 context_usage 活动呈现，usage_json 维持 "{}"。
        usage_json: "{}".into(),
    };
    match stop_reason {
        StopReason::EndTurn => succeeded(output),
        StopReason::MaxTokens => succeeded(format!(
            "{output}\n\n> 注意：本轮因达到最大 token 数提前结束，产出可能不完整。"
        )),
        StopReason::MaxTurnRequests => succeeded(format!(
            "{output}\n\n> 注意：本轮因达到单轮最大请求数提前结束，产出可能不完整。"
        )),
        StopReason::Refusal => NodeExecOutcome::Failed {
            error: "执行器拒绝了该任务（stopReason=refusal）".into(),
            usage_json: "{}".into(),
        },
        StopReason::Cancelled => NodeExecOutcome::Cancelled,
        other => NodeExecOutcome::Failed {
            error: format!("执行器以未知停止原因结束：{other:?}"),
            usage_json: "{}".into(),
        },
    }
}
