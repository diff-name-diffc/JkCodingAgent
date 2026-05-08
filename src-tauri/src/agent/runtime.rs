use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use futures::future::join_all;
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use tauri::ipc::Channel;
use tokio::sync::watch;

use super::config::DispatcherAgentConfig;
use super::db::{
    DispatcherDb, DispatcherMessageRecord, DispatcherSessionTokenUsageSource,
    DispatcherSettingsRecord, DispatcherToolArtifactRef, TOOL_RETRY_CONTEXT_PREFIX,
};
use super::debug::{render_json, ContextDebugLogger, DebugSection};
use super::llm::{
    messages_contain_inline_images, ChatMessage, FunctionCall, LlmUsage, OpenAiCompatProvider,
    OutboundToolCall, RequestedToolCall,
};
use super::prompt::{build_system_prompt, PromptBundle, PromptSection};
use super::summary::{
    build_summary_failure_message, prepare_tool_result, summarize_dispatch_result,
};
use super::tools::{
    parse_continue_instruction, parse_dispatch_instruction, parse_exit_instruction, DispatchAgent,
    ToolContext, ToolRegistry,
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

        let workspace = PathBuf::from(workspace_path);
        if !workspace.exists() {
            fs::create_dir_all(&workspace)
                .with_context(|| format!("create workspace {}", workspace.display()))?;
        }
        let _ = self.project_mcp_registry.ensure_recent(&workspace).await;
        let user = db.add_visible_message(workspace_id, "user", user_message)?;
        emit(&on_event, AgentEvent::UserMessage { message: user });

        let provider = self.provider.lock().clone();
        let reply = if provider.is_configured() {
            self.run_llm_loop(
                db,
                workspace_id,
                &workspace,
                &on_event,
                &provider,
                cancel_rx,
            )
            .await?
        } else {
            let reply = db.add_visible_message(
                workspace_id,
                "assistant",
                "主 Agent 会话接口已接入，但当前未配置 LLM API Key。请在 Dispatcher 设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。",
            )?;
            emit(
                &on_event,
                AgentEvent::AssistantMessage {
                    message: reply.clone(),
                },
            );
            reply
        };

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
        on_event: Channel<AgentEvent>,
        mut cancel_rx: watch::Receiver<bool>,
    ) -> Result<AgentTurn> {
        let debug_logger =
            ContextDebugLogger::new(self.context_debug_enabled(), PathBuf::from(workspace_path));
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
                let reply = self.emit_summary_failure_and_finish(
                    db,
                    workspace_id,
                    &on_event,
                    &summary_model,
                    error.message(),
                )?;
                let messages = db.list_visible_messages(workspace_id)?;
                return Ok(AgentTurn { reply, messages });
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

        if !provider.is_configured() {
            let messages = db.list_visible_messages(workspace_id)?;
            emit(
                &on_event,
                AgentEvent::Finished {
                    messages: messages.clone(),
                },
            );
            return Ok(AgentTurn {
                reply: result_msg,
                messages,
            });
        }

        let workspace = PathBuf::from(workspace_path);
        let _ = self.project_mcp_registry.ensure_recent(&workspace).await;
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

            let tool_definitions = self.tool_definitions_for_workspace(workspace_id, workspace);
            let prompt_snapshot =
                self.build_system_prompt_for_workspace(workspace_id, workspace, &tool_definitions)?;
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
            let mut final_message: Option<String> = None;
            let mut saw_retryable_tool_error = false;

            let tool_calls = response.tool_calls;
            let mut tool_call_index = 0usize;
            while tool_call_index < tool_calls.len() {
                if cancellation_requested(&cancel_rx) {
                    return self.emit_stop_and_finish(db, workspace_id, on_event, "");
                }

                let readonly_end = readonly_tool_run_end(&tool_calls, tool_call_index);
                let ready_tool_results = if readonly_end.saturating_sub(tool_call_index) >= 2 {
                    let run = &tool_calls[tool_call_index..readonly_end];
                    let results = self
                        .execute_parallel_readonly_tools(run, &tool_context, on_event)
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
                    let result = self
                        .tools
                        .execute(&tool_call.name, &tool_call.arguments, &tool_context)
                        .await;
                    vec![(tool_call, result)]
                };

                for (tool_call, result) in ready_tool_results {
                    if cancellation_requested(&cancel_rx) {
                        return self.emit_stop_and_finish(db, workspace_id, on_event, "");
                    }

                    let tool_arguments = serde_json::to_string_pretty(&tool_call.arguments)
                        .unwrap_or_else(|_| "{}".to_string());

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
                            return self.emit_summary_failure_and_finish(
                                db,
                                workspace_id,
                                on_event,
                                &summary_model,
                                error.message(),
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

        let content = format!(
            "已达到最大工具迭代次数（{}），本轮执行已停止。",
            self.config.max_tool_iterations
        );
        let reply = db.add_visible_message(workspace_id, "assistant", &content)?;
        emit(
            on_event,
            AgentEvent::AssistantMessage {
                message: reply.clone(),
            },
        );
        Ok(reply)
    }

    fn build_system_prompt(&self) -> Result<PromptBundle> {
        build_system_prompt(&self.config.root_dir)
    }

    fn build_system_prompt_for_workspace(
        &self,
        workspace_id: &str,
        workspace: &Path,
        tool_definitions: &[crate::agent::llm::ToolDefinition],
    ) -> Result<SystemPromptSnapshot> {
        let mut prompt_bundle = self.build_system_prompt()?;
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

        let mut sections = vec![format!("【任务目标】\n{}", task_description.trim())];

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
    ) -> Vec<crate::agent::llm::ToolDefinition> {
        let mut allowed = builtin_tool_allowlist();

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

        self.tools
            .definitions_for_workspace(workspace, Some(allowed.into_iter()))
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
            self.tools
                .execute(&tool_call.name, &tool_call.arguments, tool_context)
                .await
        }))
        .await;

        results
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

    fn emit_summary_failure_and_finish(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        summary_model: &str,
        error: &str,
    ) -> Result<DispatcherMessageRecord> {
        let reply = db.add_visible_message(
            workspace_id,
            "assistant",
            &build_summary_failure_message(summary_model, error),
        )?;
        emit(
            on_event,
            AgentEvent::AssistantMessage {
                message: reply.clone(),
            },
        );
        let messages = db.list_visible_messages(workspace_id)?;
        emit(on_event, AgentEvent::Finished { messages });
        Ok(reply)
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

fn builtin_tool_allowlist() -> HashSet<&'static str> {
    HashSet::from([
        "read_file",
        "write_file",
        "edit_file",
        "list_dir",
        "glob",
        "grep",
        "exec",
        "message",
    ])
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
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use super::{
        build_protocol_waiting_message, collect_recent_exploration_entries, readonly_tool_run_end,
        should_include_latest_user_goal, DispatchAgent, PlannedSubprocessState, ProtocolBatchState,
        ProtocolToolAction, RegisteredSubprocess, RegisteredSubprocessPhase,
    };
    use crate::agent::llm::{ChatMessage, RequestedToolCall};

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

    fn requested_tool_call(name: &str) -> RequestedToolCall {
        RequestedToolCall {
            id: name.to_string(),
            name: name.to_string(),
            arguments: serde_json::json!({}),
        }
    }
}
