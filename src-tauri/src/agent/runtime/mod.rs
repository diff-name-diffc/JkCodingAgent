pub(crate) mod helpers;
pub(crate) mod planning;
pub(crate) mod prompt;
pub(crate) mod protocol;
pub(crate) mod run_loop;
pub(crate) mod subprocess;
pub(crate) mod tool_exec;
pub(crate) mod types;

use std::sync::Arc;

use parking_lot::Mutex;
use tauri::AppHandle;

use super::config::DispatcherAgentConfig;
use super::llm::OpenAiCompatProvider;
use super::tools::ToolRegistry;
use crate::project::mcp::ProjectMcpRegistry;

pub(crate) use subprocess::DispatcherSubprocessRegistry;
pub use types::{AgentEvent, AgentTurn, DispatchFeedbackState};

// ─── Internal types ───────────────────────────────────────────────────────────

pub(super) struct Models {
    summary_model: String,
    summary_api_key: String,
    summary_api_base: String,
    vision_model: String,
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
            vision_model: self.vision_model.clone(),
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
    pub(super) sub_agent_manager: Option<Arc<super::sub_agent::SubAgentManager>>,
}

// ─── Construction & configuration ─────────────────────────────────────────────

impl DispatcherAgent {
    pub fn new(
        config: DispatcherAgentConfig,
        project_mcp_registry: ProjectMcpRegistry,
        subprocesses: Arc<DispatcherSubprocessRegistry>,
        sub_agent_manager: Option<Arc<super::sub_agent::SubAgentManager>>,
    ) -> Self {
        let provider = OpenAiCompatProvider::new(
            config.api_key.clone(),
            config.api_base.clone(),
            config.model.clone(),
            config.max_tokens,
            config.temperature,
        );

        let mut registry = ToolRegistry::default_tools(project_mcp_registry.clone());
        if let Some(manager) = &sub_agent_manager {
            registry.add_tool(Box::new(super::sub_agent::SubAgentTool::new(Arc::clone(
                manager,
            ))));
            registry.add_tool(Box::new(super::sub_agent::ListSubAgentsTool::new(
                Arc::clone(manager),
            )));
        }

        Self {
            models: Mutex::new(Models {
                summary_model: helpers::normalize_summary_model(&config.summary_model),
                summary_api_key: String::new(),
                summary_api_base: String::new(),
                vision_model: config.vision_model.trim().to_string(),
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

    pub fn apply_settings(&self, settings: &super::db::DispatcherSettingsRecord) {
        {
            let mut provider = self.provider.lock();
            *provider = OpenAiCompatProvider::new(
                if settings.api_key.is_empty() {
                    self.config.api_key.clone()
                } else {
                    settings.api_key.clone()
                },
                if settings.api_base.is_empty() {
                    self.config.api_base.clone()
                } else {
                    settings.api_base.clone()
                },
                if settings.model.is_empty() {
                    self.config.model.clone()
                } else {
                    settings.model.clone()
                },
                self.config.max_tokens,
                self.config.temperature,
            );
        }
        let mut models = self.models.lock();
        if !settings.summary_model.trim().is_empty() {
            models.summary_model = helpers::normalize_summary_model(&settings.summary_model);
        }
        let smc = &settings.summary_model_config;
        if !smc.api_key.trim().is_empty() {
            models.summary_api_key = smc.api_key.trim().to_string();
        }
        if !smc.url.trim().is_empty() {
            models.summary_api_base = smc.url.trim().to_string();
        }
        if !settings.vision_model.trim().is_empty() {
            models.vision_model = settings.vision_model.trim().to_string();
        }
        if !settings.image_model_url.trim().is_empty() {
            models.image_model_url = settings.image_model_url.trim().to_string();
        }
        if !settings.image_model_api_key.trim().is_empty() {
            models.image_model_api_key = settings.image_model_api_key.trim().to_string();
        }
        if !settings.image_model.trim().is_empty() {
            models.image_model = settings.image_model.trim().to_string();
        }
        if !settings.image_edit_model.trim().is_empty() {
            models.image_edit_model = settings.image_edit_model.trim().to_string();
        }
        *self.allowed_tools.lock() = settings.allowed_tools.clone();
    }

    pub fn apply_settings_v2(
        &self,
        settings: &super::db::AhaSettingsV2,
        context: super::db::AgentContext,
    ) {
        let ctx_config = match context {
            super::db::AgentContext::Project => &settings.project,
            super::db::AgentContext::Chat => &settings.chat,
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
        if let Some(v) = active_vision {
            if !v.model.trim().is_empty() {
                models.vision_model = v.model.trim().to_string();
            }
        }
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

    pub(super) fn vision_model(&self) -> String {
        self.models.lock().vision_model.clone()
    }

    pub(super) fn provider_for_messages(
        &self,
        provider: &OpenAiCompatProvider,
        messages: &[super::llm::ChatMessage],
        on_event: &tauri::ipc::Channel<AgentEvent>,
        notify_user: bool,
    ) -> anyhow::Result<OpenAiCompatProvider> {
        if !super::llm::messages_contain_inline_images(messages) {
            return Ok(provider.clone());
        }

        let vision_model = self.vision_model();
        if vision_model.trim().is_empty() {
            anyhow::bail!(
                "检测到用户上传了图片，但 Dispatcher 设置中的视觉模型为空。请先配置视觉模型后重试。"
            );
        }

        let selected = provider.with_model(vision_model.trim());
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
