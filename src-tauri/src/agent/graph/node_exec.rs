//! 单节点 PI SDK RPC 执行器。每个节点拥有独立 sidecar 进程与内存会话。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::AppHandle;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{watch, Mutex};

use super::harness::ResolvedNodeHarness;
use super::pi_rpc::{global_agent_dir, SidecarChildGuard, SidecarEnvelope};
use super::runner::emit_run_event;
use super::store::GraphStore;
use super::types::{AgentActivity, GraphNode, GraphRunEvent};
use crate::agent::tools::{ToolContext, ToolRegistry, ToolStatus};

const NODE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const PROTOCOL_VERSION: i64 = 2;

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
    Failed(String),
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
        Err(_error) if *ctx.cancel_rx.borrow() => NodeExecOutcome::Cancelled,
        Err(error) => NodeExecOutcome::Failed(format!("PI Agent 执行失败：{error:#}")),
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
            "model": ctx.harness.model,
            "baseToolGroup": ctx.node.base_tool_group,
            "specialTools": ctx.node.special_tools,
            "hostTools": ctx.harness.host_tools,
        });
        write_jsonl(&stdin, &start).await?;

        let mut lines = BufReader::new(stdout).lines();
        let mut cancel_rx = ctx.cancel_rx.clone();
        let mut tool_activities: HashMap<String, AgentActivity> = HashMap::new();
        let mut affected_files = HashSet::new();
        let mut tool_call_count = 0_i64;
        let mut last_sequence = 0_i64;
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
                line = lines.next_line() => {
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
                            return Err(anyhow::anyhow!(message.to_string()));
                        }
                        "host_tool_call" => {
                            let call_id = string_field(&envelope.data, "callId")?;
                            let name = string_field(&envelope.data, "name")?;
                            let args = envelope.data.get("args").cloned().unwrap_or_else(|| json!({}));
                            collect_affected_file(&name, &args, &mut affected_files);
                            let result = match await_interruptible(
                                ctx.tool_registry.execute(&name, &args, &ctx.tool_context),
                                &mut cancel_rx,
                                deadline,
                            ).await {
                                Interruptible::Completed(result) => result,
                                Interruptible::TimedOut => {
                                    request_sidecar_cancel(&stdin, &request_id, ctx, &mut host_sequence).await;
                                    return Err(anyhow::anyhow!("节点执行超过 30 分钟，已终止"));
                                }
                                Interruptible::Cancelled => {
                                    request_sidecar_cancel(&stdin, &request_id, ctx, &mut host_sequence).await;
                                    return Ok(NodeExecOutcome::Cancelled);
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
            std::future::pending::<()>().await;
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Interruptible<T> {
    Completed(T),
    Cancelled,
    TimedOut,
}

async fn await_interruptible<T>(
    future: impl std::future::Future<Output = T>,
    cancel_rx: &mut watch::Receiver<bool>,
    deadline: tokio::time::Instant,
) -> Interruptible<T> {
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
            emit_phase(ctx, "responding");
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
            emit_phase(ctx, "thinking");
        }
        "tool_call" => {
            emit_phase(ctx, "tool_running");
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
                    collect_affected_file(&name, args, affected);
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
                    "retrying"
                } else {
                    "compacting"
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
fn collect_affected_file(name: &str, args: &Value, files: &mut HashSet<String>) {
    if matches!(name, "edit" | "write" | "write_file" | "edit_file") {
        for key in ["path", "filePath", "file_path"] {
            if let Some(path) = args.get(key).and_then(Value::as_str) {
                files.insert(path.to_string());
            }
        }
    }
}
fn redact(mut value: Value) -> Value {
    fn walk(value: &mut Value) {
        match value {
            Value::Object(map) => {
                for (key, item) in map.iter_mut() {
                    let normalized = key
                        .chars()
                        .filter(|ch| ch.is_ascii_alphanumeric())
                        .flat_map(char::to_lowercase)
                        .collect::<String>();
                    if matches!(
                        normalized.as_str(),
                        "apikey"
                            | "token"
                            | "accesstoken"
                            | "refreshtoken"
                            | "idtoken"
                            | "password"
                            | "secret"
                            | "authorization"
                    ) {
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
        let result = await_interruptible(
            std::future::pending::<()>(),
            &mut cancel_rx,
            tokio::time::Instant::now() + Duration::from_secs(1),
        )
        .await;
        assert_eq!(result, Interruptible::Cancelled);
    }

    #[tokio::test]
    async fn pending_tool_future_is_interruptible_by_deadline() {
        let (_cancel_tx, mut cancel_rx) = watch::channel(false);
        let result = await_interruptible(
            std::future::pending::<()>(),
            &mut cancel_rx,
            tokio::time::Instant::now() + Duration::from_millis(10),
        )
        .await;
        assert_eq!(result, Interruptible::TimedOut);
    }
}
