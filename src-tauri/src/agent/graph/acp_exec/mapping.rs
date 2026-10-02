//! ACP `session/update` 通知 → 图运行事件（GraphRunEvent）与节点活动
//! （AgentActivity）词汇的纯函数映射。
//!
//! 映射契约（与前端 `graph-utils.ts` / `graph-store.ts` 对齐）：
//! - 工具调用每个 call 一条 activity，id/sequence 稳定（同 id 同 sequence upsert），
//!   `tool_call_update` 复用同一 activity 更新 status/content；
//! - status 词汇：started / finished / failed（前端 normalizeToolCallStatus）；
//! - 输出增量走 `NodeOutputDelta` 事件并累积进节点输出缓冲；
//! - context_usage 的 payload 键为 tokens/contextWindow/percent（前端
//!   latestContextUsage），同时保留 ACP 原始 used/size/cost。

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1::{
    ContentBlock, PermissionOption, PermissionOptionId, PermissionOptionKind, Plan, SessionUpdate,
    ToolCall, ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdate, UsageUpdate,
};
use serde_json::{json, Value};

use super::super::input::truncate_chars;
use super::super::types::{
    AgentActivity, GraphRunEvent, NODE_PHASE_RESPONDING, NODE_PHASE_THINKING,
    NODE_PHASE_TOOL_RUNNING,
};

/// activity.content（工具输出摘要等）的字符上限。
const MAX_ACTIVITY_CONTENT_CHARS: usize = 8_000;
/// payload 内单字段（如 args）的字符上限，超过降级为截断字符串。
const MAX_ACTIVITY_PAYLOAD_CHARS: usize = 4_000;
/// 节点输出缓冲的字节上限：执行器失控（无界输出）时防止宿主内存膨胀。
/// 超限后丢弃后续增量并一次性补记截断标记。
pub(super) const MAX_NODE_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
/// 思考缓冲的字节上限（同上）。
const MAX_THINKING_BYTES: usize = 1024 * 1024;

/// 映射产物：要么仅广播事件，要么产出一条活动（落库 + 广播 NodeActivity）。
#[derive(Debug)]
pub(super) enum MapperAction {
    Emit(GraphRunEvent),
    Activity(AgentActivity),
}

/// 单个工具调用的跟踪状态（跨 tool_call / tool_call_update 合并）。
struct ToolCallTrack {
    sequence: i64,
    started_at: i64,
    title: String,
    input: Option<Value>,
    locations: Vec<String>,
    /// 已到终态（completed/failed）的调用忽略后续更新，防止乱序抹掉终态。
    terminal: bool,
}

/// ACP 更新 → 图事件/活动的有状态映射器（单线程消费，内部可变）。
pub(super) struct Mapper {
    run_id: String,
    node_id: String,
    workspace_root: PathBuf,
    next_sequence: i64,
    phase: Option<&'static str>,
    output: String,
    thinking: String,
    thinking_started_at: Option<i64>,
    tool_calls: HashMap<String, ToolCallTrack>,
    tool_call_count: i64,
    affected_files: BTreeSet<String>,
    output_truncated: bool,
}

impl Mapper {
    pub(super) fn new(run_id: &str, node_id: &str, workspace_root: &Path) -> Self {
        Self {
            run_id: run_id.to_string(),
            node_id: node_id.to_string(),
            workspace_root: workspace_root.to_path_buf(),
            next_sequence: 0,
            phase: None,
            output: String::new(),
            thinking: String::new(),
            thinking_started_at: None,
            tool_calls: HashMap::new(),
            tool_call_count: 0,
            affected_files: BTreeSet::new(),
            output_truncated: false,
        }
    }

    pub(super) fn feed(&mut self, update: &SessionUpdate) -> Vec<MapperAction> {
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                let mut actions = self.flush_thinking();
                if let Some(text) = content_block_text(&chunk.content) {
                    if !text.is_empty() {
                        if self.output.len() >= MAX_NODE_OUTPUT_BYTES {
                            // 输出缓冲已达上限：丢弃增量并一次性补记截断标记
                            // （执行器无界输出不得撑爆宿主内存）。
                            if !self.output_truncated {
                                self.output_truncated = true;
                                let marker = "\n…（节点输出超过 4MB，后续内容已丢弃）";
                                self.output.push_str(marker);
                                actions.push(MapperAction::Emit(GraphRunEvent::NodeOutputDelta {
                                    node_id: self.node_id.clone(),
                                    delta: marker.to_string(),
                                }));
                            }
                            return actions;
                        }
                        self.output.push_str(text);
                        self.set_phase(NODE_PHASE_RESPONDING, &mut actions);
                        actions.push(MapperAction::Emit(GraphRunEvent::NodeOutputDelta {
                            node_id: self.node_id.clone(),
                            delta: text.to_string(),
                        }));
                    }
                }
                actions
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                let mut actions = Vec::new();
                if let Some(text) = content_block_text(&chunk.content) {
                    if !text.is_empty() && self.thinking.len() < MAX_THINKING_BYTES {
                        if self.thinking.is_empty() {
                            self.thinking_started_at = Some(now_ms());
                        }
                        self.thinking.push_str(text);
                        self.set_phase(NODE_PHASE_THINKING, &mut actions);
                    }
                }
                actions
            }
            SessionUpdate::ToolCall(tool_call) => self.feed_tool_call(tool_call),
            SessionUpdate::ToolCallUpdate(update) => self.feed_tool_call_update(update),
            SessionUpdate::Plan(plan) => self.feed_plan(plan),
            SessionUpdate::UsageUpdate(usage) => self.feed_usage(usage),
            // 其余更新（用户消息回显、可用命令、模式/配置项变更、会话信息等）
            // 对节点时间线与产出无增量，忽略。
            _ => Vec::new(),
        }
    }

    /// 收尾：flush 未落地的思考缓冲，返回（动作、节点输出、工具调用数、受影响文件）。
    pub(super) fn finish(&mut self) -> (Vec<MapperAction>, String, i64, Vec<String>) {
        let actions = self.flush_thinking();
        (
            actions,
            std::mem::take(&mut self.output),
            self.tool_call_count,
            self.affected_files.iter().cloned().collect(),
        )
    }

    /// 生命周期审计活动（权限自动应答、会话诊断等），kind=lifecycle。
    pub(super) fn lifecycle_activity(&mut self, title: &str, content: &str) -> MapperAction {
        let now = now_ms();
        let sequence = self.alloc_sequence();
        MapperAction::Activity(AgentActivity {
            id: format!("{}:{}:lifecycle:{sequence}", self.run_id, self.node_id),
            run_id: self.run_id.clone(),
            node_id: self.node_id.clone(),
            sequence,
            kind: "lifecycle".into(),
            status: "finished".into(),
            title: title.to_string(),
            content: truncate_chars(
                content,
                MAX_ACTIVITY_CONTENT_CHARS,
                ACTIVITY_TRUNCATE_SUFFIX,
            ),
            payload_json: "{}".into(),
            started_at: now,
            finished_at: Some(now),
        })
    }

    fn alloc_sequence(&mut self) -> i64 {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        sequence
    }

    fn set_phase(&mut self, phase: &'static str, actions: &mut Vec<MapperAction>) {
        if self.phase != Some(phase) {
            self.phase = Some(phase);
            actions.push(MapperAction::Emit(GraphRunEvent::NodePhaseChanged {
                node_id: self.node_id.clone(),
                phase: phase.into(),
            }));
        }
    }

    /// 思考块在「非思考更新到来 / 收尾」时落地为一条 thinking activity。
    fn flush_thinking(&mut self) -> Vec<MapperAction> {
        if self.thinking.is_empty() {
            return Vec::new();
        }
        let sequence = self.alloc_sequence();
        let started_at = self.thinking_started_at.take().unwrap_or_else(now_ms);
        let activity = AgentActivity {
            id: format!("{}:{}:thinking:{sequence}", self.run_id, self.node_id),
            run_id: self.run_id.clone(),
            node_id: self.node_id.clone(),
            sequence,
            kind: "thinking".into(),
            status: "finished".into(),
            title: "思考过程".into(),
            content: truncate_chars(
                &self.thinking,
                MAX_ACTIVITY_CONTENT_CHARS,
                ACTIVITY_TRUNCATE_SUFFIX,
            ),
            payload_json: "{}".into(),
            started_at,
            finished_at: Some(now_ms()),
        };
        self.thinking.clear();
        vec![MapperAction::Activity(activity)]
    }

    fn feed_tool_call(&mut self, tool_call: &ToolCall) -> Vec<MapperAction> {
        let mut actions = self.flush_thinking();
        let call_id = tool_call.tool_call_id.0.to_string();
        if self.tool_calls.contains_key(&call_id) {
            return actions;
        }
        self.tool_call_count += 1;
        let sequence = self.alloc_sequence();
        let started_at = now_ms();
        let locations = normalize_locations(&self.workspace_root, &tool_call.locations);
        for file in &locations {
            self.affected_files.insert(file.clone());
        }
        let title = tool_title(&tool_call.title, tool_call.name.as_deref());
        let terminal = is_terminal_status(&tool_call.status);
        let content = if terminal {
            tool_output_text(tool_call.raw_output.as_ref(), &tool_call.content)
        } else {
            String::new()
        };
        self.tool_calls.insert(
            call_id.clone(),
            ToolCallTrack {
                sequence,
                started_at,
                title: title.clone(),
                input: tool_call.raw_input.clone(),
                locations: locations.clone(),
                terminal,
            },
        );
        let activity = self.build_tool_activity(
            &call_id,
            sequence,
            map_tool_status(&tool_call.status),
            &title,
            &content,
            tool_call.raw_input.as_ref(),
            &locations,
            started_at,
            terminal.then(now_ms),
        );
        self.set_phase(NODE_PHASE_TOOL_RUNNING, &mut actions);
        actions.push(MapperAction::Activity(activity));
        actions
    }

    fn feed_tool_call_update(&mut self, update: &ToolCallUpdate) -> Vec<MapperAction> {
        let mut actions = self.flush_thinking();
        let call_id = update.tool_call_id.0.to_string();
        let fields = &update.fields;
        let now = now_ms();
        // 预先算好相对路径：后续块内持有 tool_calls 的可变借用，不能再触 self 其他字段。
        let new_locations = fields
            .locations
            .as_deref()
            .map(|items| normalize_locations(&self.workspace_root, items));
        if !self.tool_calls.contains_key(&call_id) {
            // 未知调用的更新：登记为新调用（计数与首次 activity 口径一致）。
            self.tool_call_count += 1;
            let sequence = self.alloc_sequence();
            let title = tool_title(
                fields.title.as_deref().unwrap_or_default(),
                fields.name.as_deref(),
            );
            self.tool_calls.insert(
                call_id.clone(),
                ToolCallTrack {
                    sequence,
                    started_at: now,
                    title,
                    input: fields.raw_input.clone(),
                    locations: new_locations.clone().unwrap_or_default(),
                    terminal: false,
                },
            );
        }
        let status = fields.status.as_ref();
        let terminal = status.map(is_terminal_status).unwrap_or(false);
        let content = tool_output_text(
            fields.raw_output.as_ref(),
            fields.content.as_deref().unwrap_or(&[]),
        );
        let (sequence, title, input, started_at, locations) = {
            let track = self
                .tool_calls
                .get_mut(&call_id)
                .expect("tool call track 刚登记必存在");
            if track.terminal {
                return actions;
            }
            if let Some(title) = &fields.title {
                if !title.trim().is_empty() {
                    track.title = title.clone();
                }
            }
            if let Some(input) = &fields.raw_input {
                track.input = Some(input.clone());
            }
            if let Some(locations) = new_locations.clone() {
                track.locations = locations;
            }
            track.terminal = terminal;
            (
                track.sequence,
                track.title.clone(),
                track.input.clone(),
                track.started_at,
                track.locations.clone(),
            )
        };
        for file in &locations {
            self.affected_files.insert(file.clone());
        }
        let activity = self.build_tool_activity(
            &call_id,
            sequence,
            status.map(map_tool_status).unwrap_or("started"),
            &title,
            &content,
            input.as_ref(),
            &locations,
            started_at,
            terminal.then_some(now),
        );
        self.set_phase(NODE_PHASE_TOOL_RUNNING, &mut actions);
        actions.push(MapperAction::Activity(activity));
        actions
    }

    #[allow(clippy::too_many_arguments)]
    fn build_tool_activity(
        &self,
        call_id: &str,
        sequence: i64,
        status: &str,
        title: &str,
        content: &str,
        input: Option<&Value>,
        locations: &[String],
        started_at: i64,
        finished_at: Option<i64>,
    ) -> AgentActivity {
        let payload = redact(json!({
            "kind": "tool_call",
            "callId": call_id,
            "args": input.map(|value| truncate_value(value, MAX_ACTIVITY_PAYLOAD_CHARS)),
            "locations": locations,
        }));
        AgentActivity {
            id: format!("{}:{}:tool:{call_id}", self.run_id, self.node_id),
            run_id: self.run_id.clone(),
            node_id: self.node_id.clone(),
            sequence,
            kind: "tool_call".into(),
            status: status.into(),
            title: title.to_string(),
            content: content.to_string(),
            payload_json: payload.to_string(),
            started_at,
            finished_at,
        }
    }

    fn feed_plan(&mut self, plan: &Plan) -> Vec<MapperAction> {
        let mut actions = self.flush_thinking();
        let entries = plan
            .entries
            .iter()
            .map(|entry| {
                json!({
                    "content": entry.content,
                    "priority": serde_json::to_value(&entry.priority).unwrap_or(Value::Null),
                    "status": serde_json::to_value(&entry.status).unwrap_or(Value::Null),
                })
            })
            .collect::<Vec<_>>();
        let content = plan
            .entries
            .iter()
            .map(|entry| {
                let status = serde_json::to_value(&entry.status)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_string))
                    .unwrap_or_else(|| "pending".into());
                format!("- [{status}] {}", entry.content)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let sequence = self.alloc_sequence();
        let now = now_ms();
        actions.push(MapperAction::Activity(AgentActivity {
            id: format!("{}:{}:plan:{sequence}", self.run_id, self.node_id),
            run_id: self.run_id.clone(),
            node_id: self.node_id.clone(),
            sequence,
            kind: "plan".into(),
            status: "finished".into(),
            title: "任务计划".into(),
            content,
            payload_json: json!({ "entries": entries }).to_string(),
            started_at: now,
            finished_at: Some(now),
        }));
        actions
    }

    fn feed_usage(&mut self, usage: &UsageUpdate) -> Vec<MapperAction> {
        let percent = if usage.size > 0 {
            let value = usage.used as f64 / usage.size as f64 * 100.0;
            json!((value * 100.0).round() / 100.0)
        } else {
            Value::Null
        };
        let cost = usage
            .cost
            .as_ref()
            .map(|cost| json!({ "amount": cost.amount, "currency": cost.currency }));
        let sequence = self.alloc_sequence();
        let now = now_ms();
        vec![MapperAction::Activity(AgentActivity {
            id: format!("{}:{}:usage:{sequence}", self.run_id, self.node_id),
            run_id: self.run_id.clone(),
            node_id: self.node_id.clone(),
            sequence,
            kind: "context_usage".into(),
            status: "finished".into(),
            title: "上下文占用".into(),
            content: String::new(),
            payload_json: json!({
                // 前端 latestContextUsage 消费的三键：
                "tokens": usage.used,
                "contextWindow": usage.size,
                "percent": percent,
                // ACP 原始字段与成本：
                "used": usage.used,
                "size": usage.size,
                "cost": cost,
            })
            .to_string(),
            started_at: now,
            finished_at: Some(now),
        })]
    }
}

/// 权限自动应答决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PermissionDecision {
    /// 选中某个选项（允许或拒绝）。
    Select(PermissionOptionId),
    /// 无可选选项（或唯一可选项会放大授权）时取消该工具调用。
    Cancel,
}

/// 按选项 kind 选中第一个匹配项（审查/快路径共用的选项选择器）。
pub(super) fn select_kind(
    options: &[PermissionOption],
    wanted: PermissionOptionKind,
) -> Option<PermissionDecision> {
    options
        .iter()
        .find(|option| option.kind == wanted)
        .map(|option| PermissionDecision::Select(option.option_id.clone()))
}

/// 静态快路径决策：可本地判定的请求不消耗审查模型。
///
/// - `EnterPlanMode`：放行（子智能体自发进入计划模式是允许的工作方式）；
/// - `ExitPlanMode`（"Ready to code?"）：批准并按 `select_plan_approval`
///   切换权限级别；
/// - 路径越界：拒绝（硬边界，先于审查 AI）；
/// - 其余：返回 `None`，交给全局权限审查 AI 裁决。
///
/// 无论哪种路径都不选 allow_always / reject_always 的常驻规则类选项——
/// 自动应答的作用域仅限单次调用（ExitPlanMode 的模式切换选项是例外，
/// 见 `select_plan_approval`）。
pub(super) fn decide_static(
    tool_name: &str,
    options: &[PermissionOption],
    out_of_workspace: bool,
) -> Option<PermissionDecision> {
    if tool_name == "EnterPlanMode" {
        return Some(
            select_kind(options, PermissionOptionKind::AllowOnce)
                .unwrap_or(PermissionDecision::Cancel),
        );
    }
    if tool_name == "ExitPlanMode" {
        return Some(select_plan_approval(options));
    }
    if out_of_workspace {
        return Some(
            select_kind(options, PermissionOptionKind::RejectOnce)
                .unwrap_or(PermissionDecision::Cancel),
        );
    }
    None
}

/// ExitPlanMode 的批准选项选择：批准计划后由客户端统一切回
/// **bypassPermissions**（client.rs 在批准应答后主动 `set_mode`）。
///
/// claude-agent-acp 0.79.0（launcher 锁定版本）的选项表：elevated 档恒为
/// auto 变体（auto 永远在模式目录里，"bypass permissions" 变体不可达），
/// 因此这里的选项只充当「批准载体」——不承载目标权限级别，级别由客户端
/// 后续 set_mode 保证：
/// - allow_always「Yes, and use auto mode」（setMode auto，仅作批准载体）
/// - allow_once「Yes, manually approve edits」（降为逐次审批）
/// - reject_once「No, keep planning」
/// 以及各自带 "clear context" 前缀的变体（批准并清空上下文重启——会丢弃
/// 本轮节点已积累的上下文，跳过）。
///
/// 优先级：bypass 命名选项（未来 adapter 若提供，则无需跟进 set_mode）>
/// 任意 allow_always（非 clear context）> 任意 allow_once > reject_once >
/// Cancel。前两档虽是 allow_always kind，但其 updatedPermissions 是
/// `setMode destination:"session"` 的会话级切换、不落持久规则，与「不放大
/// 授权」原则不冲突。选项名匹配绑定 adapter 版本，升级 adapter 时需回归
/// `plan_approval_prefers_elevated_option` 测试。
pub(super) fn select_plan_approval(options: &[PermissionOption]) -> PermissionDecision {
    fn name_contains(option: &PermissionOption, needle: &str) -> bool {
        option.name.to_ascii_lowercase().contains(needle)
    }
    let is_clear_context = |option: &PermissionOption| name_contains(option, "clear context");
    let bypass = options.iter().find(|option| {
        option.kind == PermissionOptionKind::AllowAlways
            && name_contains(option, "bypass permissions")
            && !is_clear_context(option)
    });
    if let Some(option) = bypass {
        return PermissionDecision::Select(option.option_id.clone());
    }
    let elevated = options.iter().find(|option| {
        option.kind == PermissionOptionKind::AllowAlways && !is_clear_context(option)
    });
    if let Some(option) = elevated {
        return PermissionDecision::Select(option.option_id.clone());
    }
    select_kind(options, PermissionOptionKind::AllowOnce)
        .or_else(|| select_kind(options, PermissionOptionKind::RejectOnce))
        .unwrap_or(PermissionDecision::Cancel)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn content_block_text(block: &ContentBlock) -> Option<&str> {
    match block {
        ContentBlock::Text(text) => Some(text.text.as_str()),
        _ => None,
    }
}

fn is_terminal_status(status: &ToolCallStatus) -> bool {
    matches!(status, ToolCallStatus::Completed | ToolCallStatus::Failed)
}

/// ACP ToolCallStatus → 前端 status 词汇（normalizeToolCallStatus 的对偶）。
fn map_tool_status(status: &ToolCallStatus) -> &'static str {
    match status {
        ToolCallStatus::Completed => "finished",
        ToolCallStatus::Failed => "failed",
        _ => "started",
    }
}

fn tool_title(title: &str, name: Option<&str>) -> String {
    let title = title.trim();
    if !title.is_empty() {
        return title.to_string();
    }
    name.map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("工具调用")
        .to_string()
}

/// 受影响文件归一化：工作区内的路径转为相对路径；工作区外的保留绝对路径
/// 并加 `[工作区外]` 前缀——越界写入必须留在审计记录里（运行回执、节点
/// 详情、verifier 输入），丢弃会恰好抹掉最具安全意义的修改。
fn normalize_locations(root: &Path, locations: &[ToolCallLocation]) -> Vec<String> {
    let mut result = Vec::new();
    for location in locations {
        let text = match location.path.strip_prefix(root) {
            Ok(relative) => relative.to_string_lossy().to_string(),
            Err(_) => format!("[工作区外] {}", location.path.display()),
        };
        if !text.is_empty() && !result.contains(&text) {
            result.push(text);
        }
    }
    result
}

/// 工具输出摘要：优先 raw_output（JSON 截断），否则汇总 content 块。
fn tool_output_text(raw_output: Option<&Value>, content: &[ToolCallContent]) -> String {
    if let Some(raw) = raw_output {
        let text = match raw {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        return truncate_chars(&text, MAX_ACTIVITY_CONTENT_CHARS, ACTIVITY_TRUNCATE_SUFFIX);
    }
    let mut parts = Vec::new();
    for item in content {
        match item {
            ToolCallContent::Content(block) => {
                if let Some(text) = content_block_text(&block.content) {
                    parts.push(text.to_string());
                }
            }
            ToolCallContent::Diff(diff) => {
                parts.push(format!("修改文件：{}", diff.path.display()));
            }
            ToolCallContent::Terminal(_) => parts.push("（终端输出）".to_string()),
            _ => {}
        }
    }
    truncate_chars(
        &parts.join("\n"),
        MAX_ACTIVITY_CONTENT_CHARS,
        ACTIVITY_TRUNCATE_SUFFIX,
    )
}

/// 活动内容/输出截断后缀（追加截断标记，避免前端把残缺 JSON 当完整数据解析；
/// 截断口径复用 `input::truncate_chars`：字符而非字节）。
const ACTIVITY_TRUNCATE_SUFFIX: &str = "…（截断）";

/// payload 单字段限长：序列化超长时降级为截断字符串（保留可读性，
/// 不把残缺 JSON 写入 payload）。
fn truncate_value(value: &Value, max_chars: usize) -> Value {
    let text = value.to_string();
    if text.chars().count() <= max_chars {
        value.clone()
    } else {
        Value::String(truncate_chars(&text, max_chars, ACTIVITY_TRUNCATE_SUFFIX))
    }
}

/// 事件 payload 的密钥脱敏（key 命中敏感名单即替换为 "***"）。
/// 任何写入 activities.payload_json 的数据必须先过本函数，
/// 防止 apiKey/token 等落库或广播到前端。
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
