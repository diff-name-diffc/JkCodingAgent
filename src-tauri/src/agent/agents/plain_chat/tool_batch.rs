use std::collections::VecDeque;

use super::*;

/// 单个已执行工具收尾的结果。
enum ExecutedToolFinalize {
    Done,
    /// 工具致命失败：当前 run 已按 fatal_error 收尾，由调用方决定中止时机
    /// （只读并行批需先把兄弟结果全部收尾，串行批可立即中止）。
    FatalTool(String),
}

struct ToolUsageContext<'a> {
    tracker: &'a mut UsageTracker,
    workspace_id: &'a str,
    on_event: &'a Channel<AgentEvent>,
}

impl PlainChatAgent {
    /// 执行一批 tool_calls，并保证每个已创建的 tool run 记录都走到终态（G9-01）：
    ///
    /// - 取消：尚未开始的工具不再创建 run 记录；已执行完的结果全部持久化并按
    ///   真实状态收尾，不再中途丢弃（旧实现会遗留永久 started 的悬挂记录并丢失结果）。
    /// - 出错（run 创建失败 / 子智能体致命失败 / 持久化失败）：已执行结果先持久化，
    ///   在途 run 以 failed/fatal_error 收尾后再向上传播错误。
    ///
    /// 返回本批工具结果对应的 LLM tool 消息（已落库），由调用方并入上下文。
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn execute_all_tools(
        &self,
        db: &DispatcherDb,
        tool_calls: &[RequestedToolCall],
        args_map: &std::collections::HashMap<String, String>,
        tool_context: &ToolContext,
        direct_capabilities: &CapabilitySet,
        on_event: &Channel<AgentEvent>,
        cancel_rx: &watch::Receiver<bool>,
        usage_tracker: &mut UsageTracker,
        workspace_id: &str,
        summary_provider: &OpenAiCompatProvider,
        summary_model: &str,
    ) -> Result<Vec<ChatMessage>> {
        let mut llm_messages = Vec::new();
        let readonly_end =
            common::readonly_tool_run_end(&self.tools, &tool_context.mcp_scope, tool_calls, 0);

        let serial_calls = if readonly_end >= 2 {
            let readonly_run = &tool_calls[..readonly_end];
            for tool_call in readonly_run {
                common::emit(
                    on_event,
                    AgentEvent::ToolStarted {
                        tool_call_id: tool_call.id.clone(),
                        name: tool_call.name.clone(),
                        arguments: args_map
                            .get(&tool_call.id)
                            .cloned()
                            .unwrap_or_else(|| "{}".to_string()),
                    },
                );
            }

            let mut run_ids = Vec::with_capacity(readonly_run.len());
            for tool_call in readonly_run {
                match self
                    .create_and_start_tool_run(db, workspace_id, tool_context, on_event, tool_call)
                    .await
                {
                    Ok(run_id) => run_ids.push(run_id),
                    Err(error) => {
                        // 补偿：把本批已创建的 run 收敛为 failed，
                        // 避免遗留「已启动未收尾」的悬挂记录。
                        let message = format!("错误：创建工具运行记录失败：{error}");
                        self.finalize_started_runs(db, on_event, &run_ids, "failed", &message)
                            .await;
                        return Err(error);
                    }
                }
            }

            let semaphore = Arc::new(tokio::sync::Semaphore::new(
                crate::agent::tools::MAX_PARALLEL_TOOL_CALLS,
            ));
            let readonly_results: Vec<ToolResult> =
                futures::future::join_all(readonly_run.iter().map(|tool_call| {
                    let cancel_rx = cancel_rx.clone();
                    let semaphore = Arc::clone(&semaphore);
                    async move {
                        let Ok(_permit) = semaphore.acquire().await else {
                            return ToolResult::fatal_error(
                                "只读工具并发调度器意外关闭，已拒绝执行",
                            );
                        };
                        ToolRuntime::execute_tool_with_cancellation(
                            &self.tools,
                            direct_capabilities,
                            tool_call,
                            tool_context,
                            cancel_rx,
                        )
                        .await
                    }
                }))
                .await;

            // 只读批的所有工具此时都已执行完、run 记录都已创建：任何退出路径
            // （取消 / 子智能体致命失败 / 持久化失败）都必须先把它们全部收尾，
            // 不允许遗留悬挂记录或丢弃已执行结果。
            let mut pending = readonly_run
                .iter()
                .zip(readonly_results)
                .zip(run_ids)
                .map(|((tool_call, result), run_id)| (tool_call.clone(), result, run_id))
                .collect::<VecDeque<_>>();
            let mut fatal_message: Option<String> = None;
            while let Some((tool_call, result, run_id)) = pending.pop_front() {
                match self
                    .persist_and_finalize_executed_tool(
                        db,
                        on_event,
                        workspace_id,
                        &tool_context.mcp_scope,
                        &tool_call,
                        result,
                        &run_id,
                        summary_provider,
                        summary_model,
                        usage_tracker,
                        &mut llm_messages,
                    )
                    .await
                {
                    Ok(ExecutedToolFinalize::Done) => {}
                    Ok(ExecutedToolFinalize::FatalTool(message)) => {
                        // 先收尾兄弟结果，循环结束后再中止。
                        fatal_message.get_or_insert(message);
                    }
                    Err(error) => {
                        // 持久化失败：其余已执行的 run 无法落库，统一收敛为
                        // failed 避免悬挂，然后向上抛原始错误。
                        let pending_run_ids = pending
                            .iter()
                            .map(|(_, _, run_id)| run_id.clone())
                            .collect::<Vec<_>>();
                        self.finalize_started_runs(
                            db,
                            on_event,
                            &pending_run_ids,
                            "failed",
                            &format!("错误：工具结果持久化失败：{error}"),
                        )
                        .await;
                        return Err(error);
                    }
                }
            }
            if let Some(message) = fatal_message {
                anyhow::bail!("{}", message);
            }

            &tool_calls[readonly_end..]
        } else {
            tool_calls
        };

        for tool_call in serial_calls {
            if cancellation_requested(cancel_rx) {
                break; // 尚未创建 run 记录，无悬挂风险
            }
            common::emit(
                on_event,
                AgentEvent::ToolStarted {
                    tool_call_id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    arguments: args_map
                        .get(&tool_call.id)
                        .cloned()
                        .unwrap_or_else(|| "{}".to_string()),
                },
            );
            let run_id = self
                .create_and_start_tool_run(db, workspace_id, tool_context, on_event, tool_call)
                .await?;
            let result = self
                .execute_single_tool_with_usage(
                    tool_call,
                    tool_context,
                    direct_capabilities,
                    ToolUsageContext {
                        tracker: usage_tracker,
                        workspace_id,
                        on_event,
                    },
                    cancel_rx,
                )
                .await;
            match self
                .persist_and_finalize_executed_tool(
                    db,
                    on_event,
                    workspace_id,
                    &tool_context.mcp_scope,
                    tool_call,
                    result,
                    &run_id,
                    summary_provider,
                    summary_model,
                    usage_tracker,
                    &mut llm_messages,
                )
                .await?
            {
                ExecutedToolFinalize::Done => {}
                // 串行批：后续工具尚未创建 run 记录，可立即中止
                ExecutedToolFinalize::FatalTool(message) => {
                    anyhow::bail!("{}", message);
                }
            }
        }

        Ok(llm_messages)
    }

    /// 持久化单个已执行工具的结果并收尾其 run 记录。
    ///
    /// - 检测到结构化致命错误：当前 run 以 fatal_error 收尾，返回
    ///   `FatalTool` 由调用方决定中止时机（不在此处直接抛错，避免
    ///   并行批中其余已执行工具的 run 被悬挂）；
    /// - 结果持久化失败：尽力把当前 run 收敛为 failed 后再向上抛错；
    /// - run 收尾失败：结果已落库，仅告警不中断主流程。
    #[allow(clippy::too_many_arguments)]
    async fn persist_and_finalize_executed_tool(
        &self,
        db: &DispatcherDb,
        on_event: &Channel<AgentEvent>,
        workspace_id: &str,
        mcp_scope: &McpScope,
        tool_call: &RequestedToolCall,
        result: ToolResult,
        run_id: &str,
        summary_provider: &OpenAiCompatProvider,
        summary_model: &str,
        usage_tracker: &mut UsageTracker,
        llm_messages: &mut Vec<ChatMessage>,
    ) -> Result<ExecutedToolFinalize> {
        let result_text = result.output_for_llm();
        let result_metadata_json = result.run_metadata_json();
        // 致命错误一律来自类型化的 `ToolResult.status`，不再做文本前缀推断。
        if result.status == crate::agent::tools::ToolStatus::FatalError {
            self.finish_tool_run(
                db,
                on_event,
                run_id,
                result.status.as_run_status(),
                None,
                None,
                result.status.error_kind(),
                Some(&result_text),
                None,
                result_metadata_json.as_deref(),
            )
            .await?;
            return Ok(ExecutedToolFinalize::FatalTool(result_text));
        }

        let tool_message = match persist_tool_result_with_compression(
            db,
            workspace_id,
            on_event,
            tool_call,
            &self.tools,
            mcp_scope,
            &result_text,
            summary_provider,
            summary_model,
            |usage| {
                usage_tracker.record(usage);
            },
        )
        .await
        {
            Ok(message) => message,
            Err(error) => {
                self.finalize_started_runs(
                    db,
                    on_event,
                    &[run_id.to_string()],
                    "failed",
                    &format!("错误：工具结果持久化失败：{error}"),
                )
                .await;
                return Err(error);
            }
        };
        if let Some(message) = tool_message.to_llm_message() {
            llm_messages.push(message);
        }
        if let Err(error) = self
            .finish_tool_run(
                db,
                on_event,
                run_id,
                result.status.as_run_status(),
                tool_message.tool_result_mode.as_deref(),
                Some(&tool_message.id),
                result.status.error_kind(),
                result.status.error_kind().map(|_| result_text.as_str()),
                result.action.as_ref().map(ToolAction::kind),
                result_metadata_json.as_deref(),
            )
            .await
        {
            eprintln!(
                "错误：工具 '{}' 的 run 记录收尾失败（结果已持久化）：{}",
                tool_call.name, error
            );
        }
        Ok(ExecutedToolFinalize::Done)
    }

    /// 尽力把一组已创建的 tool run 收敛到终态（补偿路径）：
    /// 二次失败仅告警，绝不掩盖原始错误。
    async fn finalize_started_runs(
        &self,
        db: &DispatcherDb,
        on_event: &Channel<AgentEvent>,
        run_ids: &[String],
        status: &str,
        error_message: &str,
    ) {
        for run_id in run_ids {
            if let Err(error) = self
                .finish_tool_run(
                    db,
                    on_event,
                    run_id,
                    status,
                    None,
                    None,
                    Some("internal"),
                    Some(error_message),
                    None,
                    None,
                )
                .await
            {
                eprintln!("错误：补偿收尾工具 run 记录 {run_id} 失败：{error}");
            }
        }
    }

    async fn create_and_start_tool_run(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        tool_context: &ToolContext,
        on_event: &Channel<AgentEvent>,
        tool_call: &RequestedToolCall,
    ) -> Result<String> {
        ToolRuntime::create_and_start_tool_run(
            db,
            &self.tools,
            workspace_id,
            &tool_context.mcp_scope,
            on_event,
            tool_call,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_tool_run(
        &self,
        db: &DispatcherDb,
        on_event: &Channel<AgentEvent>,
        run_id: &str,
        status: &str,
        result_mode: Option<&str>,
        message_id: Option<&str>,
        error_kind: Option<&str>,
        error_message: Option<&str>,
        action_kind: Option<&str>,
        metadata_json: Option<&str>,
    ) -> Result<()> {
        ToolRuntime::finish_tool_run(
            db,
            on_event,
            run_id,
            ToolRunFinishUpdate {
                status,
                result_mode,
                message_id,
                error_kind,
                error_message,
                action_kind,
                metadata_json,
            },
        )
        .await
    }

    /// 执行单个工具。`call_sub_agent` 期间暂停主 Agent 的用量计时，
    /// 避免子 Agent 耗时稀释主 Agent 的 token 生成速度。
    async fn execute_single_tool_with_usage(
        &self,
        tool_call: &RequestedToolCall,
        tool_context: &ToolContext,
        direct_capabilities: &CapabilitySet,
        usage: ToolUsageContext<'_>,
        cancel_rx: &watch::Receiver<bool>,
    ) -> ToolResult {
        let is_sub_agent_call = tool_call.name == "call_sub_agent";

        if is_sub_agent_call {
            with_usage_paused(
                usage.tracker,
                usage.workspace_id,
                usage.on_event,
                || async {
                    ToolRuntime::execute_tool_with_cancellation(
                        &self.tools,
                        direct_capabilities,
                        tool_call,
                        tool_context,
                        cancel_rx.clone(),
                    )
                    .await
                },
            )
            .await
        } else {
            ToolRuntime::execute_tool_with_cancellation(
                &self.tools,
                direct_capabilities,
                tool_call,
                tool_context,
                cancel_rx.clone(),
            )
            .await
        }
    }
}
