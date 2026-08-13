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
use crate::agent::tools::{ToolContext, ToolRegistry, ToolStatus};

const NODE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const PROTOCOL_VERSION: i64 = 2;

/// 节点被取消/超时后，对在途宿主工具的有界收尾等待：工具内部的
/// spawn_blocking（文件写、shell）不会随 future 被丢弃而停止，短暂等待其
/// 收敛；不收敛则记录日志后再走取消/超时路径（fail-closed 可见性）。
const IN_FLIGHT_TOOL_GRACE: Duration = Duration::from_secs(3);

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

        let mut reader = BufReader::new(stdout);
        let mut cancel_rx = ctx.cancel_rx.clone();
        let mut tool_activities: HashMap<String, AgentActivity> = HashMap::new();
        let mut affected_files = HashSet::new();
        let mut tool_call_count = 0_i64;
        let mut last_sequence = 0_i64;
        let mut host_sequence = 1_i64;
        // 协议握手门控位：首条消息必须是 ready（协议版本在其内校验），
        // 之后才允许处理业务消息（见 enforce_handshake_order）。
        let mut ready_seen = false;
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
                            let mut files = affected_files.into_iter().collect::<Vec<_>>(); files.sort();
                            return Ok(NodeExecOutcome::Succeeded { output, affected_files: files, tool_call_count, usage_json: usage.to_string() });
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
                            let call_id = string_field(&envelope.data, "callId")?;
                            let name = string_field(&envelope.data, "name")?;
                            let args = envelope.data.get("args").cloned().unwrap_or_else(|| json!({}));
                            // fail-closed：只执行本节点显式声明的宿主工具。sidecar
                            // 按名回调，未校验时可调用任意注册表工具（含被目录排除的
                            // write_file/exec 等），绕过 harness 工具选择门禁。
                            if !ctx.harness.host_tools.iter().any(|tool| tool.name == name) {
                                let message = format!("错误：宿主工具 '{name}' 未在本节点声明，拒绝执行");
                                eprintln!("[graph] PI sidecar 越权调用（节点 {}）：{message}", ctx.node.id);
                                host_sequence += 1;
                                let response = json!({"type":"host_tool_result","requestId":request_id,"runId":ctx.run_id,"nodeId":ctx.node.id,"sequence":host_sequence,"callId":call_id,"error":message});
                                write_jsonl(&stdin, &response).await?;
                                continue;
                            }
                            collect_affected_file(&ctx.workspace_root, &name, &args, &mut affected_files);
                            let mut tool_future =
                                std::pin::pin!(ctx.tool_registry.execute(&name, &args, &ctx.tool_context));
                            let interrupted = await_interruptible(
                                tool_future.as_mut(),
                                &mut cancel_rx,
                                deadline,
                            ).await;
                            let result = match interrupted {
                                Interruptible::Completed(result) => result,
                                interrupted => {
                                    // 工具 future 仍在途：其内部 spawn_blocking（文件写、
                                    // shell 等）不会随 future 丢弃立即停止，节点已取消/超时
                                    // 后工作区可能继续被修改。有界等待其收敛；不收敛则记录
                                    // 日志明示「后台副作用可能仍在进行」，再进入取消/超时收尾。
                                    if tokio::time::timeout(IN_FLIGHT_TOOL_GRACE, tool_future).await.is_err() {
                                        eprintln!(
                                            "[graph] 节点 '{}' 已中断，宿主工具 '{name}' 仍在途：后台副作用可能继续写入工作区",
                                            ctx.node.id
                                        );
                                    }
                                    match interrupted {
                                        Interruptible::TimedOut => {
                                            request_sidecar_cancel(&stdin, &request_id, ctx, &mut host_sequence).await;
                                            return Err(anyhow::anyhow!("节点执行超过 30 分钟，已终止"));
                                        }
                                        // Completed 已在上方匹配，此处仅剩 Cancelled。
                                        _ => {
                                            request_sidecar_cancel(&stdin, &request_id, ctx, &mut host_sequence).await;
                                            return Ok(NodeExecOutcome::Cancelled);
                                        }
                                    }
                                }
                            };
                            host_sequence += 1;
                            let response = if result.status == ToolStatus::Success {
                                json!({"type":"host_tool_result","requestId":request_id,"runId":ctx.run_id,"nodeId":ctx.node.id,"sequence":host_sequence,"callId":call_id,"result":result.output_for_llm()})
                            } else {
                                json!({"type":"host_tool_result","requestId":request_id,"runId":ctx.run_id,"nodeId":ctx.node.id,"sequence":host_sequence,"callId":call_id,"error":result.output_for_llm()})
                            };
                            write_jsonl(&stdin, &response).await?;
                        }
                        "agent_event" => {
                            handle_agent_event(ctx, envelope.sequence, &envelope.data, &mut assistant, &mut thinking, &mut tool_activities, &mut affected_files, &mut tool_call_count).await?;
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

async fn cancellation_requested(cancel_rx: &mut watch::Receiver<bool>) {
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
enum Interruptible<T> {
    Completed(T),
    Cancelled,
    TimedOut,
}

/// 以可重入方式等待工具 future：调用方持有 pinned future，中断（取消/超时）
/// 返回后 future 仍可用于有界收尾等待（见 IN_FLIGHT_TOOL_GRACE 相关逻辑）。
async fn await_interruptible<T, F>(
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

async fn request_sidecar_cancel(
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

fn parse_sidecar_envelope(
    line: &str,
    request_id: &str,
    run_id: &str,
    node_id: &str,
    last_sequence: i64,
) -> anyhow::Result<SidecarEnvelope> {
    let envelope: SidecarEnvelope = serde_json::from_str(line)
        .map_err(|error| anyhow::anyhow!("PI sidecar 输出非法 JSONL：{error}"))?;
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
    if envelope.sequence <= last_sequence {
        return Err(anyhow::anyhow!(
            "PI sidecar sequence 非单调递增：{} <= {last_sequence}",
            envelope.sequence
        ));
    }
    Ok(envelope)
}

/// 协议握手门控：sidecar 必须先发 ready（协议版本在 parse_sidecar_envelope
/// 内校验）才允许处理业务消息；重复 ready 同样拒绝。ready 之外的任何消息
/// 先于 ready 到达时，即使 requestId 匹配也必须拒绝——否则被替换或不兼容
/// 的 sidecar 可绕过版本校验完成节点。
fn enforce_handshake_order(message_type: &str, ready_seen: &mut bool) -> anyhow::Result<()> {
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

async fn handle_agent_event(
    ctx: &NodeExecContext,
    protocol_sequence: i64,
    data: &Value,
    assistant: &mut TextActivity,
    thinking: &mut TextActivity,
    tools: &mut HashMap<String, AgentActivity>,
    affected: &mut HashSet<String>,
    tool_call_count: &mut i64,
) -> anyhow::Result<()> {
    let activity_sequence = activity_sequence(protocol_sequence)?;
    match data.get("kind").and_then(Value::as_str).unwrap_or_default() {
        "assistant_text" => {
            let delta = data
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assistant.push(ctx, delta).await;
            emit_run_event(
                &ctx.app,
                &ctx.plan_id,
                &ctx.run_id,
                &ctx.workspace_id,
                GraphRunEvent::NodeOutputDelta {
                    node_id: ctx.node.id.clone(),
                    delta: delta.to_string(),
                },
            );
            emit_phase(ctx, NODE_PHASE_RESPONDING);
        }
        "thinking" => {
            thinking
                .push(
                    ctx,
                    data.get("delta")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
                .await;
            emit_phase(ctx, NODE_PHASE_THINKING);
        }
        "tool_call" => {
            emit_phase(ctx, NODE_PHASE_TOOL_RUNNING);
            let call_id = data
                .get("callId")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let status = data
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("updated")
                .to_string();
            let name = data
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            if status == "started" {
                *tool_call_count += 1;
                if let Some(args) = data.get("args") {
                    collect_affected_file(&ctx.workspace_root, &name, args, affected);
                }
            }
            let sequence = tools
                .get(&call_id)
                .map(|a| a.sequence)
                .unwrap_or(activity_sequence);
            let now = chrono::Utc::now().timestamp_millis();
            let activity = AgentActivity {
                id: format!("{}:{}:tool:{call_id}", ctx.run_id, ctx.node.id),
                run_id: ctx.run_id.clone(),
                node_id: ctx.node.id.clone(),
                sequence,
                kind: "tool_call".into(),
                status: status.clone(),
                title: name,
                content: data.get("result").map(value_text).unwrap_or_default(),
                payload_json: redact(data.clone()).to_string(),
                started_at: tools.get(&call_id).map(|a| a.started_at).unwrap_or(now),
                finished_at: matches!(status.as_str(), "finished" | "failed").then_some(now),
            };
            let _ = ctx.store.save_activity_async(&activity).await;
            emit_run_event(
                &ctx.app,
                &ctx.plan_id,
                &ctx.run_id,
                &ctx.workspace_id,
                GraphRunEvent::NodeActivity {
                    node_id: ctx.node.id.clone(),
                    activity: activity.clone(),
                },
            );
            tools.insert(call_id, activity);
        }
        "context_usage" => {
            // 上下文占用估算：稳定 id upsert（每秒级采样只留一条活动记录），
            // 原始数值留在 payload 供前端解析，content 为人类可读摘要。
            // upsert 的 ON CONFLICT 子句不更新 started_at，保留首次观测时间；
            // 前端按 sequence 展示与排序，不依赖该时间随采样刷新。
            let now = chrono::Utc::now().timestamp_millis();
            let activity = AgentActivity {
                id: format!("{}:{}:context_usage", ctx.run_id, ctx.node.id),
                run_id: ctx.run_id.clone(),
                node_id: ctx.node.id.clone(),
                sequence: CONTEXT_USAGE_SEQUENCE,
                kind: "context_usage".into(),
                status: "finished".into(),
                title: "上下文占用".into(),
                content: context_usage_content(data),
                payload_json: redact(data.clone()).to_string(),
                started_at: now,
                finished_at: Some(now),
            };
            let _ = ctx.store.save_activity_async(&activity).await;
            emit_run_event(
                &ctx.app,
                &ctx.plan_id,
                &ctx.run_id,
                &ctx.workspace_id,
                GraphRunEvent::NodeActivity {
                    node_id: ctx.node.id.clone(),
                    activity,
                },
            );
        }
        "retry" | "compaction" => {
            let kind = data
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("activity");
            emit_phase(
                ctx,
                if kind == "retry" {
                    NODE_PHASE_RETRYING
                } else {
                    NODE_PHASE_COMPACTING
                },
            );
            let activity = activity_from_event(ctx, data, activity_sequence);
            let _ = ctx.store.save_activity_async(&activity).await;
            emit_run_event(
                &ctx.app,
                &ctx.plan_id,
                &ctx.run_id,
                &ctx.workspace_id,
                GraphRunEvent::NodeActivity {
                    node_id: ctx.node.id.clone(),
                    activity,
                },
            );
        }
        "lifecycle" => {
            if let Some(phase) = data.get("phase").and_then(Value::as_str) {
                emit_phase(ctx, phase)
            }
        }
        _ => {}
    }
    Ok(())
}

fn activity_sequence(protocol_sequence: i64) -> anyhow::Result<i64> {
    protocol_sequence.checked_mul(10).ok_or_else(|| {
        anyhow::anyhow!("PI sidecar sequence 超出活动序号可表示范围：{protocol_sequence}")
    })
}

/// context_usage 事件的人类可读摘要；原始数值在 payload_json 中供前端解析。
fn context_usage_content(data: &Value) -> String {
    let percent = data.get("percent").and_then(Value::as_f64);
    let tokens = data.get("tokens").and_then(Value::as_i64);
    let window = data
        .get("contextWindow")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    match (percent, tokens) {
        (Some(percent), Some(tokens)) if window > 0 => {
            format!("估算占用 {percent:.1}%（{tokens}/{window} tokens）")
        }
        // contextWindow 缺失或为 0 时省略分母，避免渲染「N/0 tokens」误导文案。
        (Some(percent), Some(tokens)) => format!("估算占用 {percent:.1}%（{tokens} tokens）"),
        // compaction 后、下一次 LLM 响应前 SDK 无法确知上下文体积（tokens=null）。
        _ => "上下文压缩后重新估算中…".to_string(),
    }
}

struct TextActivity {
    id: String,
    kind: String,
    title: String,
    content: String,
    sequence: i64,
    started_at: i64,
    last_flush: tokio::time::Instant,
}
impl TextActivity {
    fn new(ctx: &NodeExecContext, kind: &str, title: &str) -> Self {
        Self {
            id: format!("{}:{}:{kind}", ctx.run_id, ctx.node.id),
            kind: kind.into(),
            title: title.into(),
            content: String::new(),
            sequence: if kind == "assistant_text" { 1 } else { 2 },
            started_at: chrono::Utc::now().timestamp_millis(),
            last_flush: tokio::time::Instant::now(),
        }
    }
    async fn push(&mut self, ctx: &NodeExecContext, delta: &str) {
        self.content.push_str(delta);
        if self.last_flush.elapsed() >= Duration::from_millis(250) {
            self.flush(ctx).await;
            self.last_flush = tokio::time::Instant::now();
        }
    }
    async fn flush(&self, ctx: &NodeExecContext) {
        self.persist(ctx, "streaming", None).await;
    }
    async fn finish(&self, ctx: &NodeExecContext) {
        self.persist(ctx, "finished", Some(chrono::Utc::now().timestamp_millis()))
            .await;
    }
    async fn persist(&self, ctx: &NodeExecContext, status: &str, finished_at: Option<i64>) {
        if self.content.is_empty() {
            return;
        }
        let activity = AgentActivity {
            id: self.id.clone(),
            run_id: ctx.run_id.clone(),
            node_id: ctx.node.id.clone(),
            sequence: self.sequence,
            kind: self.kind.clone(),
            status: status.into(),
            title: self.title.clone(),
            content: self.content.clone(),
            payload_json: "{}".into(),
            started_at: self.started_at,
            finished_at,
        };
        let _ = ctx.store.save_activity_async(&activity).await;
        emit_run_event(
            &ctx.app,
            &ctx.plan_id,
            &ctx.run_id,
            &ctx.workspace_id,
            GraphRunEvent::NodeActivity {
                node_id: ctx.node.id.clone(),
                activity,
            },
        );
    }
}

fn activity_from_event(ctx: &NodeExecContext, data: &Value, sequence: i64) -> AgentActivity {
    let now = chrono::Utc::now().timestamp_millis();
    let kind = data
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("activity")
        .to_string();
    AgentActivity {
        id: format!("{}:{}:{kind}:{sequence}", ctx.run_id, ctx.node.id),
        run_id: ctx.run_id.clone(),
        node_id: ctx.node.id.clone(),
        sequence,
        kind: kind.clone(),
        status: data
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("started")
            .into(),
        title: kind,
        content: data.get("error").map(value_text).unwrap_or_default(),
        payload_json: redact(data.clone()).to_string(),
        started_at: now,
        finished_at: None,
    }
}
fn emit_phase(ctx: &NodeExecContext, phase: &str) {
    emit_run_event(
        &ctx.app,
        &ctx.plan_id,
        &ctx.run_id,
        &ctx.workspace_id,
        GraphRunEvent::NodePhaseChanged {
            node_id: ctx.node.id.clone(),
            phase: phase.to_string(),
        },
    );
}
async fn write_jsonl(
    stdin: &Arc<Mutex<tokio::process::ChildStdin>>,
    value: &Value,
) -> anyhow::Result<()> {
    let mut writer = stdin.lock().await;
    writer.write_all(format!("{}\n", value).as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}
fn string_field(value: &Value, key: &str) -> anyhow::Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("PI 消息缺少 {key}"))
}
fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}
fn collect_affected_file(
    workspace_root: &Path,
    name: &str,
    args: &Value,
    files: &mut HashSet<String>,
) {
    if matches!(name, "edit" | "write" | "write_file" | "edit_file") {
        for key in ["path", "filePath", "file_path"] {
            if let Some(path) = args.get(key).and_then(Value::as_str) {
                // fail-closed：sidecar 提供的路径未经校验不得进入节点结果——
                // 伪造的绝对路径或 ../ 越界路径会误导图冲突检测与前端展示。
                match normalize_workspace_file(workspace_root, path) {
                    Some(normalized) => {
                        files.insert(normalized);
                    }
                    None => {
                        eprintln!(
                            "[graph] 丢弃越界受影响文件（工作区 {}）：{path}",
                            workspace_root.display()
                        );
                    }
                }
            }
        }
    }
}

/// 受影响文件路径的词典规范化（不依赖文件存在性）：相对路径挂到
/// workspace_root 下，逐段消解 `.`/`..`；规范化结果必须仍在工作区内，
/// 返回工作区相对形式（展示与写冲突预检更稳定）。越界返回 None。
fn normalize_workspace_file(workspace_root: &Path, raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let candidate = Path::new(raw);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workspace_root.join(candidate)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    let relative = normalized.strip_prefix(workspace_root).ok()?;
    Some(relative.display().to_string())
}
fn redact(mut value: Value) -> Value {
    /// key 脱敏判定：精确名单之外，追加包含/后缀匹配覆盖常见变体
    /// （X-Api-Key、client_secret、access_key、auth_token、db_password 等）。
    /// 宁可误伤少量同形键，也不让密钥变体绕过脱敏写入 payload。
    fn is_sensitive_key(normalized: &str) -> bool {
        matches!(
            normalized,
            "apikey"
                | "token"
                | "accesstoken"
                | "refreshtoken"
                | "idtoken"
                | "password"
                | "secret"
                | "authorization"
        ) || normalized.contains("secret")
            || normalized.contains("password")
            || normalized.ends_with("key")
            || normalized.ends_with("token")
    }
    fn walk(value: &mut Value) {
        match value {
            Value::Object(map) => {
                for (key, item) in map.iter_mut() {
                    let normalized = key
                        .chars()
                        .filter(|ch| ch.is_ascii_alphanumeric())
                        .flat_map(char::to_lowercase)
                        .collect::<String>();
                    if is_sensitive_key(&normalized) {
                        *item = Value::String("***".into())
                    } else {
                        walk(item)
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item)
                }
            }
            _ => {}
        }
    }
    walk(&mut value);
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursively_redacts_secrets() {
        let value = json!({
            "apiKey": "one",
            "nested": { "refresh_token": "two", "safe": "visible" },
            "items": [{ "authorization": "Bearer three" }]
        });
        let redacted = redact(value);
        assert_eq!(redacted["apiKey"], "***");
        assert_eq!(redacted["nested"]["refresh_token"], "***");
        assert_eq!(redacted["nested"]["safe"], "visible");
        assert_eq!(redacted["items"][0]["authorization"], "***");
    }

    #[test]
    fn rejects_invalid_sidecar_envelopes() {
        assert!(parse_sidecar_envelope("not-json", "request", "run", "node", 0).is_err());

        let frame = |request_id: &str, run_id: &str, node_id: &str, sequence: i64| {
            json!({
                "type": "agent_event",
                "requestId": request_id,
                "runId": run_id,
                "nodeId": node_id,
                "sequence": sequence,
                "data": {},
            })
            .to_string()
        };
        assert!(parse_sidecar_envelope(
            &frame("other", "run", "node", 1),
            "request",
            "run",
            "node",
            0
        )
        .is_err());
        assert!(parse_sidecar_envelope(
            &frame("request", "other", "node", 1),
            "request",
            "run",
            "node",
            0
        )
        .is_err());
        assert!(parse_sidecar_envelope(
            &frame("request", "run", "node", 1),
            "request",
            "run",
            "node",
            1
        )
        .is_err());
        assert!(parse_sidecar_envelope(
            &json!({
                "type": "ready",
                "requestId": "sidecar",
                "sequence": 1,
                "data": { "protocolVersion": PROTOCOL_VERSION + 1 },
            })
            .to_string(),
            "request",
            "run",
            "node",
            0,
        )
        .is_err());
    }

    #[test]
    fn rejects_agent_event_sequence_overflow() {
        assert_eq!(activity_sequence(42).unwrap(), 420);
        assert!(activity_sequence(i64::MAX).is_err());
        assert!(activity_sequence(i64::MIN).is_err());
    }

    #[test]
    fn context_usage_content_formats_reading_and_unknown() {
        let reading = json!({ "tokens": 55_000, "contextWindow": 128_000, "percent": 42.97 });
        assert_eq!(
            context_usage_content(&reading),
            "估算占用 43.0%（55000/128000 tokens）"
        );
        // compaction 后 tokens/percent 为 null：明确展示「重新估算中」而非 0%。
        let unknown = json!({ "tokens": null, "contextWindow": 128_000, "percent": null });
        assert_eq!(context_usage_content(&unknown), "上下文压缩后重新估算中…");
    }

    #[tokio::test]
    async fn pending_tool_future_is_interruptible_by_cancellation() {
        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        cancel_tx.send(true).unwrap();
        let mut pending = std::future::pending::<()>();
        let result = await_interruptible(
            std::pin::Pin::new(&mut pending),
            &mut cancel_rx,
            tokio::time::Instant::now() + Duration::from_secs(1),
        )
        .await;
        assert_eq!(result, Interruptible::Cancelled);
    }

    #[tokio::test]
    async fn pending_tool_future_is_interruptible_by_deadline() {
        let (_cancel_tx, mut cancel_rx) = watch::channel(false);
        let mut pending = std::future::pending::<()>();
        let result = await_interruptible(
            std::pin::Pin::new(&mut pending),
            &mut cancel_rx,
            tokio::time::Instant::now() + Duration::from_millis(10),
        )
        .await;
        assert_eq!(result, Interruptible::TimedOut);
    }

    #[tokio::test]
    async fn dropped_cancel_sender_is_treated_as_cancellation() {
        // 发送端丢弃后 changed() 返回 Err：必须按取消返回，禁止永久 pending。
        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        drop(cancel_tx);
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            cancellation_requested(&mut cancel_rx),
        )
        .await;
        assert!(result.is_ok());
    }

    #[test]
    fn redacts_secret_key_token_variants() {
        let value = json!({
            "X-Api-Key": "one",
            "client_secret": "two",
            "access_key": "three",
            "auth_token": "four",
            "db_password": "five",
            "nested": { "SIGNING_KEY": "six" },
            "name": "visible",
            "description": "also visible"
        });
        let redacted = redact(value);
        assert_eq!(redacted["X-Api-Key"], "***");
        assert_eq!(redacted["client_secret"], "***");
        assert_eq!(redacted["access_key"], "***");
        assert_eq!(redacted["auth_token"], "***");
        assert_eq!(redacted["db_password"], "***");
        assert_eq!(redacted["nested"]["SIGNING_KEY"], "***");
        assert_eq!(redacted["name"], "visible");
        assert_eq!(redacted["description"], "also visible");
    }

    #[test]
    fn requires_ready_handshake_before_business_messages() {
        let mut ready_seen = false;
        // ready 之前的业务消息必须拒绝。
        assert!(enforce_handshake_order("agent_event", &mut ready_seen).is_err());
        assert!(enforce_handshake_order("completed", &mut ready_seen).is_err());
        assert!(!ready_seen);
        // 首个 ready 放行，重复 ready 拒绝。
        assert!(enforce_handshake_order("ready", &mut ready_seen).is_ok());
        assert!(ready_seen);
        assert!(enforce_handshake_order("ready", &mut ready_seen).is_err());
        // 握手后的业务消息放行。
        assert!(enforce_handshake_order("completed", &mut ready_seen).is_ok());
    }

    #[test]
    fn affected_file_paths_are_confined_to_workspace() {
        let workspace = Path::new("/tmp/workspace");
        // 相对路径与绝对路径都规整为工作区相对形式。
        assert_eq!(
            normalize_workspace_file(workspace, "src/a.rs").as_deref(),
            Some("src/a.rs")
        );
        assert_eq!(
            normalize_workspace_file(workspace, "/tmp/workspace/src/a.rs").as_deref(),
            Some("src/a.rs")
        );
        // 内部 .. 可消解时允许。
        assert_eq!(
            normalize_workspace_file(workspace, "src/../lib/b.rs").as_deref(),
            Some("lib/b.rs")
        );
        // 越界路径（../ 逃逸、工作区外绝对路径）一律拒绝。
        assert_eq!(normalize_workspace_file(workspace, "../outside.rs"), None);
        assert_eq!(
            normalize_workspace_file(workspace, "src/../../escape.rs"),
            None
        );
        assert_eq!(normalize_workspace_file(workspace, "/etc/passwd"), None);
        assert_eq!(normalize_workspace_file(workspace, ""), None);
    }
}
