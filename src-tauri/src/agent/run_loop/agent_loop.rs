use anyhow::Result;

use super::super::common;
use super::super::db::DispatcherDb;
use super::super::llm::ChatMessage;

/// 单轮 LLM 交互的内存对话历史拥有者。
///
/// 解决的核心问题：避免每次工具调用后都重新从数据库加载全量历史。
/// 做法——每轮 run 开始时从 DB 加载一次历史作为起点，循环内只追加本轮
/// 新产生的消息（assistant 响应、tool 结果），不再回查 DB。
///
/// 系统提示契约：`request_messages()` 里的 system 消息来自构造时传入的
/// `system_prompt`（本轮 run 的初始快照），公共 `run_loop` 本身不会重建
/// 系统提示。需要动态内容（系统时间、工具可见性、子智能体列表等）的 Agent
/// 必须每轮迭代自行重建：项目 Agent 以 `request_messages()` + `skip(1)`
/// 替换旧 system；普通聊天 Agent 以 `history()` 取纯历史并前置重建后的
/// system 消息。
///
/// 持久化契约：`append` 只做内存追加，不写 DB。调用方必须在追加前已把同一
/// 消息同步落库（persist_tool_calls_message / persist_tool_result_with_compression
/// 等），否则崩溃 / 重启 / 切换会话后本轮消息即丢失，AgentLoop 不做校验或兜底。
///
/// 被 OrchestratorAgent 和 PlainChatAgent 共同复用，是两者的历史管理抽象。
pub(crate) struct AgentLoop {
    system_prompt: String,
    messages: Vec<ChatMessage>,
}

impl AgentLoop {
    /// 从数据库加载已有对话历史，作为本轮循环的起点。
    /// system_prompt 仅为占位，真正发给模型前会被 Agent 每轮重建覆盖。
    pub async fn new(db: &DispatcherDb, workspace_id: &str, system_prompt: String) -> Result<Self> {
        // 仅加载最近若干轮对话（MAX_LLM_DIALOGUES=5），过滤掉纯调度 plumbing 消息
        let messages = db.load_llm_history_async(workspace_id).await?;
        Ok(Self {
            system_prompt,
            messages,
        })
    }

    /// 组装发给 LLM 的完整消息序列：系统提示 + 对话历史。
    ///
    /// 注意：system 消息是构造时的初始快照；需要每轮动态系统提示的 Agent
    /// 应改用 `history()` 并自行前置重建后的 system 消息。
    pub fn request_messages(&self) -> Vec<ChatMessage> {
        let mut messages = vec![ChatMessage::system(self.system_prompt.clone())];
        messages.extend(self.messages.iter().cloned());
        messages
    }

    /// 纯对话历史（不含 system 消息），供每轮自行重建系统提示的 Agent 使用。
    pub fn history(&self) -> &[ChatMessage] {
        &self.messages
    }

    /// 仅追加本轮循环产生的新消息（assistant 工具调用、tool 结果等），
    /// 避免每次都 reload 全量历史——这是该抽象的性能关键点。
    ///
    /// 追加时套用与 DB 加载一致的过滤（`common::should_keep_llm_message`，
    /// 与 `load_llm_history` 同口径）：纯调度 plumbing 工具结果与 process-only
    /// assistant 消息不进入上下文，保证「同 run 多轮迭代」与「新 run 从 DB
    /// 重新加载」上下文口径一致。
    ///
    /// 强契约：调用方必须在调用本方法前已把同一消息同步落库（见类型级文档）。
    pub fn append(&mut self, message: ChatMessage) {
        if common::should_keep_llm_message(&message) {
            self.messages.push(message);
        }
    }

    /// 估算上下文 token 占用（4 字符/token 启发式），仅用于诊断告警，
    /// 当前不会据此压缩历史。
    pub fn estimated_tokens(&self) -> u64 {
        DispatcherDb::estimate_context_tokens(&self.messages)
    }
}
