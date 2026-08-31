//! 单节点 PI SDK RPC 执行器。每个节点拥有独立 sidecar 进程与内存会话。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::AppHandle;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::{watch, Mutex};

use super::harness::ResolvedNodeHarness;
use super::pi_rpc::{global_agent_dir, read_bounded_line, SidecarChildGuard, SidecarEnvelope};
use super::runner::emit_run_event;
use super::store::GraphStore;
use super::types::{
    AgentActivity, GraphNode, GraphRunEvent, NODE_PHASE_COMPACTING, NODE_PHASE_RESPONDING,
    NODE_PHASE_RETRYING, NODE_PHASE_THINKING, NODE_PHASE_TOOL_RUNNING,
};
mod activities;
mod host_tools;
mod protocol;

use activities::*;
use host_tools::*;
use protocol::*;

use crate::agent::tools::{
    CapabilityBroker, CapabilityInvocation, CapabilitySet, ToolContext, ToolRegistry, ToolStatus,
};

const NODE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const SIDECAR_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const PROTOCOL_VERSION: i64 = 3;
/// host_tool_result 与 sidecar 的 JSONL 控制协议共用 stdout。单条工具结果必须
/// 明显低于 1 MiB 协议读上限，给 JSON 转义和 envelope 字段留出余量。
const MAX_HOST_TOOL_RESULT_BYTES: usize = 256 * 1024;

/// 节点被取消/超时后，对在途宿主工具的有界收尾等待：工具内部的
/// spawn_blocking（文件写、shell）不会随 future 被丢弃而停止，短暂等待其
/// 收敛；不收敛则记录日志后再走取消/超时路径（fail-closed 可见性）。
//
/// 固定文本活动的序号约定：assistant_text=1、thinking=2（见 TextActivity::new），
/// context_usage 顺延其后。在 1/2 之外新增固定活动时需同步更新这里
/// （动态活动的序号为协议序号 ×10，见 activity_sequence，不会与之冲突）。
const CONTEXT_USAGE_SEQUENCE: i64 = 3;

#[derive(Debug)]
pub(crate) enum NodeExecOutcome {
    Succeeded {
        output: String,
        affected_files: Vec<String>,
        tool_call_count: i64,
        usage_json: String,
    },
    /// 失败结算携带已消耗的 usage（sidecar 若在 failed 消息中上报则透传；
    /// 当前协议版本 failed 不含 usage 时为 "{}"）。记录层不得丢弃已发生的
    /// token 消耗——尤其重试场景的首次尝试。
    Failed {
        error: String,
        usage_json: String,
    },
    Cancelled,
}

pub(crate) struct NodeExecContext {
    pub app: AppHandle,
    pub plan_id: String,
    pub run_id: String,
    pub workspace_id: String,
    pub workspace_root: PathBuf,
    pub node: GraphNode,
    pub input: String,
    pub harness: ResolvedNodeHarness,
    pub tool_registry: Arc<ToolRegistry>,
    pub tool_context: ToolContext,
    pub store: GraphStore,
    pub cancel_rx: watch::Receiver<bool>,
}

pub(crate) async fn execute_node(ctx: &NodeExecContext) -> NodeExecOutcome {
    match execute_pi_node(ctx).await {
        Ok(outcome) => outcome,
        // 真正的取消已在内部显式返回 Ok(NodeExecOutcome::Cancelled)；此处的 Err
        // 都是真实故障（协议错误、工具失败、超时等）。恰逢用户取消时仅附注说明，
        // 不把失败伪装成取消——否则前端只看到「已取消」，丢失排障信息。
        // 这些路径拿不到 sidecar 的 usage 上报，usage_json 留空。
        Err(error) => {
            let cancel_note = if *ctx.cancel_rx.borrow() {
                "（执行期间同时收到取消请求）"
            } else {
                ""
            };
            NodeExecOutcome::Failed {
                error: format!("PI Agent 执行失败：{error:#}{cancel_note}"),
                usage_json: "{}".into(),
            }
        }
    }
}

async fn execute_pi_node(ctx: &NodeExecContext) -> anyhow::Result<NodeExecOutcome> {
    let mut guard = SidecarChildGuard::spawn()?;
    let execution = execute_pi_node_process(ctx, guard.child_mut()).await;
    // spawn 成功后只有这一条正常退出路径：显式终止并回收进程组。panic 展开或
    // 任务 abort 等绕过此处的路径由守卫 Drop 兜底杀进程组（重复组杀无害）。
    guard.terminate().await;
    execution.assistant.finish(ctx).await;
    execution.thinking.finish(ctx).await;
    execution.outcome
}

struct PiNodeExecution {
    outcome: anyhow::Result<NodeExecOutcome>,
    assistant: TextActivity,
    thinking: TextActivity,
}

async fn execute_pi_node_process(
    ctx: &NodeExecContext,
    child: &mut tokio::process::Child,
) -> PiNodeExecution {
    let mut assistant = TextActivity::new(ctx, "assistant_text", "Agent 响应");
    let mut thinking = TextActivity::new(ctx, "thinking", "思考");
    let outcome = async {
        let stdin = Arc::new(Mutex::new(
            child
                .stdin
                .take()
                .ok_or_else(|| anyhow::anyhow!("PI sidecar stdin 未捕获"))?,
        ));
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("PI sidecar stdout 未捕获"))?;
        let request_id = uuid::Uuid::new_v4().to_string();

        // 先验证 sidecar 版本，再发送包含 API Key 与工作区信息的 start。
        // 这不仅是兼容性检查，也是安全边界：旧 sidecar 会直接加载项目扩展，
        // 若先写 start，即使随后发现版本不匹配，也已给了它执行工作区代码的窗口。
        let mut reader = BufReader::new(stdout);
        let ready_line = tokio::time::timeout(SIDECAR_HANDSHAKE_TIMEOUT, read_bounded_line(&mut reader))
            .await
            .map_err(|_| anyhow::anyhow!("等待 PI sidecar ready 握手超时"))??
            .ok_or_else(|| anyhow::anyhow!("PI sidecar 在 ready 握手前退出"))?;
        let ready = parse_sidecar_envelope(
            &ready_line,
            &request_id,
            &ctx.run_id,
            &ctx.node.id,
            0,
        )?;
        let mut ready_seen = false;
        enforce_handshake_order(&ready.r#type, &mut ready_seen)?;
        if ready.r#type != "ready" {
            return Err(anyhow::anyhow!("PI sidecar 首条消息不是 ready 握手"));
        }

        let start = json!({
            "type": "start",
            "requestId": request_id,
            "runId": ctx.run_id,
            "nodeId": ctx.node.id,
            "sequence": 1,
            "workspace": ctx.workspace_root,
            "agentDir": global_agent_dir()?,
            "projectResourceDir": ctx.workspace_root.join(".jkcodingagent/pi-agent"),
            "prompt": ctx.input,
            // 明文密钥仅经 sidecar_value 这一出口进入 start 消息（见 PiModelConfig）。
            "model": ctx.harness.model.sidecar_value(),
            "baseToolGroup": ctx.node.base_tool_group,
            "specialTools": ctx.node.special_tools,
            "hostTools": ctx.harness.host_tools,
        });
        write_jsonl(&stdin, &start).await?;

        let mut cancel_rx = ctx.cancel_rx.clone();
        let mut tool_state = AgentToolEventState::default();
        let mut last_sequence = ready.sequence;
        let mut host_sequence = 1_i64;
        let deadline = tokio::time::Instant::now() + NODE_TIMEOUT;

        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {
                    request_sidecar_cancel(&stdin, &request_id, ctx, &mut host_sequence).await;
                    return Err(anyhow::anyhow!("节点执行超过 30 分钟，已终止"));
                }
                _ = cancellation_requested(&mut cancel_rx) => {
                    request_sidecar_cancel(&stdin, &request_id, ctx, &mut host_sequence).await;
                    return Ok(NodeExecOutcome::Cancelled);
                }
                line = read_bounded_line(&mut reader) => {
                    let Some(line) = line? else {
                        return Err(anyhow::anyhow!("PI sidecar 在完成事件前退出"));
                    };
                    let envelope = parse_sidecar_envelope(
                        &line,
                        &request_id,
                        &ctx.run_id,
                        &ctx.node.id,
                        last_sequence,
                    )?;
                    enforce_handshake_order(&envelope.r#type, &mut ready_seen)?;
                    if envelope.r#type == "ready" {
                        continue;
                    }
                    last_sequence = envelope.sequence;
                    match envelope.r#type.as_str() {
                        "completed" => {
                            let output = envelope.data.get("output").and_then(Value::as_str).unwrap_or(&assistant.content).to_string();
                            let usage = envelope.data.get("usage").cloned().unwrap_or_else(|| json!({}));
                            let usage_activity = AgentActivity {
                                id: format!("{}:{}:usage", ctx.run_id, ctx.node.id),
                                run_id: ctx.run_id.clone(),
                                node_id: ctx.node.id.clone(),
                                sequence: 1_000_000_000,
                                kind: "usage".into(),
                                status: "finished".into(),
                                title: "Token 用量".into(),
                                content: usage.to_string(),
                                payload_json: usage.to_string(),
                                started_at: chrono::Utc::now().timestamp_millis(),
                                finished_at: Some(chrono::Utc::now().timestamp_millis()),
                            };
                            let _ = ctx.store.save_activity_async(&usage_activity).await;
                            emit_run_event(&ctx.app, &ctx.plan_id, &ctx.run_id, &ctx.workspace_id, GraphRunEvent::NodeActivity { node_id: ctx.node.id.clone(), activity: usage_activity });
                            let mut files = tool_state.affected_files.into_iter().collect::<Vec<_>>(); files.sort();
                            return Ok(NodeExecOutcome::Succeeded { output, affected_files: files, tool_call_count: tool_state.call_count, usage_json: usage.to_string() });
                        }
                        "failed" => {
                            let message = envelope.data.get("error").and_then(Value::as_str).unwrap_or("PI sidecar 未知错误");
                            // sidecar 若在 failed 中上报 usage 则透传落库（当前
                            // 协议版本 failed 仅含 error，此时为 "{}"）；失败前
                            // 的 LLM 调用已消耗 token，记录层不得丢弃。
                            let usage = envelope.data.get("usage").cloned().unwrap_or_else(|| json!({}));
                            return Ok(NodeExecOutcome::Failed {
                                error: message.to_string(),
                                usage_json: usage.to_string(),
                            });
                        }
                        "host_tool_call" => {
                            if let Some(outcome) = handle_host_tool_call(
                                HostToolCallContext {
                                    node: ctx,
                                    stdin: &stdin,
                                    request_id: &request_id,
                                    deadline,
                                },
                                &envelope.data,
                                &mut host_sequence,
                                &mut cancel_rx,
                                &mut tool_state,
                            )
                            .await?
                            {
                                return Ok(outcome);
                            }
                        }
                        "agent_event" => {
                            handle_agent_event(ctx, envelope.sequence, &envelope.data, &mut assistant, &mut thinking, &mut tool_state).await?;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    .await;
    PiNodeExecution {
        outcome,
        assistant,
        thinking,
    }
}

#[cfg(test)]
mod tests;
