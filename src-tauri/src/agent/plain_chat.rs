use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use tauri::{ipc::Channel, AppHandle};
use tokio::sync::watch;

use super::config::DispatcherAgentConfig;
use super::db::{
    DispatcherDb, DispatcherMessageRecord, DispatcherMessageUsageStats,
    DispatcherSessionTokenUsageSource, DispatcherSettingsRecord,
};
use super::llm::{
    messages_contain_inline_images, ChatMessage, FunctionCall, LlmResponse, LlmUsage,
    OpenAiCompatProvider, OutboundToolCall, RequestedToolCall,
};
use super::runtime::{AgentEvent, AgentTurn};
use super::tools::{ToolContext, ToolRegistry};
use crate::shared::truncate_for_display;

pub struct PlainChatAgent {
    config: DispatcherAgentConfig,
    provider: Mutex<OpenAiCompatProvider>,
    models: Mutex<PlainChatModels>,
    app_handle: Option<AppHandle>,
    tools: ToolRegistry,
}

struct PlainChatModels {
    vision_model: String,
    image_model_url: String,
    image_model_api_key: String,
    image_model: String,
    image_edit_model: String,
}

#[derive(Debug)]
struct PlainChatUsageTracker {
    started_at: Instant,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

enum PlainChatStreamOutcome {
    Cancelled(String),
    Response(LlmResponse),
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
            models: Mutex::new(PlainChatModels {
                vision_model: config.vision_model.trim().to_string(),
                image_model_url: config.image_model_url.clone(),
                image_model_api_key: config.image_model_api_key.clone(),
                image_model: config.image_model.clone(),
                image_edit_model: config.image_edit_model.clone(),
            }),
            config,
            provider: Mutex::new(provider),
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

        let mut models = self.models.lock();
        if !settings.vision_model.trim().is_empty() {
            models.vision_model = settings.vision_model.trim().to_string();
        }
        if !settings.image_model_url.trim().is_empty() {
            models.image_model_url = settings.image_model_url.trim().to_string();
        }
        if !settings.image_model_api_key.trim().is_empty() {
            models.image_model_api_key = settings.image_model_api_key.trim().to_string();
        }
        if !settings.image_model.trim().is_empty() {
            models.image_model = settings.image_model.trim().to_string();
        }
        if !settings.image_edit_model.trim().is_empty() {
            models.image_edit_model = settings.image_edit_model.trim().to_string();
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
        emit(
            &on_event,
            AgentEvent::Started {
                workspace_id: workspace_id.to_string(),
            },
        );

        let user = db
            .add_visible_message_async(workspace_id, "user", user_message, user_segments_json)
            .await?;
        emit(&on_event, AgentEvent::UserMessage { message: user });

        let provider = self.provider.lock().clone();
        if !provider.is_configured() {
            anyhow::bail!(
                "聊天 LLM API Key 未配置。请在设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。"
            );
        }

        let mut usage_tracker = PlainChatUsageTracker::new();
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
        emit(
            &on_event,
            AgentEvent::Finished {
                messages: messages.clone(),
            },
        );
        Ok(AgentTurn { reply, messages })
    }

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
        usage_tracker: &mut PlainChatUsageTracker,
    ) -> Result<DispatcherMessageRecord> {
        let tool_context = self
            .build_tool_context(db, workspace_id, workspace, provider)
            .await;
        let tool_definitions = self.tools.definitions_for_workspace(
            workspace,
            Option::<std::iter::Empty<&str>>::None,
            false,
        );

        for iteration in 0..self.config.max_tool_iterations {
            if cancellation_requested(&cancel_rx) {
                return emit_stop_and_finish(db, workspace_id, on_event, "", usage_tracker).await;
            }

            let history_messages = db.load_llm_history_async(workspace_id).await?;
            let request_provider =
                self.provider_for_messages(provider, &history_messages, on_event)?;
            let mut messages = vec![ChatMessage::system(build_plain_chat_system_prompt())];
            messages.extend(history_messages);

            let response = match self
                .stream_llm_response(
                    db,
                    workspace_id,
                    on_event,
                    &request_provider,
                    &messages,
                    &tool_definitions,
                    enable_thinking,
                    cancel_rx.clone(),
                    usage_tracker,
                    iteration,
                )
                .await?
            {
                PlainChatStreamOutcome::Cancelled(partial) => {
                    return emit_stop_and_finish(
                        db,
                        workspace_id,
                        on_event,
                        &partial,
                        usage_tracker,
                    )
                    .await;
                }
                PlainChatStreamOutcome::Response(response) => response,
            };

            if response.tool_calls.is_empty() {
                return handle_no_tool_response(
                    db,
                    workspace_id,
                    on_event,
                    &response,
                    usage_tracker,
                    &request_provider,
                    tool_definitions.len(),
                    enable_thinking,
                )
                .await;
            }

            let tool_calls =
                persist_assistant_tool_calls(db, workspace_id, on_event, response).await?;

            for (tool_call, tool_args_json) in tool_calls {
                if cancellation_requested(&cancel_rx) {
                    return emit_stop_and_finish(db, workspace_id, on_event, "", usage_tracker)
                        .await;
                }

                emit(
                    on_event,
                    AgentEvent::ToolStarted {
                        tool_call_id: Some(tool_call.id.clone()),
                        name: tool_call.name.clone(),
                        arguments: tool_args_json,
                    },
                );

                let result = self
                    .tools
                    .execute(&tool_call.name, &tool_call.arguments, &tool_context)
                    .await;

                persist_tool_result(db, workspace_id, on_event, &tool_call, &result).await?;
            }
        }

        anyhow::bail!(
            "已达到最大工具迭代次数（{}），本轮聊天被终止。请检查模型是否陷入工具调用循环。",
            self.config.max_tool_iterations
        )
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
    ) -> ToolContext {
        let session_title = db
            .get_session_title_async(workspace_id)
            .await
            .unwrap_or_else(|_| "untitled".to_string());
        let models = self.models.lock();
        ToolContext {
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
            vision_model: models.vision_model.clone(),
            image_model_url: models.image_model_url.clone(),
            image_model_api_key: models.image_model_api_key.clone(),
            image_model: models.image_model.clone(),
            image_edit_model: models.image_edit_model.clone(),
        }
    }

    fn provider_for_messages(
        &self,
        provider: &OpenAiCompatProvider,
        messages: &[ChatMessage],
        on_event: &Channel<AgentEvent>,
    ) -> Result<OpenAiCompatProvider> {
        if !messages_contain_inline_images(messages) {
            return Ok(provider.clone());
        }

        let vision_model = self.models.lock().vision_model.clone();
        if vision_model.trim().is_empty() {
            anyhow::bail!(
                "检测到用户上传了图片，但聊天设置中的视觉模型为空。请先配置视觉模型后重试。"
            );
        }

        let selected = provider.with_model(vision_model.trim());
        if selected.model() != provider.model() {
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

    #[allow(clippy::too_many_arguments)]
    async fn stream_llm_response(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        request_provider: &OpenAiCompatProvider,
        messages: &[ChatMessage],
        tool_definitions: &[crate::agent::llm::ToolDefinition],
        enable_thinking: bool,
        cancel_rx: watch::Receiver<bool>,
        usage_tracker: &mut PlainChatUsageTracker,
        _iteration: usize,
    ) -> Result<PlainChatStreamOutcome> {
        let stream_msg_id = uuid::Uuid::new_v4().to_string();
        emit(
            on_event,
            AgentEvent::AssistantStarted {
                message_id: stream_msg_id.clone(),
            },
        );

        let event_ref = on_event;
        let msg_id_ref = stream_msg_id.clone();
        let thinking_msg_id_ref = stream_msg_id.clone();
        let streamed_text = std::sync::Arc::new(Mutex::new(String::new()));
        let streamed_text_ref = std::sync::Arc::clone(&streamed_text);
        let on_delta = move |delta: &str| {
            let mut partial = streamed_text_ref.lock();
            partial.push_str(delta);
            let _ = event_ref.send(AgentEvent::AssistantDelta {
                message_id: msg_id_ref.clone(),
                delta: delta.to_string(),
            });
        };
        let thinking_event_ref = on_event;
        let on_thinking_delta = move |delta: &str, elapsed_ms: u64| {
            let _ = thinking_event_ref.send(AgentEvent::AssistantThinkingDelta {
                message_id: thinking_msg_id_ref.clone(),
                delta: delta.to_string(),
                elapsed_ms,
            });
        };

        let mut stream_cancel_rx = cancel_rx;
        let response = tokio::select! {
            _ = wait_for_cancellation(&mut stream_cancel_rx) => {
                let partial = streamed_text.lock().clone();
                return Ok(PlainChatStreamOutcome::Cancelled(partial));
            }
            response = request_provider.chat_stream_with_thinking(
                messages,
                tool_definitions,
                messages_contain_inline_images(messages),
                enable_thinking,
                on_delta,
                on_thinking_delta,
            ) => response
        }?;

        if let Some(usage) = response.usage.as_ref() {
            record_run_token_usage(
                db,
                workspace_id,
                request_provider.model(),
                usage,
                usage_tracker,
                on_event,
            );
        }

        Ok(PlainChatStreamOutcome::Response(response))
    }
}

impl PlainChatUsageTracker {
    fn new() -> Self {
        Self {
            started_at: Instant::now(),
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        }
    }

    fn record(&mut self, usage: &LlmUsage) -> DispatcherMessageUsageStats {
        self.prompt_tokens += usage.prompt_tokens;
        self.completion_tokens += usage.completion_tokens;
        self.total_tokens += normalized_total_tokens(usage);
        self.snapshot()
    }

    fn snapshot(&self) -> DispatcherMessageUsageStats {
        DispatcherMessageUsageStats {
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            total_tokens: self.total_tokens,
            elapsed_ms: self.started_at.elapsed().as_millis() as u64,
        }
    }
}

async fn persist_assistant_tool_calls(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    response: LlmResponse,
) -> Result<Vec<(RequestedToolCall, String)>> {
    let tool_calls = response.tool_calls;
    let tool_calls_payload: Vec<OutboundToolCall> = tool_calls
        .iter()
        .map(|call| {
            let args_json = serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".into());
            OutboundToolCall {
                id: call.id.clone(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: call.name.clone(),
                    arguments: args_json,
                },
            }
        })
        .collect();

    for tc in &tool_calls_payload {
        emit(
            on_event,
            AgentEvent::ToolPlanned {
                tool_call_id: Some(tc.id.clone()),
                name: tc.function.name.clone(),
                arguments: tc.function.arguments.clone(),
            },
        );
    }

    db.add_visible_message_with_tools_and_thinking_async(
        workspace_id,
        "assistant",
        &response.content,
        None,
        None,
        None,
        Some(&tool_calls_payload),
        Some(&response.thinking_content),
        response.thinking_elapsed_ms,
    )
    .await?;

    Ok(tool_calls
        .into_iter()
        .map(|call| {
            let args = serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".into());
            (call, args)
        })
        .collect())
}

async fn persist_tool_result(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    tool_call: &RequestedToolCall,
    result: &str,
) -> Result<()> {
    let tool_message = db
        .add_visible_tool_result_async(
            workspace_id,
            result,
            result,
            Some(&tool_call.id),
            Some(&tool_call.name),
            Some("raw"),
            &[],
        )
        .await?;
    emit(
        on_event,
        AgentEvent::ToolFinished {
            tool_call_id: Some(tool_call.id.clone()),
            name: tool_call.name.clone(),
            display_text: tool_message.content,
            result_mode: "raw".to_string(),
            detail_refs: Vec::new(),
        },
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_no_tool_response(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    response: &LlmResponse,
    usage_tracker: &PlainChatUsageTracker,
    provider: &OpenAiCompatProvider,
    tool_count: usize,
    enable_thinking: bool,
) -> Result<DispatcherMessageRecord> {
    let content = response.content.trim().to_string();
    if content.is_empty() {
        anyhow::bail!(
            "{}",
            empty_plain_chat_response_error(response, provider, tool_count, enable_thinking)
        );
    }
    let usage_stats = usage_tracker.snapshot();
    let reply = db
        .add_visible_message_with_usage_and_thinking_async(
            workspace_id,
            "assistant",
            &content,
            &usage_stats,
            Some(&response.thinking_content),
            response.thinking_elapsed_ms,
        )
        .await?;
    emit(
        on_event,
        AgentEvent::AssistantMessage {
            message: reply.clone(),
        },
    );
    Ok(reply)
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
    usage_tracker: &PlainChatUsageTracker,
) -> Result<DispatcherMessageRecord> {
    let content = build_stopped_plain_chat_reply(partial);
    let usage_stats = usage_tracker.snapshot();
    let reply = db
        .add_visible_message_with_usage_async(workspace_id, "assistant", &content, &usage_stats)
        .await?;
    emit(
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
        "- 你可以调用 generate_image 工具根据文本描述生成图片。建议提供 image_name 参数为图片指定可读的文件名（如 'logo-design'），否则会使用随机 ID。",
        "- 你可以调用 edit_image 工具对现有图片进行编辑（例如修改风格、添加元素、调整细节等）。需要提供图片的本地绝对路径（file:// 前缀会自动去除）和编辑描述。建议提供 image_name 参数指定输出文件名。",
        "- 工具返回结果中会包含该图片的本地绝对路径（如 /Users/<username>/.jkcodingagent/chat-images/<slug>/<image-id>.png）。",
        "- 如果你想在回答中展示生成的图片，请直接使用 Markdown 图片引用语法，引用工具返回的原始本地绝对路径即可。",
        "- 正确格式示例：如果工具返回的本地路径是 /Users/alice/.jkcodingagent/chat-images/untitled/abc123.png，",
        "  则在回答中写：![生成的风景图片](/Users/alice/.jkcodingagent/chat-images/untitled/abc123.png)",
        "- 注意：",
        "    - 直接使用工具返回的原始本地绝对路径即可，不需要添加任何协议前缀（如 file:// 或 asset://）。",
        "    - 路径中的空格和特殊字符不需要额外编码。",
    ]
    .join("\n")
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

fn record_run_token_usage(
    db: &DispatcherDb,
    workspace_id: &str,
    model: &str,
    usage: &LlmUsage,
    tracker: &mut PlainChatUsageTracker,
    on_event: &Channel<AgentEvent>,
) {
    let db = db.clone();
    let wid = workspace_id.to_string();
    let model_name = model.to_string();
    let usage_record = usage.clone();
    tokio::spawn(async move {
        if let Err(error) = db
            .upsert_session_token_usage_async(
                &wid,
                &model_name,
                DispatcherSessionTokenUsageSource::Primary,
                &usage_record,
            )
            .await
        {
            eprintln!(
                "failed to persist plain chat token usage for workspace {} and model {}: {}",
                wid, model_name, error
            );
        }
    });

    let stats = tracker.record(usage);
    emit(
        on_event,
        AgentEvent::RunUsageUpdated {
            workspace_id: workspace_id.to_string(),
            stats,
        },
    );
}

fn normalized_total_tokens(usage: &LlmUsage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.prompt_tokens + usage.completion_tokens
    }
}

fn emit(on_event: &Channel<AgentEvent>, event: AgentEvent) {
    let _ = on_event.send(event);
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
            .tools
            .definitions_for_workspace(&root, Option::<std::iter::Empty<&str>>::None, true)
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
    }
}
