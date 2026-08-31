use std::path::{Path, PathBuf};

use super::*;

#[async_trait]
impl AgentRunAdapter for PlainChatAgent {
    async fn prepare_run_workspace(&self, request: &AgentRunRequest<'_>) -> Result<PathBuf> {
        let workspace = self.session_workspace(request.workspace_id).await?;
        // 聊天 MCP 一律来自全局注册表：所有会话共享同一份快照，
        // 不再按会话目录各存一份相同内容。
        self.mcp_registry
            .ensure_recent(&McpScope::Global)
            .await
            .map_err(anyhow::Error::msg)
            .context("刷新聊天 MCP 状态失败")?;
        Ok(workspace)
    }

    fn provider_snapshot(&self) -> OpenAiCompatProvider {
        self.provider.lock().clone()
    }

    fn provider_missing_message(&self) -> &'static str {
        "错误：聊天 LLM API Key 未配置。请在设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。"
    }

    async fn build_run_prompt(
        &self,
        workspace_id: &str,
        _workspace: &Path,
    ) -> Result<RunPromptState> {
        // run 入口预热子智能体缓存（spawn_blocking），保证后续同步路径
        // （每轮系统提示重建、工具定义构建）不再触发同步 SQLite I/O。
        self.warm_sub_agent_exposure(workspace_id).await;
        Ok(RunPromptState {
            initial_system_prompt: self.build_effective_system_prompt(workspace_id),
            project_prompt: None,
        })
    }
}

pub(super) fn empty_plain_chat_response_error(
    response: &LlmResponse,
    provider: &OpenAiCompatProvider,
    tool_count: usize,
) -> String {
    let raw_response = response.raw_response.trim();
    let response_detail = if raw_response.is_empty() {
        "<空>".to_string()
    } else {
        truncate_for_display(raw_response, 4_000, "\n...[LLM 接口响应内容已截断]")
    };

    format!(
        "LLM 返回了空响应且没有工具调用，无法继续执行。\n请求摘要：model={}, tools={}\nLLM 接口响应内容：\n{}",
        provider.model(),
        tool_count,
        response_detail
    )
}

pub(super) async fn emit_stop_and_finish(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    partial: &str,
    usage_tracker: &UsageTracker,
    last_seq: Option<u64>,
) -> Result<DispatcherMessageRecord> {
    let content = build_stopped_plain_chat_reply(partial);
    let usage_stats = usage_tracker.snapshot();
    let reply = persist_assistant_message(db, workspace_id, &content, &usage_stats).await?;
    common::emit(
        on_event,
        AgentEvent::AssistantMessage {
            message: reply.clone(),
            last_seq,
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
