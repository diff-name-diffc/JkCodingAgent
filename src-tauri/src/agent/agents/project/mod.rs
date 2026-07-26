pub(crate) mod helpers;
pub(crate) mod prompt;
pub(crate) mod protocol;
pub(crate) mod subprocess;
pub(crate) mod tool_exec;
pub(crate) mod turn;

use std::sync::Arc;

use parking_lot::Mutex;
use tauri::AppHandle;

use crate::agent::config::DispatcherAgentConfig;
use crate::agent::llm::OpenAiCompatProvider;
use crate::agent::run_loop::AgentEvent;
use crate::agent::tools::ToolRegistry;
use crate::project::mcp::ProjectMcpRegistry;
use crate::ssh_tool::SshSessionManager;

pub(crate) use subprocess::DispatcherSubprocessRegistry;
pub(crate) use turn::DispatcherContinueAfterDispatchRequest;

// ─── Internal types ───────────────────────────────────────────────────────────

pub(super) struct Models {
    summary_model: String,
    summary_api_key: String,
    summary_api_base: String,
    vision_provider: Option<OpenAiCompatProvider>,
    image_model_url: String,
    image_model_api_key: String,
    image_model: String,
    image_edit_model: String,
}

pub(super) struct ModelsSnapshot {
    vision_model: String,
    image_model_url: String,
    image_model_api_key: String,
    image_model: String,
    image_edit_model: String,
}

impl Models {
    fn snapshot(&self) -> ModelsSnapshot {
        ModelsSnapshot {
            vision_model: self
                .vision_provider
                .as_ref()
                .map(|p| p.model().to_string())
                .unwrap_or_default(),
            image_model_url: self.image_model_url.clone(),
            image_model_api_key: self.image_model_api_key.clone(),
            image_model: self.image_model.clone(),
            image_edit_model: self.image_edit_model.clone(),
        }
    }
}

// ─── DispatcherAgent struct ───────────────────────────────────────────────────

pub struct DispatcherAgent {
    pub(super) config: DispatcherAgentConfig,
    pub(super) provider: Mutex<OpenAiCompatProvider>,
    pub(super) models: Mutex<Models>,
    pub(super) app_handle: Option<AppHandle>,
    pub(super) tools: Arc<ToolRegistry>,
    pub(super) project_mcp_registry: ProjectMcpRegistry,
    pub(super) subprocesses: Arc<DispatcherSubprocessRegistry>,
    pub(super) allowed_tools: Mutex<Vec<String>>,
    pub(super) sub_agent_manager: Option<Arc<crate::agent::sub_agent::SubAgentManager>>,
}

// ─── Construction & configuration ─────────────────────────────────────────────

impl DispatcherAgent {
    pub fn new(
        config: DispatcherAgentConfig,
        project_mcp_registry: ProjectMcpRegistry,
        ssh_manager: SshSessionManager,
        subprocesses: Arc<DispatcherSubprocessRegistry>,
        sub_agent_manager: Option<Arc<crate::agent::sub_agent::SubAgentManager>>,
    ) -> Self {
        let provider = OpenAiCompatProvider::new(
            config.api_key.clone(),
            config.api_base.clone(),
            config.model.clone(),
            config.max_tokens,
            config.temperature,
        );

        let mut registry = ToolRegistry::default_tools(project_mcp_registry.clone(), ssh_manager);
        if let Some(manager) = &sub_agent_manager {
            registry.add_tool(Box::new(crate::agent::sub_agent::SubAgentTool::new(
                Arc::clone(manager),
            )));
            registry.add_tool(Box::new(crate::agent::sub_agent::ListSubAgentsTool::new(
                Arc::clone(manager),
            )));
        }

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
                image_model_url: config.image_model_url.clone(),
                image_model_api_key: config.image_model_api_key.clone(),
                image_model: config.image_model.clone(),
                image_edit_model: config.image_edit_model.clone(),
            }),
            app_handle: None,
            config,
            provider: Mutex::new(provider),
            tools: Arc::new(registry),
            project_mcp_registry,
            subprocesses,
            allowed_tools: Mutex::new(Vec::new()),
            sub_agent_manager,
        }
    }

    pub fn tools_arc(&self) -> Arc<ToolRegistry> {
        Arc::clone(&self.tools)
    }

    pub fn with_app_handle(mut self, app_handle: AppHandle) -> Self {
        self.app_handle = Some(app_handle);
        self
    }

    pub fn apply_settings_v2(
        &self,
        settings: &crate::agent::db::AhaSettingsV2,
        context: crate::agent::db::AgentContext,
    ) {
        let ctx_config = match context {
            crate::agent::db::AgentContext::Project => &settings.project,
            crate::agent::db::AgentContext::Chat => &settings.chat,
        };
        let shared = &settings.shared;

        let active_chat = ctx_config
            .chat_model_configs
            .iter()
            .find(|c| c.active)
            .or_else(|| ctx_config.chat_model_configs.first());
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
        let active_image = shared
            .image_model_configs
            .iter()
            .find(|c| c.active)
            .or_else(|| shared.image_model_configs.first());
        let active_image_edit = shared
            .image_edit_model_configs
            .iter()
            .find(|c| c.active)
            .or_else(|| shared.image_edit_model_configs.first())
            .or(active_image);

        if let Some(chat) = active_chat {
            let mut provider = self.provider.lock();
            *provider = OpenAiCompatProvider::new(
                if chat.api_key.is_empty() {
                    self.config.api_key.clone()
                } else {
                    chat.api_key.clone()
                },
                if chat.url.is_empty() {
                    self.config.api_base.clone()
                } else {
                    chat.url.clone()
                },
                if chat.model.is_empty() {
                    self.config.model.clone()
                } else {
                    chat.model.clone()
                },
                self.config.max_tokens,
                self.config.temperature,
            );
        }

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
        if let Some(img) = active_image {
            if !img.url.trim().is_empty() {
                models.image_model_url = img.url.trim().to_string();
            }
            if !img.api_key.trim().is_empty() {
                models.image_model_api_key = img.api_key.trim().to_string();
            }
            if !img.model.trim().is_empty() {
                models.image_model = img.model.trim().to_string();
            }
        }
        if let Some(ie) = active_image_edit {
            if !ie.model.trim().is_empty() {
                models.image_edit_model = ie.model.trim().to_string();
            }
        }
        *self.allowed_tools.lock() = ctx_config.allowed_tools.clone();
    }

    pub fn auto_approve_dispatch(&self) -> bool {
        self.config.auto_approve_dispatch
    }

    pub fn set_auto_approve_dispatch(&mut self, value: bool) {
        self.config.auto_approve_dispatch = value;
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
                "检测到用户上传了图片，但 Dispatcher 设置中的视觉模型为空。请先配置视觉模型后重试。"
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

    // ─── Subprocess accessors ─────────────────────────────────────────────────

    pub(super) fn active_subprocesses_for_workspace(
        &self,
        workspace_id: &str,
    ) -> Vec<subprocess::RegisteredSubprocess> {
        self.subprocesses.list_for_workspace(workspace_id)
    }

    pub(super) fn agent_runtime_flags(
        &self,
        workspace_id: &str,
    ) -> Vec<(
        &'static str,
        bool,
        Option<subprocess::RegisteredSubprocessPhase>,
    )> {
        ["claude", "codex"]
            .into_iter()
            .map(|agent| {
                let entry = self
                    .active_subprocesses_for_workspace(workspace_id)
                    .into_iter()
                    .find(|item| item.agent == agent);
                let phase = entry.as_ref().map(|item| item.phase);
                (agent, entry.is_some(), phase)
            })
            .collect()
    }

    pub(super) fn mark_agent_exit_requested(&self, workspace_id: &str, agent: &str) {
        self.subprocesses
            .set_exit_requested_for(workspace_id, agent);
    }
}
