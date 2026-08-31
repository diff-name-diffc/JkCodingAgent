use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use futures::future::join_all;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::AppHandle;
use tauri::Emitter;
use tokio::sync::watch;
use tokio::time::timeout;

use super::config::SubAgentConfig;

mod build;
mod context;
mod events;
#[cfg(test)]
mod tests;
mod tool_exec;

use crate::agent::common;
use crate::agent::llm::{
    ChatMessage, FunctionCall, LlmUsage, OpenAiCompatProvider, OutboundToolCall, RequestedToolCall,
    ToolDefinition,
};
use crate::agent::tools::{
    CapabilitySet, ToolContext, ToolRegistry, ToolResult, ToolRuntime, ToolStatus,
};
use context::*;
pub(super) use events::record_trace_event;
pub use events::{SubAgentEvent, SubAgentEventPayload, SubAgentUsage};
use tool_exec::*;

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
    capabilities: CapabilitySet,
    parent_tool_call_id: String,
    trace_events: Arc<Mutex<Vec<Value>>>,
}

impl SubAgentRuntime {
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

            if cancel_rx.as_ref().is_some_and(|rx| *rx.borrow()) {
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

            if response.tool_calls.len() > crate::agent::tools::MAX_TOOL_CALLS_PER_BATCH {
                let err_msg = format!(
                    "子智能体 '{}' 单轮返回 {} 个工具调用，超过运行时上限 {}，已拒绝执行",
                    self.config.agent_id,
                    response.tool_calls.len(),
                    crate::agent::tools::MAX_TOOL_CALLS_PER_BATCH
                );
                self.emit_failed(&app_handle, session_id, &err_msg);
                anyhow::bail!(err_msg);
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
                let readonly_end = common::readonly_tool_run_end(
                    &self.tool_registry,
                    &self.tool_context.mcp_scope,
                    &tool_calls,
                    tc_index,
                );

                // G13-04：把整体超时预算传入工具执行层，保证每次工具等待有界。
                let executed: Vec<(&RequestedToolCall, ToolResult)> =
                    if readonly_end.saturating_sub(tc_index) >= 2 {
                        let run = &tool_calls[tc_index..readonly_end];
                        self.execute_parallel_readonly_tools(
                            run,
                            &app_handle,
                            session_id,
                            start,
                            overall_timeout,
                            cancel_rx.as_ref(),
                        )
                        .await
                    } else {
                        let tc = &tool_calls[tc_index];
                        let result = self
                            .execute_single_tool(
                                tc,
                                &app_handle,
                                session_id,
                                start,
                                overall_timeout,
                                cancel_rx.as_ref(),
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
                    if matches!(
                        result.status,
                        ToolStatus::FatalError | ToolStatus::Cancelled
                    ) {
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
                        || should_escalate_tool_failures(&failed_tool_names, &tool_failure_rounds);
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
}
