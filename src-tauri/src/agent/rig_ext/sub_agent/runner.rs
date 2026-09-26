//! 子智能体运行时（rig 形态）。
//!
//! 与旧实现（`agent/sub_agent/runtime.rs`）的差异只在「如何调模型/执行工具」：
//! 旧实现直接用 `OpenAiCompatProvider::chat_stream_with_thinking` + 旧
//! `ToolRegistry`/`ToolRuntime`，本实现消费 rig `CompletionModel::stream()` 的
//! `StreamedAssistantContent` 并对 `PortableDynamicTool` 面执行。行为语义逐条保留：
//!
//! - 独立执行上下文：内存消息历史（不落库），结果截断到
//!   `SUB_AGENT_RESULT_MAX_CHARS` 后返回父循环；
//! - 滚动压缩裁剪（与主对话同一整形层 `rig_ext::context`，容量源 = 模型库
//!   条目 contextWindow；子智能体不消耗摘要模型，规则兜底折叠）；
//! - 单次请求超时 `SUB_AGENT_LLM_REQUEST_TIMEOUT_SECS` 与整体超时
//!   `config.timeout_secs`；整体超时向工具转发取消信号（协作式收敛），
//!   并对每次工具等待施加剩余预算硬边界；
//! - 工具失败重试升级（G13-05）：同名工具跨轮再次失败 → `force_final_response`
//!   （下一轮传空工具集，逼模型给最终结论；模型仍返回工具调用时不再执行，
//!   有文本即强制收口、无文本按错误退出）；
//! - 事件流与轨迹缓冲（`SubAgentEvent` + `sub-agent-event`）。

#[path = "runner/decision.rs"]
mod decision;
#[path = "runner/lifecycle.rs"]
mod lifecycle;
use lifecycle::{forward_cancellation, resolve_sub_agent_spec};

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rig::completion::Message;
use rig::message::{ToolCall, ToolResult, ToolResultContent, UserContent};
use rig::tool::PortableDynamicTool;
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;
use tokio::time::timeout;

use super::events::{
    record_trace_event, SubAgentEvent, SubAgentEventPayload, SubAgentUsage,
    SUB_AGENT_TRACE_EVENT_LIMIT,
};
use super::tools::notify_user_progress_tool;
use crate::agent::rig_ext::context::{compact_history_offline, context_budget_chars};
use crate::agent::rig_ext::model::{build_completion_request, completions_model, PurposeModelSpec};
use crate::agent::rig_ext::tools::deps::RigToolDeps;
use crate::agent::rig_ext::tools::MAX_TOOL_CALLS_PER_BATCH;
use crate::agent::rig_ext::tools::{exec::exec_tools, media::media_tools};
use crate::agent::sub_agent::config::SubAgentConfig;

/// 返回父循环前的结果截断上限。
const SUB_AGENT_RESULT_MAX_CHARS: usize = 32_000;
/// 单次模型请求超时（秒）。
const SUB_AGENT_LLM_REQUEST_TIMEOUT_SECS: u64 = 120;
/// 上下文裁剪的保护头部条数：system + 首轮任务（恒不折叠）。
const SUB_AGENT_HEADER_LEN: usize = 2;
/// 嵌套子智能体工具（子智能体不得递归派生）。
const NESTED_SUB_AGENT_TOOLS: &[&str] = &["call_sub_agent", "list_sub_agents"];

/// 一次子智能体调用的输入。
pub struct RigSubAgentRequest<'a> {
    pub config: &'a SubAgentConfig,
    /// 父级聊天槽位规格（继承凭据/网关与容量的基准）。
    pub parent_spec: &'a PurposeModelSpec,
    /// 父级工具依赖（子智能体在其上覆盖执行者任务/取消/工具面相关字段）。
    pub deps: &'a RigToolDeps,
    pub task: &'a str,
    pub parent_tool_call_id: &'a str,
    pub app_handle: Option<AppHandle>,
    pub session_id: &'a str,
    /// 父级取消信号（图运行/父 run 取消时透传）。
    pub cancel_rx: Option<watch::Receiver<bool>>,
}

/// 子智能体独立执行运行时。
pub struct RigSubAgentRuntime {
    config: SubAgentConfig,
    db: crate::agent::db::DispatcherDb,
    policy: crate::agent::rig_ext::r#loop::AppToolExecutionPolicy,
    loop_events: tauri::ipc::Channel<crate::agent::rig_ext::events::AgentEvent>,
    spec: PurposeModelSpec,
    surface: Vec<PortableDynamicTool>,
    session_id: String,
    parent_tool_call_id: String,
    app_handle: Option<AppHandle>,
    trace_events: Arc<Mutex<Vec<Value>>>,
    /// run 级取消通道：工具面在构建期捕获其接收端，整体超时/父取消时翻转。
    tool_cancel_tx: watch::Sender<bool>,
    /// 父级取消信号（透传给工具取消转发任务）。
    parent_cancel: Option<watch::Receiver<bool>>,
}

impl RigSubAgentRuntime {
    /// 构建运行时：解析模型槽位、组装工具面（按 `config.allowed_tools` 精确
    /// 过滤并校验可用性，排除嵌套子智能体工具）。
    pub fn build(request: &RigSubAgentRequest<'_>) -> Result<Self, String> {
        let config = request.config;
        let spec = resolve_sub_agent_spec(config, request.parent_spec);

        // 排除嵌套子智能体工具（call_sub_agent / list_sub_agents），避免递归派生。
        let mut nested = config
            .allowed_tools
            .iter()
            .filter(|name| NESTED_SUB_AGENT_TOOLS.contains(&name.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        nested.sort();
        if !nested.is_empty() {
            return Err(format!(
                "错误：子智能体 '{}' 不允许递归调用子智能体工具：{}。请在设置中移除这些工具。",
                config.agent_name,
                nested.join("、")
            ));
        }

        // 执行者任务进审查上下文：安全审查据此判断「是谁、为什么」执行命令。
        let mut deps = request.deps.clone();
        deps.review.executor_task = Some(request.task.to_string());
        deps.sub_agent_manager = None;
        // run 级取消通道：必须在组装工具面前建立——工具在构造期捕获取消接收端。
        let (tool_cancel_tx, tool_cancel_rx) = watch::channel(false);
        deps.cancel_rx = Some(tool_cancel_rx);

        // 工具面 = 普通聊天 execution profile（exec + media）+ 子智能体专用的
        // 进度通知工具（携带子智能体身份与父调用关联，见 `notify_user_progress_tool`）。
        // 不混入编排器工具与嵌套子智能体工具。
        let trace_events = Arc::new(Mutex::new(Vec::with_capacity(SUB_AGENT_TRACE_EVENT_LIMIT)));
        let mut surface = exec_tools(&deps);
        surface.extend(media_tools(&deps));
        surface.push(notify_user_progress_tool(
            config.agent_id.clone(),
            config.agent_name.clone(),
            request.parent_tool_call_id.to_string(),
            request.app_handle.clone(),
            Arc::clone(&trace_events),
            request.session_id.to_string(),
        ));

        let allowed: std::collections::HashSet<&str> =
            config.allowed_tools.iter().map(String::as_str).collect();
        let available: std::collections::HashSet<&str> =
            surface.iter().map(PortableDynamicTool::name).collect();
        let mut unavailable = allowed.difference(&available).copied().collect::<Vec<_>>();
        unavailable.sort();
        if !unavailable.is_empty() {
            return Err(format!(
                "错误：子智能体 '{}' 配置了当前普通聊天执行环境不可用的工具：{}。请在设置中重新选择工具。",
                config.agent_name,
                unavailable.join("、")
            ));
        }
        surface.retain(|tool| allowed.contains(tool.name()));

        let loop_events = super::loop_events::channel(
            config.agent_id.clone(),
            config.agent_name.clone(),
            request.session_id.into(),
            request.parent_tool_call_id.into(),
            request.app_handle.clone(),
            trace_events.clone(),
        );
        let policy = crate::agent::rig_ext::r#loop::AppToolExecutionPolicy::new(
            &deps.db,
            &loop_events,
            crate::agent::rig_ext::r#loop::AppToolPolicyConfig {
                workspace_id: request.session_id.into(),
                workspace: deps.workspace.clone(),
                review: deps.review.clone(),
                cancel_rx: deps.cancel_rx.clone(),
                trace: Default::default(),
            },
        );
        Ok(Self {
            db: deps.db.clone(),
            policy,
            loop_events,
            config: config.clone(),
            spec,
            surface,
            session_id: request.session_id.to_string(),
            parent_tool_call_id: request.parent_tool_call_id.to_string(),
            app_handle: request.app_handle.clone(),
            trace_events,
            tool_cancel_tx,
            parent_cancel: request.cancel_rx.clone(),
        })
    }

    /// 运行时实际解析出的模型名（轨迹持久化与 Started 事件用）。
    pub fn model(&self) -> &str {
        &self.spec.model
    }

    /// 轨迹缓冲中 `notify_user_progress` 工具写入事件所需的句柄。
    /// 主执行循环：请求模型 → 执行工具 → 判断收口。返回最终答复文本；
    /// 失败返回「错误：」前缀的错误文本（由调用方按委派失败处理）。
    pub async fn execute(&self, task: &str) -> Result<String, String> {
        let model = completions_model(&self.spec).map_err(|error| {
            format!(
                "错误：子智能体 '{}' 模型初始化失败：{error}",
                self.config.agent_id
            )
        })?;
        let start = Instant::now();
        let overall_timeout = Duration::from_secs(self.config.timeout_secs);
        let deadline = start + overall_timeout;
        let mut usage = SubAgentUsage::default();

        // 整体超时/父取消 → 翻转 run 级取消通道（工具协作式收敛）。
        let signal_task = tokio::spawn(forward_cancellation(
            self.parent_cancel.clone(),
            self.tool_cancel_tx.clone(),
            overall_timeout,
        ));

        self.emit(SubAgentEvent::Started {
            agent_id: self.config.agent_id.clone(),
            agent_name: self.config.agent_name.clone(),
            task: task.to_string(),
            model: self.spec.model.clone(),
        });

        let user_prompt = self.config.user_prompt_template.replace("{{task}}", task);
        let mut messages = vec![
            Message::system(self.config.system_prompt.clone()),
            Message::user(user_prompt),
        ];

        // G13-05：按工具名记录「已消耗重试资格的失败轮数」。
        let mut force_final_response = false;
        let mut last_iteration: u32 = 0;

        let mut coordinator = crate::agent::rig_ext::r#loop::coordinator::Coordinator::new(
            crate::agent::rig_ext::r#loop::scheduler::TaskScheduler::new(
                self.db.clone(),
                self.session_id.clone(),
                self.tool_cancel_tx.subscribe(),
                crate::agent::rig_ext::tool_result::prepare::raw_preparer(),
                self.loop_events.clone(),
            ),
        );
        coordinator.host = crate::agent::rig_ext::r#loop::host::LoopHost::Memory;
        let outcome = self
            .run_loop(
                &model,
                &mut messages,
                &mut usage,
                start,
                deadline,
                &mut force_final_response,
                &mut last_iteration,
                &mut coordinator,
            )
            .await;
        if outcome.is_err() {
            self.tool_cancel_tx.send_replace(true);
        }
        // 结果已产出，收尾 drain 失败仅告警降级（与 after_call 同一口径），
        // 不让父 Agent 因清理失败丢失可用结论。
        if let Err(error) = coordinator.tasks.shutdown().await {
            eprintln!(
                "子智能体 '{}' 收尾 drain 失败：{error}",
                self.config.agent_id
            );
        }
        signal_task.abort();

        match outcome {
            Ok(result) => {
                self.emit(SubAgentEvent::Finished {
                    agent_id: self.config.agent_id.clone(),
                    agent_name: self.config.agent_name.clone(),
                    result: result.clone(),
                    iterations: last_iteration,
                    elapsed_ms: start.elapsed().as_millis() as u64,
                    token_usage: usage,
                });
                Ok(result)
            }
            Err(error) => {
                self.emit(SubAgentEvent::Failed {
                    agent_id: self.config.agent_id.clone(),
                    agent_name: self.config.agent_name.clone(),
                    error: error.clone(),
                });
                Err(error)
            }
        }
    }

    fn cancelled(&self) -> bool {
        self.parent_cancel
            .as_ref()
            .is_some_and(|rx| *rx.borrow() || rx.has_changed().is_err())
    }

    /// 事件下发 + 轨迹写入（对齐旧 `emit_event`）。
    pub(crate) fn emit(&self, event: SubAgentEvent) {
        let timestamp_ms = chrono::Utc::now().timestamp_millis();
        if let Ok(value) = serde_json::to_value(&event) {
            record_trace_event(&self.trace_events, value, timestamp_ms);
        }
        if let Some(handle) = &self.app_handle {
            let _ = handle.emit(
                "sub-agent-event",
                SubAgentEventPayload {
                    session_id: self.session_id.clone(),
                    tool_call_id: self.parent_tool_call_id.clone(),
                    timestamp_ms,
                    event,
                },
            );
        }
    }

    /// 轨迹事件序列化（成功/失败都要持久化，故公开）。
    pub fn trace_events_json(&self) -> Result<String, String> {
        // 只在持锁期间 clone 出事件列表，序列化放到锁外（G13-08）。
        let events = self.trace_events.lock().clone();
        serde_json::to_string(&events)
            .map_err(|error| format!("错误：子智能体轨迹序列化失败：{error}"))
    }
}

/// 组装追加进历史的 assistant 消息（正文 + 工具调用）。
/// 思考链不回灌上下文（与主对话同一口径：瞬态产物，rig 的 openai 线格式
/// 会把 Reasoning 序列化进请求体，回灌只浪费预算）。
fn build_assistant_turn(visible_text: &str, tool_calls: &[ToolCall]) -> Message {
    use rig::message::{AssistantContent, Text};
    let mut content: Vec<AssistantContent> = Vec::new();
    if !visible_text.is_empty() {
        content.push(AssistantContent::Text(Text::new(visible_text)));
    }
    for call in tool_calls {
        content.push(AssistantContent::ToolCall(call.clone()));
    }
    Message::Assistant { id: None, content }
}

/// 构造工具结果消息（结果 / 重试提示 / 未执行说明共用）。
fn tool_result_message(call: &ToolCall, content: String) -> Message {
    Message::User {
        content: vec![UserContent::ToolResult(ToolResult {
            call: call.id.clone(),
            provider: call.provider.clone(),
            name: call.function.name.clone(),
            content: vec![ToolResultContent::text(content)],
        })],
    }
}

/// 结果截断（头尾各留一半），保证子智能体结果不撑爆父上下文。
fn truncate_tool_result(result: &str) -> String {
    let char_count = result.chars().count();
    if char_count <= SUB_AGENT_RESULT_MAX_CHARS {
        return result.to_string();
    }
    let keep = SUB_AGENT_RESULT_MAX_CHARS / 2;
    let head: String = result.chars().take(keep).collect();
    let tail: String = result.chars().skip(char_count - keep).collect();
    let dropped = char_count - SUB_AGENT_RESULT_MAX_CHARS;
    format!("{head}\n\n[...已截断 {dropped} 字符...]\n\n{tail}")
}

/// 流终态 choice → (正文, 思考, 工具调用)：`<think>` 标签正文拆入思考链。
fn split_choice(choice: &[rig::message::AssistantContent]) -> (String, String, Vec<ToolCall>) {
    use rig::message::AssistantContent;
    let mut text = String::new();
    let mut thinking = String::new();
    let mut tool_calls = Vec::new();
    for content in choice {
        match content {
            AssistantContent::Text(item) => text.push_str(&item.text),
            AssistantContent::Reasoning(reasoning) => {
                let display = reasoning.display_text();
                if !display.is_empty() {
                    if !thinking.is_empty() {
                        thinking.push('\n');
                    }
                    thinking.push_str(&display);
                }
            }
            AssistantContent::ToolCall(call) => tool_calls.push(call.clone()),
            AssistantContent::Image(_) => {}
        }
    }
    let (visible, tagged_thinking) = split_tagged_thinking(&text);
    if !tagged_thinking.trim().is_empty() {
        if !thinking.trim().is_empty() {
            thinking.push_str("\n\n");
        }
        thinking.push_str(tagged_thinking.trim());
    }
    (visible.trim().to_string(), thinking, tool_calls)
}

/// 把 `<think>…</think>` 块从正文拆到思考链（DeepSeek 等把思考混在 content
/// 里的方言；与 `rig_ext::r#loop::stream` 同一实现，Phase 5 归一）。
fn split_tagged_thinking(content: &str) -> (String, String) {
    let lower = content.to_ascii_lowercase();
    let mut visible = String::new();
    let mut thinking_blocks = Vec::new();
    let mut cursor = 0usize;

    while let Some(start_rel) = lower[cursor..].find("<think>") {
        let start = cursor + start_rel;
        let body_start = start + "<think>".len();
        let Some(end_rel) = lower[body_start..].find("</think>") else {
            break;
        };
        let end = body_start + end_rel;
        let tag_end = end + "</think>".len();

        visible.push_str(&content[cursor..start]);
        let thinking = content[body_start..end].trim();
        if !thinking.is_empty() {
            thinking_blocks.push(thinking.to_string());
        }
        cursor = tag_end;
    }

    visible.push_str(&content[cursor..]);
    (visible.trim().to_string(), thinking_blocks.join("\n\n"))
}

#[cfg(test)]
#[path = "runner/tests.rs"]
mod tests;
