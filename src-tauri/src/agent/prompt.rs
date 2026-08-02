use chrono::Local;

pub(super) fn current_local_time() -> String {
    Local::now().format("%Y-%m-%d %H:%M").to_string()
}

/// 单轮 run 内缓存的静态提示词内容（不含运行态动态分片）。
///
/// 公共 `run_loop` 每次迭代会以静态内容为底重建完整系统提示，
/// 动态分片（可用工具、可用执行 Agent 等）由各 Agent 自行追加。
#[derive(Debug, Clone)]
pub(super) struct PromptBundle {
    /// Content of static-only sections (no system time, no runtime state).
    pub static_content: String,
}
