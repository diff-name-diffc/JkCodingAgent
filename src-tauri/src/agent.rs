use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use tauri::ipc::Channel;

use crate::dispatcher_config::DispatcherAgentConfig;
use crate::dispatcher_db::{DispatcherDb, DispatcherMessageRecord, DispatcherSettingsRecord};
use crate::dispatcher_llm::{ChatMessage, FunctionCall, OpenAiCompatProvider, OutboundToolCall};
use crate::dispatcher_tools::{
    is_continue_instruction, is_dispatch_instruction, is_exit_instruction,
    parse_continue_instruction, parse_dispatch_instruction, parse_exit_instruction, DispatchAgent,
    ToolContext, ToolRegistry,
};

const BUILT_IN_DISPATCH_GUIDANCE: &str = r#"# Built-in Dispatch Guidance

- Use `dispatch_claude` for tasks where speed matters more: greenfield features, algorithm experiments, debugging exploration, and broad implementation search.
- Use `dispatch_codex` for tasks where extra care matters more: refactoring, structural cleanup, consistency passes, and regression-sensitive changes.
- When continuing or exiting a subprocess, always use the matching tool for the same agent family (`continue_claude_session` / `continue_codex_session`, `exit_claude_session` / `exit_codex_session`).
"#;

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
    /// 当 dispatch_claude 被调用时，发送给前端审查
    DispatchProposed {
        dispatch_id: String,
        agent: String,
        description: String,
        permission_mode: String,
    },
    /// Dispatcher Agent 要求继续指定 agent 的活动会话
    DispatchContinue {
        dispatch_id: String,
        agent: String,
        text: String,
    },
    /// Dispatcher Agent 要求退出指定 agent 的活动会话
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

    /// Apply runtime settings from DB, overriding the env-var defaults.
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
            messages.extend(db.load_llm_history(workspace_id, 500)?);

            // Generate a message ID for streaming
            let stream_msg_id = uuid::Uuid::new_v4().to_string();
            emit(
                on_event,
                AgentEvent::AssistantStarted {
                    message_id: stream_msg_id.clone(),
                },
            );

            // Streaming callback
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
                // Pure text reply
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

            // LLM wants to call tools
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

                if let Some(agent) = dispatch_agent_for_tool_name(&tool_call.name) {
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
                                if description.len() > 200 {
                                    format!("{}...", &description[..200])
                                } else {
                                    description.clone()
                                }
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

                if let Some(agent) = continue_agent_for_tool_name(&tool_call.name) {
                    if is_continue_instruction(&result, agent) {
                        if let Some(text) = parse_continue_instruction(&result, agent) {
                            let dispatch_id = "active".to_string();
                            let agent_label = agent.display_name();

                            emit(
                                on_event,
                                AgentEvent::DispatchContinue {
                                    dispatch_id: dispatch_id.clone(),
                                    agent: agent.slug().to_string(),
                                    text: text.clone(),
                                },
                            );

                            let continue_result = format!(
                                "[已发送后续指令到 {} 会话] 指令: {}",
                                agent_label,
                                if text.len() > 200 {
                                    format!("{}...", &text[..200])
                                } else {
                                    text.clone()
                                }
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

                if let Some(agent) = exit_agent_for_tool_name(&tool_call.name) {
                    if is_exit_instruction(&result, agent) {
                        if let Some(reason) = parse_exit_instruction(&result, agent) {
                            let dispatch_id = "active".to_string();
                            let agent_label = agent.display_name();

                            emit(
                                on_event,
                                AgentEvent::DispatchExit {
                                    dispatch_id: dispatch_id.clone(),
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

    /// 子任务完成后，将结果注入对话并继续 Agent 循环
    pub async fn continue_after_dispatch(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace_path: &str,
        dispatch_result: &str,
        on_event: Channel<AgentEvent>,
    ) -> Result<AgentTurn> {
        // 将子任务的执行结果作为简短提示追加
        let result_msg = db.add_visible_message(
            workspace_id,
            "assistant",
            "✅ 子任务已完成，执行结果已在后台同步供后续分析。",
        )?;
        emit(
            &on_event,
            AgentEvent::AssistantMessage {
                message: result_msg.clone(),
            },
        );

        // 同时将结果作为隐藏消息加入 LLM 上下文
        db.add_hidden_message(
            workspace_id,
            "user",
            &format!(
                "[系统通知] 子任务执行结果如下，请根据结果给用户总结反馈：\n\n{}",
                dispatch_result
            ),
            None,
            None,
            None,
        )?;

        // 继续 Agent 循环，让主 Agent 分析 Claude 的结果
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

    fn build_system_prompt(&self) -> Result<String> {
        build_system_prompt(&self.config.root_dir)
    }
}

fn dispatch_agent_for_tool_name(name: &str) -> Option<DispatchAgent> {
    match name {
        "dispatch_claude" => Some(DispatchAgent::Claude),
        "dispatch_codex" => Some(DispatchAgent::Codex),
        _ => None,
    }
}

fn continue_agent_for_tool_name(name: &str) -> Option<DispatchAgent> {
    match name {
        "continue_claude_session" => Some(DispatchAgent::Claude),
        "continue_codex_session" => Some(DispatchAgent::Codex),
        _ => None,
    }
}

fn exit_agent_for_tool_name(name: &str) -> Option<DispatchAgent> {
    match name {
        "exit_claude_session" => Some(DispatchAgent::Claude),
        "exit_codex_session" => Some(DispatchAgent::Codex),
        _ => None,
    }
}

fn build_system_prompt(root: &Path) -> Result<String> {
    let mut parts = Vec::new();

    push_file_if_exists(&mut parts, root.join("SOUL.md"))?;
    push_file_if_exists(&mut parts, root.join("USER.md"))?;
    push_file_if_exists(&mut parts, root.join("TOOLS.md"))?;

    let skills_dir = root.join("skills");
    if skills_dir.exists() {
        let mut skill_parts = Vec::new();
        for entry in
            fs::read_dir(&skills_dir).with_context(|| format!("read {}", skills_dir.display()))?
        {
            let entry = entry?;
            let skill_md = entry.path().join("SKILL.md");
            if skill_md.exists() {
                skill_parts.push(format!(
                    "### Skill: {}\n\n{}",
                    entry.file_name().to_string_lossy(),
                    fs::read_to_string(&skill_md)
                        .with_context(|| format!("read {}", skill_md.display()))?
                ));
            }
        }
        skill_parts.sort();
        if !skill_parts.is_empty() {
            parts.push(format!(
                "---\n\n# Active Skills\n\n{}",
                skill_parts.join("\n\n")
            ));
        }
    }

    let memory = root.join("memory").join("MEMORY.md");
    if memory.exists() {
        parts.push(format!(
            "---\n\n# Memory\n\n{}",
            fs::read_to_string(&memory).with_context(|| format!("read {}", memory.display()))?
        ));
    }

    parts.push(format!("---\n\n{}", BUILT_IN_DISPATCH_GUIDANCE));

    Ok(parts.join("\n\n---\n\n"))
}

fn push_file_if_exists(parts: &mut Vec<String>, path: PathBuf) -> Result<()> {
    if path.exists() {
        parts.push(fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?);
    }
    Ok(())
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
