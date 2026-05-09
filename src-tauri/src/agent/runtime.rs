use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use futures::future::join_all;
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use tauri::ipc::Channel;
use tokio::sync::watch;

use super::config::DispatcherAgentConfig;
use super::db::{
    ChecklistPlanItem, ChecklistPlanState, ChecklistStepStatus, DispatcherDb,
    DispatcherMessageRecord, DispatcherMode, DispatcherSessionRuntimeState,
    DispatcherSessionTokenUsageSource, DispatcherSettingsRecord, DispatcherToolArtifactRef,
    PlanInteraction, PlanQuestionOption, TOOL_RETRY_CONTEXT_PREFIX,
};
use super::debug::{render_json, ContextDebugLogger, DebugSection};
use super::llm::{
    messages_contain_inline_images, ChatMessage, FunctionCall, LlmUsage, OpenAiCompatProvider,
    OutboundToolCall, RequestedToolCall,
};
use super::prompt::{build_system_prompt, PromptBundle, PromptSection};
use super::summary::{prepare_tool_result, summarize_dispatch_result};
use super::tools::{
    parse_ask_plan_question, parse_continue_instruction, parse_create_plan_document,
    parse_dispatch_instruction, parse_edit_plan_document, parse_exit_instruction,
    parse_present_plan, parse_replace_plan_document, parse_update_plan, DispatchAgent, ToolContext,
    ToolRegistry, UpdatePlanDraft,
};
use crate::project::mcp::{build_workspace_mcp_prompt_block, ProjectMcpRegistry};
use crate::shared::truncate_for_display;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchFeedbackState {
    RoundCompleted,
    ProcessDone,
    ProcessFailed,
    ProcessCancelled,
}

impl DispatchFeedbackState {
    pub fn from_wire(value: &str) -> Self {
        match value {
            "process_done" => Self::ProcessDone,
            "process_failed" => Self::ProcessFailed,
            "process_cancelled" => Self::ProcessCancelled,
            _ => Self::RoundCompleted,
        }
    }

    fn visible_message(self) -> &'static str {
        match self {
            Self::RoundCompleted => "🔄 子任务当前轮次已完成",
            Self::ProcessDone => "✅ 子任务进程已结束",
            Self::ProcessFailed => "⚠️ 子任务进程已失败退出",
            Self::ProcessCancelled => "⏹️ 子任务进程已取消",
        }
    }

    fn hidden_prefix(self) -> &'static str {
        match self {
            Self::RoundCompleted => {
                "[系统通知] 子任务当前轮次已完成，但子进程仍在运行，可继续注入后续指令，也可在确认无需继续后主动退出。请先分析执行状态，再决定下一步："
            }
            Self::ProcessDone => {
                "[系统通知] 子任务进程已结束。请根据以下执行结果总结反馈："
            }
            Self::ProcessFailed => {
                "[系统通知] 子任务进程已失败退出。请根据以下执行结果分析问题并决定下一步："
            }
            Self::ProcessCancelled => {
                "[系统通知] 子任务进程已取消。请根据以下执行结果判断是否需要重试或调整方案："
            }
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurn {
    pub reply: DispatcherMessageRecord,
    pub messages: Vec<DispatcherMessageRecord>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "event",
    content = "data"
)]
pub enum AgentEvent {
    Started {
        workspace_id: String,
    },
    UserMessage {
        message: DispatcherMessageRecord,
    },
    AssistantStarted {
        message_id: String,
    },
    ModelSwitched {
        from_model: String,
        to_model: String,
        reason: String,
    },
    AssistantDelta {
        message_id: String,
        delta: String,
    },
    AssistantMessage {
        message: DispatcherMessageRecord,
    },
    ToolPlanned {
        tool_call_id: Option<String>,
        name: String,
        arguments: String,
    },
    ToolStarted {
        tool_call_id: Option<String>,
        name: String,
        arguments: String,
    },
    ToolSummaryStarted {
        tool_call_id: Option<String>,
        name: String,
        result_mode: String,
    },
    ToolSummaryDelta {
        tool_call_id: Option<String>,
        name: String,
        delta: String,
        result_mode: String,
    },
    ToolFinished {
        tool_call_id: Option<String>,
        name: String,
        display_text: String,
        result_mode: String,
        detail_refs: Vec<DispatcherToolArtifactRef>,
    },
    ChecklistPlanUpdated {
        state: ChecklistPlanState,
    },
    PlanQuestionRequested {
        interaction: PlanInteraction,
    },
    PlanDocumentOpened {
        plan_path: String,
    },
    PlanReady {
        interaction: PlanInteraction,
    },
    PlanImplemented {
        plan_path: String,
        implemented_path: String,
        summary: String,
    },
    DispatchProposed {
        dispatch_id: String,
        agent: String,
        description: String,
        task_prompt: String,
        permission_mode: String,
    },
    DispatchContinue {
        dispatch_id: String,
        agent: String,
        text: String,
    },
    DispatchExit {
        dispatch_id: String,
        agent: String,
        reason: String,
    },
    Finished {
        messages: Vec<DispatcherMessageRecord>,
    },
}

pub struct DispatcherAgent {
    config: DispatcherAgentConfig,
    provider: Mutex<OpenAiCompatProvider>,
    summary_model: Mutex<String>,
    vision_model: Mutex<String>,
    tools: ToolRegistry,
    project_mcp_registry: ProjectMcpRegistry,
    subprocesses: Arc<DispatcherSubprocessRegistry>,
}

#[derive(Default)]
pub(crate) struct DispatcherSubprocessRegistry {
    subprocesses: Mutex<Vec<RegisteredSubprocess>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegisteredSubprocessPhase {
    Running,
    RoundCompleted,
    Stopped,
    ExitRequested,
}

#[derive(Clone, Debug)]
struct RegisteredSubprocess {
    workspace_id: String,
    task_id: String,
    dispatch_id: String,
    agent: String,
    description: String,
    phase: RegisteredSubprocessPhase,
    force_idle: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
struct SystemPromptSnapshot {
    rendered: String,
}

#[derive(Clone, Debug)]
enum PlannedSubprocessState {
    Active {
        dispatch_id: String,
        phase: RegisteredSubprocessPhase,
    },
    PendingDispatch {
        dispatch_id: String,
    },
}

#[derive(Debug)]
struct ProtocolBatchState {
    by_agent: HashMap<String, PlannedSubprocessState>,
}

#[derive(Clone, Debug)]
enum ProtocolToolAction {
    Dispatch {
        dispatch_id: String,
        agent: DispatchAgent,
        description: String,
        task_prompt: String,
        permission_mode: String,
    },
    Continue {
        dispatch_id: String,
        agent: DispatchAgent,
        text: String,
    },
    Exit {
        dispatch_id: String,
        agent: DispatchAgent,
        reason: String,
    },
}

#[derive(Debug)]
enum PlanningToolOutcome {
    ToolResult(String),
    WaitForUser(String),
}

impl ProtocolBatchState {
    fn new(subprocesses: Vec<RegisteredSubprocess>) -> Self {
        let by_agent = subprocesses
            .into_iter()
            .map(|item| {
                (
                    item.agent,
                    PlannedSubprocessState::Active {
                        dispatch_id: item.dispatch_id,
                        phase: item.phase,
                    },
                )
            })
            .collect();

        Self { by_agent }
    }

    fn dispatch_id_for_agent(&self, agent: &str) -> Option<&str> {
        match self.by_agent.get(agent) {
            Some(PlannedSubprocessState::Active { dispatch_id, .. })
            | Some(PlannedSubprocessState::PendingDispatch { dispatch_id }) => Some(dispatch_id),
            None => None,
        }
    }

    fn ensure_dispatch_allowed(
        &self,
        agent: &str,
        agent_label: &str,
    ) -> std::result::Result<(), String> {
        match self.by_agent.get(agent) {
            Some(PlannedSubprocessState::Active { dispatch_id, phase }) => Err(format!(
                "错误：当前会话已有一个活跃的 {agent_label} 子进程（dispatch_id={}, phase={}）。禁止再次调用 dispatch_{}；请改用 continue_{}_session、exit_{}_session，或直接回复用户。",
                dispatch_id,
                subprocess_phase_label(*phase),
                agent,
                agent,
                agent
            )),
            Some(PlannedSubprocessState::PendingDispatch { dispatch_id }) => Err(format!(
                "错误：当前轮已为 {agent_label} 规划一个待启动子任务（dispatch_id={}）。禁止重复调用 dispatch_{}；请等待该子任务启动后再继续协调。",
                dispatch_id, agent
            )),
            None => Ok(()),
        }
    }

    fn record_dispatch(&mut self, agent: &str, dispatch_id: &str) {
        self.by_agent.insert(
            agent.to_string(),
            PlannedSubprocessState::PendingDispatch {
                dispatch_id: dispatch_id.to_string(),
            },
        );
    }

    fn ensure_continue_allowed(
        &self,
        agent: &str,
        agent_label: &str,
    ) -> std::result::Result<(), String> {
        match self.by_agent.get(agent) {
            Some(PlannedSubprocessState::Active {
                phase:
                    RegisteredSubprocessPhase::Running | RegisteredSubprocessPhase::RoundCompleted,
                ..
            }) => Ok(()),
            Some(PlannedSubprocessState::Active {
                phase: RegisteredSubprocessPhase::Stopped,
                ..
            }) => Err(format!(
                "错误：{agent_label} 子进程当前处于 stopped 状态，请先由 UI 恢复运行后再继续注入指令。"
            )),
            Some(PlannedSubprocessState::Active {
                phase: RegisteredSubprocessPhase::ExitRequested,
                ..
            }) => Err(format!(
                "错误：{agent_label} 子进程已收到退出请求，当前只能等待其结束，不能再继续注入指令。"
            )),
            Some(PlannedSubprocessState::PendingDispatch { .. }) => Err(format!(
                "错误：{agent_label} 子任务已在当前轮提出但尚未真正启动，当前不能继续注入指令。"
            )),
            None => Err(format!(
                "错误：当前会话没有可继续的 {agent_label} 活跃子进程。"
            )),
        }
    }

    fn record_continue(&mut self, agent: &str) {
        if let Some(PlannedSubprocessState::Active { phase, .. }) = self.by_agent.get_mut(agent) {
            *phase = RegisteredSubprocessPhase::Running;
        }
    }

    fn ensure_exit_allowed(
        &self,
        agent: &str,
        agent_label: &str,
    ) -> std::result::Result<(), String> {
        match self.by_agent.get(agent) {
            Some(PlannedSubprocessState::Active {
                phase:
                    RegisteredSubprocessPhase::Running | RegisteredSubprocessPhase::RoundCompleted,
                ..
            }) => Ok(()),
            Some(PlannedSubprocessState::Active {
                phase: RegisteredSubprocessPhase::Stopped,
                ..
            }) => Err(format!(
                "错误：{agent_label} 子进程当前处于 stopped 状态，请先恢复运行后再决定是否退出。"
            )),
            Some(PlannedSubprocessState::Active {
                phase: RegisteredSubprocessPhase::ExitRequested,
                ..
            }) => Err(format!(
                "错误：{agent_label} 子进程已经收到退出命令，请等待进程结束，不要重复 exit。"
            )),
            Some(PlannedSubprocessState::PendingDispatch { .. }) => Err(format!(
                "错误：{agent_label} 子任务尚未真正启动，当前不能发送退出命令。"
            )),
            None => Err(format!(
                "错误：当前会话没有可退出的 {agent_label} 活跃子进程。"
            )),
        }
    }

    fn record_exit(&mut self, agent: &str) {
        if let Some(PlannedSubprocessState::Active { phase, .. }) = self.by_agent.get_mut(agent) {
            *phase = RegisteredSubprocessPhase::ExitRequested;
        }
    }
}

impl DispatcherSubprocessRegistry {
    pub(crate) fn register(
        &self,
        workspace_id: &str,
        task_id: &str,
        dispatch_id: &str,
        agent: &str,
        description: &str,
    ) -> Arc<AtomicBool> {
        let mut subprocesses = self.subprocesses.lock();
        let force_idle = subprocesses
            .iter()
            .find(|item| item.task_id == task_id)
            .map(|item| Arc::clone(&item.force_idle))
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));

        subprocesses.retain(|item| !(item.workspace_id == workspace_id && item.agent == agent));
        subprocesses.push(RegisteredSubprocess {
            workspace_id: workspace_id.to_string(),
            task_id: task_id.to_string(),
            dispatch_id: dispatch_id.to_string(),
            agent: agent.to_string(),
            description: description.to_string(),
            phase: RegisteredSubprocessPhase::Running,
            force_idle: Arc::clone(&force_idle),
        });
        force_idle
    }

    pub(crate) fn mark_round_completed(&self, task_id: &str) {
        self.update_phase(task_id, RegisteredSubprocessPhase::RoundCompleted);
    }

    pub(crate) fn mark_running(&self, task_id: &str) {
        self.update_phase(task_id, RegisteredSubprocessPhase::Running);
    }

    pub(crate) fn mark_stopped(&self, task_id: &str) {
        self.update_phase(task_id, RegisteredSubprocessPhase::Stopped);
    }

    pub(crate) fn mark_exit_requested(&self, task_id: &str) {
        self.update_phase(task_id, RegisteredSubprocessPhase::ExitRequested);
    }

    pub(crate) fn mark_finished(&self, task_id: &str) {
        let mut subprocesses = self.subprocesses.lock();
        subprocesses.retain(|item| item.task_id != task_id);
    }

    pub(crate) fn force_idle(&self, task_id: &str) {
        if let Some(item) = self
            .subprocesses
            .lock()
            .iter()
            .find(|item| item.task_id == task_id)
        {
            item.force_idle.store(true, Ordering::Relaxed);
        }
    }

    pub(crate) fn is_exit_requested(&self, task_id: &str) -> bool {
        self.subprocesses.lock().iter().any(|item| {
            item.task_id == task_id && item.phase == RegisteredSubprocessPhase::ExitRequested
        })
    }

    fn update_phase(&self, task_id: &str, phase: RegisteredSubprocessPhase) {
        let mut subprocesses = self.subprocesses.lock();
        if let Some(item) = subprocesses.iter_mut().find(|item| item.task_id == task_id) {
            item.phase = phase;
        }
    }
}

impl DispatcherAgent {
    pub fn new(
        config: DispatcherAgentConfig,
        project_mcp_registry: ProjectMcpRegistry,
        subprocesses: Arc<DispatcherSubprocessRegistry>,
    ) -> Self {
        let provider = OpenAiCompatProvider::new(
            config.api_key.clone(),
            config.api_base.clone(),
            config.model.clone(),
            config.max_tokens,
            config.temperature,
        );

        Self {
            summary_model: Mutex::new(normalize_summary_model(&config.summary_model)),
            vision_model: Mutex::new(config.vision_model.trim().to_string()),
            config,
            provider: Mutex::new(provider),
            tools: ToolRegistry::default_tools(project_mcp_registry.clone()),
            project_mcp_registry,
            subprocesses,
        }
    }

    pub fn apply_settings(&self, settings: &DispatcherSettingsRecord) {
        let mut provider = self.provider.lock();
        *provider = OpenAiCompatProvider::new(
            if settings.api_key.is_empty() {
                self.config.api_key.clone()
            } else {
                settings.api_key.clone()
            },
            if settings.api_base.is_empty() {
                self.config.api_base.clone()
            } else {
                settings.api_base.clone()
            },
            if settings.model.is_empty() {
                self.config.model.clone()
            } else {
                settings.model.clone()
            },
            self.config.max_tokens,
            self.config.temperature,
        );
        let mut summary_model = self.summary_model.lock();
        *summary_model = if settings.summary_model.trim().is_empty() {
            normalize_summary_model(&self.config.summary_model)
        } else {
            normalize_summary_model(&settings.summary_model)
        };
        let mut vision_model = self.vision_model.lock();
        *vision_model = if settings.vision_model.trim().is_empty() {
            self.config.vision_model.trim().to_string()
        } else {
            settings.vision_model.trim().to_string()
        };
    }

    pub fn auto_approve_dispatch(&self) -> bool {
        self.config.auto_approve_dispatch
    }

    pub fn set_auto_approve_dispatch(&mut self, value: bool) {
        self.config.auto_approve_dispatch = value;
    }

    pub fn context_debug_enabled(&self) -> bool {
        self.config.context_debug
    }

    fn summary_model(&self) -> String {
        self.summary_model.lock().clone()
    }

    fn vision_model(&self) -> String {
        self.vision_model.lock().clone()
    }

    fn provider_for_messages(
        &self,
        provider: &OpenAiCompatProvider,
        messages: &[ChatMessage],
        on_event: &Channel<AgentEvent>,
        notify_user: bool,
    ) -> Result<OpenAiCompatProvider> {
        if !messages_contain_inline_images(messages) {
            return Ok(provider.clone());
        }

        let vision_model = self.vision_model();
        if vision_model.trim().is_empty() {
            anyhow::bail!(
                "检测到用户上传了图片，但 Dispatcher 设置中的视觉模型为空。请先配置视觉模型后重试。"
            );
        }

        let selected = provider.with_model(vision_model.trim());
        if notify_user && selected.model() != provider.model() {
            emit(
                on_event,
                AgentEvent::ModelSwitched {
                    from_model: provider.model().to_string(),
                    to_model: selected.model().to_string(),
                    reason: "检测到用户上传了图片".to_string(),
                },
            );
        }

        Ok(selected)
    }

    pub fn set_context_debug(&mut self, value: bool) {
        self.config.context_debug = value;
    }

    pub async fn run(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace_path: &str,
        user_message: &str,
        on_event: Channel<AgentEvent>,
        cancel_rx: watch::Receiver<bool>,
    ) -> Result<AgentTurn> {
        emit(
            &on_event,
            AgentEvent::Started {
                workspace_id: workspace_id.to_string(),
            },
        );
        db.clear_checklist(workspace_id)
            .context("clear stale checklist before new dispatcher turn")?;
        emit(
            &on_event,
            AgentEvent::ChecklistPlanUpdated {
                state: empty_checklist_state(),
            },
        );

        let workspace = PathBuf::from(workspace_path);
        if !workspace.exists() {
            fs::create_dir_all(&workspace)
                .with_context(|| format!("create workspace {}", workspace.display()))?;
        }
        self.project_mcp_registry
            .ensure_recent(&workspace)
            .await
            .map_err(anyhow::Error::msg)
            .context("刷新项目 MCP 状态失败")?;
        let user = db.add_visible_message(workspace_id, "user", user_message)?;
        emit(&on_event, AgentEvent::UserMessage { message: user });

        let provider = self.provider.lock().clone();
        if !provider.is_configured() {
            anyhow::bail!(
                "主 Agent LLM API Key 未配置。请在 Dispatcher 设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。"
            );
        }
        let reply = self
            .run_llm_loop(
                db,
                workspace_id,
                &workspace,
                &on_event,
                &provider,
                cancel_rx,
            )
            .await?;

        let messages = db.list_visible_messages(workspace_id)?;
        emit(
            &on_event,
            AgentEvent::Finished {
                messages: messages.clone(),
            },
        );
        Ok(AgentTurn { reply, messages })
    }

    pub async fn continue_after_dispatch(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace_path: &str,
        dispatch_result: &str,
        dispatch_state: DispatchFeedbackState,
        dispatch_id: Option<&str>,
        on_event: Channel<AgentEvent>,
        mut cancel_rx: watch::Receiver<bool>,
    ) -> Result<AgentTurn> {
        let debug_logger =
            ContextDebugLogger::new(self.context_debug_enabled(), PathBuf::from(workspace_path));
        if let Some(dispatch_id) = dispatch_id {
            let checklist = match dispatch_state {
                DispatchFeedbackState::RoundCompleted => {
                    start_checklist_dispatch(db, workspace_id, dispatch_id)
                }
                DispatchFeedbackState::ProcessDone
                | DispatchFeedbackState::ProcessFailed
                | DispatchFeedbackState::ProcessCancelled => {
                    complete_checklist_dispatch(db, workspace_id, dispatch_id)
                }
            }
            .map_err(anyhow::Error::msg)?;
            if let Some(checklist) = checklist {
                emit(
                    &on_event,
                    AgentEvent::ChecklistPlanUpdated { state: checklist },
                );
            }
        }
        let result_msg =
            db.add_visible_message(workspace_id, "assistant", dispatch_state.visible_message())?;
        emit(
            &on_event,
            AgentEvent::AssistantMessage {
                message: result_msg.clone(),
            },
        );

        if cancellation_requested(&cancel_rx) {
            let reply = self.emit_stop_and_finish(db, workspace_id, &on_event, "")?;
            let messages = db.list_visible_messages(workspace_id)?;
            emit(
                &on_event,
                AgentEvent::Finished {
                    messages: messages.clone(),
                },
            );
            return Ok(AgentTurn { reply, messages });
        }

        let provider = self.provider.lock().clone();
        if !provider.is_configured() {
            anyhow::bail!(
                "主 Agent LLM API Key 未配置，无法继续处理子任务结果。请在 Dispatcher 设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。"
            );
        }
        let summary_model = self.summary_model();
        let summarized_dispatch_result = match tokio::select! {
            _ = wait_for_cancellation(&mut cancel_rx) => {
                let reply = self.emit_stop_and_finish(db, workspace_id, &on_event, "")?;
                let messages = db.list_visible_messages(workspace_id)?;
                emit(
                    &on_event,
                    AgentEvent::Finished {
                        messages: messages.clone(),
                    },
                );
                return Ok(AgentTurn { reply, messages });
            }
            result = summarize_dispatch_result(&provider, &summary_model, dispatch_result, |usage| {
                record_session_token_usage(
                    db,
                    workspace_id,
                    &summary_model,
                    DispatcherSessionTokenUsageSource::Summary,
                    usage,
                );
            }) => result
        } {
            Ok(summary) => summary,
            Err(error) => {
                debug_logger.log(
                    "子任务结果摘要失败",
                    vec![
                        ("工作区".to_string(), workspace_id.to_string()),
                        (
                            "子任务状态".to_string(),
                            format!("{dispatch_state:?}").to_lowercase(),
                        ),
                    ],
                    vec![
                        DebugSection::new("摘要调用", error.debug_context().to_string()),
                        DebugSection::new("失败原因", error.message().to_string()),
                    ],
                );
                anyhow::bail!(
                    "子任务结果摘要失败，summary_model={}：{}",
                    summary_model,
                    error.message()
                );
            }
        };

        let hidden_message = format!(
            "{}\n\n{}",
            dispatch_state.hidden_prefix(),
            summarized_dispatch_result
        );

        db.add_hidden_message(
            workspace_id,
            "user",
            &hidden_message,
            None,
            None,
            None,
            None,
        )?;

        let workspace = PathBuf::from(workspace_path);
        self.project_mcp_registry
            .ensure_recent(&workspace)
            .await
            .map_err(anyhow::Error::msg)
            .context("刷新项目 MCP 状态失败")?;
        let reply = self
            .run_llm_loop(
                db,
                workspace_id,
                &workspace,
                &on_event,
                &provider,
                cancel_rx,
            )
            .await?;

        let messages = db.list_visible_messages(workspace_id)?;
        emit(
            &on_event,
            AgentEvent::Finished {
                messages: messages.clone(),
            },
        );
        Ok(AgentTurn { reply, messages })
    }

    async fn run_llm_loop(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &Path,
        on_event: &Channel<AgentEvent>,
        provider: &OpenAiCompatProvider,
        cancel_rx: watch::Receiver<bool>,
    ) -> Result<DispatcherMessageRecord> {
        let debug_logger = ContextDebugLogger::new(self.context_debug_enabled(), workspace);
        let tool_context = ToolContext {
            workspace: workspace.to_path_buf(),
            exec_timeout_secs: self.config.exec_timeout_secs,
            restrict_to_workspace: self.config.restrict_to_workspace,
        };

        for iteration in 0..self.config.max_tool_iterations {
            if cancellation_requested(&cancel_rx) {
                return self.emit_stop_and_finish(db, workspace_id, on_event, "");
            }

            let runtime_state = db.get_session_runtime_state(workspace_id)?;
            let tool_definitions =
                self.tool_definitions_for_workspace(workspace_id, workspace, &runtime_state);
            let allowed_tool_names = tool_definitions
                .iter()
                .map(|tool| tool.function.name.clone())
                .collect::<HashSet<_>>();
            let prompt_snapshot = self.build_system_prompt_for_workspace(
                workspace_id,
                workspace,
                &tool_definitions,
                &runtime_state,
            )?;
            let history_messages = db.load_llm_history(workspace_id)?;
            let request_provider =
                self.provider_for_messages(provider, &history_messages, on_event, iteration == 0)?;
            let mut messages = vec![ChatMessage::system(prompt_snapshot.rendered.clone())];
            messages.extend(history_messages.clone());
            let request_snapshot =
                request_provider.build_request_snapshot(&messages, &tool_definitions);

            debug_logger.log(
                "发送大模型请求",
                vec![
                    ("工作区".to_string(), workspace_id.to_string()),
                    ("轮次".to_string(), (iteration + 1).to_string()),
                    ("模型".to_string(), request_provider.model().to_string()),
                    ("消息数".to_string(), messages.len().to_string()),
                    ("工具数".to_string(), tool_definitions.len().to_string()),
                ],
                vec![DebugSection::new(
                    "实际请求",
                    render_json(&request_snapshot),
                )],
            );

            let stream_msg_id = uuid::Uuid::new_v4().to_string();
            emit(
                on_event,
                AgentEvent::AssistantStarted {
                    message_id: stream_msg_id.clone(),
                },
            );

            let event_ref = on_event;
            let msg_id_ref = stream_msg_id.clone();
            let streamed_text = Arc::new(Mutex::new(String::new()));
            let streamed_text_ref = Arc::clone(&streamed_text);
            let on_delta = move |delta: &str| {
                let mut partial = streamed_text_ref.lock();
                partial.push_str(delta);
                let _ = event_ref.send(AgentEvent::AssistantDelta {
                    message_id: msg_id_ref.clone(),
                    delta: delta.to_string(),
                });
            };

            let mut stream_cancel_rx = cancel_rx.clone();
            let response = tokio::select! {
                _ = wait_for_cancellation(&mut stream_cancel_rx) => {
                    let partial = streamed_text.lock().clone();
                    return self.emit_stop_and_finish(db, workspace_id, on_event, &partial);
                }
                response = request_provider.chat_stream(
                    &messages,
                    &tool_definitions,
                    messages_contain_inline_images(&history_messages),
                    on_delta,
                ) => response
            }?;
            let response_snapshot = request_provider.build_response_snapshot(&response);
            if let Some(usage) = response.usage.as_ref() {
                record_session_token_usage(
                    db,
                    workspace_id,
                    request_provider.model(),
                    DispatcherSessionTokenUsageSource::Primary,
                    usage,
                );
            }

            debug_logger.log(
                "收到大模型响应",
                vec![
                    ("工作区".to_string(), workspace_id.to_string()),
                    ("轮次".to_string(), (iteration + 1).to_string()),
                    ("模型".to_string(), request_provider.model().to_string()),
                    ("状态码".to_string(), response.status_code.to_string()),
                    (
                        "工具调用数".to_string(),
                        response.tool_calls.len().to_string(),
                    ),
                ],
                vec![DebugSection::new(
                    "实际响应",
                    render_json(&response_snapshot),
                )],
            );

            if response.tool_calls.is_empty() {
                let content = response.content.trim().to_string();
                if content.is_empty() {
                    anyhow::bail!("LLM 返回了空响应且没有工具调用，无法继续执行。");
                }
                let reply = db.add_visible_message(workspace_id, "assistant", &content)?;
                emit(
                    on_event,
                    AgentEvent::AssistantMessage {
                        message: reply.clone(),
                    },
                );
                return Ok(reply);
            }

            let tool_calls_payload = response
                .tool_calls
                .iter()
                .map(|call| OutboundToolCall {
                    id: call.id.clone(),
                    kind: "function".to_string(),
                    function: FunctionCall {
                        name: call.name.clone(),
                        arguments: serde_json::to_string(&call.arguments)
                            .unwrap_or_else(|_| "{}".to_string()),
                    },
                })
                .collect::<Vec<_>>();

            for tool_call in &response.tool_calls {
                emit(
                    on_event,
                    AgentEvent::ToolPlanned {
                        tool_call_id: Some(tool_call.id.clone()),
                        name: tool_call.name.clone(),
                        arguments: serde_json::to_string(&tool_call.arguments)
                            .unwrap_or_else(|_| "{}".to_string()),
                    },
                );
            }

            db.add_visible_message_with_tools(
                workspace_id,
                "assistant",
                &response.content,
                None,
                None,
                None,
                Some(&tool_calls_payload),
            )?;

            let mut protocol_state =
                ProtocolBatchState::new(self.active_subprocesses_for_workspace(workspace_id));
            let mut protocol_actions = Vec::new();
            let mut planning_waiting_message: Option<String> = None;
            let mut final_message: Option<String> = None;
            let mut saw_retryable_tool_error = false;

            let tool_calls = response.tool_calls;
            let preprocessed_tool_call_ids = self
                .execute_update_plan_calls_first(
                    db,
                    workspace_id,
                    workspace,
                    on_event,
                    &tool_calls,
                    &runtime_state,
                    &allowed_tool_names,
                )
                .await?;
            let mut tool_call_index = 0usize;
            while tool_call_index < tool_calls.len() {
                if cancellation_requested(&cancel_rx) {
                    return self.emit_stop_and_finish(db, workspace_id, on_event, "");
                }
                if preprocessed_tool_call_ids.contains(&tool_calls[tool_call_index].id) {
                    tool_call_index += 1;
                    continue;
                }

                let readonly_end = readonly_tool_run_end(&tool_calls, tool_call_index);
                let ready_tool_results = if readonly_end.saturating_sub(tool_call_index) >= 2 {
                    let run = &tool_calls[tool_call_index..readonly_end];
                    let results = self
                        .execute_parallel_readonly_tools(
                            run,
                            &tool_context,
                            on_event,
                            &allowed_tool_names,
                        )
                        .await;
                    let items = run
                        .iter()
                        .cloned()
                        .zip(results)
                        .collect::<Vec<(RequestedToolCall, String)>>();
                    tool_call_index = readonly_end;
                    items
                } else {
                    let tool_call = tool_calls[tool_call_index].clone();
                    tool_call_index += 1;
                    emit(
                        on_event,
                        AgentEvent::ToolStarted {
                            tool_call_id: Some(tool_call.id.clone()),
                            name: tool_call.name.clone(),
                            arguments: serde_json::to_string(&tool_call.arguments)
                                .unwrap_or_else(|_| "{}".to_string()),
                        },
                    );
                    let result = if allowed_tool_names.contains(&tool_call.name) {
                        self.tools
                            .execute(&tool_call.name, &tool_call.arguments, &tool_context)
                            .await
                    } else {
                        disallowed_tool_result(&tool_call.name)
                    };
                    vec![(tool_call, result)]
                };

                for (tool_call, result) in ready_tool_results {
                    if cancellation_requested(&cancel_rx) {
                        return self.emit_stop_and_finish(db, workspace_id, on_event, "");
                    }
                    if preprocessed_tool_call_ids.contains(&tool_call.id) {
                        continue;
                    }

                    let tool_arguments = serde_json::to_string_pretty(&tool_call.arguments)
                        .unwrap_or_else(|_| "{}".to_string());

                    match self
                        .execute_planning_tool(
                            db,
                            workspace_id,
                            workspace,
                            on_event,
                            &tool_call,
                            &runtime_state,
                        )
                        .await
                    {
                        Ok(Some(PlanningToolOutcome::ToolResult(result))) => {
                            let tool_message = db.add_visible_tool_result(
                                workspace_id,
                                &result,
                                &result,
                                Some(&tool_call.id),
                                Some(&tool_call.name),
                                Some("raw"),
                                &[],
                            )?;
                            emit(
                                on_event,
                                AgentEvent::ToolFinished {
                                    tool_call_id: Some(tool_call.id.clone()),
                                    name: tool_call.name.clone(),
                                    display_text: tool_message.content.clone(),
                                    result_mode: "raw".to_string(),
                                    detail_refs: Vec::new(),
                                },
                            );
                            continue;
                        }
                        Ok(Some(PlanningToolOutcome::WaitForUser(result))) => {
                            let tool_message = db.add_visible_tool_result(
                                workspace_id,
                                &result,
                                &result,
                                Some(&tool_call.id),
                                Some(&tool_call.name),
                                Some("raw"),
                                &[],
                            )?;
                            emit(
                                on_event,
                                AgentEvent::ToolFinished {
                                    tool_call_id: Some(tool_call.id.clone()),
                                    name: tool_call.name.clone(),
                                    display_text: tool_message.content.clone(),
                                    result_mode: "raw".to_string(),
                                    detail_refs: Vec::new(),
                                },
                            );
                            planning_waiting_message = Some(result);
                            continue;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            if is_retryable_tool_error(&tool_call.name, &error) {
                                self.emit_tool_retry_feedback(
                                    db,
                                    workspace_id,
                                    on_event,
                                    &tool_call,
                                    &error,
                                )?;
                                saw_retryable_tool_error = true;
                            } else {
                                self.emit_tool_error(
                                    db,
                                    workspace_id,
                                    on_event,
                                    &tool_call,
                                    &error,
                                )?;
                            }
                            continue;
                        }
                    }

                    match self.plan_protocol_action(
                        db,
                        workspace_id,
                        &tool_call,
                        &mut protocol_state,
                    ) {
                        Ok(Some(action)) => {
                            if let ProtocolToolAction::Exit { agent, .. } = &action {
                                self.mark_agent_exit_requested(workspace_id, agent.slug());
                            }
                            self.emit_protocol_action(
                                db,
                                workspace_id,
                                on_event,
                                &tool_call,
                                &action,
                            )?;
                            protocol_actions.push(action);
                            continue;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            if is_retryable_tool_error(&tool_call.name, &error) {
                                self.emit_tool_retry_feedback(
                                    db,
                                    workspace_id,
                                    on_event,
                                    &tool_call,
                                    &error,
                                )?;
                                saw_retryable_tool_error = true;
                            } else {
                                self.emit_tool_error(
                                    db,
                                    workspace_id,
                                    on_event,
                                    &tool_call,
                                    &error,
                                )?;
                            }
                            continue;
                        }
                    }

                    if is_retryable_tool_error(&tool_call.name, &result) {
                        self.emit_tool_retry_feedback(
                            db,
                            workspace_id,
                            on_event,
                            &tool_call,
                            &result,
                        )?;
                        saw_retryable_tool_error = true;
                        continue;
                    }

                    let summary_model = self.summary_model();
                    let tool_result = match prepare_tool_result(
                        &request_provider,
                        &summary_model,
                        &tool_call.name,
                        &tool_call.arguments,
                        &result,
                        |result_mode| {
                            emit(
                                on_event,
                                AgentEvent::ToolSummaryStarted {
                                    tool_call_id: Some(tool_call.id.clone()),
                                    name: tool_call.name.clone(),
                                    result_mode: result_mode.to_string(),
                                },
                            );
                        },
                        |delta| {
                            emit(
                                on_event,
                                AgentEvent::ToolSummaryDelta {
                                    tool_call_id: Some(tool_call.id.clone()),
                                    name: tool_call.name.clone(),
                                    delta: delta.to_string(),
                                    result_mode: "conservative_summary".to_string(),
                                },
                            );
                        },
                        |usage| {
                            record_session_token_usage(
                                db,
                                workspace_id,
                                &summary_model,
                                DispatcherSessionTokenUsageSource::Summary,
                                usage,
                            );
                        },
                    )
                    .await
                    {
                        Ok(summary) => summary,
                        Err(error) => {
                            debug_logger.log(
                                "工具结果摘要失败",
                                vec![
                                    ("工作区".to_string(), workspace_id.to_string()),
                                    ("轮次".to_string(), (iteration + 1).to_string()),
                                    ("工具名".to_string(), tool_call.name.clone()),
                                    ("工具调用ID".to_string(), tool_call.id.clone()),
                                ],
                                vec![
                                    DebugSection::new(
                                        "摘要调用",
                                        error.debug_context().to_string(),
                                    ),
                                    DebugSection::new("工具参数", tool_arguments.clone()),
                                    DebugSection::new("失败原因", error.message().to_string()),
                                ],
                            );
                            anyhow::bail!(
                                "工具结果摘要失败，tool={}, summary_model={}：{}",
                                tool_call.name,
                                summary_model,
                                error.message()
                            );
                        }
                    };

                    let tool_message = db.add_visible_tool_result(
                        workspace_id,
                        &tool_result.display_content,
                        &tool_result.context_payload,
                        Some(&tool_call.id),
                        Some(&tool_call.name),
                        Some(tool_result.result_mode),
                        &tool_result.artifacts,
                    )?;

                    emit(
                        on_event,
                        AgentEvent::ToolFinished {
                            tool_call_id: Some(tool_call.id.clone()),
                            name: tool_call.name.clone(),
                            display_text: tool_message.content.clone(),
                            result_mode: tool_result.result_mode.to_string(),
                            detail_refs: tool_message.tool_artifacts.clone(),
                        },
                    );

                    if let Err(error) = db.compact_successful_tool_retry(
                        workspace_id,
                        &tool_call.name,
                        &tool_call.id,
                    ) {
                        eprintln!(
                            "failed to compact dispatcher tool retry messages for workspace {} and tool {}: {}",
                            workspace_id, tool_call.name, error
                        );
                    }

                    if tool_call.name == "message" {
                        if let Some(content) = extract_message_content(&tool_call.arguments) {
                            final_message = Some(content);
                        }
                    }
                }
            }

            if saw_retryable_tool_error {
                continue;
            }

            if let Some(waiting_content) = planning_waiting_message {
                let waiting_msg =
                    db.add_visible_message(workspace_id, "assistant", &waiting_content)?;
                emit(
                    on_event,
                    AgentEvent::AssistantMessage {
                        message: waiting_msg.clone(),
                    },
                );
                return Ok(waiting_msg);
            }

            if !protocol_actions.is_empty() {
                let waiting_content = build_protocol_waiting_message(
                    &protocol_actions,
                    self.auto_approve_dispatch(),
                    final_message.as_deref(),
                );
                let waiting_msg =
                    db.add_visible_message(workspace_id, "assistant", &waiting_content)?;
                emit(
                    on_event,
                    AgentEvent::AssistantMessage {
                        message: waiting_msg.clone(),
                    },
                );
                return Ok(waiting_msg);
            }

            if let Some(final_message) = final_message {
                let reply = db.add_visible_message(workspace_id, "assistant", &final_message)?;
                emit(
                    on_event,
                    AgentEvent::AssistantMessage {
                        message: reply.clone(),
                    },
                );
                return Ok(reply);
            }
        }

        anyhow::bail!(
            "已达到最大工具迭代次数（{}），本轮执行被终止。请检查模型是否陷入工具调用循环或计划协议未收口。",
            self.config.max_tool_iterations
        )
    }

    fn build_system_prompt(&self) -> Result<PromptBundle> {
        build_system_prompt(&self.config.root_dir)
    }

    fn build_system_prompt_for_workspace(
        &self,
        workspace_id: &str,
        workspace: &Path,
        tool_definitions: &[crate::agent::llm::ToolDefinition],
        runtime_state: &DispatcherSessionRuntimeState,
    ) -> Result<SystemPromptSnapshot> {
        let mut prompt_bundle = self.build_system_prompt()?;
        let mode_block = build_dispatcher_mode_block(runtime_state);
        prompt_bundle.sections.push(PromptSection {
            label: "Runtime Planning Mode".to_string(),
            source: "runtime::planning_mode".to_string(),
            content: mode_block.clone(),
        });
        prompt_bundle.content.push_str("\n\n---\n\n");
        prompt_bundle.content.push_str(&mode_block);
        let tool_block = render_available_tools_block(tool_definitions);
        if !tool_block.is_empty() {
            prompt_bundle.sections.push(PromptSection {
                label: "Runtime Tool State".to_string(),
                source: "runtime::available_tools".to_string(),
                content: tool_block.clone(),
            });
            prompt_bundle.content.push_str("\n\n---\n\n");
            prompt_bundle.content.push_str(&tool_block);
        }
        let state_block = self.build_subprocess_state_block(workspace_id);
        if !state_block.is_empty() {
            prompt_bundle.sections.push(PromptSection {
                label: "Runtime Subprocess State".to_string(),
                source: "runtime::subprocess_state".to_string(),
                content: state_block.clone(),
            });
            prompt_bundle.content.push_str("\n\n---\n\n");
            prompt_bundle.content.push_str(&state_block);
        }
        let mcp_block = build_workspace_mcp_prompt_block(
            self.project_mcp_registry
                .cached_for_workspace(workspace)
                .as_ref(),
            workspace,
        );
        if !mcp_block.is_empty() {
            prompt_bundle.sections.push(PromptSection {
                label: "Workspace MCP State".to_string(),
                source: "runtime::workspace_mcp".to_string(),
                content: mcp_block.clone(),
            });
            prompt_bundle.content.push_str("\n\n---\n\n");
            prompt_bundle.content.push_str(&mcp_block);
        }
        Ok(SystemPromptSnapshot {
            rendered: prompt_bundle.content,
        })
    }

    fn build_subprocess_state_block(&self, workspace_id: &str) -> String {
        let subprocesses = self.active_subprocesses_for_workspace(workspace_id);
        if subprocesses.is_empty() {
            return String::new();
        }

        let mut lines = vec![
            "# 当前子进程运行态".to_string(),
            "以下状态是系统权威状态，不要用聊天历史猜测：".to_string(),
        ];

        for subprocess in &subprocesses {
            lines.push(format!(
                "- agent={} dispatch_id={} task_id={} phase={} task={}",
                subprocess.agent,
                subprocess.dispatch_id,
                subprocess.task_id,
                subprocess_phase_label(subprocess.phase),
                truncate_for_display(&subprocess.description, 120, "...")
            ));
        }

        lines.push(
            "规则：如果某个 agent 已有 active subprocess，则禁止再次调用同 agent 的 dispatch_*。"
                .to_string(),
        );
        lines.push(
            "规则：phase=round_completed 时，只能在 continue_* / exit_* / 直接回复用户 之间选择。"
                .to_string(),
        );
        lines.push(
            "规则：phase=stopped 时，说明子进程已被 UI 手动停止但会话仍可恢复；此时不要继续 dispatch/continue/exit，而是先让用户决定是否恢复。"
                .to_string(),
        );
        lines.push(
            "规则：phase=exit_requested 时，不要再次调用该 agent 的 dispatch_* / continue_* / exit_*，只等待进程结束。"
                .to_string(),
        );

        lines.join("\n")
    }

    fn build_subprocess_task_prompt(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        _agent: DispatchAgent,
        task_description: &str,
    ) -> std::result::Result<String, String> {
        let history = db
            .load_llm_history(workspace_id)
            .map_err(|error| format!("读取调度历史失败：{error}"))?;
        let latest_user_goal = history
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(|message| compact_multiline(message.content.trim(), 240))
            .filter(|text| !text.is_empty());
        let explored_index_info = collect_recent_exploration_entries(&history);
        let active_plan_path = db
            .get_session_runtime_state(workspace_id)
            .map_err(|error| format!("读取调度运行态失败：{error}"))?
            .active_plan_path
            .filter(|path| !is_implemented_plan_path(Path::new(path)));

        let mut sections = vec![format!("【任务目标】\n{}", task_description.trim())];

        if let Some(plan_path) = active_plan_path {
            sections.push(format!(
                "【计划书】\n请先读取并严格按照该计划书执行编码任务：{plan_path}"
            ));
        }

        if let Some(goal) =
            latest_user_goal.filter(|goal| should_include_latest_user_goal(goal, task_description))
        {
            sections.push(format!("【用户诉求】\n{}", goal));
        }

        if !explored_index_info.is_empty() {
            sections.push(format!("【已确认上下文】\n{}", explored_index_info));
        }

        sections.push(
            "【执行要求】\n\
- 优先直接完成目标；只有在上下文不足或与代码现场冲突时，才补做最少量验证。\n\
- 输出聚焦：实际改动或结论、验证结果、剩余风险；默认使用简体中文。"
                .to_string(),
        );

        Ok(sections.join("\n\n"))
    }

    fn tool_definitions_for_workspace(
        &self,
        workspace_id: &str,
        workspace: &Path,
        runtime_state: &DispatcherSessionRuntimeState,
    ) -> Vec<crate::agent::llm::ToolDefinition> {
        let mut allowed = match runtime_state.mode {
            DispatcherMode::Default => default_mode_tool_allowlist(),
            DispatcherMode::Plan => plan_mode_tool_allowlist(),
        };

        let include_dynamic = runtime_state.mode == DispatcherMode::Default;

        if runtime_state.mode == DispatcherMode::Default {
            if runtime_state
                .active_plan_path
                .as_deref()
                .is_some_and(|path| !is_implemented_plan_path(Path::new(path)))
            {
                allowed.insert("mark_plan_implemented");
            }

            for (agent_slug, has_active, phase) in self.agent_runtime_flags(workspace_id) {
                match (has_active, phase) {
                    (false, _) => {
                        allowed.insert(dispatch_tool_name(agent_slug));
                    }
                    (true, Some(RegisteredSubprocessPhase::Running))
                    | (true, Some(RegisteredSubprocessPhase::RoundCompleted)) => {
                        allowed.insert(continue_tool_name(agent_slug));
                        allowed.insert(exit_tool_name(agent_slug));
                    }
                    (true, Some(RegisteredSubprocessPhase::Stopped)) => {}
                    (true, Some(RegisteredSubprocessPhase::ExitRequested)) => {}
                    (true, None) => {}
                }
            }
        }

        self.tools
            .definitions_for_workspace(workspace, Some(allowed.into_iter()), include_dynamic)
    }

    fn active_subprocesses_for_workspace(&self, workspace_id: &str) -> Vec<RegisteredSubprocess> {
        self.subprocesses
            .subprocesses
            .lock()
            .iter()
            .filter(|item| item.workspace_id == workspace_id)
            .cloned()
            .collect()
    }

    fn agent_runtime_flags(
        &self,
        workspace_id: &str,
    ) -> Vec<(&'static str, bool, Option<RegisteredSubprocessPhase>)> {
        ["claude", "codex"]
            .into_iter()
            .map(|agent| {
                let entry = self
                    .active_subprocesses_for_workspace(workspace_id)
                    .into_iter()
                    .find(|item| item.agent == agent);
                let phase = entry.as_ref().map(|item| item.phase);
                (agent, entry.is_some(), phase)
            })
            .collect()
    }

    fn mark_agent_exit_requested(&self, workspace_id: &str, agent: &str) {
        let mut subprocesses = self.subprocesses.subprocesses.lock();
        if let Some(item) = subprocesses
            .iter_mut()
            .find(|item| item.workspace_id == workspace_id && item.agent == agent)
        {
            item.phase = RegisteredSubprocessPhase::ExitRequested;
        }
    }

    async fn execute_parallel_readonly_tools(
        &self,
        tool_calls: &[RequestedToolCall],
        tool_context: &ToolContext,
        on_event: &Channel<AgentEvent>,
        allowed_tool_names: &HashSet<String>,
    ) -> Vec<String> {
        for tool_call in tool_calls {
            emit(
                on_event,
                AgentEvent::ToolStarted {
                    tool_call_id: Some(tool_call.id.clone()),
                    name: tool_call.name.clone(),
                    arguments: serde_json::to_string(&tool_call.arguments)
                        .unwrap_or_else(|_| "{}".to_string()),
                },
            );
        }

        let results = join_all(tool_calls.iter().map(|tool_call| async move {
            if allowed_tool_names.contains(&tool_call.name) {
                self.tools
                    .execute(&tool_call.name, &tool_call.arguments, tool_context)
                    .await
            } else {
                disallowed_tool_result(&tool_call.name)
            }
        }))
        .await;

        results
    }

    async fn execute_update_plan_calls_first(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &Path,
        on_event: &Channel<AgentEvent>,
        tool_calls: &[RequestedToolCall],
        runtime_state: &DispatcherSessionRuntimeState,
        allowed_tool_names: &HashSet<String>,
    ) -> Result<HashSet<String>> {
        let mut processed = HashSet::new();
        for tool_call in tool_calls
            .iter()
            .filter(|tool_call| tool_call.name == "update_plan")
        {
            if !allowed_tool_names.contains(&tool_call.name) {
                continue;
            }
            emit(
                on_event,
                AgentEvent::ToolStarted {
                    tool_call_id: Some(tool_call.id.clone()),
                    name: tool_call.name.clone(),
                    arguments: serde_json::to_string(&tool_call.arguments)
                        .unwrap_or_else(|_| "{}".to_string()),
                },
            );
            match self
                .execute_planning_tool(
                    db,
                    workspace_id,
                    workspace,
                    on_event,
                    tool_call,
                    runtime_state,
                )
                .await
            {
                Ok(Some(PlanningToolOutcome::ToolResult(result)))
                | Ok(Some(PlanningToolOutcome::WaitForUser(result))) => {
                    let tool_message = db.add_visible_tool_result(
                        workspace_id,
                        &result,
                        &result,
                        Some(&tool_call.id),
                        Some(&tool_call.name),
                        Some("raw"),
                        &[],
                    )?;
                    emit(
                        on_event,
                        AgentEvent::ToolFinished {
                            tool_call_id: Some(tool_call.id.clone()),
                            name: tool_call.name.clone(),
                            display_text: tool_message.content.clone(),
                            result_mode: "raw".to_string(),
                            detail_refs: Vec::new(),
                        },
                    );
                    processed.insert(tool_call.id.clone());
                }
                Ok(None) => {}
                Err(error) => {
                    self.emit_tool_error(db, workspace_id, on_event, tool_call, &error)?;
                    processed.insert(tool_call.id.clone());
                }
            }
        }
        Ok(processed)
    }

    async fn execute_planning_tool(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &Path,
        on_event: &Channel<AgentEvent>,
        tool_call: &super::llm::RequestedToolCall,
        runtime_state: &DispatcherSessionRuntimeState,
    ) -> Result<Option<PlanningToolOutcome>, String> {
        match tool_call.name.as_str() {
            "update_plan" => {
                ensure_mode(runtime_state.mode, DispatcherMode::Default, "update_plan")?;
                let draft = parse_update_plan(&tool_call.arguments)?;
                let latest_state = db
                    .get_session_runtime_state(workspace_id)
                    .map_err(|error| error.to_string())?;
                let checklist = build_checklist_state(draft, latest_state.checklist.as_ref())?;
                db.update_checklist(workspace_id, &checklist)
                    .map_err(|error| error.to_string())?;
                emit(
                    on_event,
                    AgentEvent::ChecklistPlanUpdated {
                        state: checklist.clone(),
                    },
                );
                Ok(Some(PlanningToolOutcome::ToolResult(format!(
                    "Checklist 已更新：{} 个步骤",
                    checklist.items.len()
                ))))
            }
            "ask_plan_question" => {
                ensure_mode(
                    runtime_state.mode,
                    DispatcherMode::Plan,
                    "ask_plan_question",
                )?;
                let draft = parse_ask_plan_question(&tool_call.arguments)?;
                let interaction = PlanInteraction::Question {
                    id: uuid::Uuid::new_v4().to_string(),
                    question: draft.question,
                    options: draft
                        .options
                        .into_iter()
                        .enumerate()
                        .map(|(index, option)| PlanQuestionOption {
                            id: option.id.unwrap_or_else(|| format!("option_{}", index + 1)),
                            label: option.label,
                            description: option.description,
                        })
                        .collect(),
                };
                db.set_plan_interaction(workspace_id, Some(&interaction))
                    .map_err(|error| error.to_string())?;
                emit(
                    on_event,
                    AgentEvent::PlanQuestionRequested {
                        interaction: interaction.clone(),
                    },
                );
                Ok(Some(PlanningToolOutcome::WaitForUser(
                    "规划信息不足，已向用户提出一个问题。".to_string(),
                )))
            }
            "create_plan_document" => {
                ensure_mode(
                    runtime_state.mode,
                    DispatcherMode::Plan,
                    "create_plan_document",
                )?;
                let (title, content) = parse_create_plan_document(&tool_call.arguments)?;
                let plan_path = create_plan_document(workspace, &title, &content).await?;
                let plan_path = plan_path.to_string_lossy().to_string();
                db.set_active_plan_path(workspace_id, Some(&plan_path))
                    .map_err(|error| error.to_string())?;
                emit(
                    on_event,
                    AgentEvent::PlanDocumentOpened {
                        plan_path: plan_path.clone(),
                    },
                );
                Ok(Some(PlanningToolOutcome::ToolResult(format!(
                    "计划书已创建：{plan_path}"
                ))))
            }
            "read_plan_document" => {
                ensure_mode(
                    runtime_state.mode,
                    DispatcherMode::Plan,
                    "read_plan_document",
                )?;
                let path = string_arg_required(&tool_call.arguments, "path")?;
                let content = read_plan_document(workspace, &path).await?;
                Ok(Some(PlanningToolOutcome::ToolResult(content)))
            }
            "replace_plan_document" => {
                ensure_mode(
                    runtime_state.mode,
                    DispatcherMode::Plan,
                    "replace_plan_document",
                )?;
                let (path, content) = parse_replace_plan_document(&tool_call.arguments)?;
                let plan_path = replace_plan_document(workspace, &path, &content).await?;
                let plan_path = plan_path.to_string_lossy().to_string();
                db.set_active_plan_path(workspace_id, Some(&plan_path))
                    .map_err(|error| error.to_string())?;
                emit(
                    on_event,
                    AgentEvent::PlanDocumentOpened {
                        plan_path: plan_path.clone(),
                    },
                );
                Ok(Some(PlanningToolOutcome::ToolResult(format!(
                    "计划书已替换：{plan_path}"
                ))))
            }
            "edit_plan_document" => {
                ensure_mode(
                    runtime_state.mode,
                    DispatcherMode::Plan,
                    "edit_plan_document",
                )?;
                let (path, old_text, new_text, replace_all) =
                    parse_edit_plan_document(&tool_call.arguments)?;
                let plan_path =
                    edit_plan_document(workspace, &path, &old_text, &new_text, replace_all).await?;
                let plan_path = plan_path.to_string_lossy().to_string();
                db.set_active_plan_path(workspace_id, Some(&plan_path))
                    .map_err(|error| error.to_string())?;
                emit(
                    on_event,
                    AgentEvent::PlanDocumentOpened {
                        plan_path: plan_path.clone(),
                    },
                );
                Ok(Some(PlanningToolOutcome::ToolResult(format!(
                    "计划书已编辑：{plan_path}"
                ))))
            }
            "present_plan" => {
                ensure_mode(runtime_state.mode, DispatcherMode::Plan, "present_plan")?;
                let (path, title, summary) = parse_present_plan(&tool_call.arguments)?;
                let plan_path =
                    resolve_plan_path_async(workspace, &path, PlanPathAccess::Read).await?;
                let plan_path = plan_path.to_string_lossy().to_string();
                let interaction = PlanInteraction::Ready {
                    plan_path: plan_path.clone(),
                    title,
                    summary,
                };
                db.set_active_plan_path(workspace_id, Some(&plan_path))
                    .map_err(|error| error.to_string())?;
                db.set_plan_interaction(workspace_id, Some(&interaction))
                    .map_err(|error| error.to_string())?;
                emit(
                    on_event,
                    AgentEvent::PlanReady {
                        interaction: interaction.clone(),
                    },
                );
                Ok(Some(PlanningToolOutcome::WaitForUser(
                    "计划书已完成，等待用户选择实施方式。".to_string(),
                )))
            }
            "mark_plan_implemented" => {
                ensure_mode(
                    runtime_state.mode,
                    DispatcherMode::Default,
                    "mark_plan_implemented",
                )?;
                let path = string_arg_required(&tool_call.arguments, "path")?;
                let summary = string_arg_required(&tool_call.arguments, "summary")?;
                let (original, implemented) = mark_plan_implemented(workspace, &path).await?;
                let original = original.to_string_lossy().to_string();
                let implemented = implemented.to_string_lossy().to_string();
                db.set_active_plan_path(workspace_id, Some(&implemented))
                    .map_err(|error| error.to_string())?;
                db.set_plan_interaction(workspace_id, None)
                    .map_err(|error| error.to_string())?;
                emit(
                    on_event,
                    AgentEvent::PlanImplemented {
                        plan_path: original.clone(),
                        implemented_path: implemented.clone(),
                        summary: summary.clone(),
                    },
                );
                Ok(Some(PlanningToolOutcome::ToolResult(format!(
                    "计划已标记为已实现：{implemented}\n实施摘要：{summary}"
                ))))
            }
            _ => Ok(None),
        }
    }

    fn plan_protocol_action(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        tool_call: &super::llm::RequestedToolCall,
        protocol_state: &mut ProtocolBatchState,
    ) -> std::result::Result<Option<ProtocolToolAction>, String> {
        if let Some(agent) = DispatchAgent::from_dispatch_tool_name(&tool_call.name) {
            protocol_state.ensure_dispatch_allowed(agent.slug(), agent.display_name())?;
            let (task_description, permission_mode) =
                parse_dispatch_instruction(&tool_call.arguments, agent)?;
            let dispatch_id = uuid::Uuid::new_v4().to_string();
            let description = summarize_dispatch_description(&task_description);
            let task_prompt =
                self.build_subprocess_task_prompt(db, workspace_id, agent, &task_description)?;
            protocol_state.record_dispatch(agent.slug(), &dispatch_id);
            return Ok(Some(ProtocolToolAction::Dispatch {
                dispatch_id,
                agent,
                description,
                task_prompt,
                permission_mode,
            }));
        }

        if let Some(agent) = DispatchAgent::from_continue_tool_name(&tool_call.name) {
            protocol_state.ensure_continue_allowed(agent.slug(), agent.display_name())?;
            let text = parse_continue_instruction(&tool_call.arguments, agent)?;
            let dispatch_id = protocol_state
                .dispatch_id_for_agent(agent.slug())
                .unwrap_or("active")
                .to_string();
            protocol_state.record_continue(agent.slug());
            return Ok(Some(ProtocolToolAction::Continue {
                dispatch_id,
                agent,
                text,
            }));
        }

        if let Some(agent) = DispatchAgent::from_exit_tool_name(&tool_call.name) {
            protocol_state.ensure_exit_allowed(agent.slug(), agent.display_name())?;
            let reason = parse_exit_instruction(&tool_call.arguments, agent);
            let dispatch_id = protocol_state
                .dispatch_id_for_agent(agent.slug())
                .unwrap_or("active")
                .to_string();
            protocol_state.record_exit(agent.slug());
            return Ok(Some(ProtocolToolAction::Exit {
                dispatch_id,
                agent,
                reason,
            }));
        }

        Ok(None)
    }

    fn emit_protocol_action(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        tool_call: &super::llm::RequestedToolCall,
        action: &ProtocolToolAction,
    ) -> Result<()> {
        let result = match action {
            ProtocolToolAction::Dispatch {
                dispatch_id,
                agent,
                description,
                task_prompt,
                permission_mode,
            } => {
                if let Some(checklist) = reserve_checklist_dispatch(
                    db,
                    workspace_id,
                    dispatch_id,
                    agent.slug(),
                    description,
                )
                .map_err(anyhow::Error::msg)?
                {
                    emit(
                        on_event,
                        AgentEvent::ChecklistPlanUpdated { state: checklist },
                    );
                }
                emit(
                    on_event,
                    AgentEvent::DispatchProposed {
                        dispatch_id: dispatch_id.clone(),
                        agent: agent.slug().to_string(),
                        description: description.clone(),
                        task_prompt: task_prompt.clone(),
                        permission_mode: permission_mode.clone(),
                    },
                );

                format!(
                    "[{} 子任务已提交审查] dispatch_id={}, 任务: {}",
                    agent.display_name(),
                    dispatch_id,
                    truncate_for_display(description, 200, "...")
                )
            }
            ProtocolToolAction::Continue {
                dispatch_id,
                agent,
                text,
            } => {
                emit(
                    on_event,
                    AgentEvent::DispatchContinue {
                        dispatch_id: dispatch_id.clone(),
                        agent: agent.slug().to_string(),
                        text: text.clone(),
                    },
                );

                format!(
                    "[已发送后续指令到 {} 会话] 指令: {}",
                    agent.display_name(),
                    truncate_for_display(text, 200, "...")
                )
            }
            ProtocolToolAction::Exit {
                dispatch_id,
                agent,
                reason,
            } => {
                emit(
                    on_event,
                    AgentEvent::DispatchExit {
                        dispatch_id: dispatch_id.clone(),
                        agent: agent.slug().to_string(),
                        reason: reason.clone(),
                    },
                );

                format!(
                    "[已发送退出命令到 {} 会话] 原因: {}",
                    agent.display_name(),
                    reason
                )
            }
        };

        emit(
            on_event,
            AgentEvent::ToolFinished {
                tool_call_id: Some(tool_call.id.clone()),
                name: tool_call.name.clone(),
                display_text: result.clone(),
                result_mode: "raw".to_string(),
                detail_refs: Vec::new(),
            },
        );
        db.add_visible_message_with_tools(
            workspace_id,
            "tool",
            &result,
            Some(&tool_call.id),
            Some(&tool_call.name),
            Some("raw"),
            None,
        )?;
        if let Err(error) =
            db.compact_successful_tool_retry(workspace_id, &tool_call.name, &tool_call.id)
        {
            eprintln!(
                "failed to compact dispatcher protocol retry messages for workspace {} and tool {}: {}",
                workspace_id, tool_call.name, error
            );
        }
        Ok(())
    }

    fn emit_tool_retry_feedback(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        tool_call: &super::llm::RequestedToolCall,
        error: &str,
    ) -> Result<()> {
        let context_payload = build_tool_retry_context(tool_call, error);
        let display_text = "工具调用参数需要修正，已交回模型重试。";
        emit(
            on_event,
            AgentEvent::ToolFinished {
                tool_call_id: Some(tool_call.id.clone()),
                name: tool_call.name.clone(),
                display_text: display_text.to_string(),
                result_mode: "raw".to_string(),
                detail_refs: Vec::new(),
            },
        );
        db.add_visible_tool_result(
            workspace_id,
            display_text,
            &context_payload,
            Some(&tool_call.id),
            Some(&tool_call.name),
            Some("raw"),
            &[],
        )?;
        Ok(())
    }

    fn emit_tool_error(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        tool_call: &super::llm::RequestedToolCall,
        error: &str,
    ) -> Result<()> {
        emit(
            on_event,
            AgentEvent::ToolFinished {
                tool_call_id: Some(tool_call.id.clone()),
                name: tool_call.name.clone(),
                display_text: error.to_string(),
                result_mode: "raw".to_string(),
                detail_refs: Vec::new(),
            },
        );
        db.add_visible_message_with_tools(
            workspace_id,
            "tool",
            error,
            Some(&tool_call.id),
            Some(&tool_call.name),
            Some("raw"),
            None,
        )?;
        Ok(())
    }

    fn emit_stop_and_finish(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        partial: &str,
    ) -> Result<DispatcherMessageRecord> {
        let content = build_stopped_dispatch_reply(partial);
        let reply = db.add_visible_message(workspace_id, "assistant", &content)?;
        emit(
            on_event,
            AgentEvent::AssistantMessage {
                message: reply.clone(),
            },
        );
        Ok(reply)
    }
}

fn ensure_mode(
    actual: DispatcherMode,
    expected: DispatcherMode,
    tool_name: &str,
) -> std::result::Result<(), String> {
    if actual == expected {
        return Ok(());
    }
    let expected = match expected {
        DispatcherMode::Default => "Default",
        DispatcherMode::Plan => "Plan",
    };
    Err(format!("错误：{tool_name} 只能在 {expected} 模式下使用"))
}

fn build_checklist_state(
    draft: UpdatePlanDraft,
    previous: Option<&ChecklistPlanState>,
) -> std::result::Result<ChecklistPlanState, String> {
    if draft.items.is_empty() {
        return Err("错误：plan 至少需要包含一个步骤".to_string());
    }

    let mut in_progress_count = 0usize;
    let items = draft
        .items
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let status = ChecklistStepStatus::from_wire(&item.status)
                .map_err(|error| format!("错误：{}", error))?;
            if status == ChecklistStepStatus::InProgress {
                in_progress_count += 1;
            }
            let previous_item = previous.and_then(|state| {
                state.items.iter().find(|candidate| {
                    item.id
                        .as_deref()
                        .is_some_and(|id| candidate.id.as_deref() == Some(id))
                        || candidate.step == item.step
                })
            });
            let agent = match item.agent {
                Some(agent) => {
                    let normalized = agent.trim().to_ascii_lowercase();
                    if !matches!(normalized.as_str(), "claude" | "codex") {
                        return Err(format!("错误：不支持的 checklist agent：{agent}"));
                    }
                    Some(normalized)
                }
                None => previous_item.and_then(|item| item.agent.clone()),
            };
            Ok(ChecklistPlanItem {
                id: item
                    .id
                    .or_else(|| previous_item.and_then(|item| item.id.clone()))
                    .or_else(|| Some(format!("step_{}", index + 1))),
                step: item.step,
                status,
                agent,
                dispatch_id: previous_item.and_then(|item| item.dispatch_id.clone()),
                subprocess_task_id: previous_item.and_then(|item| item.subprocess_task_id.clone()),
                detail: previous_item.and_then(|item| item.detail.clone()),
            })
        })
        .collect::<std::result::Result<Vec<_>, String>>()?;

    if in_progress_count > 1 {
        return Err(format!(
            "错误：同一时间最多只能有 1 个 in_progress 步骤，实际收到 {in_progress_count} 个"
        ));
    }

    Ok(ChecklistPlanState {
        explanation: draft.explanation,
        items,
        updated_at: Utc::now().to_rfc3339(),
    })
}

fn empty_checklist_state() -> ChecklistPlanState {
    ChecklistPlanState {
        explanation: None,
        items: Vec::new(),
        updated_at: Utc::now().to_rfc3339(),
    }
}

fn reserve_checklist_dispatch(
    db: &DispatcherDb,
    session_id: &str,
    dispatch_id: &str,
    agent: &str,
    description: &str,
) -> std::result::Result<Option<ChecklistPlanState>, String> {
    let mut state = db
        .get_session_runtime_state(session_id)
        .map_err(|error| error.to_string())?;
    let Some(mut checklist) = state.checklist.take() else {
        return Ok(None);
    };
    if checklist.items.is_empty() {
        return Ok(None);
    }

    if checklist.items.iter().any(|item| {
        item.status == ChecklistStepStatus::InProgress
            && item
                .dispatch_id
                .as_deref()
                .is_some_and(|existing| existing != dispatch_id)
    }) {
        return Err("错误：Checklist 当前已有运行中的子步骤，请等待该子步骤完成后再启动下一个子 Agent 任务。".to_string());
    }

    let item_index = checklist
        .items
        .iter()
        .position(|item| item.status == ChecklistStepStatus::InProgress)
        .or_else(|| {
            checklist.items.iter().position(|item| {
                item.status == ChecklistStepStatus::Pending
                    && item
                        .agent
                        .as_deref()
                        .is_none_or(|preferred| preferred == agent)
            })
        })
        .or_else(|| {
            checklist
                .items
                .iter()
                .position(|item| item.status == ChecklistStepStatus::Pending)
        });

    let Some(index) = item_index else {
        return Ok(None);
    };

    let item = &mut checklist.items[index];
    item.agent = Some(agent.to_string());
    item.dispatch_id = Some(dispatch_id.to_string());
    item.detail = Some(description.to_string());
    checklist.updated_at = Utc::now().to_rfc3339();

    let state = db
        .update_checklist(session_id, &checklist)
        .map_err(|error| error.to_string())?;
    Ok(state.checklist)
}

fn start_checklist_dispatch(
    db: &DispatcherDb,
    session_id: &str,
    dispatch_id: &str,
) -> std::result::Result<Option<ChecklistPlanState>, String> {
    let state = db
        .get_session_runtime_state(session_id)
        .map_err(|error| error.to_string())?;
    let Some(mut checklist) = state.checklist else {
        return Ok(None);
    };

    let Some(item) = checklist
        .items
        .iter_mut()
        .find(|item| item.dispatch_id.as_deref() == Some(dispatch_id))
    else {
        return Ok(None);
    };

    item.status = ChecklistStepStatus::InProgress;
    checklist.updated_at = Utc::now().to_rfc3339();
    let state = db
        .update_checklist(session_id, &checklist)
        .map_err(|error| error.to_string())?;
    Ok(state.checklist)
}

fn complete_checklist_dispatch(
    db: &DispatcherDb,
    session_id: &str,
    dispatch_id: &str,
) -> std::result::Result<Option<ChecklistPlanState>, String> {
    let state = db
        .get_session_runtime_state(session_id)
        .map_err(|error| error.to_string())?;
    let Some(mut checklist) = state.checklist else {
        return Ok(None);
    };

    let Some(item) = checklist
        .items
        .iter_mut()
        .find(|item| item.dispatch_id.as_deref() == Some(dispatch_id))
    else {
        return Ok(None);
    };

    item.status = ChecklistStepStatus::Completed;
    checklist.updated_at = Utc::now().to_rfc3339();
    let state = db
        .update_checklist(session_id, &checklist)
        .map_err(|error| error.to_string())?;
    Ok(state.checklist)
}

#[derive(Clone, Copy)]
enum PlanPathAccess {
    Read,
    WriteExisting,
}

async fn resolve_plan_path_async(
    workspace: &Path,
    raw_path: &str,
    access: PlanPathAccess,
) -> std::result::Result<PathBuf, String> {
    let workspace = workspace.to_path_buf();
    let raw_path = raw_path.to_string();
    tokio::task::spawn_blocking(move || resolve_plan_path(&workspace, &raw_path, access))
        .await
        .map_err(|error| format!("计划路径解析任务失败：{error}"))?
}

async fn create_plan_document(
    workspace: &Path,
    title: &str,
    content: &str,
) -> std::result::Result<PathBuf, String> {
    let workspace = workspace.to_path_buf();
    let title = title.to_string();
    let content = content.to_string();
    tokio::task::spawn_blocking(move || {
        let root = ensure_plan_root(&workspace)?;
        let filename = format!(
            "{}-{}.md",
            Utc::now().format("%Y%m%d-%H%M%S"),
            slugify_plan_title(&title)
        );
        let path = root.join(filename);
        fs::write(&path, content).map_err(|error| format!("写入计划书失败：{error}"))?;
        Ok(path)
    })
    .await
    .map_err(|error| format!("创建计划书任务失败：{error}"))?
}

async fn read_plan_document(workspace: &Path, path: &str) -> std::result::Result<String, String> {
    let plan_path = resolve_plan_path_async(workspace, path, PlanPathAccess::Read).await?;
    tokio::task::spawn_blocking(move || {
        let mut content = String::new();
        fs::File::open(&plan_path)
            .map_err(|error| format!("打开计划书失败：{error}"))?
            .read_to_string(&mut content)
            .map_err(|error| format!("读取计划书失败：{error}"))?;
        Ok(content)
    })
    .await
    .map_err(|error| format!("读取计划书任务失败：{error}"))?
}

async fn replace_plan_document(
    workspace: &Path,
    path: &str,
    content: &str,
) -> std::result::Result<PathBuf, String> {
    let plan_path = resolve_plan_path_async(workspace, path, PlanPathAccess::WriteExisting).await?;
    let content = content.to_string();
    tokio::task::spawn_blocking(move || {
        fs::write(&plan_path, content).map_err(|error| format!("替换计划书失败：{error}"))?;
        Ok(plan_path)
    })
    .await
    .map_err(|error| format!("替换计划书任务失败：{error}"))?
}

async fn edit_plan_document(
    workspace: &Path,
    path: &str,
    old_text: &str,
    new_text: &str,
    replace_all: bool,
) -> std::result::Result<PathBuf, String> {
    let plan_path = resolve_plan_path_async(workspace, path, PlanPathAccess::WriteExisting).await?;
    let old_text = old_text.to_string();
    let new_text = new_text.to_string();
    tokio::task::spawn_blocking(move || {
        let content =
            fs::read_to_string(&plan_path).map_err(|error| format!("读取计划书失败：{error}"))?;
        if !content.contains(&old_text) {
            return Err("错误：计划书中未找到 old_text".to_string());
        }
        let match_count = content.matches(&old_text).count();
        if match_count > 1 && !replace_all {
            return Err(format!(
                "错误：old_text 命中 {match_count} 处，请补充上下文或设置 replace_all=true"
            ));
        }
        let updated = if replace_all {
            content.replace(&old_text, &new_text)
        } else {
            content.replacen(&old_text, &new_text, 1)
        };
        fs::write(&plan_path, updated).map_err(|error| format!("编辑计划书失败：{error}"))?;
        Ok(plan_path)
    })
    .await
    .map_err(|error| format!("编辑计划书任务失败：{error}"))?
}

async fn mark_plan_implemented(
    workspace: &Path,
    path: &str,
) -> std::result::Result<(PathBuf, PathBuf), String> {
    let plan_path = resolve_plan_path_async(workspace, path, PlanPathAccess::WriteExisting).await?;
    tokio::task::spawn_blocking(move || {
        if is_implemented_plan_path(&plan_path) {
            return Err("错误：该计划书已经带有 -已实现.md 标记".to_string());
        }
        let file_name = plan_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "错误：计划书文件名不是有效 UTF-8".to_string())?;
        let implemented_name = match file_name.strip_suffix(".md") {
            Some(stem) => format!("{stem}-已实现.md"),
            None => format!("{file_name}-已实现.md"),
        };
        let implemented_path = plan_path.with_file_name(implemented_name);
        if implemented_path.exists() {
            return Err(format!(
                "错误：目标已存在，拒绝覆盖：{}",
                implemented_path.display()
            ));
        }
        fs::rename(&plan_path, &implemented_path)
            .map_err(|error| format!("重命名计划书失败：{error}"))?;
        Ok((plan_path, implemented_path))
    })
    .await
    .map_err(|error| format!("标记计划已实现任务失败：{error}"))?
}

fn ensure_plan_root(workspace: &Path) -> std::result::Result<PathBuf, String> {
    let root = workspace.join(".jkcodingagent").join("plan");
    fs::create_dir_all(&root).map_err(|error| format!("创建计划目录失败：{error}"))?;
    root.canonicalize()
        .map_err(|error| format!("解析计划目录失败：{error}"))
}

fn resolve_plan_path(
    workspace: &Path,
    raw_path: &str,
    access: PlanPathAccess,
) -> std::result::Result<PathBuf, String> {
    let root = ensure_plan_root(workspace)?;
    let raw = PathBuf::from(raw_path);
    let candidate = if raw.is_absolute() {
        raw
    } else {
        workspace.join(raw)
    };
    let candidate = lexical_normalize_path(&candidate);
    let resolved = match access {
        PlanPathAccess::Read => candidate
            .canonicalize()
            .map_err(|error| format!("解析计划书路径失败：{error}"))?,
        PlanPathAccess::WriteExisting => {
            if is_implemented_plan_path(&candidate) {
                return Err("错误：禁止修改文件名包含 -已实现.md 的计划书".to_string());
            }
            candidate
                .canonicalize()
                .map_err(|error| format!("解析计划书路径失败：{error}"))?
        }
    };

    if !resolved.starts_with(&root) {
        return Err(format!(
            "错误：计划书路径必须位于项目计划目录内：{}",
            root.display()
        ));
    }
    if matches!(access, PlanPathAccess::WriteExisting) && is_implemented_plan_path(&resolved) {
        return Err("错误：禁止修改文件名包含 -已实现.md 的计划书".to_string());
    }
    Ok(resolved)
}

fn lexical_normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn is_implemented_plan_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains("-已实现.md"))
}

fn slugify_plan_title(title: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in title.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "plan".to_string()
    } else {
        slug.chars().take(48).collect()
    }
}

fn string_arg_required(args: &Value, key: &str) -> std::result::Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("错误：缺少必填参数 {key}，且不能为空"))
}

fn extract_message_content(arguments: &Value) -> Option<String> {
    arguments
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn is_parallel_readonly_tool_call(tool_call: &RequestedToolCall) -> bool {
    matches!(
        tool_call.name.as_str(),
        "read_file" | "list_dir" | "glob" | "grep"
    )
}

fn readonly_tool_run_end(tool_calls: &[RequestedToolCall], start: usize) -> usize {
    tool_calls
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, tool_call)| {
            (!is_parallel_readonly_tool_call(tool_call)).then_some(index)
        })
        .unwrap_or(tool_calls.len())
}

fn disallowed_tool_result(tool_name: &str) -> String {
    format!(
        "错误：禁止调用工具 '{tool_name}'；它未在当前模式或运行状态的可用工具列表中。请改用系统提示中列出的当前实际可用工具。"
    )
}

fn build_tool_retry_context(tool_call: &super::llm::RequestedToolCall, error: &str) -> String {
    let arguments =
        serde_json::to_string_pretty(&tool_call.arguments).unwrap_or_else(|_| "{}".to_string());
    format!(
        "{TOOL_RETRY_CONTEXT_PREFIX}\n\
工具：{}\n\
工具调用 ID：{}\n\
错误详情：{}\n\n\
上次参数：\n{}\n\n\
要求：不要直接把该错误回复给用户。请根据工具 schema 和错误详情修正参数后重试同一个工具；重试成功后，系统会覆盖本次失败工具调用记录。",
        tool_call.name,
        tool_call.id,
        error.trim(),
        truncate_for_display(&arguments, 4_000, "\n...")
    )
}

fn is_retryable_tool_error(tool_name: &str, result: &str) -> bool {
    let trimmed = result.trim();
    if trimmed.is_empty() {
        return false;
    }
    if tool_name == "exec" {
        return false;
    }
    if !trimmed.starts_with("错误：") {
        return false;
    }
    trimmed.starts_with("错误：缺少必填参数")
        || trimmed.starts_with("错误：参数")
        || trimmed.contains("参数无效")
        || trimmed.contains("invalid type")
        || trimmed.contains("未找到工具")
        || trimmed.contains("禁止")
}

fn build_protocol_waiting_message(
    actions: &[ProtocolToolAction],
    auto_approve_dispatch: bool,
    final_message: Option<&str>,
) -> String {
    let mut sections = Vec::new();

    let dispatch_lines = actions
        .iter()
        .filter_map(|action| match action {
            ProtocolToolAction::Dispatch {
                agent,
                description,
                dispatch_id,
                ..
            } => Some(format!(
                "- [{}] dispatch_id={} {}",
                agent.display_name(),
                dispatch_id,
                truncate_for_display(description, 200, "...")
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !dispatch_lines.is_empty() {
        let header = if auto_approve_dispatch {
            format!(
                "📋 已自动批准 {} 个子任务，正在执行：",
                dispatch_lines.len()
            )
        } else {
            format!(
                "📋 已提交 {} 个子任务审查，等待执行：",
                dispatch_lines.len()
            )
        };
        sections.push(format!("{}\n{}", header, dispatch_lines.join("\n")));
    }

    let continue_lines = actions
        .iter()
        .filter_map(|action| match action {
            ProtocolToolAction::Continue { agent, text, .. } => Some(format!(
                "- [{}] {}",
                agent.display_name(),
                truncate_for_display(text, 200, "...")
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !continue_lines.is_empty() {
        sections.push(format!(
            "📨 已发送 {} 条后续指令，等待执行：\n{}",
            continue_lines.len(),
            continue_lines.join("\n")
        ));
    }

    let exit_lines = actions
        .iter()
        .filter_map(|action| match action {
            ProtocolToolAction::Exit { agent, reason, .. } => {
                Some(format!("- [{}] {}", agent.display_name(), reason))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if !exit_lines.is_empty() {
        sections.push(format!(
            "⏹️ 已发送 {} 条退出命令，等待进程结束：\n{}",
            exit_lines.len(),
            exit_lines.join("\n")
        ));
    }

    if let Some(message) = final_message
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        sections.push(format!("补充说明：\n{}", message));
    }

    sections.join("\n\n")
}

fn collect_recent_exploration_entries(history: &[ChatMessage]) -> String {
    const MAX_ENTRIES: usize = 3;
    const MAX_TOTAL_CHARS: usize = 900;

    let mut entries = Vec::new();
    let mut total_chars = 0usize;

    for message in history.iter().rev() {
        let content = message.content.trim();
        if content.is_empty() {
            continue;
        }

        let label = match message.role.as_str() {
            "tool" => message
                .name
                .as_deref()
                .map(|name| format!("工具 {}", name))
                .unwrap_or_else(|| "工具".to_string()),
            "assistant" => "调度结论".to_string(),
            "user" => continue,
            _ => continue,
        };
        let compact = compact_multiline(content, 220);
        if compact.is_empty() {
            continue;
        }

        let candidate = format!("- {}：{}", label, compact);
        let candidate_len = candidate.chars().count();
        if total_chars + candidate_len > MAX_TOTAL_CHARS && !entries.is_empty() {
            break;
        }

        entries.push(candidate);
        total_chars += candidate_len;
        if entries.len() >= MAX_ENTRIES {
            break;
        }
    }

    if entries.is_empty() {
        String::new()
    } else {
        entries.reverse();
        entries.join("\n")
    }
}

fn compact_multiline(content: &str, max_chars: usize) -> String {
    let normalized = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");
    truncate_for_display(&normalized, max_chars, "...")
}

fn should_include_latest_user_goal(latest_user_goal: &str, task_description: &str) -> bool {
    let normalized_task = compact_multiline(task_description.trim(), 320);
    !normalized_task.is_empty()
        && latest_user_goal != normalized_task
        && !normalized_task.contains(latest_user_goal)
}

fn summarize_dispatch_description(task_description: &str) -> String {
    let normalized = compact_multiline(task_description.trim(), 180);
    if normalized.is_empty() {
        "未命名子任务".to_string()
    } else {
        normalized
    }
}

fn cancellation_requested(cancel_rx: &watch::Receiver<bool>) -> bool {
    *cancel_rx.borrow()
}

async fn wait_for_cancellation(cancel_rx: &mut watch::Receiver<bool>) {
    if cancellation_requested(cancel_rx) {
        return;
    }

    while cancel_rx.changed().await.is_ok() {
        if cancellation_requested(cancel_rx) {
            return;
        }
    }
}

fn build_stopped_dispatch_reply(partial: &str) -> String {
    let trimmed = partial.trim();
    if trimmed.is_empty() {
        "⏹️ 本轮调度已停止。当前会话上下文与已完成内容均已保留，可稍后继续。".to_string()
    } else {
        format!(
            "{}\n\n[本轮已手动停止。当前会话上下文与以上输出均已保留，可稍后继续。]",
            trimmed
        )
    }
}

fn emit(on_event: &Channel<AgentEvent>, event: AgentEvent) {
    let _ = on_event.send(event);
}

fn record_session_token_usage(
    db: &DispatcherDb,
    workspace_id: &str,
    model: &str,
    source_kind: DispatcherSessionTokenUsageSource,
    usage: &LlmUsage,
) {
    if let Err(error) = db.upsert_session_token_usage(workspace_id, model, source_kind, usage) {
        eprintln!(
            "failed to persist dispatcher session token usage for workspace {} and model {}: {}",
            workspace_id, model, error
        );
    }
}

fn normalize_summary_model(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        super::config::DEFAULT_SUMMARY_MODEL.to_string()
    } else {
        trimmed.to_string()
    }
}

fn subprocess_phase_label(phase: RegisteredSubprocessPhase) -> &'static str {
    match phase {
        RegisteredSubprocessPhase::Running => "running",
        RegisteredSubprocessPhase::RoundCompleted => "round_completed",
        RegisteredSubprocessPhase::Stopped => "stopped",
        RegisteredSubprocessPhase::ExitRequested => "exit_requested",
    }
}

fn dispatch_tool_name(agent: &str) -> &'static str {
    match agent {
        "claude" => "dispatch_claude",
        "codex" => "dispatch_codex",
        _ => "dispatch_claude",
    }
}

fn continue_tool_name(agent: &str) -> &'static str {
    match agent {
        "claude" => "continue_claude_session",
        "codex" => "continue_codex_session",
        _ => "continue_claude_session",
    }
}

fn exit_tool_name(agent: &str) -> &'static str {
    match agent {
        "claude" => "exit_claude_session",
        "codex" => "exit_codex_session",
        _ => "exit_claude_session",
    }
}

fn default_mode_tool_allowlist() -> HashSet<&'static str> {
    HashSet::from([
        "read_file",
        "write_file",
        "edit_file",
        "list_dir",
        "glob",
        "grep",
        "exec",
        "message",
        "update_plan",
    ])
}

fn plan_mode_tool_allowlist() -> HashSet<&'static str> {
    HashSet::from([
        "read_file",
        "list_dir",
        "glob",
        "grep",
        "exec",
        "message",
        "ask_plan_question",
        "create_plan_document",
        "read_plan_document",
        "replace_plan_document",
        "edit_plan_document",
        "present_plan",
    ])
}

fn build_dispatcher_mode_block(runtime_state: &DispatcherSessionRuntimeState) -> String {
    let mut lines = Vec::new();
    match runtime_state.mode {
        DispatcherMode::Default => {
            lines.push("# 当前模式：Default".to_string());
            lines.push(
                "- 可以使用 `update_plan` 维护输入框上方的 Checklist；复杂任务应主动维护，简单任务可跳过。"
                    .to_string(),
            );
            lines.push(
                "- 如果本轮任务复杂到需要 Checklist，必须先调用 `update_plan` 创建本次任务规划步骤，再进行 glob/grep/read_file/exec 探索、委派或编码实践；不要把探索结果拿到以后才补建 Checklist。"
                    .to_string(),
            );
            lines.push(
                "- 例外：如果用户是在实施已经确认的 Plan 计划书，尤其消息中包含计划书路径，则不要调用 `update_plan` 重新规划；直接围绕计划书内容委派 Claude/Codex 子进程执行。"
                    .to_string(),
            );
            lines.push(
                "- Checklist 是子任务执行状态机：先列出待执行步骤；调用 `dispatch_claude`/`dispatch_codex` 时系统会把当前/下一个步骤绑定到该子 Agent，子进程启动后显示运行中，回流终态后显示完成。".to_string(),
            );
            lines.push(
                "- 可以使用 Claude/Codex 委派工具执行编码任务；实施计划时优先委派执行代理，Dispatcher 负责协调和验收。"
                    .to_string(),
            );
            if let Some(path) = runtime_state.active_plan_path.as_deref() {
                if is_implemented_plan_path(Path::new(path)) {
                    lines.push(format!(
                        "- 当前计划文件 `{path}` 文件名包含 `-已实现.md`，表示该计划已经实施完成，只能作为历史记录参考。"
                    ));
                } else {
                    lines.push(format!(
                        "- 当前待实施计划文件：`{path}`。实施完成后必须调用 `mark_plan_implemented`。"
                    ));
                }
            }
        }
        DispatcherMode::Plan => {
            lines.push("# 当前模式：Plan".to_string());
            lines.push("- 自主判断任务难度：简单咨询或无需落盘计划书的请求可以直接回复；只有需要形成实施计划时，才进入计划工具流程。".to_string());
            lines.push("- 需要规划时的流程：先探索当前代码与约束；若信息不足，调用 `ask_plan_question`；信息充分后创建/编辑计划书；最后调用 `present_plan`。".to_string());
            lines.push("- 禁止编码、禁止修改普通项目文件、禁止委派 Claude/Codex、禁止使用 `update_plan`。只能使用只读探索工具和计划书工具。".to_string());
            lines.push(
                "- 计划书必须写入当前项目 `.jkcodingagent/plan/*.md`，并且要足够详细到执行代理可直接开工。"
                    .to_string(),
            );
        }
    }
    lines.push(
        "- 任何文件名包含 `-已实现.md` 的计划都表示已经落地，不得重复当作待实施计划。".to_string(),
    );
    lines.join("\n")
}

fn render_available_tools_block(tool_definitions: &[crate::agent::llm::ToolDefinition]) -> String {
    if tool_definitions.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "# 当前实际可用工具".to_string(),
        "以下列表来自本轮运行时实际注入的工具定义，优先级高于静态 TOOLS.md。".to_string(),
    ];

    let mut tools = tool_definitions
        .iter()
        .map(|tool| {
            (
                tool.function.name.clone(),
                tool.function.description.trim().to_string(),
            )
        })
        .collect::<Vec<_>>();
    tools.sort_by(|left, right| left.0.cmp(&right.0));

    for (name, description) in tools {
        lines.push(format!("- `{name}`：{description}"));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use super::{
        build_checklist_state, build_dispatcher_mode_block, build_protocol_waiting_message,
        collect_recent_exploration_entries, complete_checklist_dispatch,
        default_mode_tool_allowlist, parse_update_plan, plan_mode_tool_allowlist,
        readonly_tool_run_end, reserve_checklist_dispatch, resolve_plan_path,
        should_include_latest_user_goal, start_checklist_dispatch, DispatchAgent, DispatcherAgent,
        DispatcherSubprocessRegistry, PlanPathAccess, PlannedSubprocessState, ProtocolBatchState,
        ProtocolToolAction, RegisteredSubprocess, RegisteredSubprocessPhase,
    };
    use super::{ChecklistStepStatus, DispatcherMode, DispatcherSessionRuntimeState};
    use crate::agent::config::DispatcherAgentConfig;
    use crate::agent::db::DispatcherDb;
    use crate::agent::llm::{ChatMessage, RequestedToolCall};
    use crate::project::mcp::ProjectMcpRegistry;

    #[test]
    fn protocol_state_allows_parallel_dispatch_for_different_agents() {
        let mut state = ProtocolBatchState::new(Vec::new());
        state.record_dispatch("claude", "dispatch-claude");
        assert!(state.ensure_dispatch_allowed("codex", "Codex").is_ok());
    }

    #[test]
    fn protocol_state_blocks_duplicate_dispatch_in_same_batch() {
        let mut state = ProtocolBatchState::new(Vec::new());
        state.record_dispatch("claude", "dispatch-claude");
        let error = state
            .ensure_dispatch_allowed("claude", "Claude")
            .expect_err("duplicate dispatch should be rejected");
        assert!(error.contains("待启动子任务"));
    }

    #[test]
    fn protocol_state_updates_existing_phase_on_exit() {
        let mut state = ProtocolBatchState::new(vec![RegisteredSubprocess {
            workspace_id: "ws".to_string(),
            task_id: "task".to_string(),
            dispatch_id: "dispatch".to_string(),
            agent: "claude".to_string(),
            description: "desc".to_string(),
            phase: RegisteredSubprocessPhase::RoundCompleted,
            force_idle: Arc::new(AtomicBool::new(false)),
        }]);
        state.record_exit("claude");
        match state.by_agent.get("claude") {
            Some(PlannedSubprocessState::Active { phase, .. }) => {
                assert_eq!(*phase, RegisteredSubprocessPhase::ExitRequested);
            }
            _ => panic!("expected active claude subprocess"),
        }
    }

    #[test]
    fn waiting_message_summarizes_multiple_protocol_actions() {
        let content = build_protocol_waiting_message(
            &[
                ProtocolToolAction::Dispatch {
                    dispatch_id: "dispatch-claude".to_string(),
                    agent: DispatchAgent::Claude,
                    description: "实现功能 A".to_string(),
                    task_prompt: "任务提示 A".to_string(),
                    permission_mode: "full_access".to_string(),
                },
                ProtocolToolAction::Dispatch {
                    dispatch_id: "dispatch-codex".to_string(),
                    agent: DispatchAgent::Codex,
                    description: "重构模块 B".to_string(),
                    task_prompt: "任务提示 B".to_string(),
                    permission_mode: "full_access".to_string(),
                },
                ProtocolToolAction::Exit {
                    dispatch_id: "dispatch-claude".to_string(),
                    agent: DispatchAgent::Claude,
                    reason: "当前轮完成".to_string(),
                },
            ],
            true,
            Some("主调度补充说明"),
        );

        assert!(content.contains("已自动批准 2 个子任务"));
        assert!(content.contains("已发送 1 条退出命令"));
        assert!(content.contains("主调度补充说明"));
    }

    #[test]
    fn exploration_entries_are_compact_and_skip_user_messages() {
        let history = vec![
            ChatMessage {
                role: "user".to_string(),
                content: "用户原始诉求".to_string(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "tool".to_string(),
                content: "读取文件 A，确认只需调整调度提示词拼装。".to_string(),
                tool_calls: None,
                tool_call_id: None,
                name: Some("read_file".to_string()),
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: "当前冗长主要来自已探索索引信息和输出要求重复注入。".to_string(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ];

        let result = collect_recent_exploration_entries(&history);
        assert!(result.contains("工具 read_file"));
        assert!(result.contains("调度结论"));
        assert!(!result.contains("用户原始诉求"));
    }

    #[test]
    fn latest_user_goal_is_skipped_when_task_already_covers_it() {
        let task_description = "请精简 Claude 子任务提示词，删除冗长动态规划内容，只保留必要约束。";
        let latest_user_goal = "请精简 Claude 子任务提示词，删除冗长动态规划内容，只保留必要约束。";
        assert!(!should_include_latest_user_goal(
            latest_user_goal,
            task_description
        ));
        assert!(should_include_latest_user_goal(
            "调度子任务不要再带固定工程师开头",
            task_description
        ));
    }

    #[test]
    fn readonly_tool_run_stops_before_mutating_tool() {
        let calls = ["glob", "grep", "write_file", "read_file"]
            .into_iter()
            .map(requested_tool_call)
            .collect::<Vec<_>>();

        assert_eq!(readonly_tool_run_end(&calls, 0), 2);
        assert_eq!(readonly_tool_run_end(&calls, 2), 2);
        assert_eq!(readonly_tool_run_end(&calls, 3), 4);
    }

    #[test]
    fn checklist_plan_accepts_single_in_progress() {
        let state = build_checklist_state(
            parse_update_plan(&serde_json::json!({
                "explanation": "推进中",
                "plan": [
                    { "step": "读代码", "status": "completed" },
                    { "step": "实现", "status": "in_progress" }
                ]
            }))
            .expect("draft should parse"),
            None,
        )
        .expect("valid checklist should pass");

        assert_eq!(state.items.len(), 2);
        assert_eq!(state.items[1].status, ChecklistStepStatus::InProgress);
    }

    #[test]
    fn checklist_plan_rejects_invalid_states() {
        let error = build_checklist_state(
            parse_update_plan(&serde_json::json!({
                "plan": [{ "step": "读代码", "status": "doing" }]
            }))
            .expect("draft should parse"),
            None,
        )
        .expect_err("invalid status should fail");
        assert!(error.contains("invalid checklist step status"));
    }

    #[test]
    fn checklist_plan_rejects_multiple_in_progress_steps() {
        let error = build_checklist_state(
            parse_update_plan(&serde_json::json!({
                "plan": [
                    { "step": "A", "status": "in_progress" },
                    { "step": "B", "status": "in_progress" }
                ]
            }))
            .expect("draft should parse"),
            None,
        )
        .expect_err("multiple in_progress steps should fail");
        assert!(error.contains("最多只能有 1 个"));
    }

    #[test]
    fn checklist_dispatch_lifecycle_reserves_starts_and_completes_step() {
        let workspace = temp_workspace("checklist-dispatch-lifecycle");
        let db = DispatcherDb::new(workspace.join("dispatcher.sqlite3")).unwrap();
        let session = db
            .create_session("project-1", "测试会话", DispatcherMode::Default, None)
            .unwrap();
        let checklist = build_checklist_state(
            parse_update_plan(&serde_json::json!({
                "plan": [
                    { "id": "backend", "step": "实现后端", "status": "pending", "agent": "claude" },
                    { "id": "frontend", "step": "实现前端", "status": "pending" }
                ]
            }))
            .unwrap(),
            None,
        )
        .unwrap();
        db.update_checklist(&session.id, &checklist).unwrap();

        let reserved =
            reserve_checklist_dispatch(&db, &session.id, "dispatch-1", "claude", "实现后端")
                .unwrap()
                .unwrap();
        assert_eq!(reserved.items[0].dispatch_id.as_deref(), Some("dispatch-1"));
        assert_eq!(reserved.items[0].status, ChecklistStepStatus::Pending);

        db.attach_checklist_subprocess(&session.id, "dispatch-1", "task-1")
            .unwrap();
        let started = start_checklist_dispatch(&db, &session.id, "dispatch-1")
            .unwrap()
            .unwrap();
        assert_eq!(started.items[0].status, ChecklistStepStatus::InProgress);
        assert_eq!(
            started.items[0].subprocess_task_id.as_deref(),
            Some("task-1")
        );

        let completed = complete_checklist_dispatch(&db, &session.id, "dispatch-1")
            .unwrap()
            .unwrap();
        assert_eq!(completed.items[0].status, ChecklistStepStatus::Completed);
        assert_eq!(completed.items[1].status, ChecklistStepStatus::Pending);

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn plan_path_guard_restricts_to_project_plan_dir() {
        let workspace = temp_workspace("plan-path-guard");
        let plan_dir = workspace.join(".jkcodingagent").join("plan");
        fs::create_dir_all(&plan_dir).expect("plan dir");
        let plan_path = plan_dir.join("demo.md");
        fs::write(&plan_path, "# Demo").expect("plan file");
        let implemented_path = plan_dir.join("demo-已实现.md");
        fs::write(&implemented_path, "# Done").expect("implemented plan file");
        let outside_path = workspace.join("outside.md");
        fs::write(&outside_path, "# Outside").expect("outside file");

        assert!(resolve_plan_path(
            &workspace,
            &plan_path.to_string_lossy(),
            PlanPathAccess::Read
        )
        .is_ok());
        assert!(resolve_plan_path(
            &workspace,
            &outside_path.to_string_lossy(),
            PlanPathAccess::Read
        )
        .expect_err("outside path should fail")
        .contains("计划书路径必须位于项目计划目录内"));
        assert!(resolve_plan_path(
            &workspace,
            &implemented_path.to_string_lossy(),
            PlanPathAccess::WriteExisting,
        )
        .expect_err("implemented plan should be immutable")
        .contains("禁止修改"));

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn mode_tool_allowlists_are_mutually_scoped() {
        let workspace = temp_workspace("tool-allowlist");
        let config = test_dispatcher_config(workspace.clone());
        let agent = DispatcherAgent::new(
            config,
            ProjectMcpRegistry::default(),
            Arc::new(DispatcherSubprocessRegistry::default()),
        );

        let default_state = DispatcherSessionRuntimeState {
            mode: DispatcherMode::Default,
            checklist: None,
            plan_interaction: None,
            active_plan_path: None,
        };
        let default_tools = agent
            .tool_definitions_for_workspace("session", &workspace, &default_state)
            .into_iter()
            .map(|tool| tool.function.name)
            .collect::<Vec<_>>();
        assert!(default_tools.iter().any(|name| name == "update_plan"));
        assert!(default_tools.iter().any(|name| name == "dispatch_claude"));
        assert!(default_tools.iter().any(|name| name == "dispatch_codex"));

        let plan_state = DispatcherSessionRuntimeState {
            mode: DispatcherMode::Plan,
            checklist: None,
            plan_interaction: None,
            active_plan_path: None,
        };
        let plan_tools = agent
            .tool_definitions_for_workspace("session", &workspace, &plan_state)
            .into_iter()
            .map(|tool| tool.function.name)
            .collect::<Vec<_>>();
        assert!(plan_tools.iter().any(|name| name == "create_plan_document"));
        assert!(plan_tools.iter().any(|name| name == "read_file"));
        assert!(!plan_tools.iter().any(|name| name == "update_plan"));
        assert!(!plan_tools.iter().any(|name| name == "dispatch_claude"));
        assert!(!plan_tools.iter().any(|name| name == "write_file"));

        assert!(default_mode_tool_allowlist().contains("update_plan"));
        assert!(plan_mode_tool_allowlist().contains("present_plan"));
        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn default_mode_prompt_requires_checklist_before_exploration_when_used() {
        let state = DispatcherSessionRuntimeState {
            mode: DispatcherMode::Default,
            checklist: None,
            plan_interaction: None,
            active_plan_path: None,
        };

        let prompt = build_dispatcher_mode_block(&state);

        assert!(prompt.contains("复杂任务应主动维护，简单任务可跳过"));
        assert!(prompt.contains("必须先调用 `update_plan` 创建本次任务规划步骤"));
        assert!(prompt.contains("再进行 glob/grep/read_file/exec 探索"));
    }

    #[test]
    fn plan_mode_prompt_allows_simple_direct_reply() {
        let state = DispatcherSessionRuntimeState {
            mode: DispatcherMode::Plan,
            checklist: None,
            plan_interaction: None,
            active_plan_path: None,
        };

        let prompt = build_dispatcher_mode_block(&state);

        assert!(prompt.contains("简单咨询或无需落盘计划书的请求可以直接回复"));
        assert!(prompt.contains("只有需要形成实施计划时，才进入计划工具流程"));
        assert!(prompt.contains("禁止编码、禁止修改普通项目文件"));
        assert!(prompt.contains("禁止使用 `update_plan`"));
    }

    fn requested_tool_call(name: &str) -> RequestedToolCall {
        RequestedToolCall {
            id: name.to_string(),
            name: name.to_string(),
            arguments: serde_json::json!({}),
        }
    }

    fn temp_workspace(label: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("jkcodingagent-{label}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).expect("temp workspace");
        path
    }

    fn test_dispatcher_config(root_dir: PathBuf) -> DispatcherAgentConfig {
        DispatcherAgentConfig {
            db_path: root_dir.join("jkbot.sqlite3"),
            root_dir,
            api_key: String::new(),
            api_base: String::new(),
            model: "test-model".to_string(),
            summary_model: "test-summary".to_string(),
            vision_model: String::new(),
            max_tokens: 1024,
            temperature: 0.1,
            max_tool_iterations: 4,
            exec_timeout_secs: 5,
            restrict_to_workspace: true,
            auto_approve_dispatch: false,
            context_debug: false,
        }
    }
}
