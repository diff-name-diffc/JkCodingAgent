//! 单节点执行器（ACP / claude-agent-acp）。
//!
//! 每个节点 spawn 一个 claude-agent-acp 子进程（stdio JSON-RPC，经
//! agent-client-protocol crate 的 ByteStreams 传输）：`launcher` 解析启动
//! 计划（托管安装 / 自定义命令 + env 白名单），`process` 自有 spawn
//! （env_clear + 进程组守卫 + stdout 单行上限），initialize →
//! session/new(cwd=workspace, `_meta` 钉住 `ENABLE_TOOL_SEARCH=false`，见
//! `client::pinned_settings_meta`——节点会话继承用户级 Claude Code 设置，
//! 该 flag 的延迟加载模式在代理/非官方模型下会吞掉内置工具) →
//! session/prompt(节点输入)；session/update 通知经 `mapping` 映射为
//! graph-run-event / AgentActivity 词汇，整轮结束按 stopReason 结算。
//! 子进程回收由 `process::ChildGuard` 负责。

mod client;
mod launcher;
mod mapping;
pub(crate) mod permission_review;
mod process;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    SelectedPermissionOutcome, SessionUpdate, StopReason,
};
use parking_lot::Mutex;
use tauri::AppHandle;
use tokio::sync::{mpsc, watch};

use super::harness::ResolvedNodeHarness;
use super::runner::emit_run_event;
use super::store::GraphStore;
use super::types::{AgentActivity, GraphNode, GraphRunEvent};
use crate::agent::db::settings::AcpAgentConfig as AcpSettings;
use client::AcpSessionError;
use mapping::{decide_static, Mapper, MapperAction, PermissionDecision};
use permission_review::PermissionReviewMaterials;

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
    /// 全局权限审查材料（图摘要 + 审查模型配置 + 本节点快照）。
    pub review: Arc<PermissionReviewMaterials>,
}

/// session/update 与权限请求处理器共享的上下文。
/// 处理器运行在 crate 连接事件循环上：只做同步映射 + 轻量分发（emit /
/// channel send）；activity 落库由 execute_node 的 saver 任务串行执行，
/// 避免阻塞事件循环导致后续消息停摆（crate 文档明确警告）。权限应答
/// 因需调用审查模型（秒级 LLM）是唯一例外——client.rs 把它整体挪进
/// 独立 tokio 任务，事件循环立即返回。
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
    review: Arc<PermissionReviewMaterials>,
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

    /// 权限自动应答：静态快路径（计划模式生命周期 / 路径越界）本地判定，
    /// 其余交给全局权限审查 AI（图摘要 + 节点任务 + 待审请求）。每次应答落
    /// 一条 lifecycle 审计活动；审查的 DENY 理由与 CLARIFY 补充文本写入活动
    /// content 展示（ACP 应答只有 option_id，澄清文本无法回传执行器，行为上
    /// 按拒绝处理）。审查不可用（未配置/超时/失败/解析失败）fail-open 放行。
    async fn answer_permission(
        &self,
        request: &RequestPermissionRequest,
    ) -> RequestPermissionOutcome {
        let fields = &request.tool_call.fields;
        let tool_name = fields.name.clone().unwrap_or_default();
        let locations = fields.locations.as_deref().unwrap_or(&[]);
        // 越界判定：任一声明位置越出工作区即拒绝。无 locations 的调用（如
        // Bash）无法按路径约束，交给审查 AI。
        let out_of_workspace = locations
            .iter()
            .any(|location| !location.path.starts_with(&self.workspace_root));
        let tool_title = fields
            .title
            .clone()
            .or_else(|| fields.name.clone())
            .unwrap_or_else(|| "工具调用".into());

        let (audit_title, audit_content, outcome) =
            match decide_static(&tool_name, &request.options, out_of_workspace) {
                Some(PermissionDecision::Select(option_id)) => {
                    self.static_selection(&tool_name, &request.options, &option_id, &tool_title)
                }
                Some(PermissionDecision::Cancel) => (
                    format!(
                        "权限自动应答：取消 {tool_title}（{}）",
                        if out_of_workspace {
                            "路径越出工作区，且执行器未提供拒绝选项"
                        } else {
                            "执行器未提供一次性选项"
                        }
                    ),
                    String::new(),
                    RequestPermissionOutcome::Cancelled,
                ),
                None => self.reviewed_outcome(request, &tool_title).await,
            };
        let action = self
            .mapper
            .lock()
            .lifecycle_activity(&audit_title, &audit_content);
        self.apply(vec![action]);
        outcome
    }

    /// 静态快路径选中项的审计与应答构造。
    fn static_selection(
        &self,
        tool_name: &str,
        options: &[PermissionOption],
        option_id: &agent_client_protocol::schema::v1::PermissionOptionId,
        tool_title: &str,
    ) -> (String, String, RequestPermissionOutcome) {
        let selected = options.iter().find(|option| &option.option_id == option_id);
        let is_allow = selected
            .map(|option| {
                matches!(
                    option.kind,
                    PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways
                )
            })
            .unwrap_or(false);
        let (title, content) = if tool_name == "EnterPlanMode" {
            (
                format!("权限自动应答：允许进入计划模式（{tool_title}）"),
                String::new(),
            )
        } else if tool_name == "ExitPlanMode" {
            // 批准选项名（如 "Yes, and use auto mode"）记入审计；目标权限级别
            // 由 client.rs 在批准后统一切回 bypassPermissions（0.79.0 的选项表
            // 没有 bypass 变体，选项只充当批准载体）。
            let option_name = selected
                .map(|option| option.name.as_str())
                .unwrap_or("（未知选项）");
            (
                format!("权限自动应答：批准执行计划（{tool_title}）"),
                format!("批准选项：{option_name}（批准后统一切回 bypassPermissions）"),
            )
        } else if is_allow {
            (format!("权限自动应答：允许 {tool_title}"), String::new())
        } else {
            (
                format!("权限自动应答：拒绝 {tool_title}（路径越出工作区）"),
                String::new(),
            )
        };
        (
            title,
            content,
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id.clone())),
        )
    }

    /// 审查 AI 裁决路径：输出审计（含拒绝理由/澄清文本）与应答。
    async fn reviewed_outcome(
        &self,
        request: &RequestPermissionRequest,
        tool_title: &str,
    ) -> (String, String, RequestPermissionOutcome) {
        use permission_review::ReviewOutcome;

        let verdict = permission_review::review_permission(&self.review, request).await;
        let materials = self.review.audit_label();
        let (title, content, decision) = match verdict {
            ReviewOutcome::Allowed => (
                format!("权限审查：允许 {tool_title}"),
                format!("审查对象：{materials}\n结论：允许"),
                mapping::select_kind(&request.options, PermissionOptionKind::AllowOnce),
            ),
            ReviewOutcome::Denied { reason } => (
                format!("权限审查：拒绝 {tool_title}"),
                format!("审查对象：{materials}\n结论：拒绝\n理由：{reason}"),
                mapping::select_kind(&request.options, PermissionOptionKind::RejectOnce),
            ),
            ReviewOutcome::Clarify { text } => (
                format!("权限审查：拒绝 {tool_title}（需澄清）"),
                format!("审查对象：{materials}\n结论：需澄清，本次按拒绝处理\n澄清文本：{text}"),
                mapping::select_kind(&request.options, PermissionOptionKind::RejectOnce),
            ),
            // fail-open：无人值守的图节点不应被审查链路卡死（越界等硬规则
            // 在进入审查前已拦截）。
            ReviewOutcome::Unavailable { reason } => (
                format!("权限审查：审查不可用，放行 {tool_title}"),
                format!("审查对象：{materials}\n结论：放行（fail-open）\n原因：{reason}"),
                mapping::select_kind(&request.options, PermissionOptionKind::AllowOnce),
            ),
        };
        let outcome = match decision {
            Some(PermissionDecision::Select(option_id)) => {
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id))
            }
            // 无一次性选项（或仅会放大授权的选项）时不升级授权：取消本次调用。
            Some(PermissionDecision::Cancel) | None => RequestPermissionOutcome::Cancelled,
        };
        (title, content, outcome)
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
        review: Arc::clone(&ctx.review),
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
