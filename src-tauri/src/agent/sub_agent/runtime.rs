use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;
use tauri::Emitter;
use tokio::time::timeout;

use super::config::SubAgentConfig;
use crate::agent::llm::{
    ChatMessage, FunctionCall, LlmUsage, OpenAiCompatProvider, OutboundToolCall, RequestedToolCall,
    ToolDefinition,
};
use crate::agent::tools::{ToolContext, ToolRegistry, ToolResult, ToolStatus};

const SUB_AGENT_RESULT_MAX_CHARS: usize = 32_000;
const SUB_AGENT_LLM_REQUEST_TIMEOUT_SECS: u64 = 120;
const NESTED_SUB_AGENT_TOOLS: &[&str] = &["call_sub_agent", "list_sub_agents"];

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
    #[serde(flatten)]
    pub event: SubAgentEvent,
}

pub struct SubAgentRuntime {
    config: SubAgentConfig,
    provider: OpenAiCompatProvider,
    tool_registry: Arc<ToolRegistry>,
    tool_context: ToolContext,
    tool_definitions: Vec<crate::agent::llm::ToolDefinition>,
}

impl SubAgentRuntime {
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
        tool_context.current_sub_agent_id = Some(config.agent_id.clone());
        tool_context.current_sub_agent_name = Some(config.agent_name.clone());

        let excluded: HashSet<&str> = NESTED_SUB_AGENT_TOOLS.iter().copied().collect();
        let allowed_set: HashSet<&str> = config
            .allowed_tools
            .iter()
            .filter(|s| !excluded.contains(s.as_str()))
            .map(|s| s.as_str())
            .collect();

        let tool_definitions = tool_registry.definitions_for_workspace(
            &tool_context.workspace,
            Some(allowed_set.iter().copied()),
            false,
        );

        Ok(Self {
            config: config.clone(),
            provider,
            tool_registry,
            tool_context,
            tool_definitions,
        })
    }

    pub async fn execute(
        &self,
        task: &str,
        app_handle: Option<AppHandle>,
        session_id: &str,
    ) -> Result<String> {
        let start = Instant::now();
        let overall_timeout = Duration::from_secs(self.config.timeout_secs);
        let llm_request_timeout = Duration::from_secs(SUB_AGENT_LLM_REQUEST_TIMEOUT_SECS);
        let mut usage = SubAgentUsage::default();
        let mut tool_error_seen = false;
        let mut force_final_response = false;
        let no_tools: Vec<ToolDefinition> = Vec::new();
        #[allow(unused_assignments)]
        let mut last_iteration: u32 = 0;

        if let Some(handle) = &app_handle {
            let _ = handle.emit(
                "sub-agent-event",
                SubAgentEventPayload {
                    session_id: session_id.to_string(),
                    event: SubAgentEvent::Started {
                        agent_id: self.config.agent_id.clone(),
                        agent_name: self.config.agent_name.clone(),
                        task: task.to_string(),
                    },
                },
            );
        }

        let user_prompt = self.config.user_prompt_template.replace("{{task}}", task);

        let mut messages = vec![
            ChatMessage::system(self.config.system_prompt.clone()),
            ChatMessage {
                role: "user".to_string(),
                content: user_prompt,
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

            last_iteration = iteration + 1;

            let on_delta = |delta: &str| {
                if let Some(handle) = &app_handle {
                    let _ = handle.emit(
                        "sub-agent-event",
                        SubAgentEventPayload {
                            session_id: session_id.to_string(),
                            event: SubAgentEvent::LlmDelta {
                                agent_id: self.config.agent_id.clone(),
                                agent_name: self.config.agent_name.clone(),
                                delta: delta.to_string(),
                            },
                        },
                    );
                }
            };

            let active_tool_definitions = if force_final_response {
                &no_tools
            } else {
                &self.tool_definitions
            };

            let llm_future = self.provider.chat_stream_with_thinking(
                &messages,
                active_tool_definitions,
                false,
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
                if let Some(handle) = &app_handle {
                    let _ = handle.emit(
                        "sub-agent-event",
                        SubAgentEventPayload {
                            session_id: session_id.to_string(),
                            event: SubAgentEvent::UsageUpdated {
                                agent_id: self.config.agent_id.clone(),
                                agent_name: self.config.agent_name.clone(),
                                token_usage: usage.clone(),
                                elapsed_ms: start.elapsed().as_millis() as u64,
                            },
                        },
                    );
                }
            }

            if response.tool_calls.is_empty() {
                let result = response.content.clone();
                if let Some(handle) = &app_handle {
                    let _ = handle.emit(
                        "sub-agent-event",
                        SubAgentEventPayload {
                            session_id: session_id.to_string(),
                            event: SubAgentEvent::Finished {
                                agent_id: self.config.agent_id.clone(),
                                agent_name: self.config.agent_name.clone(),
                                result: result.clone(),
                                iterations: last_iteration,
                                elapsed_ms: start.elapsed().as_millis() as u64,
                                token_usage: usage,
                            },
                        },
                    );
                }
                return Ok(result);
            }

            let assistant_msg = ChatMessage {
                role: "assistant".to_string(),
                content: response.content.clone(),
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
            let mut should_retry_after_tool_error = false;

            while tc_index < tool_calls.len() {
                let readonly_end = readonly_tool_run_end(
                    &self.tool_registry,
                    &self.tool_context.workspace,
                    &tool_calls,
                    tc_index,
                );

                let executed: Vec<(&RequestedToolCall, ToolResult)> = if readonly_end
                    .saturating_sub(tc_index)
                    >= 2
                {
                    let run = &tool_calls[tc_index..readonly_end];
                    self.execute_parallel_readonly_tools(run, &app_handle, session_id, &mut usage)
                        .await
                } else {
                    let tc = &tool_calls[tc_index];
                    let result = self
                        .execute_single_tool(tc, &app_handle, session_id, &mut usage)
                        .await;
                    vec![(tc, result)]
                };

                for (tc, result) in executed {
                    let result_text = result.output_for_llm();
                    match result.status {
                        ToolStatus::RecoverableError => {
                            let retry_hint = if tool_error_seen {
                                force_final_response = true;
                                format!(
                                    "工具重试后仍然失败。\n错误信息：{result_text}\n\n要求：不要继续调用工具。请基于当前状态判断该错误是否无法修复；如果无法修复，请明确说明已尝试的动作、失败原因和退出结论。"
                                )
                            } else {
                                tool_error_seen = true;
                                format!(
                                    "工具调用失败。\n错误信息：{result_text}\n\n要求：请根据工具 schema、上次参数和错误信息修正后重试；如果你判断无法修复，请不要猜测，直接说明无法修复并退出。"
                                )
                            };
                            messages.push(ChatMessage {
                                role: "tool".to_string(),
                                content: retry_hint,
                                reasoning_content: None,
                                tool_calls: None,
                                tool_call_id: Some(tc.id.clone()),
                                name: Some(tc.name.clone()),
                            });
                            should_retry_after_tool_error = true;
                        }
                        ToolStatus::FatalError | ToolStatus::Cancelled => {
                            let err_msg = format!(
                                "子智能体 '{}' 内部工具 '{}' 执行失败：{}",
                                self.config.agent_id, tc.name, result_text
                            );
                            self.emit_failed(&app_handle, session_id, &err_msg);
                            anyhow::bail!("{}", err_msg);
                        }
                        ToolStatus::Success => {
                            let truncated = truncate_tool_result(&result_text);
                            messages.push(ChatMessage {
                                role: "tool".to_string(),
                                content: truncated,
                                reasoning_content: None,
                                tool_calls: None,
                                tool_call_id: Some(tc.id.clone()),
                                name: Some(tc.name.clone()),
                            });
                        }
                    }
                }

                let next_index = if readonly_end.saturating_sub(tc_index) >= 2 {
                    readonly_end
                } else {
                    tc_index + 1
                };

                if should_retry_after_tool_error {
                    for skipped in &tool_calls[next_index..] {
                        let content = if force_final_response {
                            format!(
                                "未执行：前一个工具调用重试后仍失败，本轮剩余工具已暂停，等待模型确认无法修复或给出最终结论。工具：{}",
                                skipped.name
                            )
                        } else {
                            format!(
                                "未执行：前一个工具调用失败，已暂停本轮剩余工具调用。请先根据错误信息修正后重试。工具：{}",
                                skipped.name
                            )
                        };
                        messages.push(ChatMessage {
                            role: "tool".to_string(),
                            content,
                            reasoning_content: None,
                            tool_calls: None,
                            tool_call_id: Some(skipped.id.clone()),
                            name: Some(skipped.name.clone()),
                        });
                    }
                    break;
                }

                if readonly_end.saturating_sub(tc_index) >= 2 {
                    tc_index = readonly_end;
                } else {
                    tc_index += 1;
                }
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
    ) -> ToolResult {
        let _ = usage;
        if let Some(handle) = app_handle {
            let _ = handle.emit(
                "sub-agent-event",
                SubAgentEventPayload {
                    session_id: session_id.to_string(),
                    event: SubAgentEvent::ToolStarted {
                        agent_id: self.config.agent_id.clone(),
                        agent_name: self.config.agent_name.clone(),
                        tool_name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    },
                },
            );
        }

        let result = self
            .tool_registry
            .execute(&tc.name, &tc.arguments, &self.tool_context)
            .await;

        let result_text = result.output_for_llm();
        let result_preview = tool_result_preview(&tc.name, &result_text);

        if let Some(handle) = app_handle {
            let _ = handle.emit(
                "sub-agent-event",
                SubAgentEventPayload {
                    session_id: session_id.to_string(),
                    event: SubAgentEvent::ToolFinished {
                        agent_id: self.config.agent_id.clone(),
                        agent_name: self.config.agent_name.clone(),
                        tool_name: tc.name.clone(),
                        result_preview,
                    },
                },
            );
        }

        result
    }

    async fn execute_parallel_readonly_tools<'a>(
        &'a self,
        tool_calls: &'a [RequestedToolCall],
        app_handle: &'a Option<AppHandle>,
        session_id: &'a str,
        usage: &mut SubAgentUsage,
    ) -> Vec<(&'a RequestedToolCall, ToolResult)> {
        let _ = usage;
        for tc in tool_calls {
            if let Some(handle) = app_handle {
                let _ = handle.emit(
                    "sub-agent-event",
                    SubAgentEventPayload {
                        session_id: session_id.to_string(),
                        event: SubAgentEvent::ToolStarted {
                            agent_id: self.config.agent_id.clone(),
                            agent_name: self.config.agent_name.clone(),
                            tool_name: tc.name.clone(),
                            arguments: tc.arguments.clone(),
                        },
                    },
                );
            }
        }

        let results = join_all(
            tool_calls
                .iter()
                .map(|tc| async move {
                    (
                        tc,
                        self.tool_registry
                            .execute(&tc.name, &tc.arguments, &self.tool_context)
                            .await,
                    )
                })
                .collect::<Vec<_>>(),
        )
        .await;

        for (tc, result) in &results {
            let result_text = result.output_for_llm();
            let result_preview = tool_result_preview(&tc.name, &result_text);
            if let Some(handle) = app_handle {
                let _ = handle.emit(
                    "sub-agent-event",
                    SubAgentEventPayload {
                        session_id: session_id.to_string(),
                        event: SubAgentEvent::ToolFinished {
                            agent_id: self.config.agent_id.clone(),
                            agent_name: self.config.agent_name.clone(),
                            tool_name: tc.name.clone(),
                            result_preview,
                        },
                    },
                );
            }
        }

        results
    }

    fn emit_failed(&self, app_handle: &Option<AppHandle>, session_id: &str, error: &str) {
        if let Some(handle) = app_handle {
            let _ = handle.emit(
                "sub-agent-event",
                SubAgentEventPayload {
                    session_id: session_id.to_string(),
                    event: SubAgentEvent::Failed {
                        agent_id: self.config.agent_id.clone(),
                        agent_name: self.config.agent_name.clone(),
                        error: error.to_string(),
                    },
                },
            );
        }
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
