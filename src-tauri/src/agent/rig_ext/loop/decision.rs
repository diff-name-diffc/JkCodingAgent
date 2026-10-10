//! 主 scope 决策，调度与交付由 Coordinator 驱动。
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_loop_inner<M, S, P>(
    db: &DispatcherDb,
    workspace_id: &str,
    model: &M,
    mut messages: Vec<Message>,
    mut message_ids: Vec<Option<String>>,
    surface: &RigToolSurface,
    tool_policy: &P,
    summary: Option<&RigSummaryModel<'_, S>>,
    hooks: &mut RigLoopHooks,
    on_event: &Channel<AgentEvent>,
    cancel_rx: watch::Receiver<bool>,
    usage_tracker: &mut UsageTracker,
    coordinator: &mut coordinator::Coordinator,
) -> Result<DispatcherMessageRecord>
where
    M: CompletionModel,
    S: CompletionModel + Clone + 'static,
    P: ToolExecutionPolicy + Clone + 'static,
{
    // 上下文整形预算（统一整形层）：容量源为 hooks.context_window（三条
    // 装配路径均已以槽位规格回填）；上下文超限错误（400）时预算减半重试，
    // 故为循环变量而非常量。
    // run 身份唯一化为调度器的 run_id（TaskScheduler::new：顶层新 uuid、
    // 子 scope 继承父 agent_run_id）——不再另造 uuid，保证按 agent_run_id
    // 查运行树不漏行。
    let (anchor_db, anchor_workspace) = (db.clone(), workspace_id.to_string());
    let root_request_anchor = tokio::task::spawn_blocking(move || {
        anchor_db.latest_user_request_anchor(&anchor_workspace)
    })
    .await??;
    let mut history_budget_chars =
        super::super::context::context_budget_chars(hooks.context_window);
    let mut overflow_retries = 0usize;
    // 迭代边界的配对不变量由写侧保证（工具结果同批补齐、取消/致命失败补
    // 占位）；装配历史经 DB 读侧修复。这里对初始序列再做一次防御性修复，
    // 保证后续 compact_history 的下标与 message_ids 严格对齐。
    // 与 messages 平行的落库消息 id。装配历史带真实 id，摘要和占位为 None。
    // 长度对不上就放弃锚点，不能让后续 splice 越界。
    if message_ids.len() != messages.len() {
        message_ids = vec![None; messages.len()];
    }
    super::super::context::repair_pairing_aligned(&mut messages, &mut message_ids);

    let mut decisions = 0;
    while decisions < hooks.max_iterations {
        let iteration = decisions;
        if cancellation_requested(&cancel_rx) {
            // 循环边界取消时尚未开始流式输出，无 delta 序号可对账。
            return finalize_cancelled(db, workspace_id, on_event, hooks, usage_tracker, "", None)
                .await;
        }

        // 未被有效响应确认的完成结果禁止参与压缩。
        if coordinator.tasks.ready.is_empty() {
            // 历史级滚动压缩：超预算时被裁中段折叠为【前情摘要】滚动摘要消息
            //（摘要模型缺省/失败回退零 LLM 规则抽取），而非占位丢弃；旧摘要
            // 头部并入新摘要保持滚动连续。只作用于发给模型的内存视图——落库
            // 走 persist_* 路径（以本轮新内容为参数），裁剪绝不影响持久化历史。
            if let Some(outcome) = super::super::context::compact_history(
                &mut messages,
                history_budget_chars,
                1, // 保护头部 1 条（首轮任务意图；是滚动摘要时由压缩层折叠并入）
                summary,
                usage_tracker,
                Some(&cancel_rx),
            )
            .await
            {
                let removed: Vec<Option<String>> = message_ids
                    .splice(
                        outcome.splice_start..outcome.splice_start + outcome.dropped_len,
                        [None],
                    )
                    .collect();
                // 覆盖范围内最近一条已知消息 id 作为锚点持久化（best-effort：
                // 失败不影响本轮运行，下一 run 只是少了跨 run 延续）。
                if let Some(anchor) = removed.iter().rev().find_map(|id| id.as_deref()) {
                    if let Err(error) = db
                        .upsert_session_summary_async(workspace_id, &outcome.summary, anchor)
                        .await
                    {
                        eprintln!("upsert session summary failed ({workspace_id}): {error:#}");
                    }
                }
            }

            if cancellation_requested(&cancel_rx) {
                return finalize_cancelled(
                    db,
                    workspace_id,
                    on_event,
                    hooks,
                    usage_tracker,
                    "",
                    None,
                )
                .await;
            }
        }

        // 本轮工具图片附加（chat-image:// 引用 → 视觉输入，就地写入内存视图：
        // 解析一次驻留、后续迭代零磁盘重读），vision 槽位切换随之命中。
        coordinator
            .deliver(&mut messages, &mut message_ids, on_event, usage_tracker)
            .await?;
        if let Some(anchor) = root_request_anchor.as_ref() {
            let index = message_ids
                .iter()
                .position(|id| id.as_ref() == Some(anchor));
            super::super::message::attach_turn_tool_images_at(&mut messages, index).await;
        } else {
            attach_turn_tool_images(&mut messages).await;
        }
        let effective_messages = messages.clone();
        let preamble = hooks
            .preamble_for_iteration
            .as_mut()
            .and_then(|build| build(iteration));
        // 在途任务快照：让模型对「等不等、等什么」有据可依（确定性生成，不调 LLM）。
        let preamble = match wait::render_pending_snapshot(&coordinator.tasks.pending_tasks()) {
            Some(snapshot) => Some(match preamble {
                Some(base) => format!("{base}\n\n{snapshot}"),
                None => snapshot,
            }),
            None => preamble,
        };
        let request = build_completion_request(
            preamble,
            effective_messages,
            {
                let mut definitions = surface.definitions();
                // 仅当存在未结算任务时才提供等待工具：结构性消除空调用。
                if coordinator.unsettled() {
                    definitions.push(wait::wait_tool_definition());
                }
                definitions
            },
            hooks.request_max_tokens,
            hooks.request_temperature,
            hooks.request_enable_thinking,
        );

        let mut stream = match coordinator.tasks.drive(model.stream(request)).await? {
            Ok(stream) => stream,
            Err(error) => {
                let error_text = format!("{error}");
                // 上下文超限（400）：预算减半后重试（压缩在循环顶部重新执行）。
                // 整形层按估算字符数控制预算，与服务端真实 tokenizer 存在误差，
                // 这里是估算失灵时的恢复路径，而非正常路径。
                if overflow_retries < MAX_CONTEXT_OVERFLOW_RETRIES
                    && super::super::context::is_context_overflow_error(&error_text)
                {
                    overflow_retries += 1;
                    history_budget_chars = (history_budget_chars / 2).max(MIN_CONTEXT_BUDGET_CHARS);
                    // 减半只缩文本。驻留 base64 不缩，先换成引用再重发。
                    super::super::message::degrade_resident_images(&mut messages);
                    eprintln!(
                        "LLM 请求上下文超限（model={}），预算收缩至 {history_budget_chars} 字符后重试（第 {overflow_retries} 次）：{error_text}",
                        current_model_name(hooks)
                    );
                    continue;
                }
                return Err(anyhow::anyhow!(
                    "LLM 流式请求失败（model={}）：{error_text}",
                    current_model_name(hooks)
                ));
            }
        };

        // ModelSwitched：仅首轮通知（对齐 select_provider_for_messages 的
        // notify_user = iteration == 0；与聊天 provider 完全一致不通知）。
        if iteration == 0 {
            maybe_emit_model_switched(hooks, on_event);
        }

        // drive 的取消分支与 consume_stream 自带的取消检查竞态：drive 先赢时
        // 流式 future 被整体 drop，已流出的正文经 StreamProgress 带出，随取消
        // 收口落库（与下方 consumption.cancelled 路径同一语义）。
        let mut progress = StreamProgress::default();
        let consumption = match coordinator
            .tasks
            .drive(consume_stream(
                &mut stream,
                on_event,
                cancel_rx.clone(),
                &mut progress,
            ))
            .await
        {
            Ok(consumption) => consumption?,
            Err(error) if error.is::<scheduler::RuntimeCancelled>() => {
                stream.cancel();
                return finalize_cancelled(
                    db,
                    workspace_id,
                    on_event,
                    hooks,
                    usage_tracker,
                    &progress.partial_text,
                    progress.seq.checked_sub(1),
                )
                .await;
            }
            Err(error) => return Err(error),
        };
        if consumption.cancelled {
            return finalize_cancelled(
                db,
                workspace_id,
                on_event,
                hooks,
                usage_tracker,
                &consumption.partial_text,
                consumption.last_seq,
            )
            .await;
        }

        // 用量：零值是 rig 文档化的「未上报」哨兵，不记录。
        if let Some(usage) = stream
            .response
            .as_ref()
            .map(|final_record| final_record.usage)
            .filter(|usage| usage.has_values())
        {
            record_rig_usage(
                db,
                workspace_id,
                &current_model_name(hooks),
                hooks.usage_source,
                &usage,
                hooks.context_window,
                usage_tracker,
                on_event,
            );
        }

        let (visible_text, thinking, tool_calls) = split_choice(&stream.choice);
        decisions += 1;

        coordinator.tasks.collect_ready().await?;
        if coordinator.tasks.ready.values().any(|r| r.fatal) {
            anyhow::bail!("后台工具发生致命故障，停止新调用");
        }

        if tool_calls.is_empty() {
            if visible_text.is_empty() {
                let diagnostics = RigTurnDiagnostics {
                    model_name: current_model_name(hooks),
                    finish_reason: stream
                        .response
                        .as_ref()
                        .and_then(|final_record| final_record.finish_reason.as_ref())
                        .map(format_finish_reason),
                    thinking_chars: thinking.chars().count(),
                    completion_tokens: stream
                        .response
                        .as_ref()
                        .map(|final_record| final_record.usage.output_tokens),
                };
                anyhow::bail!("{}", (hooks.empty_response_error)(&diagnostics));
            }
            let usage_stats = usage_tracker.snapshot();
            let reply =
                persist_assistant_message(db, workspace_id, &visible_text, &usage_stats).await?;
            emit(
                on_event,
                AgentEvent::AssistantMessage {
                    message: Box::new(reply.clone()),
                    last_seq: consumption.last_seq,
                },
            );
            coordinator.observed(iteration as u64).await?;
            if coordinator.unsettled() {
                messages.push(Message::assistant(&visible_text));
                message_ids.push(Some(reply.id.clone()));
                // 答复已落库并发事件：等待期间被取消时直接以该答复收口，
                // 不再经外层取消路径重复落一条「已停止」stub（幂等）。
                // 隐式等待：任一任务结算即唤醒（渐进响应语义 + 合并宽限）。
                let wait = coordinator
                    .tasks
                    .wait_for(&wait::WaitCondition::any_pending())
                    .await;
                if let Err(error) = wait {
                    if error.is::<scheduler::RuntimeCancelled>() {
                        return Ok(reply);
                    }
                    return Err(error);
                }
                continue;
            }
            return Ok(reply);
        }

        if tool_calls.len() > hooks.max_tool_calls_per_batch {
            anyhow::bail!(
                "模型单轮返回 {} 个工具调用，超过运行时上限 {}；已在持久化或执行前拒绝。",
                tool_calls.len(),
                hooks.max_tool_calls_per_batch
            );
        }

        let mut call_ids = std::collections::HashSet::new();
        anyhow::ensure!(
            tool_calls
                .iter()
                .all(|call| call_ids.insert(call.wire_call_id())),
            "同一批出现重复 tool_call_id"
        );

        // G9-07/G9-14：tool_call_id 必填贯穿 Planned→Started→Finished；
        // 参数序列化失败上抛（不静默降级为 {}）。
        let outbound_calls = tool_calls
            .iter()
            .map(outbound_tool_call)
            .collect::<Result<Vec<_>>>()?;
        for call in &outbound_calls {
            emit(
                on_event,
                AgentEvent::ToolPlanned {
                    tool_call_id: call.id.clone(),
                    name: call.function.name.clone(),
                    arguments: call.function.arguments.clone(),
                },
            );
        }

        // 落库 assistant 工具调用消息（含思考），再向历史追加等价 rig 消息
        //（思考不回灌内存视图——瞬态产物，见 build_assistant_message）。
        let tool_calls_record = persist_tool_calls_message(
            db,
            workspace_id,
            &visible_text,
            &outbound_calls,
            &thinking,
            Some(consumption.thinking_elapsed_ms),
        )
        .await?;
        messages.push(build_assistant_message(
            &visible_text,
            &tool_calls,
            stream.message_id.clone(),
        ));
        message_ids.push(Some(tool_calls_record.id.clone()));
        coordinator.observed(iteration as u64).await?;

        let has_wait = tool_calls
            .iter()
            .any(|c| c.function.name == wait::WAIT_TOOL_NAME);
        let terminal = tool_calls
            .iter()
            .any(|c| matches!(c.function.name.as_str(), "message" | "submit_workflow"));
        let mixed_protocol = hooks.protocol_handler.as_ref().is_some_and(|handler| {
            tool_calls
                .iter()
                .any(|call| handler.handles(&call.function.name))
                && tool_calls
                    .iter()
                    .any(|call| !handler.handles(&call.function.name))
        });
        let control_error = if mixed_protocol {
            Some("协议工具不能与业务工具混批")
        } else if (has_wait || terminal) && tool_calls.len() != 1 {
            Some(wait::EXCLUSIVE_BATCH_VIOLATION)
        } else if terminal && coordinator.unsettled() {
            Some("pending_tasks_require_wait")
        } else {
            None
        };
        if has_wait || control_error.is_some() {
            // 纯 wait 批执行条件等待（wait_for_tools 独占一批已由 control_error
            // 保证）；混批/独占/协议屏障违规不等待，直接以违规文案立即应答。
            let waited = if control_error.is_none() {
                Some(
                    coordinator
                        .tasks
                        .wait_for(&wait::parse_wait_args(&tool_calls[0].function.arguments))
                        .await,
                )
            } else {
                None
            };
            // 与 DB 重载侧 `strip_delivered_wait_pairs` 同口径：结果 JSON 的
            // ready 数组非空 = 等待交付了进展（原因不限 tools_ready——timeout
            // 但部分就绪同样交付了进展，事实由循环顶部 deliver 的 runtime
            // 消息承载），控制对不进入模型上下文，只留 DB 与前端事件。
            let ready = match &waited {
                Some(Ok(_)) => coordinator
                    .tasks
                    .ready
                    .values()
                    .filter(|c| c.delivery_message_id.is_none())
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            };
            let text = match &waited {
                Some(Ok(reason)) => wait::render_wait_result(
                    reason.as_str(),
                    &ready,
                    coordinator.tasks.unsettled_count(),
                ),
                Some(Err(_)) => wait::render_wait_result("cancelled_or_failed", &[], 0),
                None => wait::render_wait_result(
                    control_error.unwrap_or("control_rejected"),
                    &[],
                    coordinator.tasks.unsettled_count(),
                ),
            };
            // 与 coordinator::dispatch 的拒绝路径同一收口：落库 + ToolFinished
            // 事件（控制类调用不发 ToolStarted，与立即应答语义一致），保证
            // 已发的 ToolPlanned 必有终态，前端不悬挂幽灵卡片。
            let (results, last_result_id) = coordinator
                .settle_unexecuted_batch(&tool_calls, &text, on_event)
                .await?;
            // 无进展（no_pending/timeout 空/违规拒绝）照常推入，作为模型的
            // 纠错反馈，防止上下文不变导致退化式重复调用。
            let made_progress = !ready.is_empty();
            if made_progress {
                // 成对回退此前推入的 assistant wait 调用消息，保持配对不变量
                //（DB 侧两行均保留；重载装配由 strip_delivered_wait_pairs 同口径剔除）。
                messages.pop();
                message_ids.pop();
            } else {
                messages.push(Message::User { content: results });
                message_ids.push(last_result_id);
            }
            if let Some(Err(error)) = waited {
                return Err(error);
            }
            continue;
        }
        let is_protocol = hooks
            .protocol_handler
            .as_ref()
            .is_some_and(|handler| tool_calls.iter().any(|c| handler.handles(&c.function.name)));
        if !is_protocol {
            let (content, id) = coordinator
                .dispatch(
                    &tool_calls,
                    surface,
                    tool_policy,
                    iteration as u64,
                    root_request_anchor
                        .as_deref()
                        .unwrap_or(&tool_calls_record.id),
                    on_event,
                    usage_tracker,
                )
                .await?;
            messages.push(Message::User { content });
            message_ids.push(id);
            continue;
        }

        // is_protocol 已保证 handler 存在；协议批直接收 handler（免台账短路，
        // 见 batch::execute_tool_calls 的文档）。
        let protocol_handler = hooks
            .protocol_handler
            .as_ref()
            .expect("is_protocol 分支已保证协议处理器存在");
        let batch = execute_tool_calls(
            db,
            workspace_id,
            on_event,
            &tool_calls,
            &outbound_calls,
            surface,
            protocol_handler.as_ref(),
            summary,
            usage_tracker,
            &cancel_rx,
        )
        .await?;
        let Some(result_contents) = batch.contents else {
            // 工具间取消：已执行结果已逐个落库；按取消语义收口（无部分正文——
            // 本轮流式已完整结束）。
            return finalize_cancelled(db, workspace_id, on_event, hooks, usage_tracker, "", None)
                .await;
        };
        messages.push(Message::User {
            content: result_contents,
        });
        message_ids.push(batch.last_result_message_id);

        // 协议收口（编排器）：动作 > 可重试错误 > 最终答复——三者优先级对齐旧
        // `resolve_loop_outcome`：已登记的工作流绝不因同轮另有可重试错误被丢弃；
        // 有可重试错误则让模型先自修复，不收口。
        if let Some(handler) = hooks.protocol_handler.as_ref() {
            let closing = if !batch.actions.is_empty() {
                handler
                    .render_outcome(&batch.actions, batch.final_message.as_deref())
                    .await
            } else if batch.saw_retryable_error {
                None
            } else if batch.final_message.is_some() {
                handler
                    .render_outcome(&[], batch.final_message.as_deref())
                    .await
            } else {
                None
            };
            if let Some(text) = closing {
                let usage_stats = usage_tracker.snapshot();
                let reply =
                    persist_assistant_message(db, workspace_id, &text, &usage_stats).await?;
                emit(
                    on_event,
                    AgentEvent::AssistantMessage {
                        message: Box::new(reply.clone()),
                        // 工具循环后的合成收口消息，无关联的流式 delta 序号。
                        last_seq: None,
                    },
                );
                return Ok(reply);
            }
        }
    }

    anyhow::bail!(
        "{}",
        hooks.max_iterations_error.clone().unwrap_or_else(|| {
            format!(
                "已达到最大工具迭代次数（{}），本轮执行被终止。请检查模型是否陷入工具调用循环。",
                hooks.max_iterations
            )
        })
    )
}
