use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use tauri::ipc::Channel;

use super::config::DispatcherAgentConfig;
use super::db::{DispatcherDb, DispatcherMessageRecord, DispatcherSettingsRecord};
use super::debug::{render_json, ContextDebugLogger, DebugSection};
use super::llm::{ChatMessage, FunctionCall, OpenAiCompatProvider, OutboundToolCall};
use super::prompt::{build_system_prompt, PromptBundle, PromptSection};
use super::summary::{
    build_ollama_install_message, prepare_tool_result, summarize_dispatch_result,
};
use super::tools::{
    is_continue_instruction, is_dispatch_instruction, is_exit_instruction,
    parse_continue_instruction, parse_dispatch_instruction, parse_exit_instruction, DispatchAgent,
    ToolContext, ToolRegistry,
};
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
    ToolFinished {
        tool_call_id: Option<String>,
        name: String,
        result: String,
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

impl DispatcherAgent {
    pub fn new(config: DispatcherAgentConfig) -> Self {
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
            tools: ToolRegistry::default_tools(),
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
        let summarized_dispatch_result = match summarize_dispatch_result(dispatch_result).await {
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
                    vec![DebugSection::new("失败原因", error.clone())],
                );
                let reply =
                    self.emit_ollama_failure_and_finish(db, workspace_id, &on_event, &error)?;
                let messages = db.list_visible_messages(workspace_id)?;
                return Ok(AgentTurn { reply, messages });
            }
        };

        let hidden_message = format!(
            "{}\n\n{}",
            dispatch_state.hidden_prefix(),
            summarized_dispatch_result
        );

        db.add_hidden_message(workspace_id, "user", &hidden_message, None, None, None)?;

        let provider = self.provider.lock().unwrap().clone();
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
            let tool_definitions = self.tool_definitions_for_workspace(workspace_id);
            let prompt_snapshot = self.build_system_prompt_for_workspace(workspace_id)?;
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
                vec![DebugSection::new("实际请求", render_json(&request_snapshot))],
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
                vec![DebugSection::new("实际响应", render_json(&response_snapshot))],
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
                Some(&tool_calls_payload),
            )?;

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

                if let Some(agent) = DispatchAgent::from_dispatch_tool_name(&tool_call.name) {
                    if let Err(error) = self.ensure_dispatch_allowed(
                        workspace_id,
                        agent.slug(),
                        agent.display_name(),
                    ) {
                        self.emit_tool_error(db, workspace_id, on_event, &tool_call, &error)?;
                        continue;
                    }
                    if is_dispatch_instruction(&result, agent) {
                        if let Some((description, permission_mode)) =
                            parse_dispatch_instruction(&result, agent)
                        {
                            let dispatch_id = uuid::Uuid::new_v4().to_string();
                            let agent_label = agent.display_name();

                            emit(
                                on_event,
                                AgentEvent::DispatchProposed {
                                    dispatch_id: dispatch_id.clone(),
                                    agent: agent.slug().to_string(),
                                    description: description.clone(),
                                    permission_mode: permission_mode.clone(),
                                },
                            );

                            let dispatch_result = format!(
                                "[{} 子任务已提交审查] dispatch_id={}, 任务: {}",
                                agent_label,
                                dispatch_id,
                                truncate_for_display(&description, 200, "...")
                            );

                            emit(
                                on_event,
                                AgentEvent::ToolFinished {
                                    tool_call_id: Some(tool_call.id.clone()),
                                    name: tool_call.name.clone(),
                                    result: dispatch_result.clone(),
                                },
                            );

                            db.add_visible_message_with_tools(
                                workspace_id,
                                "tool",
                                &dispatch_result,
                                Some(&tool_call.id),
                                Some(&tool_call.name),
                                None,
                            )?;

                            let waiting_content = if self.auto_approve_dispatch() {
                                format!(
                                    "📋 已自动批准 {} 子任务，正在执行...\n\n**任务描述：**\n{}",
                                    agent_label, description
                                )
                            } else {
                                format!(
                                    "📋 已提交 {} 子任务审查，等待执行...\n\n**任务描述：**\n{}",
                                    agent_label, description
                                )
                            };
                            let waiting_msg = db.add_visible_message(
                                workspace_id,
                                "assistant",
                                &waiting_content,
                            )?;
                            emit(
                                on_event,
                                AgentEvent::AssistantMessage {
                                    message: waiting_msg.clone(),
                                },
                            );
                            return Ok(waiting_msg);
                        }
                    }
                }

                if let Some(agent) = DispatchAgent::from_continue_tool_name(&tool_call.name) {
                    if let Err(error) = self.ensure_continue_allowed(
                        workspace_id,
                        agent.slug(),
                        agent.display_name(),
                    ) {
                        self.emit_tool_error(db, workspace_id, on_event, &tool_call, &error)?;
                        continue;
                    }
                    if is_continue_instruction(&result, agent) {
                        if let Some(text) = parse_continue_instruction(&result, agent) {
                            let agent_label = agent.display_name();
                            emit(
                                on_event,
                                AgentEvent::DispatchContinue {
                                    dispatch_id: "active".to_string(),
                                    agent: agent.slug().to_string(),
                                    text: text.clone(),
                                },
                            );

                            let continue_result = format!(
                                "[已发送后续指令到 {} 会话] 指令: {}",
                                agent_label,
                                truncate_for_display(&text, 200, "...")
                            );

                            emit(
                                on_event,
                                AgentEvent::ToolFinished {
                                    tool_call_id: Some(tool_call.id.clone()),
                                    name: tool_call.name.clone(),
                                    result: continue_result.clone(),
                                },
                            );

                            db.add_visible_message_with_tools(
                                workspace_id,
                                "tool",
                                &continue_result,
                                Some(&tool_call.id),
                                Some(&tool_call.name),
                                None,
                            )?;

                            let waiting_msg = db.add_visible_message(
                                workspace_id,
                                "assistant",
                                &format!(
                                    "📨 已向 {} 发送后续指令，等待执行...\n\n**指令内容：**\n{}",
                                    agent_label, text
                                ),
                            )?;
                            emit(
                                on_event,
                                AgentEvent::AssistantMessage {
                                    message: waiting_msg.clone(),
                                },
                            );
                            return Ok(waiting_msg);
                        }
                    }
                }

                if let Some(agent) = DispatchAgent::from_exit_tool_name(&tool_call.name) {
                    if let Err(error) =
                        self.ensure_exit_allowed(workspace_id, agent.slug(), agent.display_name())
                    {
                        self.emit_tool_error(db, workspace_id, on_event, &tool_call, &error)?;
                        continue;
                    }
                    if is_exit_instruction(&result, agent) {
                        if let Some(reason) = parse_exit_instruction(&result, agent) {
                            let agent_label = agent.display_name();
                            self.mark_agent_exit_requested(workspace_id, agent.slug());
                            emit(
                                on_event,
                                AgentEvent::DispatchExit {
                                    dispatch_id: "active".to_string(),
                                    agent: agent.slug().to_string(),
                                    reason: reason.clone(),
                                },
                            );

                            let exit_result =
                                format!("[已发送退出命令到 {} 会话] 原因: {}", agent_label, reason);

                            emit(
                                on_event,
                                AgentEvent::ToolFinished {
                                    tool_call_id: Some(tool_call.id.clone()),
                                    name: tool_call.name.clone(),
                                    result: exit_result.clone(),
                                },
                            );

                            db.add_visible_message_with_tools(
                                workspace_id,
                                "tool",
                                &exit_result,
                                Some(&tool_call.id),
                                Some(&tool_call.name),
                                None,
                            )?;

                            let waiting_msg = db.add_visible_message(
                                workspace_id,
                                "assistant",
                                &format!(
                                    "⏹️ 已向 {} 发送退出命令，等待进程结束...\n\n**退出原因：**\n{}",
                                    agent_label, reason
                                ),
                            )?;
                            emit(
                                on_event,
                                AgentEvent::AssistantMessage {
                                    message: waiting_msg.clone(),
                                },
                            );
                            return Ok(waiting_msg);
                        }
                    }
                }

                let tool_result = match prepare_tool_result(&tool_call.name, &result).await {
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
                                DebugSection::new("工具参数", tool_arguments.clone()),
                                DebugSection::new("失败原因", error.clone()),
                            ],
                        );
                        return self.emit_ollama_failure_and_finish(
                            db,
                            workspace_id,
                            on_event,
                            &error,
                        );
                    }
                };

                emit(
                    on_event,
                    AgentEvent::ToolFinished {
                        tool_call_id: Some(tool_call.id.clone()),
                        name: tool_call.name.clone(),
                        result: tool_result.clone(),
                    },
                );

                db.add_visible_message_with_tools(
                    workspace_id,
                    "tool",
                    &tool_result,
                    Some(&tool_call.id),
                    Some(&tool_call.name),
                    None,
                )?;

                if tool_call.name == "message" {
                    if let Some(final_message) = extract_message_content(&tool_call.arguments) {
                        let reply =
                            db.add_visible_message(workspace_id, "assistant", &final_message)?;
                        emit(
                            on_event,
                            AgentEvent::AssistantMessage {
                                message: reply.clone(),
                            },
                        );
                        return Ok(reply);
                    }
                }
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
            let phase = match subprocess.phase {
                RegisteredSubprocessPhase::Running => "running",
                RegisteredSubprocessPhase::RoundCompleted => "round_completed",
                RegisteredSubprocessPhase::ExitRequested => "exit_requested",
            };
            lines.push(format!(
                "- agent={} dispatch_id={} task_id={} phase={} task={}",
                subprocess.agent,
                subprocess.dispatch_id,
                subprocess.task_id,
                phase,
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
    ) -> Vec<crate::agent::llm::ToolDefinition> {
        let mut allowed = HashSet::from([
            "read_file",
            "write_file",
            "edit_file",
            "list_dir",
            "glob",
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

        self.tools.definitions_for_names(Some(allowed.into_iter()))
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

    fn ensure_dispatch_allowed(
        &self,
        workspace_id: &str,
        agent: &str,
        agent_label: &str,
    ) -> std::result::Result<(), String> {
        let existing = self
            .active_subprocesses_for_workspace(workspace_id)
            .into_iter()
            .find(|item| item.agent == agent);
        if let Some(item) = existing {
            let phase = match item.phase {
                RegisteredSubprocessPhase::Running => "running",
                RegisteredSubprocessPhase::RoundCompleted => "round_completed",
                RegisteredSubprocessPhase::ExitRequested => "exit_requested",
            };
            return Err(format!(
                "错误：当前会话已有一个活跃的 {agent_label} 子进程（dispatch_id={}, phase={}）。禁止再次调用 dispatch_{}；请改用 continue_{}_session、exit_{}_session，或直接回复用户。",
                item.dispatch_id, phase, agent, agent, agent
            ));
        }
        Ok(())
    }

    fn ensure_continue_allowed(
        &self,
        workspace_id: &str,
        agent: &str,
        agent_label: &str,
    ) -> std::result::Result<(), String> {
        let existing = self
            .active_subprocesses_for_workspace(workspace_id)
            .into_iter()
            .find(|item| item.agent == agent);
        match existing.map(|item| item.phase) {
            Some(RegisteredSubprocessPhase::Running)
            | Some(RegisteredSubprocessPhase::RoundCompleted) => Ok(()),
            Some(RegisteredSubprocessPhase::ExitRequested) => Err(format!(
                "错误：{agent_label} 子进程已收到退出请求，当前只能等待其结束，不能再继续注入指令。"
            )),
            None => Err(format!(
                "错误：当前会话没有可继续的 {agent_label} 活跃子进程。"
            )),
        }
    }

    fn ensure_exit_allowed(
        &self,
        workspace_id: &str,
        agent: &str,
        agent_label: &str,
    ) -> std::result::Result<(), String> {
        let existing = self
            .active_subprocesses_for_workspace(workspace_id)
            .into_iter()
            .find(|item| item.agent == agent);
        match existing.map(|item| item.phase) {
            Some(RegisteredSubprocessPhase::Running)
            | Some(RegisteredSubprocessPhase::RoundCompleted) => Ok(()),
            Some(RegisteredSubprocessPhase::ExitRequested) => Err(format!(
                "错误：{agent_label} 子进程已经收到退出命令，请等待进程结束，不要重复 exit。"
            )),
            None => Err(format!(
                "错误：当前会话没有可退出的 {agent_label} 活跃子进程。"
            )),
        }
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
                result: error.to_string(),
            },
        );
        db.add_visible_message_with_tools(
            workspace_id,
            "tool",
            error,
            Some(&tool_call.id),
            Some(&tool_call.name),
            None,
        )?;
        Ok(())
    }

    fn emit_ollama_failure_and_finish(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        error: &str,
    ) -> Result<DispatcherMessageRecord> {
        let reply = db.add_visible_message(
            workspace_id,
            "assistant",
            &build_ollama_install_message(error),
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

fn emit(on_event: &Channel<AgentEvent>, event: AgentEvent) {
    let _ = on_event.send(event);
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
