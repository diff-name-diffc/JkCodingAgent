use super::*;

pub(super) struct HostToolCallContext<'a> {
    pub(super) node: &'a NodeExecContext,
    pub(super) stdin: &'a Arc<Mutex<tokio::process::ChildStdin>>,
    pub(super) request_id: &'a str,
    pub(super) deadline: tokio::time::Instant,
}

pub(super) async fn handle_host_tool_call(
    execution: HostToolCallContext<'_>,
    data: &Value,
    host_sequence: &mut i64,
    cancel_rx: &mut watch::Receiver<bool>,
    tool_state: &mut AgentToolEventState,
) -> anyhow::Result<Option<NodeExecOutcome>> {
    let HostToolCallContext {
        node: ctx,
        stdin,
        request_id,
        deadline,
    } = execution;
    let call_id = string_field(data, "callId")?;
    let name = string_field(data, "name")?;
    let runtime_name = string_field(data, "runtimeName")?;
    let args = data.get("args").cloned().unwrap_or_else(|| json!({}));
    // fail-closed：能力名与 sidecar 运行时别名必须成对匹配。
    // 仅校验 capability name 会让被篡改的 sidecar 借任意别名
    // 调用本节点其他能力，破坏模型可见面与授权面的对应关系。
    if !host_tool_mapping_is_declared(&ctx.harness.host_tools, &runtime_name, &name) {
        let message =
            format!("错误：宿主工具映射 '{runtime_name}' -> '{name}' 未在本节点声明，拒绝执行");
        eprintln!(
            "[graph] PI sidecar 越权调用（节点 {}）：{message}",
            ctx.node.id
        );
        *host_sequence += 1;
        let response = json!({"type":"host_tool_result","requestId":request_id,"runId":ctx.run_id,"nodeId":ctx.node.id,"sequence":*host_sequence,"callId":call_id,"error":message});
        write_jsonl(stdin, &response).await?;
        return Ok(None);
    }
    collect_affected_file(
        &ctx.workspace_root,
        &name,
        &args,
        &mut tool_state.affected_files,
    );
    let capabilities =
        CapabilitySet::new(ctx.harness.host_tools.iter().map(|tool| tool.name.clone()))
            // expectedFiles 在提交/更新时已经过相对路径校验；这里把
            // 它升级为 Broker 的真实写授权，而不再只是并发冲突提示。
            // 空列表意味着该节点可运行命令/测试，但不能直接调用
            // write_file/edit_file 修改任意文件。
            .restrict_writes_to(ctx.node.expected_files.clone());
    // 节点 deadline 与外层取消都先由本循环裁决，再转发给
    // Broker。这样节点超时不仅停止等待，还会触发工具级
    // 进程终止/协作取消。
    let (tool_cancel_tx, tool_cancel_rx) = watch::channel(*cancel_rx.borrow());
    let broker = CapabilityBroker::new(&ctx.tool_registry, capabilities, &ctx.tool_context)
        .with_cancellation(tool_cancel_rx);
    let mut tool_future = std::pin::pin!(broker.invoke(CapabilityInvocation::model(
        call_id.clone(),
        name.clone(),
        args.clone()
    )));
    let interrupted = await_interruptible(tool_future.as_mut(), cancel_rx, deadline).await;
    let result = match interrupted {
        Interruptible::Completed(result) => result,
        interrupted => {
            let _ = tool_cancel_tx.send(true);
            // Broker 自带固定收敛上限；必须拿到它的终态，不能在
            // 外层再提前丢 Future，否则会重新制造后台副作用盲区。
            let settled = tool_future.await;
            if settled.metadata["termination"]["state"] == "termination_unknown" {
                eprintln!(
                    "[graph] 节点 '{}' 已中断，宿主工具 '{name}' 仍在途：后台副作用可能继续写入工作区",
                    ctx.node.id
                );
            }
            match interrupted {
                Interruptible::TimedOut => {
                    request_sidecar_cancel(stdin, request_id, ctx, host_sequence).await;
                    return Err(anyhow::anyhow!("节点执行超过 30 分钟，已终止"));
                }
                // Completed 已在上方匹配，此处仅剩 Cancelled。
                _ => {
                    request_sidecar_cancel(stdin, request_id, ctx, host_sequence).await;
                    return Ok(Some(NodeExecOutcome::Cancelled));
                }
            }
        }
    };
    *host_sequence += 1;
    let transport_output = bound_host_tool_result(result.output_for_llm());
    let response = if result.status == ToolStatus::Success {
        json!({"type":"host_tool_result","requestId":request_id,"runId":ctx.run_id,"nodeId":ctx.node.id,"sequence":*host_sequence,"callId":call_id,"result":transport_output})
    } else {
        json!({"type":"host_tool_result","requestId":request_id,"runId":ctx.run_id,"nodeId":ctx.node.id,"sequence":*host_sequence,"callId":call_id,"error":transport_output})
    };
    write_jsonl(stdin, &response).await?;
    Ok(None)
}

pub(super) fn bound_host_tool_result(mut output: String) -> String {
    if output.len() <= MAX_HOST_TOOL_RESULT_BYTES {
        return output;
    }
    let original_bytes = output.len();
    let marker = format!(
        "\n\n[宿主工具结果已按 Graph JSONL 传输预算截断：原始 {original_bytes} 字节；请缩小工具参数。]"
    );
    let mut boundary = MAX_HOST_TOOL_RESULT_BYTES.saturating_sub(marker.len());
    while !output.is_char_boundary(boundary) {
        boundary -= 1;
    }
    output.truncate(boundary);
    output.push_str(&marker);
    output
}

pub(super) fn host_tool_mapping_is_declared(
    tools: &[crate::agent::graph::harness::PiHostToolSpec],
    runtime_name: &str,
    capability_name: &str,
) -> bool {
    tools
        .iter()
        .any(|tool| tool.name == capability_name && tool.runtime_name == runtime_name)
}

pub(super) async fn cancellation_requested(cancel_rx: &mut watch::Receiver<bool>) {
    loop {
        if *cancel_rx.borrow() {
            return;
        }
        if cancel_rx.changed().await.is_err() {
            // 发送端被丢弃：按取消处理（fail-closed），不能永久 pending——
            // 否则节点只能靠 30 分钟超时兜底结算。
            return;
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Interruptible<T> {
    Completed(T),
    Cancelled,
    TimedOut,
}

/// 以可重入方式等待工具 future：调用方持有 pinned future，中断（取消/超时）
/// 返回后 future 仍由调用方交给 Broker 完成有界收敛。
pub(super) async fn await_interruptible<T, F>(
    future: std::pin::Pin<&mut F>,
    cancel_rx: &mut watch::Receiver<bool>,
    deadline: tokio::time::Instant,
) -> Interruptible<T>
where
    F: std::future::Future<Output = T> + ?Sized,
{
    tokio::select! {
        result = future => Interruptible::Completed(result),
        _ = tokio::time::sleep_until(deadline) => Interruptible::TimedOut,
        _ = cancellation_requested(cancel_rx) => Interruptible::Cancelled,
    }
}

pub(super) async fn request_sidecar_cancel(
    stdin: &Arc<Mutex<tokio::process::ChildStdin>>,
    request_id: &str,
    ctx: &NodeExecContext,
    host_sequence: &mut i64,
) {
    *host_sequence += 1;
    let _ = write_jsonl(
        stdin,
        &json!({
            "type": "cancel",
            "requestId": request_id,
            "runId": ctx.run_id,
            "nodeId": ctx.node.id,
            "sequence": *host_sequence,
        }),
    )
    .await;
}

pub(super) fn string_field(value: &Value, key: &str) -> anyhow::Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("PI 消息缺少 {key}"))
}
