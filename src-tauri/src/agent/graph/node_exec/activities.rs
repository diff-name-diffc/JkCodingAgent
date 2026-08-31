use super::*;

#[derive(Default)]
pub(super) struct AgentToolEventState {
    pub(super) activities: HashMap<String, AgentActivity>,
    pub(super) affected_files: HashSet<String>,
    pub(super) call_count: i64,
}

pub(super) async fn handle_agent_event(
    ctx: &NodeExecContext,
    protocol_sequence: i64,
    data: &Value,
    assistant: &mut TextActivity,
    thinking: &mut TextActivity,
    tool_state: &mut AgentToolEventState,
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
                tool_state.call_count += 1;
                if let Some(args) = data.get("args") {
                    collect_affected_file(
                        &ctx.workspace_root,
                        &name,
                        args,
                        &mut tool_state.affected_files,
                    );
                }
            }
            let sequence = tool_state
                .activities
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
                started_at: tool_state
                    .activities
                    .get(&call_id)
                    .map(|a| a.started_at)
                    .unwrap_or(now),
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
            tool_state.activities.insert(call_id, activity);
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

pub(super) fn activity_sequence(protocol_sequence: i64) -> anyhow::Result<i64> {
    protocol_sequence.checked_mul(10).ok_or_else(|| {
        anyhow::anyhow!("PI sidecar sequence 超出活动序号可表示范围：{protocol_sequence}")
    })
}

/// context_usage 事件的人类可读摘要；原始数值在 payload_json 中供前端解析。
pub(super) fn context_usage_content(data: &Value) -> String {
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

pub(super) struct TextActivity {
    id: String,
    kind: String,
    title: String,
    pub(super) content: String,
    sequence: i64,
    started_at: i64,
    last_flush: tokio::time::Instant,
}
impl TextActivity {
    pub(super) fn new(ctx: &NodeExecContext, kind: &str, title: &str) -> Self {
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
    pub(super) async fn push(&mut self, ctx: &NodeExecContext, delta: &str) {
        self.content.push_str(delta);
        if self.last_flush.elapsed() >= Duration::from_millis(250) {
            self.flush(ctx).await;
            self.last_flush = tokio::time::Instant::now();
        }
    }
    pub(super) async fn flush(&self, ctx: &NodeExecContext) {
        self.persist(ctx, "streaming", None).await;
    }
    pub(super) async fn finish(&self, ctx: &NodeExecContext) {
        self.persist(ctx, "finished", Some(chrono::Utc::now().timestamp_millis()))
            .await;
    }
    pub(super) async fn persist(
        &self,
        ctx: &NodeExecContext,
        status: &str,
        finished_at: Option<i64>,
    ) {
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

pub(super) fn activity_from_event(
    ctx: &NodeExecContext,
    data: &Value,
    sequence: i64,
) -> AgentActivity {
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
pub(super) fn emit_phase(ctx: &NodeExecContext, phase: &str) {
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
pub(super) fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}
pub(super) fn collect_affected_file(
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
pub(super) fn normalize_workspace_file(workspace_root: &Path, raw: &str) -> Option<String> {
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
pub(super) fn redact(mut value: Value) -> Value {
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
