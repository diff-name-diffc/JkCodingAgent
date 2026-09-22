//! 模型工厂（T1.1）：用途槽位解析 → rig OpenAI 兼容模型；
//! `PurposeSwitchingModel`（chat/vision 委托模型，rig 文档化扩展点）；
//! 请求组装助手。
//!
//! 槽位解析规则逐行对齐 `PlainChatAgent::apply_settings_v2`
//! （`agents/plain_chat/mod.rs:146`）：库条目（读取时已回填凭据与容量）为
//! 权威源；聊天槽位凭据为空回退 `DispatcherAgentConfig`，视觉槽位凭据为空
//! 回退聊天槽位，摘要槽位凭据为空回退聊天槽位、模型为空回退
//! `DEFAULT_SUMMARY_MODEL`。不发明新规则。

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rig::client::CompletionClient;
use rig::completion::{
    CompletionModel, CompletionRequest, CompletionResponse, ToolDefinition,
};
use rig::message::{Message, UserContent};
use rig::providers::openai::completion::CompletionModel as OpenAiCompletionModel;
use rig::providers::openai::CompletionsClient;
use rig::streaming::StreamingCompletionResponse;
use rig::completion::CompletionError;

use crate::agent::config::{DispatcherAgentConfig, DEFAULT_SUMMARY_MODEL};
use crate::agent::db::{AgentContext, AhaSettingsV2};

/// LLM HTTP 超时：与旧 `OpenAiCompatProvider` 一致（300s，
/// 见 `llm/provider.rs` 的 `Client::builder().timeout(...)`）。
const LLM_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// 一个用途槽位解析后的完整模型规格（凭据 + 容量 + 采样参数）。
///
/// `max_tokens` / `context_window` 的 None 语义与现状一致：未配置 → 请求体
/// 省略 max_tokens、由服务端默认预算接管；窗口容量消费方回退 1M 默认值。
#[derive(Debug, Clone)]
pub struct PurposeModelSpec {
    pub api_key: String,
    pub api_base: String,
    pub model: String,
    pub max_tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub temperature: f64,
    /// 是否允许模型输出思考链（推理模型）。默认 true 保持既有行为；
    /// false 时经 `additional_params` 注入 DashScope 方言
    /// `{"enable_thinking": false}`（见 `build_completion_request`）。
    pub enable_thinking: bool,
}

impl PurposeModelSpec {
    /// 与 `OpenAiCompatProvider::is_configured` 同口径。
    pub fn is_configured(&self) -> bool {
        !self.api_key.trim().is_empty()
    }
}

/// chat / vision / summary 三个用途槽位的解析产物。
#[derive(Debug, Clone)]
pub struct PurposeModelSpecs {
    pub chat: PurposeModelSpec,
    /// None = 未配置视觉用途（或视觉条目模型名为空）。
    pub vision: Option<PurposeModelSpec>,
    pub summary: PurposeModelSpec,
}

/// 从 `AhaSettingsV2` 解析三个用途槽位，规则对齐 `apply_settings_v2`：
/// - active 条目 = 列表中 `active` 者，缺省取第一个；
/// - 聊天槽位：url/apiKey/model 为空回退 `config`；容量
///   `chat.max_tokens.or(config.max_tokens)`；temperature 取 `config`；
/// - 视觉槽位：模型名为空视为未配置；url/apiKey 为空回退**聊天槽位解析结果**
///   （视觉模型可能部署在独立网关，只换模型名会打错网关）；
/// - 摘要槽位：model 为空回退 `DEFAULT_SUMMARY_MODEL`，url/apiKey 为空回退
///   聊天槽位；容量无条件取槽位值（None=未配置也是有效语义）再 or config 兜底。
pub fn resolve_purpose_specs(
    settings: &AhaSettingsV2,
    context: AgentContext,
    config: &DispatcherAgentConfig,
) -> PurposeModelSpecs {
    let ctx_config = match context {
        AgentContext::Project => &settings.project,
        AgentContext::Chat => &settings.chat,
    };

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
    let active_vision = settings
        .shared
        .vision_model_configs
        .iter()
        .find(|c| c.active)
        .or_else(|| settings.shared.vision_model_configs.first());

    let temperature = f64::from(config.temperature);

    let chat = match active_chat {
        Some(entry) => PurposeModelSpec {
            api_key: if entry.api_key.is_empty() {
                config.api_key.clone()
            } else {
                entry.api_key.clone()
            },
            api_base: if entry.url.is_empty() {
                config.api_base.clone()
            } else {
                entry.url.clone()
            },
            model: if entry.model.is_empty() {
                config.model.clone()
            } else {
                entry.model.clone()
            },
            // 容量以槽位（库条目回填）为权威；config 仅作 env 开发路径兜底。
            max_tokens: entry.max_tokens.or(config.max_tokens).map(u64::from),
            context_window: entry.context_window.map(u64::from),
            temperature,
            enable_thinking: true,
        },
        // 无任何聊天槽位配置：等价于 apply_settings_v2 不触达 provider，
        // 保留 `PlainChatAgent::new` 时用 config 构建的初始形态。
        None => PurposeModelSpec {
            api_key: config.api_key.clone(),
            api_base: config.api_base.clone(),
            model: config.model.clone(),
            max_tokens: config.max_tokens.map(u64::from),
            context_window: None,
            temperature,
            enable_thinking: true,
        },
    };

    let vision = active_vision
        .filter(|v| !v.model.trim().is_empty())
        .map(|v| PurposeModelSpec {
            api_key: if v.api_key.trim().is_empty() {
                chat.api_key.clone()
            } else {
                v.api_key.trim().to_string()
            },
            api_base: if v.url.trim().is_empty() {
                chat.api_base.clone()
            } else {
                v.url.trim().to_string()
            },
            model: v.model.trim().to_string(),
            max_tokens: v.max_tokens.or(config.max_tokens).map(u64::from),
            context_window: v.context_window.map(u64::from),
            temperature,
            enable_thinking: true,
        });

    let summary = PurposeModelSpec {
        model: active_summary
            .map(|s| s.model.trim().to_string())
            .filter(|model| !model.is_empty())
            .unwrap_or_else(|| DEFAULT_SUMMARY_MODEL.to_string()),
        api_key: active_summary
            .map(|s| s.api_key.trim().to_string())
            .filter(|key| !key.is_empty())
            .unwrap_or_else(|| chat.api_key.clone()),
        api_base: active_summary
            .map(|s| s.url.trim().to_string())
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| chat.api_base.clone()),
        // 容量无条件覆盖：None=未配置也是有效语义，库条目清空容量后不得残留旧值。
        max_tokens: active_summary
            .and_then(|s| s.max_tokens)
            .or(config.max_tokens)
            .map(u64::from),
        context_window: active_summary.and_then(|s| s.context_window).map(u64::from),
        temperature,
        enable_thinking: true,
    };

    PurposeModelSpecs {
        chat,
        vision,
        summary,
    }
}

/// 从槽位规格构建 rig OpenAI 兼容补全模型。
///
/// 注入自定义 reqwest 客户端以对齐现状 300s 超时（rig 默认
/// `reqwest::Client::default()` 无超时，长流式响应下行为不一致）。
pub fn completions_model(spec: &PurposeModelSpec) -> Result<OpenAiCompletionModel> {
    let http_client = reqwest::Client::builder()
        .timeout(LLM_HTTP_TIMEOUT)
        .build()
        .context("构建 LLM HTTP 客户端失败")?;
    let client = CompletionsClient::builder()
        .api_key(spec.api_key.clone())
        .base_url(spec.api_base.clone())
        .http_client(http_client)
        .build()
        .map_err(|error| anyhow::anyhow!("构建 OpenAI 兼容客户端失败：{error}"))?;
    Ok(client.completion_model(spec.model.clone()))
}

/// 一次请求实际命中的模型选择（`PurposeSwitchingModel` 的探测记录）。
#[derive(Debug, Clone)]
pub struct ModelSelection {
    /// 实际委托的模型名（用于用量落库与 `ModelSwitched` 事件）。
    pub model_name: String,
    /// 本次是否走了视觉槽位。
    pub used_vision: bool,
    /// 视觉槽位与聊天槽位是否为同一 provider（模型名/网关/密钥三者一致）。
    /// 对齐 `select_provider_for_messages` 的 `same_provider` 判定：一致不通知。
    pub differs_from_chat: bool,
}

/// 模型选择探测句柄：`PurposeSwitchingModel` 每次委托前写入，循环读取以
/// 发出 `AgentEvent::ModelSwitched` 并以真实模型名落库用量。
#[derive(Debug, Clone, Default)]
pub struct ModelSelectionHandle(Arc<Mutex<Option<ModelSelection>>>);

impl ModelSelectionHandle {
    /// 最近一次选择（Mutex 中毒时取回内部值，不 panic——锁只保护探测记录，
    /// 中毒不影响正确性语义）。
    pub fn last(&self) -> Option<ModelSelection> {
        self.0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    fn record(&self, selection: ModelSelection) {
        *self
            .0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(selection);
    }
}

/// 按请求是否含图片在 chat / vision 模型间委托的 `CompletionModel`
/// （rig 文档化扩展点：自定义 CompletionModel）。
///
/// 对应旧实现 `select_provider_for_messages`：消息含图且有视觉槽位 → 视觉
/// 模型（可能部署在独立网关，整请求委托而非只换模型名）；含图但视觉未配置
/// → 请求级错误（fail-closed，对齐旧 bail 文案）；否则 → 聊天模型。
/// 委托前以命中槽位的容量/采样参数覆盖请求字段（对齐旧行为：max_tokens /
/// temperature / enable_thinking 挂在 provider 上而非请求上）。
#[derive(Clone)]
pub struct PurposeSwitchingModel {
    chat: Arc<OpenAiCompletionModel>,
    vision: Option<Arc<OpenAiCompletionModel>>,
    chat_spec: PurposeModelSpec,
    vision_spec: Option<PurposeModelSpec>,
    selection: ModelSelectionHandle,
}

impl PurposeSwitchingModel {
    pub fn new(chat: OpenAiCompletionModel, vision: Option<OpenAiCompletionModel>, specs: &PurposeModelSpecs) -> Self {
        Self {
            chat: Arc::new(chat),
            vision: vision.map(Arc::new),
            chat_spec: specs.chat.clone(),
            vision_spec: specs.vision.clone(),
            selection: ModelSelectionHandle::default(),
        }
    }

    /// 从槽位规格一站式构建（chat/vision 各自独立客户端）。
    pub fn from_specs(specs: &PurposeModelSpecs) -> Result<Self> {
        let chat = completions_model(&specs.chat)?;
        let vision = specs
            .vision
            .as_ref()
            .map(completions_model)
            .transpose()?;
        Ok(Self::new(chat, vision, specs))
    }

    pub fn selection_handle(&self) -> ModelSelectionHandle {
        self.selection.clone()
    }

    pub fn chat_model_name(&self) -> &str {
        &self.chat_spec.model
    }

    pub fn chat_spec(&self) -> &PurposeModelSpec {
        &self.chat_spec
    }

    /// 选出本次请求的委托目标并记录探测结果。
    fn select(
        &self,
        messages: &[Message],
    ) -> std::result::Result<(&OpenAiCompletionModel, &PurposeModelSpec), CompletionError> {
        if messages_contain_images(messages) {
            let (Some(vision), Some(vision_spec)) = (&self.vision, &self.vision_spec) else {
                // 对齐 `select_provider_for_messages` 的 fail-closed 文案。
                return Err(CompletionError::RequestError(
                    anyhow::anyhow!(
                        "检测到用户上传了图片，但视觉模型未配置。请先在设置中配置视觉模型后重试。"
                    )
                    .into(),
                ));
            };
            self.selection.record(ModelSelection {
                model_name: vision_spec.model.clone(),
                used_vision: true,
                differs_from_chat: vision_spec.model != self.chat_spec.model
                    || vision_spec.api_base != self.chat_spec.api_base
                    || vision_spec.api_key != self.chat_spec.api_key,
            });
            Ok((vision, vision_spec))
        } else {
            self.selection.record(ModelSelection {
                model_name: self.chat_spec.model.clone(),
                used_vision: false,
                differs_from_chat: false,
            });
            Ok((&self.chat, &self.chat_spec))
        }
    }

    /// 以命中槽位的容量/采样参数覆盖请求（见类型级文档）。
    fn tune_request(&self, spec: &PurposeModelSpec, mut request: CompletionRequest) -> CompletionRequest {
        request.max_tokens = spec.max_tokens;
        request.temperature = Some(spec.temperature);
        if !spec.enable_thinking {
            request.additional_params = Some(inject_enable_thinking(request.additional_params.take()));
        }
        request
    }
}

impl CompletionModel for PurposeSwitchingModel {
    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> std::result::Result<CompletionResponse, CompletionError> {
        let (model, spec) = self.select(&request.chat_history)?;
        model.completion(self.tune_request(spec, request)).await
    }

    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> std::result::Result<StreamingCompletionResponse, CompletionError> {
        let (model, spec) = self.select(&request.chat_history)?;
        model.stream(self.tune_request(spec, request)).await
    }
}

/// 消息列表是否含图片（任一 user 消息携带 `UserContent::Image`）。
/// 口径与旧 `llm::messages_contain_images` 一致（全历史扫描，非仅尾部）。
pub fn messages_contain_images(messages: &[Message]) -> bool {
    messages.iter().any(|message| {
        matches!(message, Message::User { content }
            if content.iter().any(|item| matches!(item, UserContent::Image(_))))
    })
}

/// 组装 `CompletionRequest`。`enable_thinking=false` 时经
/// `additional_params` 注入 DashScope 方言 `{"enable_thinking": false}`
/// （对齐旧 provider 的 `enable_thinking` 请求字段）；true 时不注入——
/// 严格 OpenAI 兼容服务会对未知参数 400，旧实现靠 400 重试剔除该字段，
/// rig 无重试通道，只在校真需要关闭思考时才携带。
///
/// 流式 `stream_options.include_usage` 由 rig 的 openai streaming 自动注入，
/// 无需在此设置。
pub fn build_completion_request(
    preamble: Option<String>,
    messages: Vec<Message>,
    tools: Vec<ToolDefinition>,
    max_tokens: Option<u64>,
    temperature: f64,
    enable_thinking: bool,
) -> CompletionRequest {
    CompletionRequest {
        model: None,
        preamble,
        chat_history: messages,
        documents: Vec::new(),
        tools,
        temperature: Some(temperature),
        max_tokens,
        tool_choice: None,
        additional_params: if enable_thinking {
            None
        } else {
            Some(inject_enable_thinking(None))
        },
        output_schema: None,
        record_telemetry_content: false,
    }
}

/// 向 additional_params 合并 `{"enable_thinking": false}`（不覆盖调用方已有键
/// 以外的内容；rig `json_utils::merge` 为浅合并，enable_thinking 必定落为 false）。
fn inject_enable_thinking(existing: Option<serde_json::Value>) -> serde_json::Value {
    let flag = serde_json::json!({ "enable_thinking": false });
    match existing {
        Some(params) => rig::json_utils::merge(params, flag),
        None => flag,
    }
}
