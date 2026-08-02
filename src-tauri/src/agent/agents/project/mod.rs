//! 项目编排 Agent（OrchestratorAgent）。
//!
//! 彻底替换旧 DispatcherAgent（直接编码 + dispatch PTY 委派）：项目 Agent 现在
//! 只做「任务拆解与编排」——用固定只读工具探索项目，核心产物是执行图（DAG），
//! 通过 `submit_graph` 协议工具提交，经校验后落 `graph_plans` 并等待用户确认；
//! 图的实际执行由 `agent::graph::runner` 承担。
//!
//! 模块划分：
//! - `mod.rs`：结构体、构造与设置应用（模型配置沿用「模型用途」项目上下文）。
//! - `prompt.rs`：编排器系统提示（角色 / 图 schema / 节点设计原则 / 可用 agent 清单）。
//! - `turn.rs`：`RunLoopAgent` + `AgentRunAdapter` 实现（复用公共 run_loop）。
//! - `iteration.rs`：单次迭代的 LLM 交互与消息收口。
//! - `tool_exec.rs`：工具批量执行（只读并行组 / 压缩 / 重试）。
//! - `graph_submit.rs`：`submit_graph` 协议拦截与本轮收口。
//! - `helpers.rs`：事件、用量、错误分类等小工具。

pub(crate) mod graph_submit;
pub(crate) mod helpers;
pub(crate) mod iteration;
pub(crate) mod prompt;
pub(crate) mod tool_exec;
pub(crate) mod turn;

use std::sync::Arc;

use parking_lot::Mutex;
use tauri::AppHandle;

use crate::agent::config::DispatcherAgentConfig;
use crate::agent::db::{AhaSettingsV2, AgentContext};
use crate::agent::llm::OpenAiCompatProvider;
use crate::agent::run_loop::AgentEvent;
use crate::agent::tools::ToolRegistry;

// ─── Internal types ───────────────────────────────────────────────────────────

pub(super) struct Models {
    summary_model: String,
    summary_api_key: String,
    summary_api_base: String,
    vision_provider: Option<OpenAiCompatProvider>,
}

pub(super) struct ModelsSnapshot {
    vision_model: String,
}

impl Models {
    fn snapshot(&self) -> ModelsSnapshot {
        ModelsSnapshot {
            vision_model: self
                .vision_provider
                .as_ref()
                .map(|p| p.model().to_string())
                .unwrap_or_default(),
        }
    }
}

// ─── OrchestratorAgent struct ─────────────────────────────────────────────────

pub struct OrchestratorAgent {
    pub(super) config: DispatcherAgentConfig,
    pub(super) provider: Mutex<OpenAiCompatProvider>,
    pub(super) models: Mutex<Models>,
    pub(super) app_handle: Option<AppHandle>,
    pub(super) tools: Arc<ToolRegistry>,
    pub(super) sub_agent_manager: Option<Arc<crate::agent::sub_agent::SubAgentManager>>,
}

// ─── Construction & configuration ─────────────────────────────────────────────

impl OrchestratorAgent {
    pub fn new(
        config: DispatcherAgentConfig,
        sub_agent_manager: Option<Arc<crate::agent::sub_agent::SubAgentManager>>,
    ) -> Self {
        let provider = OpenAiCompatProvider::new(
            config.api_key.clone(),
            config.api_base.clone(),
            config.model.clone(),
            config.max_tokens,
            config.temperature,
        );

        Self {
            models: Mutex::new(Models {
                summary_model: helpers::normalize_summary_model(&config.summary_model),
                summary_api_key: String::new(),
                summary_api_base: String::new(),
                // env 兜底（VISION_MODEL_NAME）：只有模型名，沿用主模型凭据。
                vision_provider: if config.vision_model.trim().is_empty() {
                    None
                } else {
                    Some(OpenAiCompatProvider::new(
                        config.api_key.clone(),
                        config.api_base.clone(),
                        config.vision_model.trim().to_string(),
                        config.max_tokens,
                        config.temperature,
                    ))
                },
            }),
            app_handle: None,
            tools: Arc::new(ToolRegistry::orchestrator_tools(sub_agent_manager.clone())),
            config,
            provider: Mutex::new(provider),
            sub_agent_manager,
        }
    }

    pub fn with_app_handle(mut self, app_handle: AppHandle) -> Self {
        self.app_handle = Some(app_handle);
        self
    }

    pub fn apply_settings_v2(&self, settings: &AhaSettingsV2, context: AgentContext) {
        let ctx_config = match context {
            AgentContext::Project => &settings.project,
            AgentContext::Chat => &settings.chat,
        };
        let shared = &settings.shared;

        {
            let mut provider = self.provider.lock();
            *provider = resolve_chat_provider(&self.config, ctx_config);
        }

        let active_summary = ctx_config
            .summary_model_configs
            .iter()
            .find(|c| c.active)
            .or_else(|| ctx_config.summary_model_configs.first());
        let active_vision = shared
            .vision_model_configs
            .iter()
            .find(|c| c.active)
            .or_else(|| shared.vision_model_configs.first());

        let mut models = self.models.lock();
        if let Some(smc) = active_summary {
            if !smc.model.trim().is_empty() {
                models.summary_model = helpers::normalize_summary_model(&smc.model);
            }
            if !smc.api_key.trim().is_empty() {
                models.summary_api_key = smc.api_key.trim().to_string();
            }
            if !smc.url.trim().is_empty() {
                models.summary_api_base = smc.url.trim().to_string();
            }
        }
        // 视觉模型切换使用设置中视觉用途的完整配置（默认第一个 active，否则
        // 第一个条目），url/apiKey 为空时回退聊天主模型凭据。
        models.vision_provider = active_vision
            .filter(|v| !v.model.trim().is_empty())
            .map(|v| {
                let fallback = self.provider.lock();
                OpenAiCompatProvider::new(
                    if v.api_key.trim().is_empty() {
                        fallback.api_key().to_string()
                    } else {
                        v.api_key.trim().to_string()
                    },
                    if v.url.trim().is_empty() {
                        fallback.api_base().to_string()
                    } else {
                        v.url.trim().to_string()
                    },
                    v.model.trim().to_string(),
                    self.config.max_tokens,
                    self.config.temperature,
                )
            });
    }

    pub fn context_debug_enabled(&self) -> bool {
        self.config.context_debug
    }

    pub fn set_context_debug(&mut self, value: bool) {
        self.config.context_debug = value;
    }

    // ─── Model accessors ──────────────────────────────────────────────────────

    pub(super) fn summary_model(&self) -> String {
        self.models.lock().summary_model.clone()
    }

    /// Build the provider to use for summary operations.
    /// If `summary_model_config` has its own api_key/url, use those;
    /// otherwise fall back to the chat provider's credentials.
    pub(super) fn summary_provider(&self, fallback: &OpenAiCompatProvider) -> OpenAiCompatProvider {
        let models = self.models.lock();
        let api_key = if models.summary_api_key.is_empty() {
            fallback.api_key().to_string()
        } else {
            models.summary_api_key.clone()
        };
        let api_base = if models.summary_api_base.is_empty() {
            fallback.api_base().to_string()
        } else {
            models.summary_api_base.clone()
        };
        OpenAiCompatProvider::new(
            api_key,
            api_base,
            models.summary_model.clone(),
            self.config.max_tokens,
            self.config.temperature,
        )
    }

    /// 图片消息切视觉模型：消息中含图片时必须使用视觉用途 provider。
    pub(super) fn provider_for_messages(
        &self,
        provider: &OpenAiCompatProvider,
        messages: &[crate::agent::llm::ChatMessage],
        on_event: &tauri::ipc::Channel<AgentEvent>,
        notify_user: bool,
    ) -> anyhow::Result<OpenAiCompatProvider> {
        if !crate::agent::llm::messages_contain_images(messages) {
            return Ok(provider.clone());
        }

        let vision_provider = self.models.lock().vision_provider.clone();
        let Some(selected) = vision_provider else {
            anyhow::bail!(
                "检测到用户上传了图片，但设置中的视觉模型为空。请先配置视觉模型后重试。"
            );
        };

        if notify_user && selected.model() != provider.model() {
            helpers::emit(
                on_event,
                AgentEvent::ModelSwitched {
                    from_model: provider.model().to_string(),
                    to_model: selected.model().to_string(),
                    reason: "检测到用户上传了图片".to_string(),
                },
            );
        }

        Ok(selected)
    }
}

/// 按「模型用途」项目上下文配置解析当前生效的对话模型 provider。
/// 图 Runner 执行 subAgent 节点时用它作为父级 provider（子智能体默认继承）。
pub(crate) fn resolve_project_chat_provider(
    config: &DispatcherAgentConfig,
    settings: &AhaSettingsV2,
) -> OpenAiCompatProvider {
    resolve_chat_provider(config, &settings.project)
}

fn resolve_chat_provider(
    config: &DispatcherAgentConfig,
    ctx_config: &crate::agent::db::AhaContextConfig,
) -> OpenAiCompatProvider {
    let active_chat = ctx_config
        .chat_model_configs
        .iter()
        .find(|c| c.active)
        .or_else(|| ctx_config.chat_model_configs.first());

    match active_chat {
        Some(chat) => OpenAiCompatProvider::new(
            if chat.api_key.is_empty() {
                config.api_key.clone()
            } else {
                chat.api_key.clone()
            },
            if chat.url.is_empty() {
                config.api_base.clone()
            } else {
                chat.url.clone()
            },
            if chat.model.is_empty() {
                config.model.clone()
            } else {
                chat.model.clone()
            },
            config.max_tokens,
            config.temperature,
        ),
        None => OpenAiCompatProvider::new(
            config.api_key.clone(),
            config.api_base.clone(),
            config.model.clone(),
            config.max_tokens,
            config.temperature,
        ),
    }
}
