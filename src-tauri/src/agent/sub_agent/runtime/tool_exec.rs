use super::*;

/// 构造 tool 角色消息（工具结果 / 重试收口提示 / 未执行说明共用）。
pub(super) fn tool_result_message(tc: &RequestedToolCall, content: String) -> ChatMessage {
    ChatMessage {
        role: "tool".to_string(),
        content,
        content_parts: Vec::new(),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: Some(tc.id.clone()),
        name: Some(tc.name.clone()),
    }
}

/// 收集执行组内可恢复失败工具的名字（按出现顺序去重）。
pub(super) fn distinct_failed_tool_names(
    executed: &[(&RequestedToolCall, ToolResult)],
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (tc, result) in executed {
        if result.status == ToolStatus::RecoverableError
            && !names.iter().any(|existing| existing == &tc.name)
        {
            names.push(tc.name.clone());
        }
    }
    names
}

pub(super) fn truncate_tool_result(result: &str) -> String {
    let char_count = result.chars().count();
    if char_count <= SUB_AGENT_RESULT_MAX_CHARS {
        return result.to_string();
    }
    let keep = SUB_AGENT_RESULT_MAX_CHARS / 2;
    let head: String = result.chars().take(keep).collect();
    let tail: String = result.chars().skip(char_count - keep).collect();
    let dropped = char_count - SUB_AGENT_RESULT_MAX_CHARS;
    format!("{head}\n\n[...已截断 {dropped} 字符...]\n\n{tail}")
}

pub(super) fn tool_result_preview(tool_name: &str, result: &str) -> String {
    let preview_limit = if is_command_review_result(tool_name, result) {
        4_000
    } else {
        200
    };
    if result.chars().count() > preview_limit {
        format!(
            "{}...",
            result.chars().take(preview_limit).collect::<String>()
        )
    } else {
        result.to_string()
    }
}

pub(super) fn is_command_review_result(tool_name: &str, result: &str) -> bool {
    matches!(tool_name, "ssh_exec" | "local_zsh")
        && (result.starts_with("## SSH 命令审查记录")
            || (result.starts_with("## local_zsh 执行结果") && result.contains("审查结论: `拦截`")))
}

impl SubAgentRuntime {
    pub(super) async fn execute_single_tool(
        &self,
        tc: &RequestedToolCall,
        app_handle: &Option<AppHandle>,
        session_id: &str,
        started_at: Instant,
        overall_timeout: Duration,
        cancel_rx: Option<&watch::Receiver<bool>>,
    ) -> ToolResult {
        self.emit_event(
            app_handle,
            session_id,
            SubAgentEvent::ToolStarted {
                agent_id: self.config.agent_id.clone(),
                agent_name: self.config.agent_name.clone(),
                tool_name: tc.name.clone(),
                arguments: tc.arguments.clone(),
            },
        );

        let result = self
            .execute_tool_with_budget(tc, started_at, overall_timeout, cancel_rx)
            .await;

        let result_text = result.output_for_llm();
        let result_preview = tool_result_preview(&tc.name, &result_text);

        self.emit_event(
            app_handle,
            session_id,
            SubAgentEvent::ToolFinished {
                agent_id: self.config.agent_id.clone(),
                agent_name: self.config.agent_name.clone(),
                tool_name: tc.name.clone(),
                result_preview,
            },
        );

        result
    }

    /// 子智能体整体预算到期时向 Broker 发送取消，而不是丢弃执行 Future。
    /// Broker 负责调用工具级取消并在固定宽限期内等待收敛；因此这里返回时，
    /// 结果会明确区分“已终止”和“终止状态未知”。
    pub(super) async fn execute_tool_with_budget(
        &self,
        tc: &RequestedToolCall,
        started_at: Instant,
        overall_timeout: Duration,
        cancel_rx: Option<&watch::Receiver<bool>>,
    ) -> ToolResult {
        let remaining = overall_timeout.saturating_sub(started_at.elapsed());
        let parent_cancelled = cancel_rx.is_some_and(|cancel_rx| *cancel_rx.borrow());
        let (budget_cancel_tx, budget_cancel_rx) = watch::channel(parent_cancelled);
        let deadline_triggered = Arc::new(AtomicBool::new(false));

        let signal_task = if parent_cancelled {
            None
        } else {
            let mut parent_cancel_rx = cancel_rx.cloned();
            let deadline_triggered = Arc::clone(&deadline_triggered);
            Some(tokio::spawn(async move {
                let deadline = tokio::time::sleep(remaining);
                tokio::pin!(deadline);

                if let Some(parent_cancel_rx) = &mut parent_cancel_rx {
                    loop {
                        tokio::select! {
                            _ = &mut deadline => {
                                deadline_triggered.store(true, Ordering::Release);
                                let _ = budget_cancel_tx.send(true);
                                return;
                            }
                            changed = parent_cancel_rx.changed() => {
                                match changed {
                                    Ok(()) if *parent_cancel_rx.borrow() => {
                                        let _ = budget_cancel_tx.send(true);
                                        return;
                                    }
                                    Ok(()) => {}
                                    Err(_) => {
                                        let _ = budget_cancel_tx.send(true);
                                        return;
                                    }
                                }
                            }
                        }
                    }
                } else {
                    deadline.await;
                    deadline_triggered.store(true, Ordering::Release);
                    let _ = budget_cancel_tx.send(true);
                }
            }))
        };

        let result = ToolRuntime::execute_tool_with_cancellation(
            &self.tool_registry,
            &self.capabilities,
            tc,
            &self.tool_context,
            budget_cancel_rx,
        )
        .await;
        if let Some(signal_task) = signal_task {
            signal_task.abort();
        }

        if deadline_triggered.load(Ordering::Acquire) {
            let original_status = result.status.as_run_status();
            let termination = result.metadata.get("termination").cloned();
            let mut timeout_result = ToolResult::fatal_error(format!(
                "错误：工具 '{}' 达到子智能体 '{}' 的整体超时边界（{}秒）；底层终止状态见结果元数据。",
                tc.name, self.config.agent_id, self.config.timeout_secs
            ));
            timeout_result.metadata = json!({
                "overallTimeout": {
                    "timeoutSeconds": self.config.timeout_secs,
                    "originalStatus": original_status,
                    "termination": termination,
                }
            });
            timeout_result
        } else {
            result
        }
    }

    pub(super) async fn execute_parallel_readonly_tools<'a>(
        &'a self,
        tool_calls: &'a [RequestedToolCall],
        app_handle: &'a Option<AppHandle>,
        session_id: &'a str,
        started_at: Instant,
        overall_timeout: Duration,
        cancel_rx: Option<&watch::Receiver<bool>>,
    ) -> Vec<(&'a RequestedToolCall, ToolResult)> {
        for tc in tool_calls {
            self.emit_event(
                app_handle,
                session_id,
                SubAgentEvent::ToolStarted {
                    agent_id: self.config.agent_id.clone(),
                    agent_name: self.config.agent_name.clone(),
                    tool_name: tc.name.clone(),
                    arguments: tc.arguments.clone(),
                },
            );
        }

        // 并行调用共享同一个绝对整体截止时间。每个分支自行把该截止时间
        // 转成 Broker 取消信号，join_all 只负责等待所有已启动分支完成收敛。
        let semaphore = Arc::new(tokio::sync::Semaphore::new(
            crate::agent::tools::MAX_PARALLEL_TOOL_CALLS,
        ));
        let results = join_all(tool_calls.iter().map(|tc| {
            let cancel_rx = cancel_rx.cloned();
            let semaphore = Arc::clone(&semaphore);
            async move {
                let Ok(_permit) = semaphore.acquire().await else {
                    return (
                        tc,
                        ToolResult::fatal_error("只读工具并发调度器意外关闭，已拒绝执行"),
                    );
                };
                let result = self
                    .execute_tool_with_budget(tc, started_at, overall_timeout, cancel_rx.as_ref())
                    .await;
                (tc, result)
            }
        }))
        .await;

        for (tc, result) in &results {
            let result_text = result.output_for_llm();
            let result_preview = tool_result_preview(&tc.name, &result_text);
            self.emit_event(
                app_handle,
                session_id,
                SubAgentEvent::ToolFinished {
                    agent_id: self.config.agent_id.clone(),
                    agent_name: self.config.agent_name.clone(),
                    tool_name: tc.name.clone(),
                    result_preview,
                },
            );
        }

        results
    }

    pub(super) fn emit_failed(
        &self,
        app_handle: &Option<AppHandle>,
        session_id: &str,
        error: &str,
    ) {
        self.emit_event(
            app_handle,
            session_id,
            SubAgentEvent::Failed {
                agent_id: self.config.agent_id.clone(),
                agent_name: self.config.agent_name.clone(),
                error: error.to_string(),
            },
        );
    }
}
