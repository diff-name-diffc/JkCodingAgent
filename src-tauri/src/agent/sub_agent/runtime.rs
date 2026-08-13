use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use futures::future::join_all;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;
use tauri::Emitter;
use tokio::sync::watch;
use tokio::time::timeout;

use super::config::SubAgentConfig;
use crate::agent::llm::{
    ChatMessage, FunctionCall, LlmUsage, OpenAiCompatProvider, OutboundToolCall, RequestedToolCall,
    ToolDefinition,
};
use crate::agent::tools::{ToolContext, ToolRegistry, ToolResult, ToolRuntime, ToolStatus};

const SUB_AGENT_RESULT_MAX_CHARS: usize = 32_000;
const SUB_AGENT_LLM_REQUEST_TIMEOUT_SECS: u64 = 120;
const NESTED_SUB_AGENT_TOOLS: &[&str] = &["call_sub_agent", "list_sub_agents"];

/// 追踪事件缓冲容量上限（G1-20）：长任务中事件可能积累数千条，
/// 超限后丢弃最旧事件并以 traceTruncated 标记事件记录累计丢弃数，
/// 保证缓冲有界且丢弃行为对消费方可见。
const SUB_AGENT_TRACE_EVENT_LIMIT: usize = 500;
const TRACE_TRUNCATED_EVENT: &str = "traceTruncated";

/// 上下文裁剪参数（G13-07）：消息历史按「轮次」滑动窗口裁剪——
/// 保留 system + 首轮 user + 最近若干轮；总字符数超过上限时进一步
/// 收紧窗口。单条工具结果已有 SUB_AGENT_RESULT_MAX_CHARS 截断，
/// 两者协同约束上下文总量。
const SUB_AGENT_CONTEXT_MAX_CHARS: usize = 120_000;
const SUB_AGENT_CONTEXT_KEEP_ROUNDS: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl Default for SubAgentUsage {
    fn default() -> Self {
        Self {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        }
    }
}

impl SubAgentUsage {
    fn record(&mut self, usage: &LlmUsage) {
        self.prompt_tokens += usage.prompt_tokens;
        self.completion_tokens += usage.completion_tokens;
        self.total_tokens += if usage.total_tokens > 0 {
            usage.total_tokens
        } else {
            usage.prompt_tokens + usage.completion_tokens
        };
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum SubAgentEvent {
    Started {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "agentName")]
        agent_name: String,
        task: String,
    },
    ToolStarted {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "agentName")]
        agent_name: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        arguments: Value,
    },
    ToolFinished {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "agentName")]
        agent_name: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        #[serde(rename = "resultPreview")]
        result_preview: String,
    },
    Progress {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "agentName")]
        agent_name: String,
        message: String,
    },
    #[serde(rename = "llmDelta")]
    LlmDelta {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "agentName")]
        agent_name: String,
        delta: String,
    },
    #[serde(rename = "UsageUpdated")]
    UsageUpdated {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "agentName")]
        agent_name: String,
        #[serde(rename = "tokenUsage")]
        token_usage: SubAgentUsage,
        #[serde(rename = "elapsedMs")]
        elapsed_ms: u64,
    },
    Finished {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "agentName")]
        agent_name: String,
        result: String,
        iterations: u32,
        #[serde(rename = "elapsedMs")]
        elapsed_ms: u64,
        #[serde(rename = "tokenUsage")]
        token_usage: SubAgentUsage,
    },
    Failed {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "agentName")]
        agent_name: String,
        error: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct SubAgentEventPayload {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "toolCallId")]
    pub tool_call_id: String,
    #[serde(rename = "timestampMs")]
    pub timestamp_ms: i64,
    #[serde(flatten)]
    pub event: SubAgentEvent,
}

/// 子智能体独立执行运行时。
///
/// 与主编排 Agent 的区别：无图编排、无计划/checklist、无协议动作，
/// 是一个纯粹的"LLM ↔ 工具"循环。支持工具错误重试（一次重试后强制收口）和
/// 超时控制。结果会截断到 SUB_AGENT_RESULT_MAX_CHARS 后返回给父循环。
pub struct SubAgentRuntime {
    config: SubAgentConfig,
    provider: OpenAiCompatProvider,
    tool_registry: Arc<ToolRegistry>,
    tool_context: ToolContext,
    tool_definitions: Vec<crate::agent::llm::ToolDefinition>,
    allowed_tool_names: HashSet<String>,
    parent_tool_call_id: String,
    trace_events: Arc<Mutex<Vec<Value>>>,
}

impl SubAgentRuntime {
    /// 构建子智能体运行时。provider 可继承父级配置或使用子智能体独立配置；
    /// 关键约束：排除嵌套的 call_sub_agent / list_sub_agents 工具，防止子智能体
    /// 递归派生导致无限调用栈。
    pub fn build(
        config: &SubAgentConfig,
        parent_provider: &OpenAiCompatProvider,
        tool_registry: Arc<ToolRegistry>,
        tool_context: ToolContext,
    ) -> Result<Self> {
        let provider = if config.model_config.inherit_from_parent {
            let model_name = config
                .model_config
                .model_name
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(parent_provider.model());
            parent_provider.with_model(model_name)
        } else {
            let api_base = config
                .model_config
                .api_base
                .as_deref()
                .unwrap_or(parent_provider.api_base());
            let api_key = config
                .model_config
                .api_key
                .as_deref()
                .unwrap_or(parent_provider.api_key());
            let model_name = config
                .model_config
                .model_name
                .as_deref()
                .unwrap_or(parent_provider.model());
            OpenAiCompatProvider::new(
                api_key.to_string(),
                api_base.to_string(),
                model_name.to_string(),
                config.max_output_tokens,
                config.temperature as f32,
            )
        };

        let mut tool_context = tool_context;
        let parent_tool_call_id = tool_context
            .current_tool_call_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("构建子智能体运行时缺少父级 tool_call_id"))?;
        // G1-20：缓冲容量治理在写入端（record_trace_event）强制执行；
        // 这里按上限预分配，避免运行期反复扩容。
        let trace_events = Arc::new(Mutex::new(Vec::with_capacity(SUB_AGENT_TRACE_EVENT_LIMIT)));
        tool_context.current_sub_agent_id = Some(config.agent_id.clone());
        tool_context.current_sub_agent_name = Some(config.agent_name.clone());
        tool_context.sub_agent_parent_tool_call_id = Some(parent_tool_call_id.clone());
        tool_context.sub_agent_trace_events = Some(Arc::clone(&trace_events));

        // 排除嵌套子智能体工具（call_sub_agent / list_sub_agents），避免递归派生。
        let excluded: HashSet<&str> = NESTED_SUB_AGENT_TOOLS.iter().copied().collect();
        let allowed_tool_names: HashSet<String> = config
            .allowed_tools
            .iter()
            .filter(|s| !excluded.contains(s.as_str()))
            .cloned()
            .collect();

        let tool_definitions = tool_registry.definitions_for_workspace(
            &tool_context.workspace,
            Some(allowed_tool_names.iter().map(String::as_str)),
            false,
        );

        Ok(Self {
            config: config.clone(),
            provider,
            tool_registry,
            tool_context,
            tool_definitions,
            allowed_tool_names,
            parent_tool_call_id,
            trace_events,
        })
    }

    pub fn trace_events_json(&self) -> Result<String> {
        // G13-08：只在持锁期间 clone 出事件列表，序列化放到锁外执行，
        // 避免长任务下对数千条事件做 serde_json::to_string 时长时间持锁，
        // 阻塞 emit_event / record_trace_event 等所有写入路径。
        let events = self.trace_events.lock().clone();
        serde_json::to_string(&events)
            .map_err(|error| anyhow::anyhow!("serialize sub-agent trace events: {error}"))
    }

    fn emit_event(&self, app_handle: &Option<AppHandle>, session_id: &str, event: SubAgentEvent) {
        let timestamp_ms = chrono::Utc::now().timestamp_millis();
        if let Ok(value) = serde_json::to_value(&event) {
            record_trace_event(&self.trace_events, value, timestamp_ms);
        }
        if let Some(handle) = app_handle {
            let _ = handle.emit(
                "sub-agent-event",
                SubAgentEventPayload {
                    session_id: session_id.to_string(),
                    tool_call_id: self.parent_tool_call_id.clone(),
                    timestamp_ms,
                    event,
                },
            );
        }
    }

    /// 子智能体主执行循环：重复"请求 LLM → 执行工具 → 判断收口"。
    /// 收口条件：模型返回无工具调用的最终答复，或超时，或达到最大迭代次数。
    /// 工具错误处理：首次失败允许重试，再次失败则强制要求模型给出最终结论（force_final_response）。
    pub async fn execute(
        &self,
        task: &str,
        app_handle: Option<AppHandle>,
        session_id: &str,
    ) -> Result<String> {
        self.execute_with_cancellation(task, app_handle, session_id, None)
            .await
    }

    /// 带取消信号的变体：图运行器在迭代边界检查取消标志，
    /// 命中后子智能体在当前迭代结束后退出（不强行打断进行中的单次请求/工具）。
    pub async fn execute_with_cancellation(
        &self,
        task: &str,
        app_handle: Option<AppHandle>,
        session_id: &str,
        cancel_rx: Option<watch::Receiver<bool>>,
    ) -> Result<String> {
        let start = Instant::now();
        let overall_timeout = Duration::from_secs(self.config.timeout_secs);
        let llm_request_timeout = Duration::from_secs(SUB_AGENT_LLM_REQUEST_TIMEOUT_SECS);
        let mut usage = SubAgentUsage::default();
        // G13-05：按工具名记录「已消耗重试资格的失败轮数」，替代原先的
        // 全局一次性标志 tool_error_seen：
        // - 同名工具跨轮再次失败才视为「重试后仍失败」并升级强制收口；
        // - 同一轮内同名工具多次失败只计一轮（并行批次不误伤）；
        // - 该工具一旦成功即清零，不会被误当成「已重试过」。
        let mut tool_failure_rounds: HashMap<String, u32> = HashMap::new();
        let mut force_final_response = false;
        let no_tools: Vec<ToolDefinition> = Vec::new();
        #[allow(unused_assignments)]
        let mut last_iteration: u32 = 0;

        self.trace_events.lock().clear();
        self.emit_event(
            &app_handle,
            session_id,
            SubAgentEvent::Started {
                agent_id: self.config.agent_id.clone(),
                agent_name: self.config.agent_name.clone(),
                task: task.to_string(),
            },
        );

        let user_prompt = self.config.user_prompt_template.replace("{{task}}", task);

        let mut messages = vec![
            ChatMessage::system(self.config.system_prompt.clone()),
            ChatMessage {
                role: "user".to_string(),
                content: user_prompt,
                content_parts: Vec::new(),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ];

        for iteration in 0..self.config.max_iterations {
            if start.elapsed() > overall_timeout {
                let err_msg = format!(
                    "子智能体 '{}' 执行超时（{}秒）",
                    self.config.agent_id, self.config.timeout_secs
                );
                self.emit_failed(&app_handle, session_id, &err_msg);
                anyhow::bail!("{}", err_msg);
            }

            if cancel_rx
                .as_ref()
                .is_some_and(|rx| *rx.borrow())
            {
                let err_msg = format!("子智能体 '{}' 执行已取消", self.config.agent_id);
                self.emit_failed(&app_handle, session_id, &err_msg);
                anyhow::bail!("{}", err_msg);
            }

            last_iteration = iteration + 1;

            let on_delta = |delta: &str| {
                self.emit_event(
                    &app_handle,
                    session_id,
                    SubAgentEvent::LlmDelta {
                        agent_id: self.config.agent_id.clone(),
                        agent_name: self.config.agent_name.clone(),
                        delta: delta.to_string(),
                    },
                );
            };

            // G13-07：请求前对消息历史做滑动窗口裁剪，防止上下文随迭代无界增长。
            // 返回 None 表示无需裁剪，避免对已很大的历史做全量 clone。
            if let Some(trimmed) = trim_context_messages(
                &messages,
                SUB_AGENT_CONTEXT_MAX_CHARS,
                SUB_AGENT_CONTEXT_KEEP_ROUNDS,
            ) {
                messages = trimmed;
            }

            // 若已触发强制收口（工具重试后仍失败），则传入空工具集，逼模型给出最终结论。
            let active_tool_definitions = if force_final_response {
                &no_tools
            } else {
                &self.tool_definitions
            };

            let llm_future = self.provider.chat_stream_with_thinking(
                &messages,
                active_tool_definitions,
                false,
                on_delta,
                |_delta: &str, _elapsed: u64| {},
            );

            let response = match timeout(llm_request_timeout, llm_future).await {
                Ok(Ok(response)) => response,
                Ok(Err(error)) => {
                    let err_msg = format!(
                        "子智能体 '{}' 模型请求失败：{}",
                        self.config.agent_id, error
                    );
                    self.emit_failed(&app_handle, session_id, &err_msg);
                    anyhow::bail!("{}", err_msg);
                }
                Err(_) => {
                    let err_msg = format!(
                        "子智能体 '{}' 单次模型请求超时（{}秒）",
                        self.config.agent_id, SUB_AGENT_LLM_REQUEST_TIMEOUT_SECS
                    );
                    self.emit_failed(&app_handle, session_id, &err_msg);
                    anyhow::bail!("{}", err_msg);
                }
            };

            if let Some(usage_info) = response.usage.as_ref() {
                usage.record(usage_info);
                self.emit_event(
                    &app_handle,
                    session_id,
                    SubAgentEvent::UsageUpdated {
                        agent_id: self.config.agent_id.clone(),
                        agent_name: self.config.agent_name.clone(),
                        token_usage: usage.clone(),
                        elapsed_ms: start.elapsed().as_millis() as u64,
                    },
                );
            }

            // 无工具调用 ⇒ 模型给出最终答复，子智能体正常收口。
            // G13-06：force_final_response 置位后即使模型不遵守约束仍返回
            // tool_calls，也绝不执行这些工具——直接以当前 content 强制收口；
            // content 为空则按错误退出，避免静默产出空结果或继续消耗预算。
            let force_ignore_tool_calls = force_final_response && !response.tool_calls.is_empty();
            if response.tool_calls.is_empty() || force_ignore_tool_calls {
                if force_ignore_tool_calls && response.content.trim().is_empty() {
                    let err_msg = format!(
                        "子智能体 '{}' 在强制收口阶段仍返回工具调用且未提供文本结论，无法收口",
                        self.config.agent_id
                    );
                    self.emit_failed(&app_handle, session_id, &err_msg);
                    anyhow::bail!("{}", err_msg);
                }
                let result = response.content.clone();
                self.emit_event(
                    &app_handle,
                    session_id,
                    SubAgentEvent::Finished {
                        agent_id: self.config.agent_id.clone(),
                        agent_name: self.config.agent_name.clone(),
                        result: result.clone(),
                        iterations: last_iteration,
                        elapsed_ms: start.elapsed().as_millis() as u64,
                        token_usage: usage,
                    },
                );
                return Ok(result);
            }

            let assistant_msg = ChatMessage {
                role: "assistant".to_string(),
                content: response.content.clone(),
                content_parts: Vec::new(),
                reasoning_content: if response.thinking_content.is_empty() {
                    None
                } else {
                    Some(response.thinking_content.clone())
                },
                tool_calls: Some(
                    response
                        .tool_calls
                        .iter()
                        .map(|tc| OutboundToolCall {
                            id: tc.id.clone(),
                            kind: "function".to_string(),
                            function: FunctionCall {
                                name: tc.name.clone(),
                                arguments: serde_json::to_string(&tc.arguments).unwrap_or_default(),
                            },
                        })
                        .collect(),
                ),
                tool_call_id: None,
                name: None,
            };
            messages.push(assistant_msg);

            let tool_calls = response.tool_calls;
            let mut tc_index = 0usize;

            while tc_index < tool_calls.len() {
                let readonly_end = readonly_tool_run_end(
                    &self.tool_registry,
                    &self.tool_context.workspace,
                    &tool_calls,
                    tc_index,
                );

                // G13-04：把整体超时预算传入工具执行层，保证每次工具等待有界。
                let executed: Vec<(&RequestedToolCall, ToolResult)> = if readonly_end
                    .saturating_sub(tc_index)
                    >= 2
                {
                    let run = &tool_calls[tc_index..readonly_end];
                    self.execute_parallel_readonly_tools(
                        run,
                        &app_handle,
                        session_id,
                        &mut usage,
                        start,
                        overall_timeout,
                    )
                    .await
                } else {
                    let tc = &tool_calls[tc_index];
                    let result = self
                        .execute_single_tool(
                            tc,
                            &app_handle,
                            session_id,
                            &mut usage,
                            start,
                            overall_timeout,
                        )
                        .await;
                    vec![(tc, result)]
                };

                let next_index = if readonly_end.saturating_sub(tc_index) >= 2 {
                    readonly_end
                } else {
                    tc_index + 1
                };

                // 致命错误优先处理：任何致命/取消结果立即收口退出。
                for (tc, result) in &executed {
                    if matches!(result.status, ToolStatus::FatalError | ToolStatus::Cancelled) {
                        let err_msg = format!(
                            "子智能体 '{}' 内部工具 '{}' 执行失败：{}",
                            self.config.agent_id,
                            tc.name,
                            result.output_for_llm()
                        );
                        self.emit_failed(&app_handle, session_id, &err_msg);
                        anyhow::bail!("{}", err_msg);
                    }
                }

                // G13-05：先收集本组全部结果，再统一决定「重试」还是「强制收口」，
                // 消除同一轮内既要求重试又要求停止调用的矛盾指令。
                let failed_tool_names = distinct_failed_tool_names(&executed);

                if failed_tool_names.is_empty() {
                    // 全部成功：照常写入上下文；成功的工具清零失败记录，
                    // 恢复其重试资格（G13-05：标志只置位不复位的修复）。
                    for (tc, result) in &executed {
                        tool_failure_rounds.remove(&tc.name);
                        let truncated = truncate_tool_result(&result.output_for_llm());
                        messages.push(tool_result_message(tc, truncated));
                    }
                } else {
                    // 统一决策：任一失败工具已消耗重试资格（此前轮次已失败过）
                    // ⇒ 升级强制收口；否则允许一次重试。
                    let escalate = force_final_response
                        || should_escalate_tool_failures(
                            &failed_tool_names,
                            &tool_failure_rounds,
                        );
                    if escalate {
                        force_final_response = true;
                    } else {
                        for name in &failed_tool_names {
                            *tool_failure_rounds.entry(name.clone()).or_insert(0) += 1;
                        }
                    }

                    // 按统一决策写消息：失败工具一律采用同一措辞（G13-09：
                    // 重试/收口提示统一以「错误：」开头，与 ToolRuntime 错误语义一致）。
                    for (tc, result) in &executed {
                        let result_text = result.output_for_llm();
                        match result.status {
                            ToolStatus::Success => {
                                tool_failure_rounds.remove(&tc.name);
                                let truncated = truncate_tool_result(&result_text);
                                messages.push(tool_result_message(tc, truncated));
                            }
                            ToolStatus::RecoverableError => {
                                let hint = if escalate {
                                    format!(
                                        "错误：工具重试后仍然失败。\n错误信息：{result_text}\n\n要求：不要继续调用工具。请基于当前状态判断该错误是否无法修复；如果无法修复，请明确说明已尝试的动作、失败原因和退出结论。"
                                    )
                                } else {
                                    format!(
                                        "错误：工具调用失败。\n错误信息：{result_text}\n\n要求：请根据工具 schema、上次参数和错误信息修正后重试；如果你判断无法修复，请不要猜测，直接说明无法修复并退出。"
                                    )
                                };
                                messages.push(tool_result_message(tc, hint));
                            }
                            ToolStatus::FatalError | ToolStatus::Cancelled => {
                                // 已在上方致命分支处理，此处不可达。
                            }
                        }
                    }

                    // 本轮剩余未执行工具：暂停，措辞跟随统一决策保持一致。
                    for skipped in &tool_calls[next_index..] {
                        let content = if escalate {
                            format!(
                                "未执行：工具重试后仍失败，本轮剩余工具已暂停，等待模型确认无法修复或给出最终结论。工具：{}",
                                skipped.name
                            )
                        } else {
                            format!(
                                "未执行：前一个工具调用失败，已暂停本轮剩余工具调用。请先根据错误信息修正后重试。工具：{}",
                                skipped.name
                            )
                        };
                        messages.push(tool_result_message(skipped, content));
                    }
                    break;
                }

                tc_index = next_index;
            }
        }

        let err_msg = format!(
            "子智能体 '{}' 达到最大迭代次数（{}）",
            self.config.agent_id, self.config.max_iterations
        );
        self.emit_failed(&app_handle, session_id, &err_msg);
        anyhow::bail!("{}", err_msg)
    }

    async fn execute_single_tool(
        &self,
        tc: &RequestedToolCall,
        app_handle: &Option<AppHandle>,
        session_id: &str,
        usage: &mut SubAgentUsage,
        started_at: Instant,
        overall_timeout: Duration,
    ) -> ToolResult {
        let _ = usage;
        self.emit_event(
            app_handle,
            session_id,
            SubAgentEvent::ToolStarted {
                agent_id: self.config.agent_id.clone(),
                agent_name: self.config.agent_name.clone(),
                tool_name: tc.name.clone(),
                arguments: tc.arguments.clone(),
            },
        );

        let result = self
            .execute_tool_with_budget(tc, started_at, overall_timeout)
            .await;

        let result_text = result.output_for_llm();
        let result_preview = tool_result_preview(&tc.name, &result_text);

        self.emit_event(
            app_handle,
            session_id,
            SubAgentEvent::ToolFinished {
                agent_id: self.config.agent_id.clone(),
                agent_name: self.config.agent_name.clone(),
                tool_name: tc.name.clone(),
                result_preview,
            },
        );

        result
    }

    /// G13-04：子智能体层面的工具执行看门狗——以整体剩余时间为上限包裹
    /// `ToolRuntime::execute_tool`，保证单次工具等待绝不越过整体超时预算。
    ///
    /// 可终止性评估：`tokio::time::timeout` 超时后仅中止「等待」（drop future）。
    /// 若工具内部是阻塞 I/O、`spawn_blocking` 任务或子进程，底层工作仍会继续运行
    /// 并可能产生副作用——真正终止它们需要工具层（agent/tools/）支持
    /// CancellationToken / 显式 kill（见报告「跨界依赖」）。因此这里按
    /// fail-closed 处理：超限时返回显式致命错误，既让本循环立即收口，
    /// 也把「底层任务可能仍在运行」如实上报给父循环与前端。
    async fn execute_tool_with_budget(
        &self,
        tc: &RequestedToolCall,
        started_at: Instant,
        overall_timeout: Duration,
    ) -> ToolResult {
        let remaining = overall_timeout.saturating_sub(started_at.elapsed());
        match timeout(
            remaining,
            ToolRuntime::execute_tool(
                &self.tool_registry,
                &self.tool_context.workspace,
                &self.allowed_tool_names,
                tc,
                &self.tool_context,
            ),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => ToolResult::fatal_error(format!(
                "错误：工具 '{}' 执行达到子智能体 '{}' 的整体超时边界（{}秒），已中止等待；底层任务（阻塞 I/O / 子进程）可能仍在运行。",
                tc.name, self.config.agent_id, self.config.timeout_secs
            )),
        }
    }

    async fn execute_parallel_readonly_tools<'a>(
        &'a self,
        tool_calls: &'a [RequestedToolCall],
        app_handle: &'a Option<AppHandle>,
        session_id: &'a str,
        usage: &mut SubAgentUsage,
        started_at: Instant,
        overall_timeout: Duration,
    ) -> Vec<(&'a RequestedToolCall, ToolResult)> {
        let _ = usage;
        for tc in tool_calls {
            self.emit_event(
                app_handle,
                session_id,
                SubAgentEvent::ToolStarted {
                    agent_id: self.config.agent_id.clone(),
                    agent_name: self.config.agent_name.clone(),
                    tool_name: tc.name.clone(),
                    arguments: tc.arguments.clone(),
                },
            );
        }

        // G13-04：并行批次共享同一份整体剩余预算；超时后整批按致命错误收口。
        // 底层任务的可终止性限制与单工具路径相同（见 execute_tool_with_budget 注释）。
        let remaining = overall_timeout.saturating_sub(started_at.elapsed());
        let results = match timeout(
            remaining,
            join_all(
                tool_calls
                    .iter()
                    .map(|tc| async move {
                        (
                            tc,
                            ToolRuntime::execute_tool(
                                &self.tool_registry,
                                &self.tool_context.workspace,
                                &self.allowed_tool_names,
                                tc,
                                &self.tool_context,
                            )
                            .await,
                        )
                    })
                    .collect::<Vec<_>>(),
            ),
        )
        .await
        {
            Ok(results) => results,
            Err(_) => tool_calls
                .iter()
                .map(|tc| {
                    (
                        tc,
                        ToolResult::fatal_error(format!(
                            "错误：只读工具 '{}' 所在并行批次达到子智能体 '{}' 的整体超时边界（{}秒），已中止等待；底层任务可能仍在运行。",
                            tc.name, self.config.agent_id, self.config.timeout_secs
                        )),
                    )
                })
                .collect(),
        };

        for (tc, result) in &results {
            let result_text = result.output_for_llm();
            let result_preview = tool_result_preview(&tc.name, &result_text);
            self.emit_event(
                app_handle,
                session_id,
                SubAgentEvent::ToolFinished {
                    agent_id: self.config.agent_id.clone(),
                    agent_name: self.config.agent_name.clone(),
                    tool_name: tc.name.clone(),
                    result_preview,
                },
            );
        }

        results
    }

    fn emit_failed(&self, app_handle: &Option<AppHandle>, session_id: &str, error: &str) {
        self.emit_event(
            app_handle,
            session_id,
            SubAgentEvent::Failed {
                agent_id: self.config.agent_id.clone(),
                agent_name: self.config.agent_name.clone(),
                error: error.to_string(),
            },
        );
    }
}

pub(super) fn record_trace_event(
    trace: &Arc<Mutex<Vec<Value>>>,
    mut event: Value,
    timestamp_ms: i64,
) {
    if let Some(object) = event.as_object_mut() {
        object.insert("timestampMs".to_string(), Value::from(timestamp_ms));
    }
    let mut events = trace.lock();
    let is_delta = event.get("event").and_then(Value::as_str) == Some("llmDelta");
    if is_delta {
        let incoming = event
            .get("data")
            .and_then(|data| data.get("delta"))
            .and_then(Value::as_str);
        if let (Some(delta), Some(last)) = (incoming, events.last_mut()) {
            if last.get("event").and_then(Value::as_str) == Some("llmDelta") {
                let existing = last
                    .get_mut("data")
                    .and_then(|data| data.get_mut("delta"))
                    .and_then(|value| value.as_str())
                    .map(str::to_owned);
                if let Some(existing) = existing {
                    let merged = format!("{existing}{delta}");
                    if let Some(slot) = last.get_mut("data").and_then(|data| data.get_mut("delta"))
                    {
                        *slot = Value::String(merged);
                        return;
                    }
                }
            }
        }
    }
    events.push(event);
    // G1-20：插入时强制执行容量上限，防止长任务中缓冲无界增长。
    trim_trace_events_to_limit(&mut events, timestamp_ms);
}

/// 追踪事件容量治理（G1-20）：超过上限时丢弃最旧事件，并在缓冲头部维护一个
/// traceTruncated 标记事件记录累计丢弃数量，保证丢弃行为可见（fail-closed）。
/// 标记事件始终占据索引 0，裁剪只作用于其后的普通事件，避免丢失计数。
fn trim_trace_events_to_limit(events: &mut Vec<Value>, timestamp_ms: i64) {
    if events.len() <= SUB_AGENT_TRACE_EVENT_LIMIT {
        return;
    }
    let mut dropped = events.len() - SUB_AGENT_TRACE_EVENT_LIMIT;
    let has_marker = events
        .first()
        .and_then(|event| event.get("event"))
        .and_then(Value::as_str)
        == Some(TRACE_TRUNCATED_EVENT);
    if has_marker {
        // 保留索引 0 的标记事件，丢弃其后最旧的 dropped 条。
        events.drain(1..=dropped);
        if let Some(marker) = events.first_mut() {
            let previous = marker
                .get("data")
                .and_then(|data| data.get("dropped"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if let Some(slot) = marker.get_mut("data").and_then(|data| data.get_mut("dropped")) {
                *slot = Value::from(previous + dropped as u64);
            }
            if let Some(slot) = marker.get_mut("timestampMs") {
                *slot = Value::from(timestamp_ms);
            }
        }
    } else {
        // 首次裁剪：多腾出 1 个槽位放置 traceTruncated 标记事件。
        dropped += 1;
        events.drain(0..dropped);
        events.insert(
            0,
            serde_json::json!({
                "event": TRACE_TRUNCATED_EVENT,
                "timestampMs": timestamp_ms,
                "data": {
                    "dropped": dropped,
                    "note": "追踪事件超出容量上限，最早的若干事件已被丢弃",
                },
            }),
        );
    }
}

fn readonly_tool_run_end(
    registry: &ToolRegistry,
    workspace: &std::path::Path,
    tool_calls: &[RequestedToolCall],
    start: usize,
) -> usize {
    tool_calls
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, tool_call)| {
            (!registry.is_parallel_readonly(workspace, &tool_call.name, true)).then_some(index)
        })
        .unwrap_or(tool_calls.len())
}

/// 构造 tool 角色消息（工具结果 / 重试收口提示 / 未执行说明共用）。
fn tool_result_message(tc: &RequestedToolCall, content: String) -> ChatMessage {
    ChatMessage {
        role: "tool".to_string(),
        content,
        content_parts: Vec::new(),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: Some(tc.id.clone()),
        name: Some(tc.name.clone()),
    }
}

/// 收集执行组内可恢复失败工具的名字（按出现顺序去重）。
fn distinct_failed_tool_names(executed: &[(&RequestedToolCall, ToolResult)]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (tc, result) in executed {
        if result.status == ToolStatus::RecoverableError
            && !names.iter().any(|existing| existing == &tc.name)
        {
            names.push(tc.name.clone());
        }
    }
    names
}

/// G13-05 升级决策（纯函数）：任一失败工具在此前轮次已失败过
/// （按工具名记录的失败轮数 ≥ 1，即已消耗重试资格）时升级为强制收口。
/// 同一轮内同名工具的多次失败（并行批次）在计数时只记一轮，
/// 因此不会被误判为「重试后仍失败」。
fn should_escalate_tool_failures(
    failed_tool_names: &[String],
    tool_failure_rounds: &HashMap<String, u32>,
) -> bool {
    failed_tool_names
        .iter()
        .any(|name| tool_failure_rounds.get(name).copied().unwrap_or(0) >= 1)
}

/// 估算单条消息的上下文占用（字符数）：content + reasoning + 工具调用参数。
fn chat_message_chars(message: &ChatMessage) -> usize {
    let mut chars = message.content.chars().count();
    if let Some(reasoning) = &message.reasoning_content {
        chars += reasoning.chars().count();
    }
    if let Some(tool_calls) = &message.tool_calls {
        for tc in tool_calls {
            chars += tc.function.name.chars().count() + tc.function.arguments.chars().count();
        }
    }
    chars
}

/// G13-07：消息历史滑动窗口裁剪（纯函数）。
///
/// 保留头部两条消息（system + 首轮 user）与最近若干轮完整对话；
/// 中间轮次整体移除，并插入一条占位说明，避免模型误以为任务刚开始。
/// 「轮次」= 一条 assistant 消息 + 紧随其后的全部 tool 响应，是不可拆分的
/// 最小单元——只按轮次边界裁剪才不会产生孤儿 tool 消息，保证发往
/// OpenAI 兼容接口的消息序列始终合法。单条工具结果在写入时已按
/// SUB_AGENT_RESULT_MAX_CHARS 截断，本函数在其上约束上下文总量：
/// 最近轮次数量与总字符数任一超限即收紧窗口，但至少保留最后一轮。
///
/// 返回 None 表示无需裁剪——调用方据此跳过整份历史的 clone。
fn trim_context_messages(
    messages: &[ChatMessage],
    max_chars: usize,
    keep_recent_rounds: usize,
) -> Option<Vec<ChatMessage>> {
    const HEADER_LEN: usize = 2; // system + 首轮 user
    if messages.len() <= HEADER_LEN || keep_recent_rounds == 0 {
        return None;
    }
    let (header, rest) = messages.split_at(HEADER_LEN);

    // 以 assistant 消息为起点切分轮次。
    let mut rounds: Vec<&[ChatMessage]> = Vec::new();
    let mut start = 0usize;
    for (index, message) in rest.iter().enumerate() {
        if message.role == "assistant" && index > start {
            rounds.push(&rest[start..index]);
            start = index;
        }
    }
    rounds.push(&rest[start..]);

    // 从最后一轮向前选择保留窗口：轮数与字符数双重约束，至少保留一轮。
    let mut kept_chars = 0usize;
    let mut keep_from = rounds.len();
    for (index, round) in rounds.iter().enumerate().rev() {
        let kept_count = rounds.len() - keep_from;
        if kept_count >= keep_recent_rounds {
            break;
        }
        let round_chars: usize = round.iter().map(chat_message_chars).sum();
        if kept_count > 0 && kept_chars.saturating_add(round_chars) > max_chars {
            break;
        }
        keep_from = index;
        kept_chars = kept_chars.saturating_add(round_chars);
    }

    if keep_from == 0 {
        return None;
    }

    let dropped_messages: usize = rounds[..keep_from].iter().map(|round| round.len()).sum();
    let mut trimmed = Vec::with_capacity(messages.len() - dropped_messages + 1);
    trimmed.extend_from_slice(header);
    trimmed.push(ChatMessage {
        role: "user".to_string(),
        content: format!(
            "【上下文裁剪】因上下文长度限制，此前 {dropped_messages} 条工具调用相关消息已被省略。如需其中的信息，请重新调用相应工具获取。"
        ),
        content_parts: Vec::new(),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
    });
    for round in &rounds[keep_from..] {
        trimmed.extend_from_slice(round);
    }
    Some(trimmed)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.to_string(),
            content: content.to_string(),
            content_parts: Vec::new(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    #[test]
    fn trace_collector_merges_adjacent_llm_deltas_and_keeps_timestamps() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        record_trace_event(
            &trace,
            json!({"event":"llmDelta","data":{"delta":"你"}}),
            10,
        );
        record_trace_event(
            &trace,
            json!({"event":"llmDelta","data":{"delta":"好"}}),
            11,
        );
        record_trace_event(
            &trace,
            json!({"event":"UsageUpdated","data":{"elapsedMs":20}}),
            20,
        );

        let events = trace.lock();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["data"]["delta"], "你好");
        assert_eq!(events[0]["timestampMs"], 10);
        assert_eq!(events[1]["timestampMs"], 20);
    }

    #[test]
    fn trace_events_are_capped_with_visible_truncation_marker() {
        // G1-20：超限时丢弃最旧事件，头部 traceTruncated 标记记录累计丢弃数。
        let trace = Arc::new(Mutex::new(Vec::new()));
        let total = SUB_AGENT_TRACE_EVENT_LIMIT + 50;
        for index in 0..total {
            record_trace_event(
                &trace,
                json!({"event":"ToolStarted","data":{"index": index}}),
                index as i64,
            );
        }

        let events = trace.lock();
        assert_eq!(events.len(), SUB_AGENT_TRACE_EVENT_LIMIT);
        assert_eq!(events[0]["event"], TRACE_TRUNCATED_EVENT);
        // 丢弃 50 条超限事件 + 1 条为标记腾位 = 51。
        assert_eq!(events[0]["data"]["dropped"], 51);
        // 最新事件必须保留。
        assert_eq!(
            events.last().unwrap()["data"]["index"],
            (total - 1) as u64
        );
    }

    #[test]
    fn escalation_requires_a_previous_failed_round_for_the_same_tool() {
        // G13-05：按工具名记录重试资格。
        let mut rounds: HashMap<String, u32> = HashMap::new();

        // 首次失败：不升级，允许重试。
        assert!(!should_escalate_tool_failures(
            &["read_file".to_string()],
            &rounds
        ));

        // 同名工具此前轮次已失败过：升级强制收口。
        rounds.insert("read_file".to_string(), 1);
        assert!(should_escalate_tool_failures(
            &["read_file".to_string()],
            &rounds
        ));

        // 其他工具的首次失败不受牵连（修复原全局标志的交叉污染）。
        assert!(!should_escalate_tool_failures(
            &["search".to_string()],
            &rounds
        ));

        // 无失败工具：不升级。
        assert!(!should_escalate_tool_failures(&[], &rounds));
    }

    #[test]
    fn trim_context_keeps_header_and_recent_rounds_only() {
        // 头部（system + 首轮 user）+ 三轮 assistant/tool 对话。
        let messages = vec![
            msg("system", "sys"),
            msg("user", "task"),
            msg("assistant", "a1"),
            msg("tool", "t1"),
            msg("assistant", "a2"),
            msg("tool", "t2"),
            msg("assistant", "a3"),
            msg("tool", "t3"),
        ];

        let trimmed = trim_context_messages(&messages, 120_000, 1).expect("应当触发裁剪");
        // 头部 2 条 + 占位说明 1 条 + 最后一轮 2 条。
        assert_eq!(trimmed.len(), 5);
        assert_eq!(trimmed[0].content, "sys");
        assert_eq!(trimmed[1].content, "task");
        assert!(trimmed[2].content.contains("上下文裁剪"));
        // 保留的轮次必须完整（assistant + 其 tool 响应），无孤儿消息。
        assert_eq!(trimmed[3].role, "assistant");
        assert_eq!(trimmed[3].content, "a3");
        assert_eq!(trimmed[4].role, "tool");
        assert_eq!(trimmed[4].content, "t3");
    }

    #[test]
    fn trim_context_noop_when_within_limits() {
        let messages = vec![
            msg("system", "sys"),
            msg("user", "task"),
            msg("assistant", "a1"),
            msg("tool", "t1"),
        ];
        // 无需裁剪时返回 None，调用方跳过全量 clone。
        assert!(trim_context_messages(&messages, 120_000, 8).is_none());
    }

    #[test]
    fn trim_context_shrinks_window_under_char_budget() {
        // 三轮各约 50k 字符；窗口上限 120k ⇒ 只能保留最近两轮。
        let big: String = "测".repeat(50_000);
        let messages = vec![
            msg("system", "sys"),
            msg("user", "task"),
            msg("assistant", &big),
            msg("tool", "t1"),
            msg("assistant", &big),
            msg("tool", "t2"),
            msg("assistant", &big),
            msg("tool", "t3"),
        ];
        let trimmed = trim_context_messages(&messages, 120_000, 8).expect("应当触发裁剪");
        // 头部 2 + 占位 1 + 两轮 4 = 7；第一轮被裁剪。
        assert_eq!(trimmed.len(), 7);
        assert!(trimmed[2].content.contains("上下文裁剪"));
        assert!(!trimmed.iter().any(|m| m.content == "t1".to_string()));
    }

    #[test]
    fn trim_context_always_keeps_at_least_one_round() {
        // 单轮即超预算时仍保留最后一轮（最近的上下文最关键）⇒ 无需裁剪。
        let big: String = "测".repeat(200_000);
        let messages = vec![
            msg("system", "sys"),
            msg("user", "task"),
            msg("assistant", &big),
            msg("tool", "t1"),
        ];
        assert!(trim_context_messages(&messages, 120_000, 8).is_none());
    }
}
