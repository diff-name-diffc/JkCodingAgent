use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use tauri::ipc::Channel;

use super::config::DispatcherAgentConfig;
use super::db::{DispatcherDb, DispatcherMessageRecord, DispatcherSettingsRecord};
use super::llm::{ChatMessage, FunctionCall, OpenAiCompatProvider, OutboundToolCall};
use super::prompt::build_system_prompt;
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
            Self::RoundCompleted => {
                "🔄 子任务当前轮次已完成"
            }
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
        let result_msg =
            db.add_visible_message(workspace_id, "assistant", dispatch_state.visible_message())?;
        emit(
            &on_event,
            AgentEvent::AssistantMessage {
                message: result_msg.clone(),
            },
        );

        db.add_hidden_message(
            workspace_id,
            "user",
            &format!(
                "{}\n\n{}",
                dispatch_state.hidden_prefix(),
                summarize_dispatch_result(dispatch_result)
            ),
            None,
            None,
            None,
        )?;

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
        let tool_definitions = self.tools.definitions();
        let tool_context = ToolContext {
            workspace: workspace.to_path_buf(),
            max_result_chars: self.config.max_tool_result_chars,
            exec_timeout_secs: self.config.exec_timeout_secs,
            restrict_to_workspace: self.config.restrict_to_workspace,
        };

        for _ in 0..self.config.max_tool_iterations {
            let mut messages = vec![ChatMessage::system(self.build_system_prompt()?)];
            messages.extend(db.load_llm_history(workspace_id)?);

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
                    if is_exit_instruction(&result, agent) {
                        if let Some(reason) = parse_exit_instruction(&result, agent) {
                            let agent_label = agent.display_name();
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

                            continue;
                        }
                    }
                }

                emit(
                    on_event,
                    AgentEvent::ToolFinished {
                        tool_call_id: Some(tool_call.id.clone()),
                        name: tool_call.name.clone(),
                        result: result.clone(),
                    },
                );

                db.add_visible_message_with_tools(
                    workspace_id,
                    "tool",
                    &result,
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

    fn build_system_prompt(&self) -> Result<String> {
        build_system_prompt(&self.config.root_dir)
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

fn summarize_dispatch_result(dispatch_result: &str) -> String {
    let trimmed = dispatch_result.trim();
    if trimmed.is_empty() {
        return "【子任务回流摘要】\n- 无可用输出".to_string();
    }

    let (headline, output) = trimmed
        .split_once("\n\n终端输出：\n")
        .map(|(left, right)| (left.trim(), right.trim()))
        .unwrap_or((trimmed, ""));

    let mut sections = vec!["【子任务回流摘要】".to_string()];
    sections.push(format!(
        "- 状态：{}",
        truncate_for_display(headline, 200, "...")
    ));

    if output.is_empty() {
        sections.push("- 关键输出：无终端输出".to_string());
        return sections.join("\n");
    }

    let key_lines = extract_key_output_lines(output);
    let snippet = if key_lines.is_empty() {
        truncate_for_display(output, 1200, "...")
    } else {
        truncate_for_display(&key_lines.join("\n"), 2000, "\n...")
    };

    sections.push("- 关键输出：".to_string());
    sections.push(snippet);
    sections.join("\n")
}

fn extract_key_output_lines(output: &str) -> Vec<String> {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    let mut result = Vec::new();
    for line in &lines {
        if looks_like_key_output_line(line) {
            let candidate = (*line).to_string();
            if result.last() != Some(&candidate) {
                result.push(candidate);
            }
            if result.len() >= 12 {
                break;
            }
        }
    }

    if !result.is_empty() {
        return result;
    }

    lines
        .iter()
        .rev()
        .take(12)
        .rev()
        .map(|line| (*line).to_string())
        .collect()
}

fn looks_like_key_output_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "error",
        "failed",
        "panic",
        "exception",
        "traceback",
        "warning",
        "assert",
        "test",
        "build",
        "lint",
        "成功",
        "失败",
        "报错",
        "错误",
        "警告",
        "通过",
        "未通过",
    ]
    .iter()
    .any(|keyword| lower.contains(keyword))
}
