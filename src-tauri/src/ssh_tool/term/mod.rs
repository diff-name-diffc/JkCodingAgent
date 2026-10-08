//! SSH 交互终端工具组（ssh_term_*）的域层：屏幕模型、会话与注册表。
//!
//! 设计文档：`docs/ssh-term-agent-2026-10-08.md`。核心原则「tmux 优先、avt 兜底」：
//! open 默认探测远端 tmux，存在则 attach-or-create（交互程序由远端 tmux daemon
//! 保活，断连后同名 open 可恢复现场）；否则回退 russh PTY + avt 自建虚拟终端。

pub(crate) mod registry;
pub(crate) mod screen;
pub(crate) mod session;

#[cfg(test)]
mod tests;

pub(crate) use registry::{TermError, TermOpenParams, TermSessionRegistry, TmuxPreference};
pub(crate) use screen::{DEFAULT_COLS, DEFAULT_ROWS};

use std::sync::OnceLock;

/// 进程级共享单例：调度器 claims 解析（term_id → server_id）深处没有依赖注入
/// 通道，全局入口是唯一可行形态；由 `DispatcherState` 构造时登记一次。
/// 应用生命周期与进程同寿，无清理需求。
static GLOBAL_REGISTRY: OnceLock<TermSessionRegistry> = OnceLock::new();

pub(crate) fn set_global_registry(registry: TermSessionRegistry) {
    let _ = GLOBAL_REGISTRY.set(registry);
}

/// 全局终端会话表（未登记时 None——测试或极早启动期）。
pub(crate) fn global_registry() -> Option<&'static TermSessionRegistry> {
    GLOBAL_REGISTRY.get()
}

use chrono::{DateTime, Utc};

/// 终端会话标识（`term_` 前缀 + uuid 片段）。
pub(crate) type TermId = String;

/// tmux 会话名前缀：级联清理按此前缀识别 agent 遗留会话，避免误伤用户会话。
pub(crate) const TMUX_SESSION_PREFIX: &str = "jkagent-";

/// `ssh_term_list` 的单条摘要。
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct TermInfo {
    pub(crate) term_id: TermId,
    pub(crate) server_id: String,
    pub(crate) session_id: String,
    /// 实际 tmux 会话名；null = 裸终端（无 tmux）。
    pub(crate) tmux_session: Option<String>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) last_activity_at: DateTime<Utc>,
    pub(crate) exited: bool,
}

/// `ssh_term_read` 的双轨载荷。
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct TermReadPayload {
    pub(crate) term_id: TermId,
    /// 自上次 read 以来定稿的新增行（增量轨）。
    pub(crate) new_lines: Vec<String>,
    /// 当前可见屏幕纯文本（快照轨，含全屏程序渲染结果）。
    pub(crate) screen: String,
    pub(crate) cursor: TermCursor,
    pub(crate) alt_screen: bool,
    pub(crate) tmux_session: Option<String>,
    pub(crate) exited: bool,
    pub(crate) exit_code: Option<i32>,
    /// 距上一帧输出的毫秒数，辅助判断「停在提示符等输入」。
    pub(crate) idle_ms: u64,
    pub(crate) truncated: bool,
    /// 断连 / detach 等语义提示（含 tmux 恢复指引）。
    pub(crate) note: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct TermCursor {
    pub(crate) row: usize,
    pub(crate) col: usize,
}

/// `ssh_term_open` / `ssh_term_close` 的返回。
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct TermHandlePayload {
    pub(crate) term_id: TermId,
    pub(crate) screen: String,
    pub(crate) cursor: TermCursor,
    pub(crate) tmux_session: Option<String>,
    pub(crate) exited: bool,
    pub(crate) exit_code: Option<i32>,
    pub(crate) note: Option<String>,
}

/// 校验 tmux 会话名：`jkagent-` 前缀 + `[A-Za-z0-9_-]{1,48}`，注入面封闭
/// （模板命令 `tmux new -A -s <name>` 的唯一模型可控输入，设计文档 §7）。
pub(crate) fn validate_tmux_session_name(name: &str) -> Result<(), String> {
    let rest = name.strip_prefix(TMUX_SESSION_PREFIX).ok_or_else(|| {
        format!(
            "错误：tmuxSession 必须以 {TMUX_SESSION_PREFIX} 前缀开头（如 jkagent-install-nginx）"
        )
    })?;
    if rest.is_empty() || rest.len() > 48 {
        return Err("错误：tmuxSession 前缀后的名称长度须为 1..48".into());
    }
    if !rest
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("错误：tmuxSession 仅允许字母、数字、-、_".into());
    }
    Ok(())
}

#[cfg(test)]
mod name_tests {
    use super::*;

    #[test]
    fn tmux_session_name_validation() {
        assert!(validate_tmux_session_name("jkagent-install-nginx").is_ok());
        assert!(validate_tmux_session_name("jkagent-a").is_ok());
        assert!(validate_tmux_session_name("agent-x").is_err()); // 缺前缀
        assert!(validate_tmux_session_name("jkagent-").is_err()); // 空名
        assert!(validate_tmux_session_name("jkagent-a b").is_err()); // 空格
        assert!(validate_tmux_session_name("jkagent-a;rm").is_err()); // 元字符
        assert!(validate_tmux_session_name(&format!("jkagent-{}", "x".repeat(49))).is_err());
    }
}
