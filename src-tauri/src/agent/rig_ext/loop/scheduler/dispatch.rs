//! 整批登记后再启动 owned worker，执行结果只结算到 outbox。
use super::*;

impl TaskScheduler {
    pub(crate) async fn enqueue<P: ToolExecutionPolicy + Clone + 'static>(
        &mut self,
        calls: &[ToolCall],
        tools: &[PortableDynamicTool],
        policy: &P,
        result_policies: &[RigToolResultPolicy],
        round: u64,
        anchor: &str,
    ) -> Result<Vec<String>> {
        let queue_deadline = Instant::now() + Duration::from_secs(60);
        let mut capacity = tools
            .iter()
            .map(|tool| self.budgets.reserve(is_composite(tool.name())))
            .collect::<Result<Vec<_>>>()?;
        // 参数错误在登记前暴露，不能留下半批已登记任务。
        for (call, tool) in calls.iter().zip(tools) {
            prepare_arguments(
                tool.name(),
                &tool.definition().parameters,
                &call.function.arguments,
            )
            .map_err(|error| super::super::budgets::AdmissionError(error.message))?;
        }
        let root = policy.resource_workspace();
        let claim_inputs = tools
            .iter()
            .cloned()
            .zip(calls.iter().cloned())
            .collect::<Vec<_>>();
        let workspace = self.workspace.clone();
        let declarations = tokio::task::spawn_blocking(move || {
            claim_inputs
                .iter()
                .map(|(tool, call)| claims(tool, call, &workspace, root.as_deref()))
                .collect::<Vec<_>>()
        })
        .await?;
        let mut declarations = declarations.into_iter();
        let parent = ToolInvocationContext::current();
        let anchor = parent
            .as_ref()
            .map(|p| p.root_request_message_id.clone())
            .unwrap_or_else(|| anchor.into());
        let drafts = calls
            .iter()
            .zip(tools)
            .enumerate()
            .map(|(index, (call, tool))| {
                let definition = tool.definition();
                let spec = ToolSpec::new(
                    tool.name(),
                    &definition.description,
                    definition.parameters.clone(),
                );
                let effective = prepare_arguments(
                    tool.name(),
                    &definition.parameters,
                    &call.function.arguments,
                )
                .map_err(|e| anyhow::anyhow!(e.message))?;
                Ok((
                    NewToolRun {
                        workspace_id: self.workspace.clone(),
                        tool_call_id: call.wire_call_id().to_string(),
                        tool_name: call.function.name.clone(),
                        provider: spec.provider,
                        category: spec.category.as_str().into(),
                        arguments_json: call.function.arguments.to_string(),
                        effective_arguments_json: effective.to_string(),
                        metadata_json: "{}".into(),
                    },
                    ToolRunTraceContext {
                        parent_run_id: parent.as_ref().map(|p| p.task_id.clone()),
                        // 同一父 run 下 sequence 必须唯一（idx_dispatcher_tool_runs_
                        // parent_sequence）。`32` 与 RunBudgets 的叶子准入上限耦合：
                        // 单批 calls 超过 32 个时 `budgets.reserve` 先失败，走不到这里。
                        sequence: round * 32 + index as u64,
                        ..policy.registration_trace()
                    },
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let (db, run, scope, root_anchor) = (
            self.db.clone(),
            self.run_id.clone(),
            self.scope_id.clone(),
            anchor.clone(),
        );
        let rows = tokio::task::spawn_blocking(move || {
            db.register_tool_task_batch(drafts, &run, &scope, round, &root_anchor)
        })
        .await??;
        let registered = rows
            .into_iter()
            .zip(calls)
            .map(|(row, call)| ToolInvocationContext {
                workspace_id: self.workspace.clone(),
                agent_run_id: self.run_id.clone(),
                task_id: row.id,
                tool_call_id: call.wire_call_id().into(),
                root_request_message_id: anchor.clone(),
                cancel_rx: self.cancel.clone(),
            })
            .collect::<Vec<_>>();
        capacity.reverse();
        let ids = registered.iter().map(|c| c.task_id.clone()).collect();
        for (((mut context, call), tool), result_policy) in registered
            .into_iter()
            .zip(calls)
            .zip(tools)
            .zip(result_policies)
        {
            self.calls.insert(context.task_id.clone(), call.clone());
            let permit = capacity.pop().expect("已为整批预留容量");
            let composite_permit = if is_composite(tool.name()) {
                Some(permit)
            } else {
                self.delivery_permits
                    .insert(context.task_id.clone(), permit);
                None
            };
            let (task_cancel, task_rx) = watch::channel(false);
            let active = Arc::new(std::sync::atomic::AtomicBool::new(false));
            self.controls.insert(
                context.task_id.clone(),
                (round, task_cancel.clone(), active.clone()),
            );
            let mut root_cancel = context.cancel_rx.clone();
            tokio::spawn(async move {
                loop {
                    if *root_cancel.borrow() || root_cancel.has_changed().is_err() {
                        task_cancel.send_replace(true);
                        break;
                    }
                    tokio::select! { _ = task_cancel.closed() => break, _ = root_cancel.changed() => {} }
                }
            });
            context.cancel_rx = task_rx;
            let (db, call, tool, policy, prepare, summaries) = (
                self.db.clone(),
                call.clone(),
                tool.clone(),
                policy.clone(),
                self.prepare.clone(),
                self.budgets.summaries.clone(),
            );
            let result_policy = *result_policy;
            let events = self.events.clone();
            let composite = is_composite(tool.name());
            let declaration = declarations.next().expect("整批资源声明已完成");
            let reservation = if composite {
                None
            } else {
                Some(ResourceArbiter::shared().reserve(declaration))
            };
            let lease = self.run_lease.clone();
            let runtime = self.runtime.clone();
            self.jobs.spawn(async move {
                let _scope = runtime;
                let _capacity = composite_permit;
                let execution = context.clone().scope(async {
                    update_phase(&db, &context.task_id, "reviewing")
                        .await
                        .map_err(|e| ToolExecutionError::other(e.to_string()).with_code("fatal"))?;
                    let guard = policy.before_call(&tool, &call).await;
                    if let Some(error) = guard.rejection {
                        return Err(error);
                    }
                    update_phase(&db, &context.task_id, "queued")
                        .await
                        .map_err(|e| ToolExecutionError::other(e.to_string()).with_code("fatal"))?;
                    let _resource = if let Some(reservation) = reservation {
                        Some(
                            reservation
                                .wait(context.cancel_rx.clone(), queue_deadline)
                                .await
                                .map_err(|e| match e {
                                    super::super::resources::AcquireError::Cancelled => {
                                        ToolExecutionError::cancelled(
                                            "错误：工具尚未执行，本轮运行已取消",
                                        )
                                    }
                                    super::super::resources::AcquireError::QueueTimeout { .. } => {
                                        ToolExecutionError::timeout(e.timeout_message())
                                    }
                                })?,
                        )
                    } else {
                        None
                    };
                    let _permit = if composite {
                        None
                    } else {
                        let semaphore = LEAF_LIMIT
                                .get_or_init(|| Arc::new(Semaphore::new(4)))
                                .clone();
                        Some(tokio::select! {
                            permit = semaphore.acquire_owned() => permit
                                .map_err(|e| ToolExecutionError::other(e.to_string()))?,
                            _ = tokio::time::sleep_until(queue_deadline) => {
                                return Err(ToolExecutionError::timeout("错误：工具执行额度排队超时"));
                            }
                            _ = wait_for_cancel(context.cancel_rx.clone()) => {
                                return Err(ToolExecutionError::cancelled("错误：工具排队期间已取消，未执行"));
                            }
                        })
                    };
                    if *context.cancel_rx.borrow() {
                        return Err(ToolExecutionError::cancelled(
                            "错误：工具尚未执行，本轮调用已取消",
                        ));
                    }
                    if active
                        .compare_exchange(
                            false,
                            true,
                            std::sync::atomic::Ordering::AcqRel,
                            std::sync::atomic::Ordering::Acquire,
                        )
                        .is_err()
                    {
                        return Err(ToolExecutionError::cancelled(
                            "错误：同批失败，工具尚未执行",
                        ));
                    }
                    db.mark_tool_run_started_async(&context.task_id)
                        .await
                        .map_err(|e| ToolExecutionError::other(e.to_string()).with_code("fatal"))?;
                    update_phase(&db, &context.task_id, "running")
                        .await
                        .map_err(|e| ToolExecutionError::other(e.to_string()).with_code("fatal"))?;
                    crate::agent::common::emit(
                        &events,
                        crate::agent::rig_ext::events::AgentEvent::ToolStarted {
                            task_id: Some(context.task_id.clone()),
                            tool_call_id: call.wire_call_id().into(),
                            name: call.function.name.clone(),
                            arguments: call.function.arguments.to_string(),
                        },
                    );
                    policy.execute(&tool, &call).await
                });
                let result = std::panic::AssertUnwindSafe(async {
                    match &lease {
                        Some(lease) => lease.scope(execution).await,
                        None => execution.await,
                    }
                })
                .catch_unwind()
                .await;
                let result = result.unwrap_or_else(|_| {
                    Err(ToolExecutionError::other("错误：工具 worker panic").with_code("fatal"))
                });
                let (status, error_kind, fatal, retryable, raw) = match result {
                    Ok(output) => ("succeeded", None, false, false, tool_output_text(&output)),
                    Err(error) => {
                        let (status, kind, fatal) = classify_tool_error(&error);
                        (
                            status,
                            Some(kind.to_string()),
                            fatal,
                            error.retryable() == Some(true),
                            tool_error_text(&error),
                        )
                    }
                };
                update_phase(&db, &context.task_id, "preparing").await?;
                // 摘要额度信号量从不 close（见 loop/budgets.rs），此处不可能拿到关闭错误；
                // 仅保留错误传播，不再挂不可达的上下文案。
                let _summary = summaries.acquire().await?;
                let prepared = prepare(call, raw.clone(), result_policy).await;
                let draft = CompletionDraft {
                    tool_run_id: context.task_id,
                    status: status.into(),
                    error_kind,
                    fatal,
                    retryable,
                    display_content: prepared.display,
                    context_payload: prepared.context,
                    result_mode: prepared.mode,
                    usage_json: prepared
                        .usage
                        .map(|u| serde_json::to_string(&u))
                        .transpose()?,
                    artifact: ToolArtifactDraft::raw_tool_output(tool.name(), &raw),
                };
                tokio::task::spawn_blocking(move || db.settle_tool_completion(draft)).await?
            });
        }
        Ok(ids)
    }
}

async fn wait_for_cancel(mut rx: watch::Receiver<bool>) {
    while !*rx.borrow() {
        if rx.changed().await.is_err() {
            break;
        }
    }
}
