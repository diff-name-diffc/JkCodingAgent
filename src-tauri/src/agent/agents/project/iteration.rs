//! 单次迭代的 LLM 交互与消息收口：工具上下文构建、带调试日志的流式请求、
//! 无工具响应 / 手动停止时的 assistant 消息落库。

use std::path::{Path, PathBuf};

use anyhow::Result;
use tauri::ipc::Channel;

use crate::agent::common::{stream_llm_response, LlmStreamOutcome, UsageTracker};
use crate::agent::db::{
    DispatcherDb, DispatcherMessageRecord, DispatcherSessionTokenUsageSource,
};
use crate::agent::debug::{render_json, ContextDebugLogger, DebugSection};
use crate::agent::llm::{LlmResponse, OpenAiCompatProvider};
use crate::agent::run_loop::core::{RunLoopContext, RunLoopIteration};
use crate::agent::run_loop::AgentEvent;
use crate::agent::tools::ToolContext;

use super::helpers::{emit, empty_llm_response_error};
use super::OrchestratorAgent;

impl OrchestratorAgent {
    pub(super) async fn build_tool_context(
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
        let user_task = db
            .get_latest_user_message_content_async(workspace_id)
            .await
            .ok()
            .flatten();
        let ms = self.models.lock().snapshot();
        ToolContext {
            workspace_id: workspace_id.to_string(),
            workspace: workspace.to_path_buf(),
            session_title,
            user_task,
            ssh_review: None,
            exec_timeout_secs: self.config.exec_timeout_secs,
            restrict_to_workspace: self.config.restrict_to_workspace,
            extra_allowed_dirs: dirs::home_dir()
                .map(|h| vec![h.join(".jkcodingagent")])
                .unwrap_or_default(),
            app_handle: self.app_handle.clone(),
            llm_provider: Some(provider.clone()),
            vision_model: ms.vision_model,
            image_model_url: String::new(),
            image_model_api_key: String::new(),
            image_model: String::new(),
            image_edit_model: String::new(),
            sub_agent_tool_registry: None,
            current_sub_agent_id: None,
            current_sub_agent_name: None,
            current_tool_call_id: None,
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
            if let LlmStreamOutcome::Response(ref response) = outcome {
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
            },
        );
        Ok(reply)
    }

    pub(super) async fn emit_stop_and_finish(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        partial: &str,
        usage_tracker: Option<&UsageTracker>,
    ) -> Result<DispatcherMessageRecord> {
        let content = build_stopped_orchestration_reply(partial);
        let usage_stats = usage_tracker.map(UsageTracker::snapshot);
        let reply = if let Some(usage_stats) = usage_stats.as_ref() {
            db.add_visible_message_with_usage_async(
                workspace_id,
                "assistant",
                &content,
                usage_stats,
            )
            .await?
        } else {
            db.add_visible_message_async(workspace_id, "assistant", &content, None)
                .await?
        };
        emit(
            on_event,
            AgentEvent::AssistantMessage {
                message: reply.clone(),
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
