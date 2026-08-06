//! `RunLoopAgent` + `AgentRunAdapter` 实现：把编排器接入公共 run_loop 骨架
//! （循环 / DB 持久化 / 用量统计 / ActiveRunStore 取消全部复用）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;
use tauri::Manager;

use crate::agent::common::LlmStreamOutcome;
use crate::agent::db::{DispatcherMessageRecord, DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS};
use crate::agent::debug::ContextDebugLogger;
use crate::agent::llm::{ChatMessage, LlmResponse, OpenAiCompatProvider, ToolDefinition};
use crate::agent::prompt::PromptBundle;
use crate::agent::run_loop::agent_loop::AgentLoop;
use crate::agent::run_loop::core::{
    AgentRunAdapter, AgentRunRequest, RunLoopAgent, RunLoopContext, RunLoopIteration,
    RunLoopToolOutcome, RunPromptState, RuntimeAgentKind,
};
use crate::agent::tools::ToolContext;

use super::OrchestratorAgent;

#[async_trait]
impl AgentRunAdapter for OrchestratorAgent {
    async fn prepare_run_workspace(&self, request: &AgentRunRequest<'_>) -> Result<PathBuf> {
        let workspace_path = request
            .workspace_path
            .ok_or_else(|| anyhow::anyhow!("项目 Agent 启动缺少 workspace_path"))?;
        let workspace = PathBuf::from(workspace_path);
        let workspace_for_create = workspace.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&workspace_for_create)
                .with_context(|| format!("create workspace {}", workspace_for_create.display()))
        })
        .await
        .map_err(|error| anyhow::anyhow!("create workspace task failed: {error}"))??;
        Ok(workspace)
    }

    fn provider_snapshot(&self) -> OpenAiCompatProvider {
        self.provider.lock().clone()
    }

    fn provider_missing_message(&self) -> &'static str {
        "项目编排 Agent 的 LLM API Key 未配置。请在设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。"
    }

    async fn build_run_prompt(
        &self,
        workspace_id: &str,
        _workspace: &Path,
    ) -> Result<RunPromptState> {
        let mut static_prompt = self.build_static_prompt().await?;
        let app = self
            .app_handle
            .as_ref()
            .context("项目 Agent 缺少 AppHandle")?;
        let state = app.state::<crate::agent::state::DispatcherState>();
        let catalog = crate::agent::graph::commands::catalog_for_workspace(&state, workspace_id)
            .await
            .map_err(anyhow::Error::msg)?;
        // 轻量学习回路：既往节点运行统计回注目录，辅助编排器选模型。
        // 统计查询失败时跳过历史统计、不阻塞提示词构建，但必须留下日志，
        // 否则学习回路静默失效时无任何可诊断痕迹。
        let stats = match crate::agent::graph::GraphStore::new(state.db())
            .node_run_stats_async(workspace_id)
            .await
        {
            Ok(stats) => stats,
            Err(error) => {
                eprintln!(
                    "[graph] 读取节点运行统计失败（{workspace_id}），目录回注不含历史统计：{error:#}"
                );
                Vec::new()
            }
        };
        static_prompt.push_str("\n\n---\n\n");
        static_prompt.push_str(&self.render_graph_harness_catalog(&catalog, &stats));
        Ok(RunPromptState {
            initial_system_prompt: static_prompt.clone(),
            project_prompt: Some(PromptBundle {
                static_content: static_prompt,
            }),
        })
    }
}

#[async_trait]
impl RunLoopAgent for OrchestratorAgent {
    async fn build_loop_tool_context(&self, ctx: &RunLoopContext<'_>) -> ToolContext {
        self.build_tool_context(ctx.db, ctx.workspace_id, ctx.workspace, &ctx.provider)
            .await
    }

    fn max_tool_iterations(&self) -> usize {
        self.config.max_tool_iterations
    }

    /// 编排器工具集固定（只读探索 + message + submit_graph + graph_plan_report），
    /// 不随设置变化；注册表本身即权威列表。
    fn tool_definitions_for_loop(
        &self,
        _workspace_id: &str,
        workspace: &Path,
    ) -> Vec<ToolDefinition> {
        self.tools.definitions_for_workspace(
            workspace,
            Option::<std::iter::Empty<&str>>::None,
            false,
        )
    }

    fn build_iteration_messages(
        &self,
        ctx: &RunLoopContext<'_>,
        agent_loop: &AgentLoop,
        tool_definitions: &[ToolDefinition],
    ) -> Result<Vec<ChatMessage>> {
        let Some(static_prompt) = ctx.project_prompt.as_ref() else {
            anyhow::bail!("编排器 run_loop 缺少静态提示词状态");
        };
        let debug_logger =
            ContextDebugLogger::new(self.context_debug_enabled(), PathBuf::from(ctx.workspace));
        let estimated_tokens = agent_loop.estimated_tokens();
        if estimated_tokens > DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS * 8 / 10 {
            debug_logger.log(
                "上下文窗口接近上限",
                vec![
                    ("工作区".to_string(), ctx.workspace_id.to_string()),
                    ("估算tokens".to_string(), estimated_tokens.to_string()),
                    (
                        "容量".to_string(),
                        DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS.to_string(),
                    ),
                ],
                vec![],
            );
        }

        let system_prompt =
            self.build_iteration_system_prompt(static_prompt, ctx.workspace_id, tool_definitions);
        let mut messages = vec![ChatMessage::system(system_prompt)];
        messages.extend(agent_loop.request_messages().into_iter().skip(1));
        Ok(messages)
    }

    fn provider_for_iteration(
        &self,
        ctx: &RunLoopContext<'_>,
        messages: &[ChatMessage],
        iteration: usize,
    ) -> Result<OpenAiCompatProvider> {
        self.provider_for_messages(&ctx.provider, messages, ctx.on_event, iteration == 0)
    }

    async fn stream_iteration_response(
        &self,
        ctx: &mut RunLoopContext<'_>,
        iteration: &RunLoopIteration,
        iteration_index: usize,
    ) -> Result<LlmStreamOutcome> {
        self.stream_llm_response_inner(ctx, iteration, iteration_index)
            .await
    }

    async fn handle_cancelled_loop(
        &self,
        ctx: &RunLoopContext<'_>,
        partial: &str,
    ) -> Result<DispatcherMessageRecord> {
        self.emit_stop_and_finish(
            ctx.db,
            ctx.workspace_id,
            ctx.on_event,
            partial,
            Some(&ctx.usage_tracker),
        )
        .await
    }

    async fn handle_no_tool_response(
        &self,
        ctx: &RunLoopContext<'_>,
        response: &LlmResponse,
    ) -> Result<DispatcherMessageRecord> {
        OrchestratorAgent::handle_no_tool_response(
            self,
            ctx.db,
            ctx.workspace_id,
            ctx.on_event,
            response,
            &ctx.usage_tracker,
        )
        .await
    }

    async fn execute_loop_tool_calls(
        &self,
        ctx: &mut RunLoopContext<'_>,
        iteration: &RunLoopIteration,
        tool_context: &ToolContext,
        response: LlmResponse,
    ) -> Result<RunLoopToolOutcome> {
        self.execute_tool_calls(
            ctx.db,
            ctx.workspace_id,
            ctx.workspace,
            ctx.on_event,
            response,
            &iteration.allowed_tool_names,
            tool_context,
            &ctx.cancel_rx,
            &iteration.request_provider,
            &mut ctx.usage_tracker,
        )
        .await
    }

    async fn resolve_loop_outcome(
        &self,
        ctx: &RunLoopContext<'_>,
        outcome: RunLoopToolOutcome,
    ) -> Result<Option<DispatcherMessageRecord>> {
        OrchestratorAgent::resolve_loop_outcome(
            self,
            ctx.db,
            ctx.workspace_id,
            ctx.on_event,
            outcome,
            &ctx.usage_tracker,
        )
        .await
    }

    fn max_iterations_error(&self, kind: RuntimeAgentKind) -> String {
        match kind {
            RuntimeAgentKind::Project => format!(
                "已达到最大工具迭代次数（{}），本轮编排被终止。请检查模型是否陷入工具调用循环。",
                self.config.max_tool_iterations
            ),
            RuntimeAgentKind::PlainChat => format!(
                "已达到最大工具迭代次数（{}），本轮聊天被终止。请检查模型是否陷入工具调用循环。",
                self.config.max_tool_iterations
            ),
        }
    }
}
