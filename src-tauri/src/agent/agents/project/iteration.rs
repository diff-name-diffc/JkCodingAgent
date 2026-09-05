//! 单次迭代的 LLM 交互与消息收口：工具上下文构建、带调试日志的流式请求、
//! 无工具响应 / 手动停止时的 assistant 消息落库。

use std::path::{Path, PathBuf};

use anyhow::Result;
use tauri::ipc::Channel;

use crate::agent::common::{stream_llm_response, LlmStreamOutcome, UsageTracker};
use crate::agent::db::{DispatcherDb, DispatcherMessageRecord, DispatcherSessionTokenUsageSource};
use crate::agent::debug::{render_json, ContextDebugLogger, DebugSection};
use crate::agent::llm::{LlmResponse, OpenAiCompatProvider};
use crate::agent::run_loop::core::{RunLoopContext, RunLoopIteration};
use crate::agent::run_loop::AgentEvent;
use crate::agent::tools::ToolContext;

use super::helpers::{self, emit, empty_llm_response_error};
use super::OrchestratorAgent;

impl OrchestratorAgent {
    pub(super) async fn build_tool_context(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &Path,
        provider: &OpenAiCompatProvider,
    ) -> ToolContext {
        // 两个查询失败都降级兜底，但必须留下持久化警告：user_task 是工具上下文
        // 的关键信息，缺失时 LLM/工具可能不知道用户任务，无日志则问题无法追踪。
        let session_title = match db.get_session_title_async(workspace_id).await {
            Ok(title) => title,
            Err(error) => {
                helpers::log_warning(&format!(
                    "[orchestrator] 读取会话标题失败，降级为 untitled（workspace_id={workspace_id}）：{error:#}"
                ));
                "untitled".to_string()
            }
        };
        let user_task = match db.get_latest_user_message_content_async(workspace_id).await {
            Ok(content) => content,
            Err(error) => {
                helpers::log_warning(&format!(
                    "[orchestrator] 读取最新用户消息失败，工具上下文将不含用户任务（workspace_id={workspace_id}）：{error:#}"
                ));
                None
            }
        };
        // 安全审查的「对话上下文」：最近若干轮用户/助手对话（截断渲染）。
        // 读取失败降级为 None（审查仍可依据任务/意图/命令本身判定）。
        let review_conversation = match db
            .get_recent_review_dialogue_async(
                workspace_id,
                crate::agent::ssh_review::REVIEW_DIALOGUE_FETCH_LIMIT,
            )
            .await
        {
            Ok(messages) => crate::agent::ssh_review::render_dialogue_for_review(&messages),
            Err(error) => {
                helpers::log_warning(&format!(
                    "[orchestrator] 读取审查对话上下文失败，降级为无对话上下文（workspace_id={workspace_id}）：{error:#}"
                ));
                None
            }
        };
        let ms = self.models.lock().snapshot();
        // 与普通聊天对齐：授权本会话的图片目录（chat-images/{workspace_id}），
        // 项目 Agent 的文件工具因此可以直接读取用户粘贴/工具生成的图片；
        // 其他会话图片与全局设置目录仍不可见。
        let chat_image_dir = match crate::chat_images::workspace_image_dir(workspace_id) {
            Ok(dir) => vec![dir],
            Err(error) => {
                helpers::log_warning(&format!(
                    "[orchestrator] 构造会话图片目录失败，按空白名单收紧（workspace_id={workspace_id}）：{error}"
                ));
                Vec::new()
            }
        };
        // 图片模型凭据（generate_image / edit_image 工具）：同步 SQLite 读取
        // 经 spawn_blocking，失败按空凭据降级（工具侧有可读的未配置报错）。
        let credentials_db = db.clone();
        let image_credentials = match tokio::task::spawn_blocking(move || {
            credentials_db
                .get_settings_v2()
                .map(|s| s.shared.image_model_credentials())
        })
        .await
        {
            Ok(Ok(credentials)) => credentials,
            Ok(Err(error)) => {
                helpers::log_warning(&format!(
                    "[orchestrator] 读取图片模型凭据失败，按未配置降级（workspace_id={workspace_id}）：{error:#}"
                ));
                Default::default()
            }
            Err(error) => {
                helpers::log_warning(&format!(
                    "[orchestrator] 读取图片模型凭据任务失败，按未配置降级：{error}"
                ));
                Default::default()
            }
        };
        ToolContext {
            workspace_id: workspace_id.to_string(),
            workspace: workspace.to_path_buf(),
            // 项目编排器运行在受管项目工作区（turn.rs 已 canonicalize 校验）：
            // MCP 走「全局 ∪ 项目」合并作用域。
            mcp_scope: crate::mcp::McpScope::Project(workspace.to_path_buf()),
            session_title,
            user_task,
            executor_task: None,
            review_conversation,
            ssh_review: None,
            exec_timeout_secs: self.config.exec_timeout_secs,
            restrict_to_workspace: self.config.restrict_to_workspace,
            // memory/skills 已由提示词加载器读取；工具本身只允许项目工作区，
            // 不能把含设置、数据库与模型密钥的全局配置目录暴露给模型。
            extra_allowed_dirs: chat_image_dir,
            app_handle: self.app_handle.clone(),
            llm_provider: Some(provider.clone()),
            vision_model: ms.vision_model,
            vision_provider: ms.vision_provider,
            image_model_url: image_credentials.url,
            image_model_api_key: image_credentials.api_key,
            image_model: image_credentials.model,
            image_edit_model: image_credentials.edit_model,
            sub_agent_tool_registry: None,
            current_sub_agent_id: None,
            current_sub_agent_name: None,
            current_tool_call_id: None,
            current_tool_spec_hash: None,
            cancel_rx: None,
            sub_agent_parent_tool_call_id: None,
            sub_agent_trace_events: None,
        }
    }

    pub(super) async fn stream_llm_response_inner(
        &self,
        ctx: &mut RunLoopContext<'_>,
        iteration_ctx: &RunLoopIteration,
        iteration: usize,
    ) -> Result<LlmStreamOutcome> {
        let debug_logger =
            ContextDebugLogger::new(self.context_debug_enabled(), PathBuf::from(ctx.workspace));
        if debug_logger.enabled() {
            let request_snapshot = iteration_ctx
                .request_provider
                .build_request_snapshot(&iteration_ctx.messages, &iteration_ctx.tool_definitions);
            debug_logger.log(
                "发送大模型请求",
                vec![
                    ("工作区".to_string(), ctx.workspace_id.to_string()),
                    ("轮次".to_string(), (iteration + 1).to_string()),
                    (
                        "模型".to_string(),
                        iteration_ctx.request_provider.model().to_string(),
                    ),
                    (
                        "消息数".to_string(),
                        iteration_ctx.messages.len().to_string(),
                    ),
                    (
                        "工具数".to_string(),
                        iteration_ctx.tool_definitions.len().to_string(),
                    ),
                ],
                vec![DebugSection::new(
                    "实际请求",
                    render_json(&request_snapshot),
                )],
            );
        }

        let outcome = stream_llm_response(
            ctx.db,
            ctx.workspace_id,
            iteration_ctx.request_provider.model(),
            DispatcherSessionTokenUsageSource::Primary,
            &mut ctx.usage_tracker,
            ctx.on_event,
            &iteration_ctx.request_provider,
            &iteration_ctx.messages,
            &iteration_ctx.tool_definitions,
            ctx.cancel_rx.clone(),
        )
        .await?;

        if debug_logger.enabled() {
            if let LlmStreamOutcome::Response { response, .. } = &outcome {
                let response_snapshot = iteration_ctx
                    .request_provider
                    .build_response_snapshot(response);
                debug_logger.log(
                    "收到大模型响应",
                    vec![
                        ("工作区".to_string(), ctx.workspace_id.to_string()),
                        ("轮次".to_string(), (iteration + 1).to_string()),
                        (
                            "模型".to_string(),
                            iteration_ctx.request_provider.model().to_string(),
                        ),
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
            }
        }

        Ok(outcome)
    }

    pub(super) async fn handle_no_tool_response(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        response: &LlmResponse,
        usage_tracker: &UsageTracker,
        last_seq: Option<u64>,
    ) -> Result<DispatcherMessageRecord> {
        let content = response.content.trim().to_string();
        if content.is_empty() {
            anyhow::bail!("{}", empty_llm_response_error(response));
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
                last_seq,
            },
        );
        Ok(reply)
    }

    /// 手动停止收口：usage_tracker 必传——停止路径同样要落用量，
    /// 否则会话用量统计/计费不完整（审查项 G8-08）。
    pub(super) async fn emit_stop_and_finish(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        partial: &str,
        usage_tracker: &UsageTracker,
        last_seq: Option<u64>,
    ) -> Result<DispatcherMessageRecord> {
        let content = build_stopped_orchestration_reply(partial);
        let usage_stats = usage_tracker.snapshot();
        let reply = db
            .add_visible_message_with_usage_async(workspace_id, "assistant", &content, &usage_stats)
            .await?;
        emit(
            on_event,
            AgentEvent::AssistantMessage {
                message: reply.clone(),
                last_seq,
            },
        );
        Ok(reply)
    }
}

fn build_stopped_orchestration_reply(partial: &str) -> String {
    let trimmed = partial.trim();
    if trimmed.is_empty() {
        "⏹️ 本轮编排已停止。当前会话上下文与已完成内容均已保留，可稍后继续。".to_string()
    } else {
        format!(
            "{}\n\n[本轮已手动停止。当前会话上下文与以上输出均已保留，可稍后继续。]",
            trimmed
        )
    }
}
