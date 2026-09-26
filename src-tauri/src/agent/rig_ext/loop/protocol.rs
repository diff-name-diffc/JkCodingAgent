//! 协议工具拦截（编排器收口路径）。
//!
//! 与普通工具的区别：协议工具（`submit_graph` / `graph_plan_report` /
//! `message`）在模型侧是**壳工具**——定义照常发给模型，但执行不留真值，
//! 真正的动作（校验/落库/广播/收口）由宿主拦截完成：按工具名分派的动作
//! 落在可注入的处理器里，循环只负责「有动作/最终答复则收口、只有可重试
//! 错误则继续」的通用规则。

use serde_json::Value;

/// 协议动作：宿主拦截到的收口信号。
#[derive(Clone, Debug)]
pub enum RigProtocolAction {
    /// 编排器已产出执行图并登记为待确认计划，等待用户在图面板确认。
    GraphSubmitted { title: String, node_count: usize },
}

/// 一次协议工具调用的拦截结果。
pub struct RigProtocolResult {
    /// 回灌给模型/前端的文本（壳工具回显、报告正文、或「错误：」前缀的拒绝原因）。
    pub text: String,
    /// 可重试错误：本轮不收口，让模型修正后重试（如 submit_graph 校验失败）。
    pub retryable_error: bool,
    /// 协议动作（如图已提交），非空即收口。
    pub actions: Vec<RigProtocolAction>,
    /// 最终答复（`message` 工具），无动作时据此收口。
    pub final_message: Option<String>,
}

impl RigProtocolResult {
    pub fn text_feedback(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            retryable_error: false,
            actions: Vec::new(),
            final_message: None,
        }
    }

    pub fn retryable_error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            retryable_error: true,
            actions: Vec::new(),
            final_message: None,
        }
    }
}

/// 协议工具处理器：按工具名拦截，返回 None 表示「非协议工具，走正常执行」。
///
/// `handles` 由各实现显式声明拦截集合（无默认实现）：默认值只能内嵌编排器
/// 专属工具名，对其它实现是错误契约，且与决策层的收口名单构成第二处硬编码。
#[async_trait::async_trait]
pub trait ProtocolToolHandler: Send + Sync {
    /// 本处理器拦截的工具名；未命中的调用走正常工具执行路径。
    fn handles(&self, name: &str) -> bool;

    async fn handle(&self, tool_name: &str, arguments: &Value) -> Option<RigProtocolResult>;

    /// 本轮收口文案：输入协议动作与最终答复，输出落库的 assistant 文本；
    /// 返回 None 表示不收口（继续循环）。
    async fn render_outcome(
        &self,
        actions: &[RigProtocolAction],
        final_message: Option<&str>,
    ) -> Option<String>;
}
