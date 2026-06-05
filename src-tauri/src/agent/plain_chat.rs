use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use parking_lot::Mutex;
use tauri::{ipc::Channel, AppHandle};
use tokio::sync::watch;

use super::common::{
    self, build_args_map, build_tool_calls_payload, cancellation_requested,
    persist_assistant_message, persist_tool_calls_message, persist_tool_result,
    select_provider_for_messages, stream_llm_response, LlmStreamOutcome, UsageTracker,
};
use super::config::DispatcherAgentConfig;
use super::db::{
    DispatcherDb, DispatcherMessageRecord,
    DispatcherSessionTokenUsageSource, DispatcherSettingsRecord,
};
use super::llm::{
    ChatMessage, LlmResponse, OpenAiCompatProvider,
    RequestedToolCall, ToolDefinition,
};
use super::runtime::{AgentEvent, AgentTurn};
use super::tools::ToolRegistry;
use crate::shared::truncate_for_display;

pub struct PlainChatAgent {
    config: DispatcherAgentConfig,
    provider: Mutex<OpenAiCompatProvider>,
    vision_model: Mutex<String>,
    app_handle: Option<AppHandle>,
    tools: ToolRegistry,
}

impl PlainChatAgent {
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
            vision_model: Mutex::new(String::new()),
            app_handle: None,
            tools: ToolRegistry::plain_chat_tools(),
        }
    }

    pub fn with_app_handle(mut self, app_handle: AppHandle) -> Self {
        self.app_handle = Some(app_handle);
        self
    }

    pub fn apply_settings(&self, settings: &DispatcherSettingsRecord) {
        {
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
        }
        if !settings.vision_model.trim().is_empty() {
            *self.vision_model.lock() = settings.vision_model.trim().to_string();
        }
    }

    pub async fn run(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        user_message: &str,
        user_segments_json: Option<String>,
        enable_thinking: bool,
        on_event: Channel<AgentEvent>,
        cancel_rx: watch::Receiver<bool>,
    ) -> Result<AgentTurn> {
        common::emit(
            &on_event,
            AgentEvent::Started {
                workspace_id: workspace_id.to_string(),
            },
        );

        let user = db
            .add_visible_message_async(workspace_id, "user", user_message, user_segments_json)
            .await?;
        common::emit(&on_event, AgentEvent::UserMessage { message: user });

        let provider = self.provider.lock().clone();
        if !provider.is_configured() {
            anyhow::bail!(
                "聊天 LLM API Key 未配置。请在设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。"
            );
        }

        let mut usage_tracker = UsageTracker::new();
        let workspace = self.browser_workspace().await?;
        let reply = self
            .run_loop(
                db,
                workspace_id,
                &workspace,
                &on_event,
                &provider,
                enable_thinking,
                cancel_rx,
                &mut usage_tracker,
            )
            .await?;

        let messages = db.list_visible_messages_async(workspace_id).await?;
        common::emit(
            &on_event,
            AgentEvent::Finished {
                messages: messages.clone(),
            },
        );
        Ok(AgentTurn { reply, messages })
    }

    /// Core agent loop: stream → execute tools → loop until no tools or cancelled.
    #[allow(clippy::too_many_arguments)]
    async fn run_loop(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &Path,
        on_event: &Channel<AgentEvent>,
        provider: &OpenAiCompatProvider,
        enable_thinking: bool,
        cancel_rx: watch::Receiver<bool>,
        usage_tracker: &mut UsageTracker,
    ) -> Result<DispatcherMessageRecord> {
        let tool_context = self.build_tool_context(db, workspace_id, workspace, provider).await;
        let tool_definitions = self.build_tool_definitions(workspace);
        let allowed_tool_names = tool_definitions
            .iter()
            .map(|t| t.function.name.clone())
            .collect::<std::collections::HashSet<_>>();

        for iteration in 0..self.config.max_tool_iterations {
            if cancellation_requested(&cancel_rx) {
                return emit_stop_and_finish(db, workspace_id, on_event, "", usage_tracker).await;
            }

            let history_messages = db.load_llm_history_async(workspace_id).await?;
            let vision_model = self.vision_model.lock().clone();
            let request_provider = select_provider_for_messages(
                provider,
                &history_messages,
                &vision_model,
                on_event,
                iteration == 0,
            )?;

            let mut messages = vec![ChatMessage::system(build_plain_chat_system_prompt())];
            messages.extend(history_messages);

            // Stream LLM response
            let response = match stream_llm_response(
                db,
                workspace_id,
                request_provider.model(),
                DispatcherSessionTokenUsageSource::Primary,
                usage_tracker,
                on_event,
                &request_provider,
                &messages,
                &tool_definitions,
                enable_thinking,
                cancel_rx.clone(),
            )
            .await?
            {
                LlmStreamOutcome::Cancelled(partial) => {
                    return emit_stop_and_finish(
                        db,
                        workspace_id,
                        on_event,
                        &partial,
                        usage_tracker,
                    )
                    .await;
                }
                LlmStreamOutcome::Response(response) => response,
            };

            // If no tool calls, persist final message and return
            if response.tool_calls.is_empty() {
                let content = response.content.trim().to_string();
                if content.is_empty() {
                    anyhow::bail!(
                        "{}",
                        empty_plain_chat_response_error(
                            &response,
                            &request_provider,
                            tool_definitions.len(),
                            enable_thinking,
                        )
                    );
                }
                let usage_stats = usage_tracker.snapshot();
                let reply =
                    persist_assistant_message(db, workspace_id, &content, &usage_stats).await?;
                common::emit(
                    on_event,
                    AgentEvent::AssistantMessage {
                        message: reply.clone(),
                    },
                );
                return Ok(reply);
            }

            // Persist tool_call message, emit ToolPlanned events
            let tool_calls_payload = build_tool_calls_payload(&response.tool_calls);
            let args_map = build_args_map(&response.tool_calls);

            for tc in &tool_calls_payload {
                common::emit(
                    on_event,
                    AgentEvent::ToolPlanned {
                        tool_call_id: Some(tc.id.clone()),
                        name: tc.function.name.clone(),
                        arguments: tc.function.arguments.clone(),
                    },
                );
            }

            persist_tool_calls_message(
                db,
                workspace_id,
                &response.content,
                &tool_calls_payload,
                &response.thinking_content,
                Some(response.thinking_elapsed_ms),
            )
            .await?;

            // Execute tool calls sequentially (with parallel for readonly)
            let executed = self
                .execute_all_tools(
                    &response.tool_calls,
                    &args_map,
                    &tool_context,
                    &allowed_tool_names,
                    on_event,
                    &cancel_rx,
                )
                .await?;

            // Persist each tool result with rule-based truncation
            for (tool_call, result) in &executed {
                if cancellation_requested(&cancel_rx) {
                    break;
                }
                persist_tool_result(db, workspace_id, on_event, tool_call, result).await?;
            }
        }

        anyhow::bail!(
            "已达到最大工具迭代次数（{}），本轮聊天被终止。请检查模型是否陷入工具调用循环。",
            self.config.max_tool_iterations
        )
    }

    async fn execute_all_tools(
        &self,
        tool_calls: &[RequestedToolCall],
        args_map: &std::collections::HashMap<String, String>,
        tool_context: &super::tools::ToolContext,
        allowed_tool_names: &std::collections::HashSet<String>,
        on_event: &Channel<AgentEvent>,
        cancel_rx: &watch::Receiver<bool>,
    ) -> Result<Vec<(RequestedToolCall, String)>> {
        let readonly_end = common::readonly_tool_run_end(tool_calls, 0);

        if readonly_end >= 2 {
            let readonly_run = &tool_calls[..readonly_end];
            for tool_call in readonly_run {
                common::emit(
                    on_event,
                    AgentEvent::ToolStarted {
                        tool_call_id: Some(tool_call.id.clone()),
                        name: tool_call.name.clone(),
                        arguments: args_map
                            .get(&tool_call.id)
                            .cloned()
                            .unwrap_or_else(|| "{}".to_string()),
                    },
                );
            }

            let mut results = Vec::with_capacity(tool_calls.len());

            let readonly_results: Vec<String> =
                futures::future::join_all(readonly_run.iter().map(|tool_call| async move {
                    if allowed_tool_names.contains(&tool_call.name) {
                        self.tools
                            .execute(&tool_call.name, &tool_call.arguments, tool_context)
                            .await
                    } else {
                        format!("错误：禁止调用工具 '{}'；请检查可用工具列表。", tool_call.name)
                    }
                }))
                .await;

            for (tool_call, result) in readonly_run.iter().zip(readonly_results) {
                if cancellation_requested(cancel_rx) {
                    return Ok(results);
                }
                results.push((tool_call.clone(), result));
            }

            let remaining = &tool_calls[readonly_end..];
            for tool_call in remaining {
                if cancellation_requested(cancel_rx) {
                    break;
                }
                common::emit(
                    on_event,
                    AgentEvent::ToolStarted {
                        tool_call_id: Some(tool_call.id.clone()),
                        name: tool_call.name.clone(),
                        arguments: args_map
                            .get(&tool_call.id)
                            .cloned()
                            .unwrap_or_else(|| "{}".to_string()),
                    },
                );
                let result = if allowed_tool_names.contains(&tool_call.name) {
                    self.tools
                        .execute(&tool_call.name, &tool_call.arguments, tool_context)
                        .await
                } else {
                    format!("错误：禁止调用工具 '{}'；请检查可用工具列表。", tool_call.name)
                };
                results.push((tool_call.clone(), result));
            }

            Ok(results)
        } else {
            let mut results = Vec::with_capacity(tool_calls.len());
            for tool_call in tool_calls {
                if cancellation_requested(cancel_rx) {
                    break;
                }
                common::emit(
                    on_event,
                    AgentEvent::ToolStarted {
                        tool_call_id: Some(tool_call.id.clone()),
                        name: tool_call.name.clone(),
                        arguments: args_map
                            .get(&tool_call.id)
                            .cloned()
                            .unwrap_or_else(|| "{}".to_string()),
                    },
                );
                let result = if allowed_tool_names.contains(&tool_call.name) {
                    self.tools
                        .execute(&tool_call.name, &tool_call.arguments, tool_context)
                        .await
                } else {
                    format!("错误：禁止调用工具 '{}'；请检查可用工具列表。", tool_call.name)
                };
                results.push((tool_call.clone(), result));
            }
            Ok(results)
        }
    }

    async fn browser_workspace(&self) -> Result<PathBuf> {
        let workspace = self.config.root_dir.join("plain-chat-browser");
        let config_dir = workspace.join(".jkcodingagent");
        let create_dir = config_dir.clone();
        tokio::task::spawn_blocking(move || fs::create_dir_all(&create_dir))
            .await
            .map_err(|error| {
                anyhow::anyhow!("create plain chat browser workspace panicked: {error}")
            })?
            .with_context(|| format!("create {}", config_dir.display()))?;
        Ok(workspace)
    }

    async fn build_tool_context(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &Path,
        provider: &OpenAiCompatProvider,
    ) -> super::tools::ToolContext {
        let session_title = db
            .get_session_title_async(workspace_id)
            .await
            .unwrap_or_else(|_| "untitled".to_string());
        super::tools::ToolContext {
            workspace_id: workspace_id.to_string(),
            workspace: workspace.to_path_buf(),
            session_title,
            exec_timeout_secs: self.config.exec_timeout_secs,
            restrict_to_workspace: true,
            extra_allowed_dirs: dirs::home_dir()
                .map(|home| vec![home.join(".jkcodingagent")])
                .unwrap_or_default(),
            app_handle: self.app_handle.clone(),
            llm_provider: Some(provider.clone()),
            vision_model: self.vision_model.lock().clone(),
            image_model_url: String::new(),
            image_model_api_key: String::new(),
            image_model: String::new(),
            image_edit_model: String::new(),
        }
    }

    fn build_tool_definitions(&self, workspace: &Path) -> Vec<ToolDefinition> {
        self.tools
            .definitions_for_workspace(workspace, Option::<std::iter::Empty<&str>>::None, false)
    }
}

fn empty_plain_chat_response_error(
    response: &LlmResponse,
    provider: &OpenAiCompatProvider,
    tool_count: usize,
    enable_thinking: bool,
) -> String {
    let raw_response = response.raw_response.trim();
    let response_detail = if raw_response.is_empty() {
        "<空>".to_string()
    } else {
        truncate_for_display(raw_response, 4_000, "\n...[LLM 接口响应内容已截断]")
    };

    format!(
        "LLM 返回了空响应且没有工具调用，无法继续执行。\n请求摘要：model={}, tools={}, enable_thinking={}\nLLM 接口响应内容：\n{}",
        provider.model(),
        tool_count,
        enable_thinking,
        response_detail
    )
}

async fn emit_stop_and_finish(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    partial: &str,
    usage_tracker: &UsageTracker,
) -> Result<DispatcherMessageRecord> {
    let content = build_stopped_plain_chat_reply(partial);
    let usage_stats = usage_tracker.snapshot();
    let reply = persist_assistant_message(db, workspace_id, &content, &usage_stats).await?;
    common::emit(
        on_event,
        AgentEvent::AssistantMessage {
            message: reply.clone(),
        },
    );
    Ok(reply)
}

fn build_stopped_plain_chat_reply(partial: &str) -> String {
    let trimmed = partial.trim();
    if trimmed.is_empty() {
        "⏹️ 本轮聊天已停止。当前会话上下文已保留，可稍后继续。".to_string()
    } else {
        format!(
            "{}\n\n[本轮聊天已手动停止。当前会话上下文与以上输出均已保留，可稍后继续。]",
            trimmed
        )
    }
}

fn build_plain_chat_system_prompt() -> String {
    [
        "# 普通聊天",
        "",
        "你是桌面客户端中的普通聊天助手。",
        "当前会话不是项目 Agent 会话，没有项目目录、文件系统、终端、MCP 或子进程能力。",
        "你可以按需使用浏览器工具打开网页、点击、输入、等待、读取页面可访问性树快照、请求视觉辅助分析和关闭浏览器，用于网页自动化与公开信息检索。",
        "需要当前日期、时间、时区或时间戳时，调用 get_current_time，不要猜测系统时间。",
        "浏览器自动化统一使用 ref：先调用 browser_read_text 获取 Accessibility Tree 快照，再使用快照中的 ref 调用点击、输入或局部读取工具；不要使用 CSS selector。",
        "浏览器工具只代表当前普通聊天会话中的临时浏览器，不代表用户本地项目环境。",
        "检索问题信息时，优先打开明确网址；没有网址时可打开搜索引擎结果页并读取页面文本，不要伪造检索结果。",
        "不要声称已经读取、修改或执行了本地文件；如果用户要求操作项目或文件，请说明普通聊天不具备该能力，并建议切换到项目会话。",
        "可以基于用户直接提供的文本、代码片段、错误信息或图片进行解释、分析、改写和建议。",
        "默认使用简体中文，表达直接、清晰、面向有经验的开发者。",
        "",
        "## 图片生成与引用",
        "",
        "- 你可以调用 generate_image 工具根据文本描述生成图片。建议提供 image_name 参数为图片命名。",
        "- 你可以调用 edit_image 工具对现有图片进行编辑。需要提供图片的本地绝对路径。",
        "- 工具返回结果中会包含该图片的本地绝对路径。",
        "- 如果你想在回答中展示生成的图片，直接使用 Markdown 图片引用语法引用工具返回的原始本地绝对路径即可。",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{build_plain_chat_system_prompt, PlainChatAgent};
    use crate::agent::config::DispatcherAgentConfig;
    use std::path::PathBuf;

    fn test_config(root: PathBuf) -> DispatcherAgentConfig {
        DispatcherAgentConfig {
            root_dir: root.clone(),
            db_path: root.join("db.sqlite3"),
            api_key: "test-key".to_string(),
            api_base: "https://example.com/v1".to_string(),
            model: "test-model".to_string(),
            summary_model: "summary".to_string(),
            vision_model: "vision".to_string(),
            image_model_url: "https://example.com/images".to_string(),
            image_model_api_key: "image-key".to_string(),
            image_model: "image".to_string(),
            image_edit_model: "image-edit".to_string(),
            max_tokens: 1024,
            temperature: 0.1,
            max_tool_iterations: 8,
            exec_timeout_secs: 60,
            restrict_to_workspace: true,
            auto_approve_dispatch: false,
            context_debug: false,
        }
    }

    #[test]
    fn plain_chat_prompt_excludes_project_runtime_capabilities() {
        let prompt = build_plain_chat_system_prompt();

        assert!(prompt.contains("普通聊天"));
        assert!(prompt.contains("没有项目目录"));
        assert!(!prompt.contains("dispatch_claude"));
        assert!(!prompt.contains("update_plan"));
    }

    #[test]
    fn plain_chat_agent_has_no_project_tools_or_dynamic_mcp() {
        let root =
            std::env::temp_dir().join(format!("plain-chat-agent-tools-{}", uuid::Uuid::new_v4()));
        let agent = PlainChatAgent::new(test_config(root.clone()));
        let tools = agent
            .build_tool_definitions(&root)
            .into_iter()
            .map(|tool| tool.function.name)
            .collect::<Vec<_>>();

        assert!(tools.iter().any(|name| name == "browser_open_url"));
        assert!(tools.iter().any(|name| name == "get_current_time"));
        assert!(tools.iter().any(|name| name == "generate_image"));
        assert!(!tools.iter().any(|name| name == "read_file"));
        assert!(!tools.iter().any(|name| name == "exec"));
        assert!(!tools.iter().any(|name| name == "dispatch_claude"));
        assert!(!tools.iter().any(|name| name == "update_plan"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
