//! 模型参数、预算和宿主协议配置。
use super::ProtocolToolHandler;
use crate::agent::db::DispatcherSessionTokenUsageSource;
use crate::agent::rig_ext::model::{ModelSelectionHandle, PurposeModelSpec};
use crate::agent::rig_ext::tools::MAX_TOOL_CALLS_PER_BATCH;

// ─── 循环钩子 ─────────────────────────────────────────────────────────────────

/// 空响应（无正文且无工具调用）的诊断上下文，供 `empty_response_error` 构造文案。
pub struct RigTurnDiagnostics {
    pub model_name: String,
    pub finish_reason: Option<String>,
    pub thinking_chars: usize,
    pub completion_tokens: Option<u64>,
}

/// 循环的可注入钩子（Phase 3 各 agent 的差异点集中于此）。
pub struct RigLoopHooks {
    /// 最大工具迭代次数（对齐 `DispatcherAgentConfig::max_tool_iterations`）。
    pub max_iterations: usize,
    /// 单轮工具调用数上限（对齐 `MAX_TOOL_CALLS_PER_BATCH`）。
    pub max_tool_calls_per_batch: usize,
    /// 请求采样/容量参数：模型非 `PurposeSwitchingModel` 时生效；
    /// 是 PurposeSwitchingModel 时以命中槽位为准（见 model.rs 的 tune_request）。
    pub request_max_tokens: Option<u64>,
    pub request_temperature: f64,
    pub request_enable_thinking: bool,
    /// 每轮迭代的系统提示（preamble）。普通聊天每轮重建系统提示（G9-17：
    /// 系统时间/分类上下文等动态内容不随 run 陈旧），故为闭包而非静态值。
    pub preamble_for_iteration: Option<Box<dyn FnMut(usize) -> Option<String> + Send + Sync>>,
    /// 取消收口文案：输入已流出的部分正文，输出落库的 assistant 文本。
    pub cancelled_reply: Box<dyn Fn(&str) -> String + Send + Sync>,
    /// 空响应错误构造。
    pub empty_response_error: Box<dyn Fn(&RigTurnDiagnostics) -> String + Send + Sync>,
    /// 达到迭代上限的错误文案（None 用默认）。
    pub max_iterations_error: Option<String>,
    /// 用量落库来源（primary / summary）。
    pub usage_source: DispatcherSessionTokenUsageSource,
    /// `PurposeSwitchingModel` 的选择探测句柄：ModelSwitched 事件与真实用量模型名。
    pub model_selection: Option<ModelSelectionHandle>,
    /// 无探测句柄时的模型名（用量落库 / 诊断）。
    pub default_model_name: String,
    /// 上下文窗口容量（tokens），随用量落库。
    pub context_window: Option<u64>,
    /// 协议工具处理器（编排器注入：submit_graph / graph_plan_report / message）。
    /// None（聊天路径）= 全部工具按普通工具执行。
    pub protocol_handler: Option<std::sync::Arc<dyn ProtocolToolHandler>>,
}

impl RigLoopHooks {
    /// 以聊天槽位规格构造默认钩子（文案对齐 plain_chat 语义）。
    pub fn from_chat_spec(spec: &PurposeModelSpec) -> Self {
        Self {
            max_iterations: 200,
            max_tool_calls_per_batch: MAX_TOOL_CALLS_PER_BATCH,
            request_max_tokens: spec.max_tokens,
            request_temperature: spec.temperature,
            request_enable_thinking: spec.enable_thinking,
            preamble_for_iteration: None,
            cancelled_reply: Box::new(default_cancelled_reply),
            empty_response_error: Box::new(default_empty_response_error),
            max_iterations_error: None,
            usage_source: DispatcherSessionTokenUsageSource::Primary,
            model_selection: None,
            default_model_name: spec.model.clone(),
            context_window: spec.context_window,
            protocol_handler: None,
        }
    }
}

/// 对齐旧 `build_stopped_plain_chat_reply` 的取消收口文案。
fn default_cancelled_reply(partial: &str) -> String {
    let trimmed = partial.trim();
    if trimmed.is_empty() {
        "⏹️ 本轮聊天已停止。当前会话上下文已保留，可稍后继续。".to_string()
    } else {
        format!(
            "{}\n\n[本轮聊天已手动停止。当前会话上下文与以上输出均已保留，可稍后继续。]",
            trimmed
        )
    }
}

fn default_empty_response_error(diagnostics: &RigTurnDiagnostics) -> String {
    format!(
        "LLM 返回了空响应且没有工具调用，无法继续执行。\n请求摘要：model={}\n诊断：finish_reason={}，思考链={} 字符，completion_tokens={}",
        diagnostics.model_name,
        diagnostics.finish_reason.as_deref().unwrap_or("<未提供>"),
        diagnostics.thinking_chars,
        diagnostics
            .completion_tokens
            .map(|tokens| tokens.to_string())
            .unwrap_or_else(|| "<未上报>".to_string()),
    )
}
