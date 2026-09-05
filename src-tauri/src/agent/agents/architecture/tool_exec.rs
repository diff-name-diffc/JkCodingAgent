use super::*;
use crate::agent::tools::{ToolRuntime, ToolStatus, MAX_TOOL_CALLS_PER_BATCH};

impl ArchitectureAgent {
    /// 单工具执行循环（精简版 execute_loop_tool_calls）：
    ///
    /// 架构 Agent 只有 `architecture_run` 一个工具——无只读并行批、无
    /// `dispatcher_tool_runs` 记录（历史渲染由工具消息重建，不依赖 run 表）、
    /// 无子智能体用量暂停。事件链保持完整：Planned → Started →
    /// Finished（Finished 由 `persist_tool_result_with_compression` 发出）。
    pub(super) async fn execute_single_tool_turn(
        &self,
        ctx: &mut RunLoopContext<'_>,
        iteration: &RunLoopIteration,
        tool_context: &ToolContext,
        response: LlmResponse,
    ) -> Result<RunLoopToolOutcome> {
        if response.tool_calls.len() > MAX_TOOL_CALLS_PER_BATCH {
            anyhow::bail!(
                "模型单轮返回 {} 个工具调用，超过运行时上限 {}；已在持久化或执行前拒绝。",
                response.tool_calls.len(),
                MAX_TOOL_CALLS_PER_BATCH
            );
        }
        let tool_calls_payload =
            common::build_tool_calls_payload(&response.tool_calls, &self.tools)?;
        let args_map = common::build_args_map(&response.tool_calls, &self.tools)?;
        let mut llm_messages = Vec::new();

        for tool_call in &tool_calls_payload {
            common::emit(
                ctx.on_event,
                AgentEvent::ToolPlanned {
                    tool_call_id: tool_call.id.clone(),
                    name: tool_call.function.name.clone(),
                    arguments: tool_call.function.arguments.clone(),
                },
            );
        }

        let assistant_message = common::persist_tool_calls_message(
            ctx.db,
            ctx.workspace_id,
            &response.content,
            &tool_calls_payload,
            &response.thinking_content,
            Some(response.thinking_elapsed_ms),
        )
        .await?;
        if let Some(message) = assistant_message.to_llm_message() {
            llm_messages.push(message);
        }

        for tool_call in &response.tool_calls {
            if common::cancellation_requested(&ctx.cancel_rx) {
                break; // 尚未执行，无悬挂状态
            }
            common::emit(
                ctx.on_event,
                AgentEvent::ToolStarted {
                    tool_call_id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    arguments: args_map
                        .get(&tool_call.id)
                        .cloned()
                        .unwrap_or_else(|| "{}".to_string()),
                },
            );

            let result = ToolRuntime::execute_tool_with_cancellation(
                &self.tools,
                &iteration.direct_capabilities,
                tool_call,
                tool_context,
                ctx.cancel_rx.clone(),
            )
            .await;
            if result.status == ToolStatus::FatalError {
                anyhow::bail!("错误：画布工具致命失败：{}", result.output_for_llm());
            }

            // 策略表已登记 architecture_run 为 default_compress=false：
            // needs_summary 恒假，不会触发摘要——报告内的 `chat-image://`
            // 截图引用原样保留，供下一轮 attach_turn_tool_images 附加。
            // summary_provider 传本轮请求 provider 仅为满足签名。
            let result_text = result.output_for_llm();
            let tool_message = common::persist_tool_result_with_compression(
                ctx.db,
                ctx.workspace_id,
                ctx.on_event,
                tool_call,
                &self.tools,
                &McpScope::Global,
                &result_text,
                &iteration.request_provider,
                "",
                |usage| {
                    ctx.usage_tracker.record(usage);
                },
            )
            .await?;
            if let Some(message) = tool_message.to_llm_message() {
                llm_messages.push(message);
            }
        }

        Ok(RunLoopToolOutcome {
            saw_retryable_tool_error: false,
            final_message: None,
            protocol_actions: Vec::new(),
            llm_messages,
        })
    }
}
