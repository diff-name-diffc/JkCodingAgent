use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;
use tauri::Emitter;

use super::config::SubAgentConfig;
use crate::agent::llm::{
    ChatMessage, FunctionCall, LlmUsage, OpenAiCompatProvider, OutboundToolCall,
};
use crate::agent::tools::{ToolContext, ToolRegistry};

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

        Ok(Self {
            config: config.clone(),
            provider,
            tool_registry,
            tool_context,
        })
    }

    pub async fn execute(
        &self,
        task: &str,
        app_handle: Option<AppHandle>,
        session_id: &str,
    ) -> Result<String> {
        let start = Instant::now();
        let timeout = Duration::from_secs(self.config.timeout_secs);
        let mut usage = SubAgentUsage::default();
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

        let allowed_set: HashSet<&str> = self
            .config
            .allowed_tools
            .iter()
            .map(|s| s.as_str())
            .collect();

        let tool_definitions = self.tool_registry.definitions_for_workspace(
            &self.tool_context.workspace,
            Some(allowed_set.iter().copied()),
            false,
        );

        for iteration in 0..self.config.max_iterations {
            if start.elapsed() > timeout {
                let err_msg = format!(
                    "子智能体 '{}' 执行超时（{}秒）",
                    self.config.agent_id, self.config.timeout_secs
                );
                if let Some(handle) = &app_handle {
                    let _ = handle.emit(
                        "sub-agent-event",
                        SubAgentEventPayload {
                            session_id: session_id.to_string(),
                            event: SubAgentEvent::Failed {
                                agent_id: self.config.agent_id.clone(),
                                agent_name: self.config.agent_name.clone(),
                                error: err_msg.clone(),
                            },
                        },
                    );
                }
                anyhow::bail!("{}", err_msg);
            }

            last_iteration = iteration + 1;

            let response = match self
                .provider
                .chat_stream_with_thinking(
                    &messages,
                    &tool_definitions,
                    false,
                    false,
                    |delta: &str| {
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
                    },
                    |_delta: &str, _elapsed: u64| {},
                )
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    let err_msg = format!(
                        "子智能体 '{}' 模型请求失败：{}",
                        self.config.agent_id, error
                    );
                    self.emit_failed(&app_handle, session_id, &err_msg);
                    anyhow::bail!("{}", err_msg);
                }
            };

            if let Some(usage_info) = response.usage.as_ref() {
                usage.record(usage_info);
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

            for tc in &response.tool_calls {
                if let Some(handle) = &app_handle {
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

                if is_tool_error_result(&result) {
                    let err_msg = format!(
                        "子智能体 '{}' 内部工具 '{}' 执行失败：{}",
                        self.config.agent_id,
                        tc.name,
                        result.trim()
                    );
                    self.emit_failed(&app_handle, session_id, &err_msg);
                    anyhow::bail!("{}", err_msg);
                }

                let result_preview = if result.chars().count() > 200 {
                    format!("{}...", result.chars().take(200).collect::<String>())
                } else {
                    result.clone()
                };

                if let Some(handle) = &app_handle {
                    let _ = handle.emit(
                        "sub-agent-event",
                        SubAgentEventPayload {
                            session_id: session_id.to_string(),
                            event: SubAgentEvent::ToolFinished {
                                agent_id: self.config.agent_id.clone(),
                                agent_name: self.config.agent_name.clone(),
                                tool_name: tc.name.clone(),
                                result_preview: result_preview.clone(),
                            },
                        },
                    );
                }

                let tool_result_msg = ChatMessage {
                    role: "tool".to_string(),
                    content: result,
                    reasoning_content: None,
                    tool_calls: None,
                    tool_call_id: Some(tc.id.clone()),
                    name: Some(tc.name.clone()),
                };
                messages.push(tool_result_msg);
            }
        }

        let err_msg = format!(
            "子智能体 '{}' 达到最大迭代次数（{}）",
            self.config.agent_id, self.config.max_iterations
        );
        if let Some(handle) = &app_handle {
            let _ = handle.emit(
                "sub-agent-event",
                SubAgentEventPayload {
                    session_id: session_id.to_string(),
                    event: SubAgentEvent::Failed {
                        agent_id: self.config.agent_id.clone(),
                        agent_name: self.config.agent_name.clone(),
                        error: err_msg.clone(),
                    },
                },
            );
        }
        anyhow::bail!("{}", err_msg)
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

fn is_tool_error_result(result: &str) -> bool {
    let trimmed = result.trim_start();
    trimmed.starts_with("错误：") || trimmed.starts_with("__SUB_AGENT_FAILURE__:")
}
