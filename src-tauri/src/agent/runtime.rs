use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use tauri::ipc::Channel;

use super::config::DispatcherAgentConfig;
use super::db::{
    DispatcherDb, DispatcherMessageRecord, DispatcherSettingsRecord, DispatcherToolArtifactRef,
};
use super::debug::{render_json, ContextDebugLogger, DebugSection};
use super::llm::{ChatMessage, FunctionCall, OpenAiCompatProvider, OutboundToolCall};
use super::prompt::{build_system_prompt, PromptBundle, PromptSection};
use super::summary::{
    build_summary_failure_message, prepare_tool_result, summarize_dispatch_result,
};
use super::tools::{
    is_continue_instruction, is_dispatch_instruction, is_exit_instruction,
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
    AssistantDelta {
        message_id: String,
        delta: String,
    },
    AssistantMessage {
        message: DispatcherMessageRecord,
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
    tools: ToolRegistry,
    project_mcp_registry: ProjectMcpRegistry,
    subprocesses: Mutex<Vec<RegisteredSubprocess>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegisteredSubprocessPhase {
    Running,
    RoundCompleted,
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

impl DispatcherAgent {
    pub fn new(config: DispatcherAgentConfig, project_mcp_registry: ProjectMcpRegistry) -> Self {
        let provider = OpenAiCompatProvider::new(
            config.api_key.clone(),
            config.api_base.clone(),
            config.model.clone(),
            config.max_tokens,
            config.temperature,
        );

        Self {
            config,
            provider: Mutex::new(provider),
            tools: ToolRegistry::default_tools(project_mcp_registry.clone()),
            project_mcp_registry,
            subprocesses: Mutex::new(Vec::new()),
        }
    }

    pub fn apply_settings(&self, settings: &DispatcherSettingsRecord) {
        let mut provider = self.provider.lock().unwrap();
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

    pub fn set_context_debug(&mut self, value: bool) {
        self.config.context_debug = value;
    }

    pub fn register_subprocess(
        &self,
        workspace_id: &str,
        task_id: &str,
        dispatch_id: &str,
        agent: &str,
        description: &str,
    ) {
        let mut subprocesses = self.subprocesses.lock().unwrap();
        subprocesses.retain(|item| !(item.workspace_id == workspace_id && item.agent == agent));
        subprocesses.push(RegisteredSubprocess {
            workspace_id: workspace_id.to_string(),
            task_id: task_id.to_string(),
            dispatch_id: dispatch_id.to_string(),
            agent: agent.to_string(),
            description: description.to_string(),
            phase: RegisteredSubprocessPhase::Running,
        });
    }

    pub fn mark_subprocess_round_completed(&self, task_id: &str) {
        self.update_subprocess_phase(task_id, RegisteredSubprocessPhase::RoundCompleted);
    }

    pub fn mark_subprocess_running(&self, task_id: &str) {
        self.update_subprocess_phase(task_id, RegisteredSubprocessPhase::Running);
    }

    pub fn mark_subprocess_finished(&self, task_id: &str) {
        let mut subprocesses = self.subprocesses.lock().unwrap();
        subprocesses.retain(|item| item.task_id != task_id);
    }

    pub async fn run(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace_path: &str,
        user_message: &str,
        on_event: Channel<AgentEvent>,
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

        let provider = self.provider.lock().unwrap().clone();
        let reply = if provider.is_configured() {
            self.run_llm_loop(db, workspace_id, &workspace, &on_event, &provider)
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
        let provider = self.provider.lock().unwrap().clone();
        let summarized_dispatch_result =
            match summarize_dispatch_result(&provider, dispatch_result).await {
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
            .run_llm_loop(db, workspace_id, &workspace, &on_event, &provider)
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
    ) -> Result<DispatcherMessageRecord> {
        let debug_logger = ContextDebugLogger::new(self.context_debug_enabled(), workspace);
        let tool_context = ToolContext {
            workspace: workspace.to_path_buf(),
            exec_timeout_secs: self.config.exec_timeout_secs,
            restrict_to_workspace: self.config.restrict_to_workspace,
        };

        for iteration in 0..self.config.max_tool_iterations {
            let tool_definitions = self.tool_definitions_for_workspace(workspace_id, workspace);
            let prompt_snapshot =
                self.build_system_prompt_for_workspace(workspace_id, workspace)?;
            let history_messages = db.load_llm_history(workspace_id)?;
            let mut messages = vec![ChatMessage::system(prompt_snapshot.rendered.clone())];
            messages.extend(history_messages.clone());
            let request_snapshot = provider.build_request_snapshot(&messages, &tool_definitions);

            debug_logger.log(
                "发送大模型请求",
                vec![
                    ("工作区".to_string(), workspace_id.to_string()),
                    ("轮次".to_string(), (iteration + 1).to_string()),
                    ("模型".to_string(), provider.model().to_string()),
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
            let on_delta = move |delta: &str| {
                let _ = event_ref.send(AgentEvent::AssistantDelta {
                    message_id: msg_id_ref.clone(),
                    delta: delta.to_string(),
                });
            };

            let response = provider
                .chat_stream(&messages, &tool_definitions, on_delta)
                .await?;
            let response_snapshot = provider.build_response_snapshot(&response);

            debug_logger.log(
                "收到大模型响应",
                vec![
                    ("工作区".to_string(), workspace_id.to_string()),
                    ("轮次".to_string(), (iteration + 1).to_string()),
                    ("模型".to_string(), provider.model().to_string()),
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

            for tool_call in response.tool_calls {
                let tool_arguments = serde_json::to_string_pretty(&tool_call.arguments)
                    .unwrap_or_else(|_| "{}".to_string());
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

                match self.plan_protocol_action(&tool_call, &result, &mut protocol_state) {
                    Ok(Some(action)) => {
                        if let ProtocolToolAction::Exit { agent, .. } = &action {
                            self.mark_agent_exit_requested(workspace_id, agent.slug());
                        }
                        self.emit_protocol_action(db, workspace_id, on_event, &tool_call, &action)?;
                        protocol_actions.push(action);
                        continue;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.emit_tool_error(db, workspace_id, on_event, &tool_call, &error)?;
                        continue;
                    }
                }

                let tool_result = match prepare_tool_result(
                    provider,
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
                                DebugSection::new("摘要调用", error.debug_context().to_string()),
                                DebugSection::new("工具参数", tool_arguments.clone()),
                                DebugSection::new("失败原因", error.message().to_string()),
                            ],
                        );
                        return self.emit_summary_failure_and_finish(
                            db,
                            workspace_id,
                            on_event,
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

                if tool_call.name == "message" {
                    if let Some(content) = extract_message_content(&tool_call.arguments) {
                        final_message = Some(content);
                    }
                }
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
    ) -> Result<SystemPromptSnapshot> {
        let mut prompt_bundle = self.build_system_prompt()?;
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
            "规则：phase=exit_requested 时，不要再次调用该 agent 的 dispatch_* / continue_* / exit_*，只等待进程结束。"
                .to_string(),
        );

        lines.join("\n")
    }

    fn tool_definitions_for_workspace(
        &self,
        workspace_id: &str,
        workspace: &Path,
    ) -> Vec<crate::agent::llm::ToolDefinition> {
        let mut allowed = HashSet::from([
            "read_file",
            "write_file",
            "edit_file",
            "list_dir",
            "glob",
            "grep",
            "exec",
            "message",
        ]);

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
                (true, Some(RegisteredSubprocessPhase::ExitRequested)) => {}
                (true, None) => {}
            }
        }

        self.tools
            .definitions_for_workspace(workspace, Some(allowed.into_iter()))
    }

    fn active_subprocesses_for_workspace(&self, workspace_id: &str) -> Vec<RegisteredSubprocess> {
        self.subprocesses
            .lock()
            .unwrap()
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

    fn update_subprocess_phase(&self, task_id: &str, phase: RegisteredSubprocessPhase) {
        let mut subprocesses = self.subprocesses.lock().unwrap();
        if let Some(item) = subprocesses.iter_mut().find(|item| item.task_id == task_id) {
            item.phase = phase;
        }
    }

    fn mark_agent_exit_requested(&self, workspace_id: &str, agent: &str) {
        let mut subprocesses = self.subprocesses.lock().unwrap();
        if let Some(item) = subprocesses
            .iter_mut()
            .find(|item| item.workspace_id == workspace_id && item.agent == agent)
        {
            item.phase = RegisteredSubprocessPhase::ExitRequested;
        }
    }

    fn plan_protocol_action(
        &self,
        tool_call: &super::llm::RequestedToolCall,
        result: &str,
        protocol_state: &mut ProtocolBatchState,
    ) -> std::result::Result<Option<ProtocolToolAction>, String> {
        if let Some(agent) = DispatchAgent::from_dispatch_tool_name(&tool_call.name) {
            protocol_state.ensure_dispatch_allowed(agent.slug(), agent.display_name())?;
            if let Some((description, permission_mode)) = parse_dispatch_instruction(result, agent)
                .filter(|_| is_dispatch_instruction(result, agent))
            {
                let dispatch_id = uuid::Uuid::new_v4().to_string();
                protocol_state.record_dispatch(agent.slug(), &dispatch_id);
                return Ok(Some(ProtocolToolAction::Dispatch {
                    dispatch_id,
                    agent,
                    description,
                    permission_mode,
                }));
            }
            return Ok(None);
        }

        if let Some(agent) = DispatchAgent::from_continue_tool_name(&tool_call.name) {
            protocol_state.ensure_continue_allowed(agent.slug(), agent.display_name())?;
            if let Some(text) = parse_continue_instruction(result, agent)
                .filter(|_| is_continue_instruction(result, agent))
            {
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
            return Ok(None);
        }

        if let Some(agent) = DispatchAgent::from_exit_tool_name(&tool_call.name) {
            protocol_state.ensure_exit_allowed(agent.slug(), agent.display_name())?;
            if let Some(reason) =
                parse_exit_instruction(result, agent).filter(|_| is_exit_instruction(result, agent))
            {
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
                permission_mode,
            } => {
                emit(
                    on_event,
                    AgentEvent::DispatchProposed {
                        dispatch_id: dispatch_id.clone(),
                        agent: agent.slug().to_string(),
                        description: description.clone(),
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
        error: &str,
    ) -> Result<DispatcherMessageRecord> {
        let reply = db.add_visible_message(
            workspace_id,
            "assistant",
            &build_summary_failure_message(error),
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
}

fn extract_message_content(arguments: &Value) -> Option<String> {
    arguments
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_string)
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

fn emit(on_event: &Channel<AgentEvent>, event: AgentEvent) {
    let _ = on_event.send(event);
}

fn subprocess_phase_label(phase: RegisteredSubprocessPhase) -> &'static str {
    match phase {
        RegisteredSubprocessPhase::Running => "running",
        RegisteredSubprocessPhase::RoundCompleted => "round_completed",
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

#[cfg(test)]
mod tests {
    use super::{
        build_protocol_waiting_message, DispatchAgent, PlannedSubprocessState, ProtocolBatchState,
        ProtocolToolAction, RegisteredSubprocess, RegisteredSubprocessPhase,
    };

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
                    permission_mode: "full_access".to_string(),
                },
                ProtocolToolAction::Dispatch {
                    dispatch_id: "dispatch-codex".to_string(),
                    agent: DispatchAgent::Codex,
                    description: "重构模块 B".to_string(),
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
}
