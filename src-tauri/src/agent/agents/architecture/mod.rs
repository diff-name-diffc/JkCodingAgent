//! 架构设计视觉 Agent：tldraw 画布的专属聊天智能体。
//!
//! 与 `PlainChatAgent` 的关键差异：
//! - 工具注册表只含 `architecture_run`（画布程序执行），无 MCP / 子智能体 / 通知工具；
//! - **主模型即视觉模型**（从设置中心视觉分类库条目/视觉用途绑定解析），
//!   `provider_for_iteration` 恒返回主模型——用户消息附带的画布截图直接由
//!   主模型消费，绝不走 `select_provider_for_messages`（那里含图且无独立
//!   视觉槽位会直接 bail）；
//! - 无聊天分类配置层；系统提示词为 DSL 专用常量（`prompt.rs`）。

use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use tauri::AppHandle;

use crate::agent::common::{
    self, persist_assistant_message, stream_llm_response, LlmStreamOutcome,
};
use crate::agent::config::DispatcherAgentConfig;
use crate::agent::db::{DispatcherDb, DispatcherMessageRecord, DispatcherSessionTokenUsageSource};
use crate::agent::llm::{ChatMessage, LlmResponse, OpenAiCompatProvider, ToolDefinition};
use crate::agent::run_loop::agent_loop::AgentLoop;
use crate::agent::run_loop::core::{
    RunLoopAgent, RunLoopContext, RunLoopIteration, RunLoopToolOutcome, RuntimeAgentKind,
};
use crate::agent::run_loop::AgentEvent;
use crate::agent::tools::{ToolContext, ToolRegistry, ToolSurface};
use crate::mcp::McpScope;

mod adapter;
mod context;
mod prompt;
mod tool_exec;

pub(crate) mod program;
mod program_ast;
pub(crate) mod program_schema;
mod program_validate;

pub struct ArchitectureAgent {
    config: DispatcherAgentConfig,
    provider: Mutex<OpenAiCompatProvider>,
    app_handle: Option<AppHandle>,
    tools: Arc<ToolRegistry>,
}

impl ArchitectureAgent {
    pub fn new(config: DispatcherAgentConfig, provider: OpenAiCompatProvider) -> Self {
        Self {
            config,
            provider: Mutex::new(provider),
            app_handle: None,
            tools: Arc::new(ToolRegistry::architecture_tools()),
        }
    }

    pub fn with_app_handle(mut self, app_handle: AppHandle) -> Self {
        self.app_handle = Some(app_handle);
        self
    }

    /// 系统提示词 = DSL 常量 + 系统时间（与 plain_chat 相同的动态段惯例）。
    pub(super) fn build_effective_system_prompt(&self) -> String {
        format!(
            "{}\n\n## 系统时间\n\n当前本地时间：{}",
            prompt::ARCHITECTURE_SYSTEM_PROMPT,
            crate::agent::prompt::current_local_time()
        )
    }
}

#[async_trait]
impl RunLoopAgent for ArchitectureAgent {
    async fn build_loop_tool_context(&self, ctx: &RunLoopContext<'_>) -> ToolContext {
        self.build_tool_context(ctx.db, ctx.workspace_id, ctx.workspace, &ctx.provider)
            .await
    }

    fn max_tool_iterations(&self) -> usize {
        self.config.max_tool_iterations
    }

    fn tool_surface_for_loop(&self, _tool_context: &ToolContext) -> ToolSurface {
        // 注册表无动态 provider：include_dynamic 取 false，工具面恒为单工具。
        let definitions = self.tools.definitions_for_scope(
            &McpScope::Global,
            Option::<std::iter::Empty<&str>>::None,
            false,
        );
        ToolSurface::direct(definitions)
    }

    fn build_iteration_messages(
        &self,
        _ctx: &RunLoopContext<'_>,
        agent_loop: &AgentLoop,
        _tool_definitions: &[ToolDefinition],
    ) -> Result<Vec<ChatMessage>> {
        // 每轮迭代重建系统提示（同 plain_chat G9-17 惯例）；
        // history() 返回不含 system 的纯历史。
        let mut messages = Vec::with_capacity(agent_loop.history().len() + 1);
        messages.push(ChatMessage::system(self.build_effective_system_prompt()));
        messages.extend(agent_loop.history().iter().cloned());
        Ok(messages)
    }

    fn provider_for_iteration(
        &self,
        ctx: &RunLoopContext<'_>,
        _messages: &[ChatMessage],
        _iteration: usize,
    ) -> Result<OpenAiCompatProvider> {
        // 主模型即视觉模型：含图消息直接发给主模型。不得复用
        // select_provider_for_messages——无独立视觉槽位时含图会 bail。
        Ok(ctx.provider.clone())
    }

    async fn stream_iteration_response(
        &self,
        ctx: &mut RunLoopContext<'_>,
        iteration: &RunLoopIteration,
        _iteration_index: usize,
    ) -> Result<LlmStreamOutcome> {
        stream_llm_response(
            ctx.db,
            ctx.workspace_id,
            iteration.request_provider.model(),
            DispatcherSessionTokenUsageSource::Primary,
            &mut ctx.usage_tracker,
            ctx.on_event,
            &iteration.request_provider,
            &iteration.messages,
            &iteration.tool_definitions,
            ctx.cancel_rx.clone(),
        )
        .await
    }

    async fn handle_cancelled_loop(
        &self,
        ctx: &RunLoopContext<'_>,
        partial: &str,
        last_seq: Option<u64>,
    ) -> Result<DispatcherMessageRecord> {
        let content = build_stopped_reply(partial);
        let usage_stats = ctx.usage_tracker.snapshot();
        let reply =
            persist_assistant_message(ctx.db, ctx.workspace_id, &content, &usage_stats).await?;
        common::emit(
            ctx.on_event,
            AgentEvent::AssistantMessage {
                message: reply.clone(),
                last_seq,
            },
        );
        Ok(reply)
    }

    async fn handle_no_tool_response(
        &self,
        ctx: &RunLoopContext<'_>,
        response: &LlmResponse,
        last_seq: Option<u64>,
    ) -> Result<DispatcherMessageRecord> {
        let content = response.content.trim().to_string();
        if content.is_empty() {
            anyhow::bail!(
                "视觉模型返回空响应且没有工具调用，无法继续执行。（model={}）",
                ctx.provider.model()
            );
        }
        let usage_stats = ctx.usage_tracker.snapshot();
        let reply =
            persist_assistant_message(ctx.db, ctx.workspace_id, &content, &usage_stats).await?;
        common::emit(
            ctx.on_event,
            AgentEvent::AssistantMessage {
                message: reply.clone(),
                last_seq,
            },
        );
        Ok(reply)
    }

    async fn execute_loop_tool_calls(
        &self,
        ctx: &mut RunLoopContext<'_>,
        iteration: &RunLoopIteration,
        tool_context: &ToolContext,
        response: LlmResponse,
    ) -> Result<RunLoopToolOutcome> {
        self.execute_single_tool_turn(ctx, iteration, tool_context, response)
            .await
    }

    async fn resolve_loop_outcome(
        &self,
        _ctx: &RunLoopContext<'_>,
        _outcome: RunLoopToolOutcome,
    ) -> Result<Option<DispatcherMessageRecord>> {
        Ok(None)
    }

    fn max_iterations_error(&self, kind: RuntimeAgentKind) -> String {
        let iterations = self.config.max_tool_iterations;
        match kind {
            RuntimeAgentKind::Architecture => format!(
                "已达到最大工具迭代次数（{iterations}），本轮画布操作被终止。请检查模型是否陷入工具调用循环。"
            ),
            other => format!("已达到最大工具迭代次数（{iterations}），本轮运行被终止。（kind={other:?}）"),
        }
    }
}

fn build_stopped_reply(partial: &str) -> String {
    let trimmed = partial.trim();
    if trimmed.is_empty() {
        "⏹️ 本轮画布操作已停止。当前会话上下文已保留，可稍后继续。".to_string()
    } else {
        format!(
            "{}\n\n[本轮画布操作已手动停止。当前会话上下文与以上输出均已保留，可稍后继续。]",
            trimmed
        )
    }
}
