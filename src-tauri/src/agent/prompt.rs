use chrono::Local;

pub(super) fn current_local_time() -> String {
    Local::now().format("%Y-%m-%d %H:%M").to_string()
}

/// 单轮 run 内缓存的静态提示词内容（不含运行态动态分片）。
///
/// 系统提示重建契约：公共 `run_loop` 本身不会重建系统提示，而是由各 Agent
/// 在 `build_iteration_messages` 中每轮迭代自行重建完整系统提示——以静态内容
/// 为底，追加动态分片（系统时间、可用工具、可用执行 Agent、子智能体列表等）。
/// 项目 Agent 以 `PromptBundle.static_content` 为静态底；普通聊天 Agent 以
/// 配置/内置提示词为底并每轮刷新动态内容（见 plain_chat 的
/// `build_iteration_messages`）。
#[derive(Debug, Clone)]
pub(super) struct PromptBundle {
    /// Content of static-only sections (no system time, no runtime state).
    pub static_content: String,
}
