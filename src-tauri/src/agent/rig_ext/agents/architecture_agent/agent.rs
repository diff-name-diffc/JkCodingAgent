//! 架构设计 Agent 的画布工具与装配（rig 形态）。
//!
//! - `architecture_run`：把类型化画布程序交给前端画布解释器执行（绝不 eval）：
//!   登记 oneshot → emit `architecture-run-request` → 等待
//!   `architecture_run_complete` 回传报告（前端取得执行权后另有
//!   `ARCH_REPORT_TIMEOUT` 兜底，避免前端失联时永久挂起）；超时/取消一律返回
//!   可恢复错误文本并显式清槽（fail-closed）。迁移自 旧自实现工具层（已随迁移删除）的 architecture_run。
//! - `RigArchitectureAgent`：单工具视觉循环（主模型即视觉模型），会话沙箱
//!   为 `root_dir/architecture/<会话子目录>`。

use std::path::PathBuf;
use std::time::Duration;

use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde::Serialize;
use serde_json::Value;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{oneshot, watch};

use super::super::architecture::program_schema::architecture_run_parameters_schema;
use super::super::architecture::prompt::ARCHITECTURE_SYSTEM_PROMPT;
use super::super::architecture::{validate_program, ArchProgram};
use super::super::plain_chat::render_runtime_workspace;
use crate::agent::config::DispatcherAgentConfig;
use crate::agent::db::{DispatcherDb, DispatcherMessageRecord};
use crate::agent::rig_ext::events::AgentEvent;
use crate::agent::rig_ext::message::{apply_stored_session_summary, chat_history_to_rig_with_ids};
use crate::agent::rig_ext::model::{completions_model, PurposeModelSpec};
use crate::agent::rig_ext::r#loop::{
    run_rig_loop, AppToolExecutionPolicy, AppToolPolicyConfig, RigLoopHooks, RigToolSurface,
};
use crate::agent::rig_ext::review::RigReviewContext;
use crate::agent::rig_ext::tool_result::RigSummaryModel;

/// 等待画布前端执行与回传的总时限（含截图耗时）。
const ARCH_RUN_WAIT_TIMEOUT: Duration = Duration::from_secs(20);

/// 前端取得执行权（claim）后，等待其回传执行报告的兜底时限。
///
/// 选型依据：claim 之后的等待按既有语义不接受取消打断（执行权已交出，只能等
/// 结算），因此前端崩溃 / 页面重载 / 用户离开时这份等待此前永不返回——工具
/// worker 与本次 run 的租赁一起永久悬挂。取 10 分钟：远高于正常执行的一次
/// 往返（首次等待 `ARCH_RUN_WAIT_TIMEOUT` + 画布程序 + 截图），足以覆盖用户在
/// 大程序上的人工停顿；再长则一个死前端会长期占住当次 run，再短会误伤真实的
/// 大画布操作。
const ARCH_REPORT_TIMEOUT: Duration = Duration::from_secs(600);

/// 事件载荷：前端画布监听器收到后在 editor 上执行程序并经
/// `architecture_run_complete` 回传报告。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchRunRequestPayload<'a> {
    run_id: &'a str,
    workspace_id: &'a str,
    program: &'a ArchProgram,
}

/// `architecture_run`：架构 Agent 唯一的画布操作工具。
pub(crate) fn architecture_run_tool(
    workspace_id: String,
    app_handle: Option<AppHandle>,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "architecture_run",
        "向架构设计画布提交类型化画布程序：创建/更新/删除/移动形状、创建与修改箭头（update_arrow：label/labelPosition/kind/箭头/样式）、声明式布局（grid/row/column）、frame 容器（create_shape.into 直接建在容器内；reparent 把已有形状移入容器或移回页面根）、选中与相机导航（select_shapes 圈出形状让用户看到、可选缩放；camera 缩放全图或居中到坐标）。画布解释器整体执行，all-or-nothing——任一指令失败整个程序回滚、画布无变化。程序内用 ref 别名引用本次新建的形状，用画布快照提供的 shapeId 引用已有形状，严禁编造形状 id；形状用 update_shape、箭头用 update_arrow，两者不通用；无需手算坐标，成组排布用 layout，省略 x/y 的 create_shape 会自动放置，move_shape 可只给一个轴。校验错误会一次列出全部问题，修正后重新提交完整程序即可。执行报告给出 ref→shapeId 映射与受影响区域截图（下一轮自动可见）。",
        architecture_run_parameters_schema(),
        move |args| {
            let workspace_id = workspace_id.clone();
            let app_handle = app_handle.clone();
            let cancel_rx = cancel_rx.clone();
            Box::pin(async move {
                run_architecture_program(&args, &workspace_id, app_handle, cancel_rx).await
            })
        },
    )
}

async fn run_architecture_program(
    args: &Value,
    workspace_id: &str,
    app_handle: Option<AppHandle>,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> Result<ToolOutput, ToolExecutionError> {
    let Some(program_value) = args.get("program") else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少 program 参数（类型化画布程序）。",
        ));
    };
    let program: ArchProgram = match serde_json::from_value(program_value.clone()) {
        Ok(program) => program,
        Err(error) => {
            return Err(ToolExecutionError::invalid_args(format!(
                "错误：程序不符合画布程序 DSL：{error}"
            )));
        }
    };
    if let Err(error) = validate_program(&program) {
        return Err(ToolExecutionError::invalid_args(error));
    }

    let Some(app_handle) = app_handle else {
        return Err(ToolExecutionError::other(
            "错误：应用句柄不可用，无法操作画布。",
        ));
    };
    let state = app_handle.state::<crate::agent::DispatcherState>();
    let (run_id, mut report_rx) = state.begin_arch_run(workspace_id);
    let payload = ArchRunRequestPayload {
        run_id: &run_id,
        workspace_id,
        program: &program,
    };
    if let Err(error) = app_handle.emit("architecture-run-request", payload) {
        state.remove_arch_run(&run_id);
        return Err(ToolExecutionError::other(format!(
            "画布请求发送失败：{error}"
        )));
    }
    // 分支优先级确定化：报告已就绪 > 取消 > 超时（与 app_policy 的 biased 约定一致）。
    let interrupted = tokio::select! {
        biased;
        received = &mut report_rx => {
            return received.map(ToolOutput::text)
                .map_err(|_| ToolExecutionError::other("错误：画布响应通道已关闭").with_code("fatal"));
        }
        () = wait_for_cancellation(cancel_rx) => "本轮已取消",
        () = tokio::time::sleep(ARCH_RUN_WAIT_TIMEOUT) => "画布执行超时",
    };
    if state.cancel_unstarted_arch_run(&run_id) {
        return Err(ToolExecutionError::cancelled(format!(
            "{interrupted}，画布程序未开始执行。"
        )));
    }
    // 已取得执行权，清槽不能撤销操作。发出协作取消并保持租约直到报告回传
    // （等待另有 ARCH_REPORT_TIMEOUT 兜底，不再接受取消打断）。
    if let Err(error) = app_handle.emit(
        "architecture-run-cancel",
        serde_json::json!({
            "runId": run_id, "workspaceId": workspace_id
        }),
    ) {
        eprintln!("发送画布取消失败，继续等待实际结算：{error}");
    }
    let report = match wait_for_arch_report(&mut report_rx, ARCH_REPORT_TIMEOUT).await {
        ArchReportWait::Report(report) => report,
        ArchReportWait::Closed => {
            return Err(ToolExecutionError::other("画布结算通道关闭，状态未知").with_code("fatal"))
        }
        ArchReportWait::TimedOut => {
            // 前端 claim 之后崩溃 / 重载 / 离开，再无人提交报告：显式清槽释放
            // 执行权，否则条目会以 started=true 永久留在注册表里。
            state.remove_arch_run(&run_id);
            return Err(ToolExecutionError::timeout(
                "错误：画布报告超时未提交，已释放执行权；请重试。",
            ));
        }
    };
    Err(ToolExecutionError::cancelled(format!(
        "{interrupted}；执行已结算：{report}"
    )))
}

/// claim 之后等待前端回传报告的三种结局。
enum ArchReportWait {
    /// 前端回传了执行报告。
    Report(String),
    /// 报告通道关闭（发送端已销毁），执行状态不可知。
    Closed,
    /// 兜底时限内无人提交报告（前端已死或已离开）。
    TimedOut,
}

/// 等待前端回传报告，带 [`ARCH_REPORT_TIMEOUT`] 兜底。
///
/// 超时判定与报告回传可能落在同一时刻（前端恰好在截止时提交）：超时后先
/// `try_recv` 取走已入箱的报告，不把「正好赶到」的结算误判成前端未提交。
async fn wait_for_arch_report(
    report_rx: &mut oneshot::Receiver<String>,
    timeout: Duration,
) -> ArchReportWait {
    // 超时 future 在语句末尾释放（`&mut *` 重借用），后续才能再取接收端。
    let outcome = tokio::time::timeout(timeout, &mut *report_rx).await;
    match outcome {
        Ok(Ok(report)) => ArchReportWait::Report(report),
        Ok(Err(_)) => ArchReportWait::Closed,
        Err(_) => match report_rx.try_recv() {
            Ok(report) => ArchReportWait::Report(report),
            Err(_) => ArchReportWait::TimedOut,
        },
    }
}

/// 等待取消信号；无取消通道时永不唤醒。通道关闭按已取消处理
/// （与 `common::cancellation_requested` 的语义一致）。
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

// ─── Agent ───────────────────────────────────────────────────────────────────

/// 一轮架构画布运行的输入。
pub struct ArchitectureTurnRequest<'a> {
    pub db: &'a DispatcherDb,
    pub workspace_id: &'a str,
    pub user_segments_json: String,
    pub on_event: Channel<AgentEvent>,
    pub cancel_rx: watch::Receiver<bool>,
}

/// 架构设计视觉 Agent：主模型即视觉模型，工具面仅 `architecture_run`。
pub struct RigArchitectureAgent {
    config: DispatcherAgentConfig,
    spec: PurposeModelSpec,
    app_handle: Option<AppHandle>,
}

impl RigArchitectureAgent {
    pub fn new(config: DispatcherAgentConfig, spec: PurposeModelSpec) -> Self {
        Self {
            config,
            spec,
            app_handle: None,
        }
    }

    pub fn with_app_handle(mut self, app_handle: AppHandle) -> Self {
        self.app_handle = Some(app_handle);
        self
    }

    /// 与旧实现同口径的就绪判定：URL 与模型名非空即可（本地 OpenAI 兼容端点
    /// 允许空 Key；云端漏配 key 由首次请求的鉴权失败显式报出）。
    pub fn is_configured(&self) -> bool {
        !self.spec.api_base.trim().is_empty() && !self.spec.model.trim().is_empty()
    }

    /// 会话沙箱：`root_dir/architecture/<会话子目录>`。
    pub async fn session_workspace(&self, workspace_id: &str) -> anyhow::Result<PathBuf> {
        let workspace = self
            .config
            .root_dir
            .join("architecture")
            .join(super::super::session_workspace_dir_name(workspace_id));
        tokio::task::spawn_blocking({
            let workspace = workspace.clone();
            move || std::fs::create_dir_all(&workspace)
        })
        .await
        .map_err(|error| {
            anyhow::anyhow!("create architecture session workspace panicked: {error}")
        })?
        .map_err(|error| anyhow::anyhow!("create {}: {error}", workspace.display()))?;
        Ok(workspace)
    }

    /// 执行一轮画布操作。
    pub async fn run_turn(
        &self,
        request: ArchitectureTurnRequest<'_>,
    ) -> anyhow::Result<DispatcherMessageRecord> {
        let db = request.db;
        let workspace_id = request.workspace_id;
        let on_event = request.on_event;

        crate::agent::common::emit(
            &on_event,
            AgentEvent::Started {
                workspace_id: workspace_id.to_string(),
            },
        );
        db.validate_chat_image_segments_async(&request.user_segments_json)
            .await?;
        let user = db
            .add_visible_message_from_segments_async(
                workspace_id,
                "user",
                request.user_segments_json.clone(),
            )
            .await?;
        crate::agent::common::emit(&on_event, AgentEvent::UserMessage { message: user });

        let workspace = self.session_workspace(workspace_id).await?;
        if !self.is_configured() {
            anyhow::bail!("错误：未配置视觉模型。请在设置中心「模型服务」添加视觉模型后重试。");
        }

        // 工具面：单工具（画布操作）。会话图片目录先创建再放行（截图落盘于此）。
        let mut extra_dirs = Vec::new();
        if let Ok(dir) = crate::chat_images::workspace_image_dir(workspace_id) {
            match tokio::fs::create_dir_all(&dir).await {
                Ok(()) => extra_dirs.push(dir),
                Err(error) => eprintln!(
                    "创建会话图片目录失败（{}），按空白名单收紧：{error}",
                    dir.display()
                ),
            }
        }
        let surface = RigToolSurface::new(vec![architecture_run_tool(
            workspace_id.to_string(),
            self.app_handle.clone(),
            Some(request.cancel_rx.clone()),
        )]);
        let history = db.load_llm_history_async(workspace_id).await?;
        // 前插已持久化的滚动摘要（历史级压缩的跨 run 延续）。
        let history = apply_stored_session_summary(db, workspace_id, history).await?;
        let (messages, message_ids) = chat_history_to_rig_with_ids(history).await;

        let model = completions_model(&self.spec)
            .map_err(|error| anyhow::anyhow!("初始化架构视觉模型失败：{error}"))?;
        let summary_model = completions_model(&self.spec)
            .map_err(|error| anyhow::anyhow!("初始化架构摘要模型失败：{error}"))?;
        let summary = RigSummaryModel {
            model: &summary_model,
            max_tokens: self.spec.max_tokens,
            temperature: self.spec.temperature,
        };

        let base_preamble = format!(
            "{}{}",
            ARCHITECTURE_SYSTEM_PROMPT,
            render_runtime_workspace(&workspace, true, &extra_dirs, false)
        );
        let mut hooks = RigLoopHooks::from_chat_spec(&self.spec);
        hooks.max_iterations = self.config.max_tool_iterations;
        hooks.default_model_name = self.spec.model.clone();
        hooks.context_window = self.spec.context_window;
        hooks.cancelled_reply = Box::new(build_stopped_reply);
        hooks.max_iterations_error = Some(format!(
            "已达到最大工具迭代次数（{}），本轮画布操作被终止。请检查模型是否陷入工具调用循环。",
            self.config.max_tool_iterations
        ));
        hooks.preamble_for_iteration = Some(Box::new(move |_iteration| {
            Some(format!(
                "{base_preamble}\n\n## 系统时间\n\n当前本地时间：{}",
                crate::agent::prompt::current_local_time()
            ))
        }));

        let policy = AppToolExecutionPolicy::new(
            db,
            &on_event,
            AppToolPolicyConfig {
                workspace_id: workspace_id.to_string(),
                workspace: workspace.clone(),
                review: RigReviewContext::unconfigured(),
                cancel_rx: Some(request.cancel_rx.clone()),
                trace: Default::default(),
            },
        );

        let mut usage_tracker = crate::agent::common::UsageTracker::new();
        run_rig_loop(
            db,
            workspace_id,
            &model,
            messages,
            message_ids,
            &surface,
            &policy,
            Some(&summary),
            &mut hooks,
            &on_event,
            request.cancel_rx.clone(),
            &mut usage_tracker,
        )
        .await
    }
}

/// 取消收口文案（对齐旧 `build_stopped_reply`）。
fn build_stopped_reply(partial: &str) -> String {
    let trimmed = partial.trim();
    if trimmed.is_empty() {
        "⏹️ 本轮画布操作已停止。当前会话上下文已保留，可稍后继续。".to_string()
    } else {
        format!(
            "{}\n\n[本轮画布操作已手动停止。当前会话上下文与以上输出均已保留，可稍后继续。]",
            trimmed
        )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::super::architecture::program_schema::architecture_run_parameters_schema;
    use crate::agent::rig_ext::tools::run_record::prepare_arguments;

    use super::{wait_for_arch_report, ArchReportWait, ARCH_REPORT_TIMEOUT};
    use std::time::Duration;
    use tokio::sync::oneshot;

    /// 报告在兜底时限前到达：按正常结算返回，超时逻辑不参与。
    #[tokio::test(start_paused = true)]
    async fn arch_report_wait_returns_report_before_timeout() {
        let (tx, mut rx) = oneshot::channel();
        tx.send("修改已提交".to_string()).expect("发送报告");
        assert!(matches!(
            wait_for_arch_report(&mut rx, ARCH_REPORT_TIMEOUT).await,
            ArchReportWait::Report(report) if report == "修改已提交"
        ));
    }

    /// 前端 claim 后崩溃/重载/离开，无人提交报告：兜底超时收口，
    /// 而不是让工具 worker 永久挂起。
    #[tokio::test(start_paused = true)]
    async fn arch_report_wait_times_out_without_report() {
        // 发送端保持存活（通道未关闭），模拟「前端还在但永不回传」。
        let (_tx, mut rx) = oneshot::channel::<String>();
        assert!(matches!(
            wait_for_arch_report(&mut rx, ARCH_REPORT_TIMEOUT).await,
            ArchReportWait::TimedOut
        ));
    }

    /// 发送端销毁（工具侧已清槽等极端路径）：按通道关闭上报，不误报超时。
    #[tokio::test(start_paused = true)]
    async fn arch_report_wait_reports_closed_channel() {
        let (tx, mut rx) = oneshot::channel::<String>();
        drop(tx);
        assert!(matches!(
            wait_for_arch_report(&mut rx, ARCH_REPORT_TIMEOUT).await,
            ArchReportWait::Closed
        ));
    }

    /// 零时限下报告已入箱：超时不早于内层 future 的就绪，报告不被超时丢弃。
    #[tokio::test(start_paused = true)]
    async fn arch_report_wait_prefers_buffered_report_over_deadline() {
        let (tx, mut rx) = oneshot::channel();
        tx.send("边界报告".to_string()).expect("发送报告");
        assert!(matches!(
            wait_for_arch_report(&mut rx, Duration::ZERO).await,
            ArchReportWait::Report(report) if report == "边界报告"
        ));
    }

    /// 端到端：模型拿 update_shape 改箭头 labelPosition（历史真实故障）时，
    /// 参数校验错误必须直接点出 labelPosition 字段，而不是笼统的 oneOf 文本。
    #[test]
    fn architecture_run_reports_labelposition_field_error() {
        let error = prepare_arguments(
            "architecture_run",
            &architecture_run_parameters_schema(),
            &json!({
                "program": {
                    "version": 1,
                    "instructions": [
                        {
                            "_type": "update_shape",
                            "labelPosition": 0.3,
                            "target": "shape:jIpSCG3QVzhAw6bjPa2iw"
                        },
                    ],
                },
            }),
        )
        .expect_err("非法 labelPosition 必须被拒绝");
        assert!(
            error.message.contains("labelPosition"),
            "错误应点出具体字段：{}",
            error.message
        );
        assert!(
            !error.message.contains("not valid under any of the schemas"),
            "不应保留笼统 oneOf 文本：{}",
            error.message
        );
    }
}
