//! 子 scope 模型决策；工具调度与交付由共享协调器管理。
use super::*;

impl RigSubAgentRuntime {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_loop(
        &self,
        model: &impl rig::completion::CompletionModel,
        messages: &mut Vec<Message>,
        usage: &mut SubAgentUsage,
        start: Instant,
        deadline: Instant,
        force_final_response: &mut bool,
        last_iteration: &mut u32,
        coordinator: &mut crate::agent::rig_ext::r#loop::coordinator::Coordinator,
    ) -> Result<String, String> {
        let mut definitions_all = self
            .surface
            .iter()
            .map(PortableDynamicTool::definition)
            .collect::<Vec<_>>();

        definitions_all.push(rig::completion::ToolDefinition { name: "wait_for_tools".into(), description: "等待工具完成，独占一批。宿主会自动唤醒。".into(), parameters: serde_json::json!({"type":"object","properties":{"reason":{"type":"string"}},"additionalProperties":false}) });
        let channel = self.loop_events.clone();
        let mut task_usage = crate::agent::common::UsageTracker::new();
        let surface = crate::agent::rig_ext::r#loop::RigToolSurface::new(self.surface.clone());
        let mut ids = vec![None; messages.len()];
        let mut failure_policy = super::super::failure::FailurePolicy::default();
        for iteration in 0..self.config.max_iterations {
            if Instant::now() >= deadline {
                return Err(format!(
                    "子智能体 '{}' 执行超时（{}秒）",
                    self.config.agent_id, self.config.timeout_secs
                ));
            }
            if self.cancelled() {
                return Err(format!("子智能体 '{}' 执行已取消", self.config.agent_id));
            }
            *last_iteration = iteration + 1;

            coordinator
                .tasks
                .collect_ready()
                .await
                .map_err(|e| e.to_string())?;
            let (failed_rounds, escalate) = failure_policy.ingest(coordinator.tasks.ready.values());
            for round in failed_rounds {
                coordinator.tasks.cancel_queued(Some(round as u64));
            }
            if escalate {
                *force_final_response = true;
                coordinator.tasks.cancel_all();
                coordinator.tasks.drain().await.map_err(|e| e.to_string())?;
            }

            // 请求前滚动压缩（与主对话同一整形层；子智能体不消耗摘要模型，
            // 规则兜底折叠被裁中段）。头部 2 条（system + 首轮任务）恒保护。
            if coordinator.tasks.ready.is_empty() {
                compact_history_offline(
                    messages,
                    context_budget_chars(self.spec.context_window),
                    SUB_AGENT_HEADER_LEN,
                );
            }
            ids.resize(messages.len(), None);
            coordinator
                .deliver(messages, &mut ids, &channel, &mut task_usage)
                .await
                .map_err(|e| e.to_string())?;

            // 强制收口阶段传空工具集，逼模型给出最终结论。
            let definitions = if *force_final_response {
                Vec::new()
            } else {
                definitions_all.clone()
            };
            let request = build_completion_request(
                None,
                messages.clone(),
                definitions,
                Some(u64::from(self.config.max_output_tokens)),
                self.config.temperature,
                true,
            );

            let mut stream = match timeout(
                Duration::from_secs(SUB_AGENT_LLM_REQUEST_TIMEOUT_SECS),
                coordinator.tasks.drive(model.stream(request)),
            )
            .await
            {
                Ok(Ok(Ok(stream))) => stream,
                Ok(Ok(Err(error))) => {
                    return Err(format!(
                        "子智能体 '{}' 模型请求失败：{error}",
                        self.config.agent_id
                    ))
                }
                Ok(Err(error)) => {
                    return Err(format!(
                        "子智能体 '{}' 模型请求失败：{error}",
                        self.config.agent_id
                    ))
                }
                Err(_) => {
                    return Err(format!(
                        "子智能体 '{}' 单次模型请求超时（{}秒）",
                        self.config.agent_id, SUB_AGENT_LLM_REQUEST_TIMEOUT_SECS
                    ))
                }
            };

            // 流式消费：正文增量 → llmDelta 事件；思考/工具增量忽略
            //（子智能体 UI 只展示正文增量，与旧实现一致）。
            {
                use futures::StreamExt;
                use rig::streaming::StreamedAssistantContent;
                while let Some(item) = coordinator
                    .tasks
                    .drive(stream.next())
                    .await
                    .map_err(|e| e.to_string())?
                {
                    match item {
                        Ok(StreamedAssistantContent::Text(text)) => {
                            if !text.text.is_empty() {
                                self.emit(SubAgentEvent::LlmDelta {
                                    agent_id: self.config.agent_id.clone(),
                                    agent_name: self.config.agent_name.clone(),
                                    delta: text.text,
                                });
                            }
                        }
                        Ok(_) => {}
                        Err(error) => {
                            return Err(format!(
                                "子智能体 '{}' 流式响应失败：{error}",
                                self.config.agent_id
                            ))
                        }
                    }
                }
            }

            if let Some(final_record) = stream.response.as_ref() {
                if final_record.usage.has_values() {
                    usage.record(&final_record.usage);
                    self.emit(SubAgentEvent::UsageUpdated {
                        agent_id: self.config.agent_id.clone(),
                        agent_name: self.config.agent_name.clone(),
                        token_usage: usage.clone(),
                        elapsed_ms: start.elapsed().as_millis() as u64,
                    });
                }
            }

            coordinator
                .tasks
                .collect_ready()
                .await
                .map_err(|e| e.to_string())?;
            if coordinator.tasks.ready.values().any(|event| event.fatal) {
                return Err("子智能体工具发生致命故障".into());
            }
            let (failed_rounds, escalate) = failure_policy.ingest(coordinator.tasks.ready.values());
            for round in failed_rounds {
                coordinator.tasks.cancel_queued(Some(round as u64));
            }
            if escalate && !*force_final_response {
                *force_final_response = true;
                coordinator.tasks.cancel_all();
                coordinator.tasks.drain().await.map_err(|e| e.to_string())?;
                continue;
            }
            let (visible_text, _thinking, tool_calls) = split_choice(&stream.choice);

            // 无工具调用 ⇒ 模型给出最终答复；强制收口阶段即使仍返回工具调用，
            // 也绝不执行——有文本即收口，无文本按错误退出（G13-06）。
            let force_ignore_tool_calls = *force_final_response && !tool_calls.is_empty();
            if tool_calls.is_empty() || force_ignore_tool_calls {
                if force_ignore_tool_calls && visible_text.trim().is_empty() {
                    return Err(format!(
                        "子智能体 '{}' 在强制收口阶段仍返回工具调用且未提供文本结论，无法收口",
                        self.config.agent_id
                    ));
                }
                if visible_text.trim().is_empty() {
                    return Err("子智能体返回空响应，未确认完成结果观察".into());
                }
                coordinator
                    .observed(u64::from(iteration))
                    .await
                    .map_err(|e| e.to_string())?;
                if coordinator.unsettled() {
                    messages.push(Message::assistant(&visible_text));
                    coordinator.tasks.wait().await.map_err(|e| e.to_string())?;
                    continue;
                }
                return Ok(truncate_tool_result(&visible_text));
            }

            if tool_calls.len() > MAX_TOOL_CALLS_PER_BATCH {
                return Err(format!(
                    "子智能体 '{}' 单轮返回 {} 个工具调用，超过运行时上限 {}，已拒绝执行",
                    self.config.agent_id,
                    tool_calls.len(),
                    MAX_TOOL_CALLS_PER_BATCH
                ));
            }

            let mut call_ids = std::collections::HashSet::new();
            if !tool_calls
                .iter()
                .all(|call| call_ids.insert(call.wire_call_id()))
            {
                return Err("子智能体同一批出现重复 tool_call_id，已拒绝执行".into());
            }
            messages.push(build_assistant_turn(&visible_text, &tool_calls));

            coordinator
                .observed(u64::from(iteration))
                .await
                .map_err(|e| e.to_string())?;
            if tool_calls
                .iter()
                .any(|call| call.function.name == "wait_for_tools")
            {
                let reason = if tool_calls.len() != 1 {
                    "控制工具必须独占一批"
                } else {
                    coordinator.tasks.wait().await.map_err(|e| e.to_string())?;
                    if coordinator.tasks.ready.is_empty() {
                        "no_pending_tasks"
                    } else {
                        "tools_ready"
                    }
                };
                for call in &tool_calls {
                    messages.push(tool_result_message(call, reason.into()));
                }
                continue;
            }
            failure_policy.register(
                tool_calls.iter().map(|call| call.function.name.clone()),
                i64::from(iteration),
            );
            let (contents, _) = coordinator
                .dispatch(
                    &tool_calls,
                    &surface,
                    &self.policy,
                    u64::from(iteration),
                    "",
                    &channel,
                    &mut task_usage,
                )
                .await
                .map_err(|e| e.to_string())?;
            messages.push(Message::User { content: contents });
        }

        Err(format!(
            "子智能体 '{}' 达到最大迭代次数（{}）",
            self.config.agent_id, self.config.max_iterations
        ))
    }
}
