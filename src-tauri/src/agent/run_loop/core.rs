use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;
use tauri::ipc::Channel;
use tokio::sync::watch;

use super::super::common::{cancellation_requested, LlmStreamOutcome, UsageTracker};
use super::super::config::validate_provider_completeness;
use super::super::db::{DispatcherDb, DispatcherMessageRecord};
use super::super::llm::{ChatMessage, LlmResponse, OpenAiCompatProvider, ToolDefinition};
use super::super::prompt::PromptBundle;
use super::super::tools::ToolContext;
use super::agent_loop::AgentLoop;
use super::types::{AgentEvent, AgentTurn};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeAgentKind {
    Project,
    PlainChat,
}

pub(crate) struct RunLoopContext<'a> {
    pub kind: RuntimeAgentKind,
    pub db: &'a DispatcherDb,
    pub workspace_id: &'a str,
    pub workspace: &'a Path,
    pub on_event: &'a Channel<AgentEvent>,
    pub provider: OpenAiCompatProvider,
    pub cancel_rx: watch::Receiver<bool>,
    pub usage_tracker: UsageTracker,
    pub initial_system_prompt: String,
    pub project_prompt: Option<PromptBundle>,
}

pub(crate) struct AgentRunRequest<'a> {
    pub kind: RuntimeAgentKind,
    pub db: &'a DispatcherDb,
    pub workspace_id: &'a str,
    pub workspace_path: Option<&'a str>,
    pub user_segments_json: String,
    pub on_event: Channel<AgentEvent>,
    pub cancel_rx: watch::Receiver<bool>,
}

pub(crate) struct RunPromptState {
    pub initial_system_prompt: String,
    pub project_prompt: Option<PromptBundle>,
}

pub(crate) struct RunLoopIteration {
    pub tool_definitions: Vec<ToolDefinition>,
    pub allowed_tool_names: HashSet<String>,
    pub messages: Vec<ChatMessage>,
    pub request_provider: OpenAiCompatProvider,
}

/// 循环内由 Agent 拦截处理的协议级动作。
///
/// 对应工具只回显文本；真正的动作（校验、落库、广播）由 Agent 在
/// `execute_loop_tool_calls` 中按工具名拦截完成，`resolve_loop_outcome`
/// 再据此收口本轮。
pub(crate) enum LoopProtocolAction {
    /// 编排器已产出执行图并登记为待确认计划，等待用户在图面板确认。
    GraphSubmitted { title: String, node_count: usize },
}

pub(crate) struct RunLoopToolOutcome {
    pub saw_retryable_tool_error: bool,
    pub final_message: Option<String>,
    pub protocol_actions: Vec<LoopProtocolAction>,
    pub llm_messages: Vec<ChatMessage>,
}

#[async_trait]
pub(crate) trait RunLoopAgent: Sync {
    async fn build_loop_tool_context(&self, ctx: &RunLoopContext<'_>) -> ToolContext;

    fn max_tool_iterations(&self) -> usize;

    fn tool_definitions_for_loop(
        &self,
        workspace_id: &str,
        workspace: &Path,
    ) -> Vec<ToolDefinition>;

    fn build_iteration_messages(
        &self,
        ctx: &RunLoopContext<'_>,
        agent_loop: &AgentLoop,
        tool_definitions: &[ToolDefinition],
    ) -> Result<Vec<ChatMessage>>;

    fn provider_for_iteration(
        &self,
        ctx: &RunLoopContext<'_>,
        messages: &[ChatMessage],
        iteration: usize,
    ) -> Result<OpenAiCompatProvider>;

    async fn stream_iteration_response(
        &self,
        ctx: &mut RunLoopContext<'_>,
        iteration: &RunLoopIteration,
        iteration_index: usize,
    ) -> Result<LlmStreamOutcome>;

    async fn handle_cancelled_loop(
        &self,
        ctx: &RunLoopContext<'_>,
        partial: &str,
        last_seq: Option<u64>,
    ) -> Result<DispatcherMessageRecord>;

    async fn handle_no_tool_response(
        &self,
        ctx: &RunLoopContext<'_>,
        response: &LlmResponse,
        last_seq: Option<u64>,
    ) -> Result<DispatcherMessageRecord>;

    async fn execute_loop_tool_calls(
        &self,
        ctx: &mut RunLoopContext<'_>,
        iteration: &RunLoopIteration,
        tool_context: &ToolContext,
        response: LlmResponse,
    ) -> Result<RunLoopToolOutcome>;

    async fn resolve_loop_outcome(
        &self,
        ctx: &RunLoopContext<'_>,
        outcome: RunLoopToolOutcome,
    ) -> Result<Option<DispatcherMessageRecord>>;

    fn max_iterations_error(&self, kind: RuntimeAgentKind) -> String;
}

#[async_trait]
pub(crate) trait AgentRunAdapter: RunLoopAgent {
    async fn prepare_run_workspace(&self, request: &AgentRunRequest<'_>) -> Result<PathBuf>;

    fn provider_snapshot(&self) -> OpenAiCompatProvider;

    fn provider_missing_message(&self) -> &'static str;

    async fn build_run_prompt(
        &self,
        workspace_id: &str,
        workspace: &Path,
    ) -> Result<RunPromptState>;
}

/// 唯一的用户消息入口：准备工作区、刷新工具元数据、保存用户消息、校验模型配置、进入公共 run_loop。
pub(crate) async fn run_agent_turn<A>(agent: &A, request: AgentRunRequest<'_>) -> Result<AgentTurn>
where
    A: AgentRunAdapter,
{
    emit_started(&request.on_event, request.workspace_id);
    let result: Result<AgentTurn> = async {
        let workspace = agent.prepare_run_workspace(&request).await?;
        let provider = agent.provider_snapshot();

        let user = request
            .db
            .add_visible_message_from_segments_async(
                request.workspace_id,
                "user",
                request.user_segments_json.clone(),
            )
            .await?;
        let on_event = &request.on_event;
        let workspace_id = request.workspace_id;
        emit(on_event, AgentEvent::UserMessage { message: user });

        if !provider.is_configured() {
            anyhow::bail!("{}", agent.provider_missing_message());
        }
        // 完整性校验（G9-15）：API Key / Base URL / 模型名任一缺失都在 run 入口
        // 显式失败并给出「错误：」提示，避免延迟到 HTTP 请求时才以晦涩错误暴露。
        validate_provider_completeness(provider.api_key(), provider.api_base(), provider.model())?;

        let prompt = agent.build_run_prompt(workspace_id, &workspace).await?;
        let reply = run_loop(
            agent,
            RunLoopContext {
                kind: request.kind,
                db: request.db,
                workspace_id,
                workspace: &workspace,
                on_event,
                provider,
                cancel_rx: request.cancel_rx.clone(),
                usage_tracker: UsageTracker::new(),
                initial_system_prompt: prompt.initial_system_prompt,
                project_prompt: prompt.project_prompt,
            },
        )
        .await?;

        // G7-11：Finished 改为轻量负载——不再全量加载可见消息（含
        // segments_json/context_payload）随事件下发；前端收到 finished 后自行
        // 调用 dispatcher_list_messages 拉全量刷新。此处只查计数供对账。
        let message_count = request
            .db
            .count_visible_messages_async(workspace_id)
            .await?;
        emit(
            on_event,
            AgentEvent::Finished {
                workspace_id: workspace_id.to_string(),
                message_count,
            },
        );
        Ok(AgentTurn { reply })
    }
    .await;

    if let Err(error) = &result {
        emit(
            &request.on_event,
            AgentEvent::Failed {
                workspace_id: request.workspace_id.to_string(),
                message: error.to_string(),
            },
        );
    }

    result
}

/// 通用 LLM 工具循环：请求模型、执行工具、把工具结果回灌，再决定是否收口。
///
/// 主项目 Agent 和普通聊天 Agent 共享这条骨架；差异由 `RunLoopAgent`
/// 的状态适配方法承接，少数语义差异通过 `RuntimeAgentKind` 显式区分。
pub(crate) async fn run_loop<A>(
    agent: &A,
    mut ctx: RunLoopContext<'_>,
) -> Result<DispatcherMessageRecord>
where
    A: RunLoopAgent,
{
    let tool_context = agent.build_loop_tool_context(&ctx).await;
    let mut tool_definitions = agent.tool_definitions_for_loop(ctx.workspace_id, ctx.workspace);
    let mut allowed_tool_names: HashSet<String> = tool_definitions
        .iter()
        .map(|tool| tool.function.name.clone())
        .collect();
    let mut agent_loop =
        AgentLoop::new(ctx.db, ctx.workspace_id, ctx.initial_system_prompt.clone()).await?;

    for iteration_index in 0..agent.max_tool_iterations() {
        if cancellation_requested(&ctx.cancel_rx) {
            // 循环边界取消时尚未开始流式输出，无 delta 序号可对账。
            return agent.handle_cancelled_loop(&ctx, "", None).await;
        }

        if iteration_index > 0 {
            tool_definitions = agent.tool_definitions_for_loop(ctx.workspace_id, ctx.workspace);
            allowed_tool_names = tool_definitions
                .iter()
                .map(|tool| tool.function.name.clone())
                .collect();
        }

        let messages = agent.build_iteration_messages(&ctx, &agent_loop, &tool_definitions)?;
        let request_provider = agent.provider_for_iteration(&ctx, &messages, iteration_index)?;
        let iteration = RunLoopIteration {
            tool_definitions: tool_definitions.clone(),
            allowed_tool_names: allowed_tool_names.clone(),
            messages,
            request_provider,
        };

        let (response, last_seq) = match agent
            .stream_iteration_response(&mut ctx, &iteration, iteration_index)
            .await?
        {
            LlmStreamOutcome::Cancelled { partial, last_seq } => {
                return agent.handle_cancelled_loop(&ctx, &partial, last_seq).await;
            }
            LlmStreamOutcome::Response {
                response,
                last_seq,
            } => (response, last_seq),
        };

        if response.tool_calls.is_empty() {
            return agent
                .handle_no_tool_response(&ctx, &response, last_seq)
                .await;
        }

        let outcome = agent
            .execute_loop_tool_calls(&mut ctx, &iteration, &tool_context, response)
            .await?;
        for message in &outcome.llm_messages {
            agent_loop.append(message.clone());
        }

        if let Some(reply) = agent.resolve_loop_outcome(&ctx, outcome).await? {
            return Ok(reply);
        }
    }

    anyhow::bail!("{}", agent.max_iterations_error(ctx.kind))
}

fn emit_started(on_event: &Channel<AgentEvent>, workspace_id: &str) {
    emit(
        on_event,
        AgentEvent::Started {
            workspace_id: workspace_id.to_string(),
        },
    );
}

fn emit(on_event: &Channel<AgentEvent>, event: AgentEvent) {
    let _ = on_event.send(event);
}
