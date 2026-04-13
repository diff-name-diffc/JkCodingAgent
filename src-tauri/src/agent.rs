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
    parse_continue_instruction, parse_dispatch_instruction, parse_exit_instruction, ToolContext,
    ToolRegistry,
};

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
        description: String,
        permission_mode: String,
    },
    /// 用户批准后，Claude 进程已启动
    DispatchStarted {
        dispatch_id: String,
        task_id: String,
    },
    /// Claude 进程结束，输出摘要
    DispatchFinished {
        dispatch_id: String,
        result: String,
    },
    /// Dispatcher Agent 要求继续 Claude 会话
    DispatchContinue {
        dispatch_id: String,
        text: String,
    },
    /// Dispatcher Agent 要求退出 Claude 会话
    DispatchExit {
        dispatch_id: String,
        reason: String,
    },
    Finished {
        messages: Vec<DispatcherMessageRecord>,
    },
    Error {
        message: String,
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

                // 拦截 dispatch_claude 的特殊返回值
                if tool_call.name == "dispatch_claude" && is_dispatch_instruction(&result) {
                    if let Some((description, permission_mode)) =
                        parse_dispatch_instruction(&result)
                    {
                        let dispatch_id = uuid::Uuid::new_v4().to_string();

                        // 发送待审查事件给前端
                        emit(
                            on_event,
                            AgentEvent::DispatchProposed {
                                dispatch_id: dispatch_id.clone(),
                                description: description.clone(),
                                permission_mode: permission_mode.clone(),
                            },
                        );

                        // 将 dispatch 指令作为 tool result 记录（前端会通过
                        // dispatcher_approve_dispatch 或 dispatcher_reject_dispatch
                        // 来注入真正的结果）
                        let dispatch_result = format!(
                            "[Claude 子任务已提交审查] dispatch_id={}, 任务: {}",
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

                        // 在这里不继续循环 — 等待前端通过 dispatcher_inject_result
                        // 注入 Claude 的执行结果后，由前端再次调用 dispatcher_continue_after_dispatch
                        // 来继续 Agent 循环。
                        // 为此，返回一条临时的 "等待中" 消息。
                        let waiting_content = if self.auto_approve_dispatch() {
                            format!(
                                "📋 已自动批准 Claude 子任务，正在执行...\n\n**任务描述：**\n{}",
                                description
                            )
                        } else {
                            format!(
                                "📋 已提交 Claude 子任务审查，等待执行...\n\n**任务描述：**\n{}",
                                description
                            )
                        };
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
                }

                // 拦截 continue_claude_session 的特殊返回值
                if tool_call.name == "continue_claude_session" && is_continue_instruction(&result) {
                    if let Some(text) = parse_continue_instruction(&result) {
                        let dispatch_id = "active".to_string();

                        emit(
                            on_event,
                            AgentEvent::DispatchContinue {
                                dispatch_id: dispatch_id.clone(),
                                text: text.clone(),
                            },
                        );

                        let continue_result = format!(
                            "[已发送后续指令到 Claude 会话] 指令: {}",
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

                        // 返回等待消息，前端会向终端注入文本并等待下一次 idle 触发
                        let waiting_msg = db.add_visible_message(
                            workspace_id,
                            "assistant",
                            &format!(
                                "📨 已向 Claude 发送后续指令，等待执行...\n\n**指令内容：**\n{}",
                                text
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

                // 拦截 exit_claude_session 的特殊返回值
                if tool_call.name == "exit_claude_session" && is_exit_instruction(&result) {
                    if let Some(reason) = parse_exit_instruction(&result) {
                        let dispatch_id = "active".to_string();

                        emit(
                            on_event,
                            AgentEvent::DispatchExit {
                                dispatch_id: dispatch_id.clone(),
                                reason: reason.clone(),
                            },
                        );

                        let exit_result =
                            format!("[已发送退出命令到 Claude 会话] 原因: {}", reason);

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

                        // 不返回等待消息，直接继续 agent 循环
                        // 前端会向终端注入 /exit 并标记为主动退出
                        continue;
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

    /// Claude 子任务完成后，将结果注入对话并继续 Agent 循环
    pub async fn continue_after_dispatch(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace_path: &str,
        dispatch_result: &str,
        on_event: Channel<AgentEvent>,
    ) -> Result<AgentTurn> {
        // 将 Claude 的执行结果作为简短提示追加
        let result_msg = db.add_visible_message(
            workspace_id,
            "assistant",
            "✅ Claude 子任务已完成，执行结果已在后台同步供后续分析。",
        )?;
        emit(
            &on_event,
            AgentEvent::AssistantMessage {
                message: result_msg.clone(),
            },
        );

        // 同时将结果作为隐藏的 system 消息加入 LLM 上下文
        db.add_hidden_message(
            workspace_id,
            "user",
            &format!(
                "[系统通知] Claude 子任务执行结果如下，请根据结果给用户总结反馈：\n\n{}",
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
