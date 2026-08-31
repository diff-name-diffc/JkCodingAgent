use super::*;

impl CapabilityBroker<'_> {
    pub(super) async fn await_without_timeout<F>(
        &self,
        invocation: &CapabilityInvocation,
        cancellable: bool,
        execution_cancel_tx: &watch::Sender<bool>,
        mut execution: std::pin::Pin<&mut F>,
    ) -> ToolResult
    where
        F: std::future::Future<Output = ToolResult>,
    {
        let Some(mut cancel_rx) = self.cancel_rx.clone().filter(|_| cancellable) else {
            return execution.await;
        };
        if *cancel_rx.borrow() {
            return cancelled_before_start(invocation);
        }

        tokio::select! {
            result = execution.as_mut() => result,
            changed = cancel_rx.changed() => {
                match changed {
                    Ok(()) if *cancel_rx.borrow() => {
                        cancel_and_settle(invocation, execution_cancel_tx, execution.as_mut()).await
                    }
                    Ok(()) => execution.await,
                    Err(_) => {
                        cancel_and_settle(invocation, execution_cancel_tx, execution.as_mut()).await
                    }
                }
            }
        }
    }

    pub(super) async fn await_with_timeout<F>(
        &self,
        invocation: &CapabilityInvocation,
        cancellable: bool,
        timeout: Duration,
        execution_cancel_tx: &watch::Sender<bool>,
        mut execution: std::pin::Pin<&mut F>,
    ) -> ToolResult
    where
        F: std::future::Future<Output = ToolResult>,
    {
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);

        let Some(mut cancel_rx) = self.cancel_rx.clone().filter(|_| cancellable) else {
            return tokio::select! {
                biased;
                result = execution.as_mut() => result,
                _ = &mut deadline => cancel_and_settle_timeout(
                    invocation,
                    timeout,
                    execution_cancel_tx,
                    execution.as_mut(),
                ).await,
            };
        };
        if *cancel_rx.borrow() {
            return cancelled_before_start(invocation);
        }

        tokio::select! {
            biased;
            result = execution.as_mut() => result,
            _ = &mut deadline => cancel_and_settle_timeout(
                invocation,
                timeout,
                execution_cancel_tx,
                execution.as_mut(),
            ).await,
            changed = cancel_rx.changed() => {
                match changed {
                    Ok(()) if *cancel_rx.borrow() => {
                        cancel_and_settle(invocation, execution_cancel_tx, execution.as_mut()).await
                    }
                    Ok(()) => tokio::select! {
                        biased;
                        result = execution.as_mut() => result,
                        _ = &mut deadline => cancel_and_settle_timeout(
                            invocation,
                            timeout,
                            execution_cancel_tx,
                            execution.as_mut(),
                        ).await,
                    },
                    Err(_) => {
                        cancel_and_settle(invocation, execution_cancel_tx, execution.as_mut()).await
                    }
                }
            }
        }
    }
}

async fn cancel_and_settle<F>(
    invocation: &CapabilityInvocation,
    execution_cancel_tx: &watch::Sender<bool>,
    mut execution: std::pin::Pin<&mut F>,
) -> ToolResult
where
    F: std::future::Future<Output = ToolResult>,
{
    let _ = execution_cancel_tx.send(true);
    match tokio::time::timeout(CANCELLATION_SETTLE_GRACE, execution.as_mut()).await {
        Ok(result) => settled_after_cancellation(result),
        Err(_) => cancellation_unsettled(invocation),
    }
}

async fn cancel_and_settle_timeout<F>(
    invocation: &CapabilityInvocation,
    timeout: Duration,
    execution_cancel_tx: &watch::Sender<bool>,
    mut execution: std::pin::Pin<&mut F>,
) -> ToolResult
where
    F: std::future::Future<Output = ToolResult>,
{
    let _ = execution_cancel_tx.send(true);
    match tokio::time::timeout(CANCELLATION_SETTLE_GRACE, execution.as_mut()).await {
        Ok(result) => settled_after_timeout(invocation, timeout, result),
        Err(_) => timeout_result(invocation, timeout),
    }
}

fn timeout_result(invocation: &CapabilityInvocation, timeout: Duration) -> ToolResult {
    with_termination_metadata(
        ToolResult::recoverable_error(format!(
            "错误：工具 '{}' 执行超过统一策略超时 {} 秒，已中止等待；底层阻塞任务可能仍在运行。",
            invocation.name,
            timeout.as_secs()
        )),
        "termination_unknown",
        true,
    )
}

fn settled_after_timeout(
    invocation: &CapabilityInvocation,
    timeout: Duration,
    result: ToolResult,
) -> ToolResult {
    let original_status = result.status.as_run_status();
    let termination_state = if result.status == super::super::ToolStatus::Cancelled {
        "terminated"
    } else {
        "completed_after_deadline"
    };
    let message = format!(
        "错误：工具 '{}' 执行超过统一策略超时 {} 秒；取消请求后底层已收敛，原始状态为 {}。",
        invocation.name,
        timeout.as_secs(),
        original_status
    );
    let timeout_result = if result.status == super::super::ToolStatus::FatalError {
        ToolResult::fatal_error(message)
    } else {
        ToolResult::recoverable_error(message)
    };
    let mut timeout_result = with_termination_metadata(timeout_result, termination_state, true);
    if let Value::Object(metadata) = &mut timeout_result.metadata {
        metadata.insert(
            "deadline".to_string(),
            json!({
                "timeoutSeconds": timeout.as_secs(),
                "originalStatus": original_status,
            }),
        );
    }
    timeout_result
}

pub(super) fn cancelled_before_start(invocation: &CapabilityInvocation) -> ToolResult {
    with_termination_metadata(
        ToolResult::cancelled(format!("工具 '{}' 在开始执行前已取消。", invocation.name)),
        "not_started",
        true,
    )
}

fn cancellation_unsettled(invocation: &CapabilityInvocation) -> ToolResult {
    with_termination_metadata(
        ToolResult::cancelled(format!(
            "错误：工具 '{}' 的等待已取消；底层阻塞任务可能仍在运行。",
            invocation.name
        )),
        "termination_unknown",
        true,
    )
}

fn settled_after_cancellation(result: ToolResult) -> ToolResult {
    let state = if result.status == super::super::ToolStatus::Cancelled {
        "terminated"
    } else {
        // 调用在取消请求后自行完成；保留真实成功/失败状态，避免把已经发生的
        // 副作用伪装成“已取消”。外层 run loop 仍会在本调用落库后停止。
        "completed_after_cancel_request"
    };
    with_termination_metadata(result, state, true)
}

fn with_termination_metadata(
    mut result: ToolResult,
    termination_state: &str,
    cancel_requested: bool,
) -> ToolResult {
    let termination = json!({
        "cancelRequested": cancel_requested,
        "state": termination_state,
        "confirmed": termination_state != "termination_unknown",
    });
    match &mut result.metadata {
        Value::Object(metadata) => {
            metadata.insert("termination".to_string(), termination);
        }
        Value::Null => result.metadata = json!({ "termination": termination }),
        other => {
            result.metadata = json!({
                "value": mem::take(other),
                "termination": termination,
            });
        }
    }
    result
}

pub(super) fn with_broker_metadata(
    mut result: ToolResult,
    invocation: &CapabilityInvocation,
) -> ToolResult {
    let broker = json!({
        "invocationId": invocation.id,
        "origin": invocation.origin.as_str(),
        "stepId": invocation.step_id,
        "sequence": invocation.sequence,
    });
    match &mut result.metadata {
        Value::Object(metadata) => {
            metadata.insert("broker".to_string(), broker);
        }
        Value::Null => result.metadata = json!({ "broker": broker }),
        other => {
            result.metadata = json!({
                "value": std::mem::take(other),
                "broker": broker,
            });
        }
    }
    result
}

pub(super) fn with_policy_metadata(mut result: ToolResult, policy: Value) -> ToolResult {
    match &mut result.metadata {
        Value::Object(metadata) => {
            metadata.insert("policyDecision".to_string(), policy);
        }
        Value::Null => result.metadata = json!({ "policyDecision": policy }),
        other => {
            result.metadata = json!({
                "value": mem::take(other),
                "policyDecision": policy,
            });
        }
    }
    result
}

pub(super) fn enforce_tool_program_result_budget(
    mut result: ToolResult,
    invocation: &CapabilityInvocation,
    raw_audited: bool,
) -> ToolResult {
    if result.status != super::super::ToolStatus::Success {
        return result;
    }
    let data_bytes = result
        .data
        .as_ref()
        .map(|data| serialized_json_size(data).unwrap_or(usize::MAX))
        .unwrap_or(0);
    let output_bytes = serialized_json_size(result.output_for_llm_ref()).unwrap_or(usize::MAX);
    let metadata_bytes = serialized_json_size(&result.metadata).unwrap_or(usize::MAX);
    let total_bytes = data_bytes
        .saturating_add(output_bytes)
        .saturating_add(metadata_bytes)
        .saturating_add(128);
    if total_bytes <= MAX_TOOL_PROGRAM_RESULT_BYTES {
        return result;
    }

    let artifacts = mem::take(&mut result.artifacts);
    let audit_note = if raw_audited {
        "完整原文已保存为该子调用的审计产物；"
    } else {
        ""
    };
    let mut error = ToolResult::recoverable_error(format!(
        "错误：ToolProgram 步骤 '{}' 调用工具 '{}' 的结构化结果约 {} 字节，超过单步 {} 字节预算；{audit_note}请缩小 paths、limit、max_results 或搜索范围后重试。",
        invocation.step_id.as_deref().unwrap_or("<unknown>"),
        invocation.name,
        total_bytes,
        MAX_TOOL_PROGRAM_RESULT_BYTES,
    ));
    error.artifacts = artifacts;
    error.metadata = json!({
        "broker": {
            "invocationId": invocation.id,
            "origin": invocation.origin.as_str(),
            "stepId": invocation.step_id,
            "sequence": invocation.sequence,
        },
        "resultBudget": {
            "exceeded": true,
            "estimatedBytes": total_bytes,
            "maxBytes": MAX_TOOL_PROGRAM_RESULT_BYTES,
            "dataBytes": data_bytes,
            "outputBytes": output_bytes,
            "metadataBytes": metadata_bytes,
        }
    });
    error
}

fn serialized_json_size<T: Serialize + ?Sized>(value: &T) -> io::Result<usize> {
    #[derive(Default)]
    struct Counter(usize);
    impl io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut counter = Counter::default();
    serde_json::to_writer(&mut counter, value).map_err(io::Error::other)?;
    Ok(counter.0)
}
