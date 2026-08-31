use super::*;

pub(super) fn parse_sidecar_envelope(
    line: &str,
    request_id: &str,
    run_id: &str,
    node_id: &str,
    last_sequence: i64,
) -> anyhow::Result<SidecarEnvelope> {
    let envelope: SidecarEnvelope = serde_json::from_str(line)
        .map_err(|error| anyhow::anyhow!("PI sidecar 输出非法 JSONL：{error}"))?;
    if envelope.sequence <= last_sequence {
        return Err(anyhow::anyhow!(
            "PI sidecar sequence 非单调递增：{} <= {last_sequence}",
            envelope.sequence
        ));
    }
    if envelope.r#type == "ready" {
        let version = envelope
            .data
            .get("protocolVersion")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        if version != PROTOCOL_VERSION {
            return Err(anyhow::anyhow!(
                "PI sidecar 协议版本不匹配：期望 {PROTOCOL_VERSION}，实际 {version}"
            ));
        }
        return Ok(envelope);
    }
    if envelope.request_id != request_id {
        return Err(anyhow::anyhow!("PI sidecar 返回了未知 requestId"));
    }
    if envelope.run_id.as_deref() != Some(run_id) || envelope.node_id.as_deref() != Some(node_id) {
        return Err(anyhow::anyhow!(
            "PI sidecar 返回的 runId/nodeId 与当前节点不匹配"
        ));
    }
    Ok(envelope)
}

/// 协议握手门控：sidecar 必须先发 ready（协议版本在 parse_sidecar_envelope
/// 内校验）才允许处理业务消息；重复 ready 同样拒绝。ready 之外的任何消息
/// 先于 ready 到达时，即使 requestId 匹配也必须拒绝——否则被替换或不兼容
/// 的 sidecar 可绕过版本校验完成节点。
pub(super) fn enforce_handshake_order(
    message_type: &str,
    ready_seen: &mut bool,
) -> anyhow::Result<()> {
    if message_type == "ready" {
        if *ready_seen {
            return Err(anyhow::anyhow!("PI sidecar 重复发送 ready 握手"));
        }
        *ready_seen = true;
        return Ok(());
    }
    if !*ready_seen {
        return Err(anyhow::anyhow!(
            "PI sidecar 在 ready 握手前发送业务消息（{message_type}），协议版本未校验"
        ));
    }
    Ok(())
}

pub(super) async fn write_jsonl(
    stdin: &Arc<Mutex<tokio::process::ChildStdin>>,
    value: &Value,
) -> anyhow::Result<()> {
    let mut writer = stdin.lock().await;
    writer.write_all(format!("{}\n", value).as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}
