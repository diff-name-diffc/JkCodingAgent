use anyhow::Result;

use super::super::db::DispatcherDb;
use super::super::llm::ChatMessage;

/// 单轮 LLM 交互的内存对话历史拥有者。
///
/// 解决的核心问题：避免每次工具调用后都重新从数据库加载全量历史。
/// 做法——每轮 run 开始时从 DB 加载一次历史作为起点，循环内只追加本轮
/// 新产生的消息（assistant 响应、tool 结果），不再回查 DB。
///
/// 注意：系统提示虽存在此结构里，但 `run_llm_loop` 每轮会重建动态系统
/// 提示并替换掉这里的 system 消息（通过 `skip(1)` 跳过旧 system），
/// 保证动态分片（工具可见性、子进程状态等）始终是最新的。
///
/// 被 DispatcherAgent 和 PlainChatAgent 共同复用，是两者的历史管理抽象。
pub(crate) struct AgentLoop {
    system_prompt: String,
    messages: Vec<ChatMessage>,
}

impl AgentLoop {
    /// 从数据库加载已有对话历史，作为本轮循环的起点。
    /// system_prompt 仅为占位，真正发给模型前会被 run_loop 重建覆盖。
    pub async fn new(db: &DispatcherDb, workspace_id: &str, system_prompt: String) -> Result<Self> {
        // 仅加载最近若干轮对话（MAX_LLM_DIALOGUES=5），过滤掉纯调度 plumbing 消息
        let messages = db.load_llm_history_async(workspace_id).await?;
        Ok(Self {
            system_prompt,
            messages,
        })
    }

    /// 组装发给 LLM 的完整消息序列：系统提示 + 对话历史。
    pub fn request_messages(&self) -> Vec<ChatMessage> {
        let mut messages = vec![ChatMessage::system(self.system_prompt.clone())];
        messages.extend(self.messages.iter().cloned());
        messages
    }

    /// 仅追加本轮循环产生的新消息（assistant 工具调用、tool 结果等），
    /// 避免每次都 reload 全量历史——这是该抽象的性能关键点。
    pub fn append(&mut self, message: ChatMessage) {
        self.messages.push(message);
    }

    /// 估算上下文 token 占用（4 字符/token 启发式），仅用于诊断告警，
    /// 当前不会据此压缩历史。
    pub fn estimated_tokens(&self) -> u64 {
        DispatcherDb::estimate_context_tokens(&self.messages)
    }
}
