//! 子智能体运行时（rig 形态）。
//!
//! 与旧实现（`agent/sub_agent/runtime.rs`）的差异只在「如何调模型/执行工具」：
//! 旧实现直接用 `OpenAiCompatProvider::chat_stream_with_thinking` + 旧
//! `ToolRegistry`/`ToolRuntime`，本实现消费 rig `CompletionModel::stream()` 的
//! `StreamedAssistantContent` 并对 `PortableDynamicTool` 面执行。行为语义逐条保留：
//!
//! - 独立执行上下文：内存消息历史（不落库），结果截断到
//!   `SUB_AGENT_RESULT_MAX_CHARS` 后返回父循环；
//! - 滑窗裁剪（`context_budget_chars`，容量源 = 模型库条目 contextWindow）；
//! - 单次请求超时 `SUB_AGENT_LLM_REQUEST_TIMEOUT_SECS` 与整体超时
//!   `config.timeout_secs`；整体超时向工具转发取消信号（协作式收敛），
//!   并对每次工具等待施加剩余预算硬边界；
//! - 工具失败重试升级（G13-05）：同名工具跨轮再次失败 → `force_final_response`
//!   （下一轮传空工具集，逼模型给最终结论；模型仍返回工具调用时不再执行，
//!   有文本即强制收口、无文本按错误退出）；
//! - 事件流与轨迹缓冲（`SubAgentEvent` + `sub-agent-event`）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rig::completion::Message;
use rig::message::{ToolCall, ToolResult, ToolResultContent, UserContent};
use rig::tool::{PortableDynamicTool, ToolErrorKind, ToolExecutionError, ToolOutput};
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;
use tokio::time::timeout;

use super::context::{context_budget_chars, trim_context_messages};
use super::events::{
    record_trace_event, SubAgentEvent, SubAgentEventPayload, SubAgentUsage,
    SUB_AGENT_TRACE_EVENT_LIMIT,
};
use super::tools::notify_user_progress_tool;
use crate::agent::rig_ext::model::{build_completion_request, completions_model, PurposeModelSpec};
use crate::agent::rig_ext::tools::deps::RigToolDeps;
use crate::agent::rig_ext::tools::run_record::prepare_arguments;
use crate::agent::rig_ext::tools::spec::ToolSpec;
use crate::agent::rig_ext::tools::MAX_TOOL_CALLS_PER_BATCH;
use crate::agent::rig_ext::tools::{exec::exec_tools, media::media_tools};
use crate::agent::sub_agent::config::SubAgentConfig;

/// 返回父循环前的结果截断上限。
const SUB_AGENT_RESULT_MAX_CHARS: usize = 32_000;
/// 单次模型请求超时（秒）。
const SUB_AGENT_LLM_REQUEST_TIMEOUT_SECS: u64 = 120;
/// 上下文裁剪保留的最近轮数上限（安全兜底；真正约束是字符预算）。
const SUB_AGENT_CONTEXT_KEEP_ROUNDS: usize = 200;
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

        Ok(Self {
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
        let mut tool_failure_rounds: HashMap<String, u32> = HashMap::new();
        let mut force_final_response = false;
        let mut last_iteration: u32 = 0;

        let outcome = self
            .run_loop(
                &model,
                &mut messages,
                &mut usage,
                start,
                deadline,
                &mut tool_failure_rounds,
                &mut force_final_response,
                &mut last_iteration,
            )
            .await;
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

    #[allow(clippy::too_many_arguments)]
    async fn run_loop(
        &self,
        model: &impl rig::completion::CompletionModel,
        messages: &mut Vec<Message>,
        usage: &mut SubAgentUsage,
        start: Instant,
        deadline: Instant,
        tool_failure_rounds: &mut HashMap<String, u32>,
        force_final_response: &mut bool,
        last_iteration: &mut u32,
    ) -> Result<String, String> {
        let definitions_all = self
            .surface
            .iter()
            .map(PortableDynamicTool::definition)
            .collect::<Vec<_>>();

        for iteration in 0..self.config.max_iterations {
            if Instant::now() >= deadline {
                return Err(format!(
                    "子智能体 '{}' 执行超时（{}秒）",
                    self.config.agent_id, self.config.timeout_secs
                ));
            }
            if self.cancelled() {
                return Err(format!("子智能体 '{}' 执行已取消", self.config.agent_id));
            }
            *last_iteration = iteration + 1;

            // 请求前滑动窗口裁剪（None = 无需裁剪，跳过 clone）。
            if let Some(trimmed) = trim_context_messages(
                messages,
                context_budget_chars(self.spec.context_window),
                SUB_AGENT_CONTEXT_KEEP_ROUNDS,
            ) {
                *messages = trimmed;
            }

            // 强制收口阶段传空工具集，逼模型给出最终结论。
            let definitions = if *force_final_response {
                Vec::new()
            } else {
                definitions_all.clone()
            };
            let request = build_completion_request(
                None,
                messages.clone(),
                definitions,
                Some(u64::from(self.config.max_output_tokens)),
                self.config.temperature,
                true,
            );

            let mut stream = match timeout(
                Duration::from_secs(SUB_AGENT_LLM_REQUEST_TIMEOUT_SECS),
                model.stream(request),
            )
            .await
            {
                Ok(Ok(stream)) => stream,
                Ok(Err(error)) => {
                    return Err(format!(
                        "子智能体 '{}' 模型请求失败：{error}",
                        self.config.agent_id
                    ))
                }
                Err(_) => {
                    return Err(format!(
                        "子智能体 '{}' 单次模型请求超时（{}秒）",
                        self.config.agent_id, SUB_AGENT_LLM_REQUEST_TIMEOUT_SECS
                    ))
                }
            };

            // 流式消费：正文增量 → llmDelta 事件；思考/工具增量忽略
            //（子智能体 UI 只展示正文增量，与旧实现一致）。
            {
                use futures::StreamExt;
                use rig::streaming::StreamedAssistantContent;
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(StreamedAssistantContent::Text(text)) => {
                            if !text.text.is_empty() {
                                self.emit(SubAgentEvent::LlmDelta {
                                    agent_id: self.config.agent_id.clone(),
                                    agent_name: self.config.agent_name.clone(),
                                    delta: text.text,
                                });
                            }
                        }
                        Ok(_) => {}
                        Err(error) => {
                            return Err(format!(
                                "子智能体 '{}' 流式响应失败：{error}",
                                self.config.agent_id
                            ))
                        }
                    }
                }
            }

            if let Some(final_record) = stream.response.as_ref() {
                if final_record.usage.has_values() {
                    usage.record(&final_record.usage);
                    self.emit(SubAgentEvent::UsageUpdated {
                        agent_id: self.config.agent_id.clone(),
                        agent_name: self.config.agent_name.clone(),
                        token_usage: usage.clone(),
                        elapsed_ms: start.elapsed().as_millis() as u64,
                    });
                }
            }

            let (visible_text, thinking, tool_calls) = split_choice(&stream.choice);

            // 无工具调用 ⇒ 模型给出最终答复；强制收口阶段即使仍返回工具调用，
            // 也绝不执行——有文本即收口，无文本按错误退出（G13-06）。
            let force_ignore_tool_calls = *force_final_response && !tool_calls.is_empty();
            if tool_calls.is_empty() || force_ignore_tool_calls {
                if force_ignore_tool_calls && visible_text.trim().is_empty() {
                    return Err(format!(
                        "子智能体 '{}' 在强制收口阶段仍返回工具调用且未提供文本结论，无法收口",
                        self.config.agent_id
                    ));
                }
                return Ok(visible_text);
            }

            if tool_calls.len() > MAX_TOOL_CALLS_PER_BATCH {
                return Err(format!(
                    "子智能体 '{}' 单轮返回 {} 个工具调用，超过运行时上限 {}，已拒绝执行",
                    self.config.agent_id,
                    tool_calls.len(),
                    MAX_TOOL_CALLS_PER_BATCH
                ));
            }

            messages.push(build_assistant_turn(&visible_text, &thinking, &tool_calls));

            self.execute_batch(
                &tool_calls,
                deadline,
                usage,
                tool_failure_rounds,
                force_final_response,
                messages,
            )
            .await?;
        }

        Err(format!(
            "子智能体 '{}' 达到最大迭代次数（{}）",
            self.config.agent_id, self.config.max_iterations
        ))
    }

    /// 执行一批工具调用：只读并发批 → 结果统一决策（重试 / 强制收口）→
    /// 写回工具结果消息。致命/取消结果立即向上抛错（run 收口）。
    #[allow(clippy::too_many_arguments)]
    async fn execute_batch(
        &self,
        tool_calls: &[ToolCall],
        deadline: Instant,
        usage: &mut SubAgentUsage,
        tool_failure_rounds: &mut HashMap<String, u32>,
        force_final_response: &mut bool,
        messages: &mut Vec<Message>,
    ) -> Result<(), String> {
        let mut index = 0usize;
        while index < tool_calls.len() {
            let readonly_end = self.readonly_run_end(tool_calls, index);
            let batch_len = readonly_end.saturating_sub(index);
            let executed: Vec<(&ToolCall, Result<ToolOutput, ToolExecutionError>)> =
                if batch_len >= 2 {
                    self.execute_parallel_readonly(&tool_calls[index..readonly_end], deadline)
                        .await
                } else {
                    let call = &tool_calls[index];
                    vec![(call, self.execute_single(call, deadline).await)]
                };
            let next_index = if batch_len >= 2 {
                readonly_end
            } else {
                index + 1
            };

            // 致命/取消结果立即收口（子智能体不再基于不完整的工具结果继续推理）。
            for (call, result) in &executed {
                if let Err(error) = result {
                    if error.kind() == ToolErrorKind::Cancelled {
                        return Err(format!(
                            "子智能体 '{}' 内部工具 '{}' 执行失败：{}",
                            self.config.agent_id,
                            call.function.name,
                            tool_error_text(error)
                        ));
                    }
                }
            }

            let failed_names = distinct_failed_tool_names(&executed);
            if failed_names.is_empty() {
                // 全部成功：写回结果（成功的工具清零失败记录，恢复重试资格）。
                for (call, result) in &executed {
                    if let Ok(output) = result {
                        tool_failure_rounds.remove(&call.function.name);
                        let text = truncate_tool_result(&tool_output_text(output));
                        messages.push(tool_result_message(call, text));
                    }
                }
                index = next_index;
                continue;
            }

            // 统一决策：任一失败工具已消耗重试资格（此前轮次已失败过）
            // ⇒ 升级强制收口；否则允许一次重试。
            let escalate = *force_final_response
                || failed_names
                    .iter()
                    .any(|name| tool_failure_rounds.get(name).copied().unwrap_or(0) >= 1);
            if escalate {
                *force_final_response = true;
            } else {
                for name in &failed_names {
                    *tool_failure_rounds.entry(name.clone()).or_insert(0) += 1;
                }
            }

            for (call, result) in &executed {
                match result {
                    Ok(output) => {
                        tool_failure_rounds.remove(&call.function.name);
                        let text = truncate_tool_result(&tool_output_text(output));
                        messages.push(tool_result_message(call, text));
                    }
                    Err(error) => {
                        let result_text = tool_error_text(error);
                        let hint = if escalate {
                            format!(
                                "错误：工具重试后仍然失败。\n错误信息：{result_text}\n\n要求：不要继续调用工具。请基于当前状态判断该错误是否无法修复；如果无法修复，请明确说明已尝试的动作、失败原因和退出结论。"
                            )
                        } else {
                            format!(
                                "错误：工具调用失败。\n错误信息：{result_text}\n\n要求：请根据工具 schema、上次参数和错误信息修正后重试；如果你判断无法修复，请不要猜测，直接说明无法修复并退出。"
                            )
                        };
                        messages.push(tool_result_message(call, hint));
                    }
                }
            }

            // 本轮剩余未执行工具：暂停，措辞跟随统一决策保持一致。
            for skipped in &tool_calls[next_index..] {
                let content = if escalate {
                    format!(
                        "未执行：工具重试后仍失败，本轮剩余工具已暂停，等待模型确认无法修复或给出最终结论。工具：{}",
                        skipped.function.name
                    )
                } else {
                    format!(
                        "未执行：前一个工具调用失败，已暂停本轮剩余工具调用。请先根据错误信息修正后重试。工具：{}",
                        skipped.function.name
                    )
                };
                messages.push(tool_result_message(skipped, content));
            }
            break;
        }
        let _ = usage; // 子智能体用量只统计模型请求（工具自身不回灌用量）
        Ok(())
    }

    /// 只读批边界：从 `start` 起连续「只读 + 可并行」的工具（策略表口径）。
    fn readonly_run_end(&self, tool_calls: &[ToolCall], start: usize) -> usize {
        let mut end = start;
        while end < tool_calls.len() {
            let name = &tool_calls[end].function.name;
            if !self.surface.iter().any(|tool| tool.name() == name) {
                break;
            }
            let spec = ToolSpec::new(name, "", serde_json::json!({}));
            if !spec.supports_parallel_readonly() {
                break;
            }
            end += 1;
        }
        end
    }

    /// 并行执行只读批（上限 `MAX_PARALLEL_TOOL_CALLS`）。
    async fn execute_parallel_readonly<'a>(
        &'a self,
        calls: &'a [ToolCall],
        deadline: Instant,
    ) -> Vec<(&'a ToolCall, Result<ToolOutput, ToolExecutionError>)> {
        use futures::future::join_all;

        for call in calls {
            self.emit_tool_started(call);
        }
        let semaphore = Arc::new(tokio::sync::Semaphore::new(
            crate::agent::rig_ext::tools::MAX_PARALLEL_TOOL_CALLS,
        ));
        let results = join_all(calls.iter().map(|call| {
            let semaphore = Arc::clone(&semaphore);
            async move {
                let Ok(_permit) = semaphore.acquire().await else {
                    return (
                        call,
                        Err(ToolExecutionError::other(
                            "错误：只读工具并发调度器意外关闭，已拒绝执行",
                        )),
                    );
                };
                (call, self.execute_single(call, deadline).await)
            }
        }))
        .await;
        for (call, result) in &results {
            self.emit_tool_finished(call, result);
        }
        results
    }

    /// 单工具执行：参数校验 → 剩余预算硬边界 → 工具回调。
    async fn execute_single(
        &self,
        call: &ToolCall,
        deadline: Instant,
    ) -> Result<ToolOutput, ToolExecutionError> {
        self.emit_tool_started(call);
        let result = self.execute_single_inner(call, deadline).await;
        self.emit_tool_finished(call, &result);
        result
    }

    async fn execute_single_inner(
        &self,
        call: &ToolCall,
        deadline: Instant,
    ) -> Result<ToolOutput, ToolExecutionError> {
        let Some(tool) = self
            .surface
            .iter()
            .find(|tool| tool.name() == call.function.name)
        else {
            return Err(ToolExecutionError::invalid_args(format!(
                "错误：未找到工具 '{}'",
                call.function.name
            )));
        };
        let definition = tool.definition();
        if let Err(error) = prepare_arguments(
            &call.function.name,
            &definition.parameters,
            &call.function.arguments,
        ) {
            return Err(ToolExecutionError::other(error.message).with_code(error.code.to_string()));
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ToolExecutionError::timeout(format!(
                "错误：工具 '{}' 达到子智能体 '{}' 的整体超时边界（{}秒）。",
                call.function.name, self.config.agent_id, self.config.timeout_secs
            )));
        }
        match timeout(remaining, tool.execute(call.function.arguments.clone())).await {
            Ok(result) => result,
            Err(_) => Err(ToolExecutionError::timeout(format!(
                "错误：工具 '{}' 达到子智能体 '{}' 的整体超时边界（{}秒）；已停止等待。",
                call.function.name, self.config.agent_id, self.config.timeout_secs
            ))),
        }
    }

    fn cancelled(&self) -> bool {
        self.parent_cancel
            .as_ref()
            .is_some_and(|rx| *rx.borrow() || rx.has_changed().is_err())
    }

    fn emit_tool_started(&self, call: &ToolCall) {
        self.emit(SubAgentEvent::ToolStarted {
            agent_id: self.config.agent_id.clone(),
            agent_name: self.config.agent_name.clone(),
            tool_name: call.function.name.clone(),
            arguments: call.function.arguments.clone(),
        });
    }

    fn emit_tool_finished(&self, call: &ToolCall, result: &Result<ToolOutput, ToolExecutionError>) {
        let text = match result {
            Ok(output) => tool_output_text(output),
            Err(error) => tool_error_text(error),
        };
        self.emit(SubAgentEvent::ToolFinished {
            agent_id: self.config.agent_id.clone(),
            agent_name: self.config.agent_name.clone(),
            tool_name: call.function.name.clone(),
            result_preview: tool_result_preview(&call.function.name, &text),
        });
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

/// 解析子智能体模型槽位：继承父级凭据/网关（仅换模型名）或使用独立配置；
/// 输出预算与温度取子智能体自身配置。
fn resolve_sub_agent_spec(config: &SubAgentConfig, parent: &PurposeModelSpec) -> PurposeModelSpec {
    let mut spec = parent.clone();
    let model_config = &config.model_config;
    if !model_config.inherit_from_parent {
        if let Some(api_base) = model_config.api_base.as_deref() {
            spec.api_base = api_base.to_string();
        }
        if let Some(api_key) = model_config.api_key.as_deref() {
            spec.api_key = api_key.to_string();
        }
    }
    if let Some(model) = model_config
        .model_name
        .as_deref()
        .filter(|name| !name.is_empty())
    {
        spec.model = model.to_string();
    }
    spec.max_tokens = Some(u64::from(config.max_output_tokens));
    spec.temperature = config.temperature;
    spec
}

/// 转发取消：父取消或整体超时 → 翻转 run 级取消通道（协作式收敛）。
async fn forward_cancellation(
    parent: Option<watch::Receiver<bool>>,
    tx: watch::Sender<bool>,
    overall_timeout: Duration,
) {
    let deadline = tokio::time::sleep(overall_timeout);
    tokio::pin!(deadline);
    match parent {
        Some(mut parent) => loop {
            tokio::select! {
                _ = &mut deadline => {
                    let _ = tx.send(true);
                    return;
                }
                changed = parent.changed() => {
                    match changed {
                        Ok(()) if *parent.borrow() => {
                            let _ = tx.send(true);
                            return;
                        }
                        Ok(()) => {}
                        Err(_) => {
                            let _ = tx.send(true);
                            return;
                        }
                    }
                }
            }
        },
        None => {
            deadline.await;
            let _ = tx.send(true);
        }
    }
}

/// 组装追加进历史的 assistant 消息（思考 + 正文 + 工具调用）。
fn build_assistant_turn(visible_text: &str, thinking: &str, tool_calls: &[ToolCall]) -> Message {
    use rig::message::{AssistantContent, Reasoning, Text};
    let mut content: Vec<AssistantContent> = Vec::new();
    if !thinking.trim().is_empty() {
        content.push(AssistantContent::Reasoning(Reasoning::new(thinking.trim())));
    }
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

/// 收集本批可恢复失败的工具名（按出现顺序去重）。
fn distinct_failed_tool_names(
    executed: &[(&ToolCall, Result<ToolOutput, ToolExecutionError>)],
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (call, result) in executed {
        if result.is_err() && !names.iter().any(|existing| existing == &call.function.name) {
            names.push(call.function.name.clone());
        }
    }
    names
}

/// 工具结果 → 回灌文本（与主循环同口径）。
fn tool_output_text(output: &ToolOutput) -> String {
    if let Some(text) = output.as_text() {
        return text.to_string();
    }
    output
        .as_content()
        .iter()
        .map(|content| match content {
            ToolResultContent::Text(text) => text.text.clone(),
            ToolResultContent::Json { value } => value.to_string(),
            ToolResultContent::Image(_) => "[图片结果]".to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 工具执行错误 → 模型可见文本（保持「错误：」前缀契约）。
fn tool_error_text(error: &ToolExecutionError) -> String {
    let text = tool_output_text(error.model_output());
    let trimmed = text.trim();
    if trimmed.is_empty() {
        "错误：工具执行失败".to_string()
    } else if trimmed.starts_with("错误：") {
        trimmed.to_string()
    } else {
        format!("错误：{trimmed}")
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

/// 事件结果预览：命令审查结果类保留更长预览（便于排障），其余 200 字符。
fn tool_result_preview(tool_name: &str, result: &str) -> String {
    let preview_limit = if is_command_review_result(tool_name, result) {
        4_000
    } else {
        200
    };
    if result.chars().count() > preview_limit {
        format!(
            "{}...",
            result.chars().take(preview_limit).collect::<String>()
        )
    } else {
        result.to_string()
    }
}

fn is_command_review_result(tool_name: &str, result: &str) -> bool {
    matches!(tool_name, "ssh_exec" | "local_zsh")
        && (result.starts_with("## SSH 命令审查记录")
            || (result.starts_with("## local_zsh 执行结果") && result.contains("审查结论: `拦截`")))
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
mod tests {
    use super::*;

    /// 夹具：临时库 + 最小工具依赖（只构造不执行）。
    fn test_deps(temp_dir: &std::path::Path) -> RigToolDeps {
        let db = crate::agent::db::DispatcherDb::new(temp_dir.join("jkbot.sqlite3"))
            .expect("open temp db");
        let mut deps = RigToolDeps {
            workspace_id: "sub-agent-test".to_string(),
            workspace: temp_dir.to_path_buf(),
            mcp_scope: crate::mcp::McpScope::Global,
            exec_timeout_secs: 30,
            restrict_to_workspace: true,
            extra_allowed_dirs: Vec::new(),
            app_handle: None,
            db: db.clone(),
            ssh_manager: crate::ssh_tool::SshSessionManager::new(db.pool()),
            mcp_registry: crate::mcp::McpRegistry::new(db),
            sub_agent_manager: None,
            cancel_rx: None,
            vision_spec: None,
            image: crate::agent::rig_ext::tools::deps::ImageToolConfig {
                url: String::new(),
                api_key: String::new(),
                model: String::new(),
                edit_model: String::new(),
            },
            review: crate::agent::rig_ext::review::RigReviewContext::unconfigured(),
            tool_call_id: crate::agent::rig_ext::tools::deps::ToolCallSlot::default(),
        };
        deps.review.executor_task = None;
        deps
    }

    fn config(allowed_tools: Vec<&str>) -> crate::agent::sub_agent::config::SubAgentConfig {
        crate::agent::sub_agent::config::SubAgentConfig {
            agent_id: "a1".to_string(),
            agent_name: "测试子智能体".to_string(),
            description: "测试".to_string(),
            system_prompt: "你是测试子智能体。".to_string(),
            user_prompt_template: "任务：{{task}}".to_string(),
            allowed_tools: allowed_tools.into_iter().map(str::to_string).collect(),
            model_config: Default::default(),
            max_iterations: 2,
            max_output_tokens: 128,
            temperature: 0.0,
            timeout_secs: 30,
            enabled: true,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn surface_offers_progress_tool_and_rejects_nested_sub_agents() {
        let temp_dir = std::env::temp_dir().join(format!("rig-sub-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).expect("create temp dir");
        let deps = test_deps(&temp_dir);
        let parent_spec = PurposeModelSpec {
            api_key: "test".to_string(),
            api_base: "http://127.0.0.1:1/v1".to_string(),
            model: "mock".to_string(),
            max_tokens: None,
            context_window: None,
            temperature: 0.0,
            enable_thinking: true,
        };
        let cfg = config(vec!["local_zsh", "notify_user_progress"]);
        let runtime = RigSubAgentRuntime::build(&RigSubAgentRequest {
            config: &cfg,
            parent_spec: &parent_spec,
            deps: &deps,
            task: "跑一下",
            parent_tool_call_id: "call-1",
            app_handle: None,
            session_id: "ws-1",
            cancel_rx: None,
        })
        .expect("构建应成功");
        let mut names = runtime
            .surface
            .iter()
            .map(|tool| tool.name().to_string())
            .collect::<Vec<_>>();
        names.sort();
        // 允许列表精确生效：只有显式列出的两个工具。
        assert_eq!(names, vec!["local_zsh", "notify_user_progress"]);

        // 嵌套子智能体工具一律拒绝（防递归派生）。
        let nested = config(vec!["call_sub_agent"]);
        let error = RigSubAgentRuntime::build(&RigSubAgentRequest {
            config: &nested,
            parent_spec: &parent_spec,
            deps: &deps,
            task: "跑一下",
            parent_tool_call_id: "call-1",
            app_handle: None,
            session_id: "ws-1",
            cancel_rx: None,
        })
        .err()
        .expect("嵌套子智能体工具必须被拒绝");
        assert!(error.contains("不允许递归调用子智能体工具"), "{error}");

        // 不可用工具名（编排器专属）在构建期报错，而不是运行期静默缺失。
        let unavailable = config(vec!["submit_graph"]);
        let error = RigSubAgentRuntime::build(&RigSubAgentRequest {
            config: &unavailable,
            parent_spec: &parent_spec,
            deps: &deps,
            task: "跑一下",
            parent_tool_call_id: "call-1",
            app_handle: None,
            session_id: "ws-1",
            cancel_rx: None,
        })
        .err()
        .expect("编排器工具对子智能体不可用");
        assert!(error.contains("不可用的工具"), "{error}");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn truncation_keeps_head_and_tail() {
        let long = "a".repeat(SUB_AGENT_RESULT_MAX_CHARS + 100);
        let truncated = truncate_tool_result(&long);
        assert!(truncated.starts_with(&"a".repeat(64)));
        assert!(truncated.contains("已截断 100 字符"));
    }

    #[test]
    fn command_review_results_keep_longer_previews() {
        let review = format!("## SSH 命令审查记录\n{}", "x".repeat(1_000));
        // 命令审查结果在 4000 字符内全量保留（便于排障），普通工具 200 字符后截断。
        assert_eq!(
            tool_result_preview("ssh_exec", &review).chars().count(),
            review.chars().count()
        );
        assert_eq!(
            tool_result_preview("read_file", &"y".repeat(500))
                .chars()
                .count(),
            203
        );
    }

    #[test]
    fn tagged_thinking_is_split_into_reasoning() {
        let (visible, thinking) = split_tagged_thinking("前<think>推理</think>后");
        assert_eq!(visible, "前后");
        assert_eq!(thinking, "推理");
    }
}
