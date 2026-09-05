//! 会话级命令执行历史（内存台账）：为安全审查模型提供「本会话已经做过什么」的执行上下文。
//!
//! 审查模型需要结合本会话已执行的命令（及结果）判断当前命令的上下文风险——
//! 典型场景：本任务先前命令派生的进程（开发服务器、测试任务），后续对其
//! `kill/pkill` 属于正当的自我清理；没有历史记录时这类命令只能因「来路不明」
//! 被拒绝。
//!
//! 设计取舍：
//! - 按 `workspace_id`（会话）为键，单会话条目与总会话数均有上限（LRU 淘汰），
//!   纯内存实现：任务派生进程的生命周期与应用进程基本同寿，无需持久化；
//!   同一轮的多次命令调用需要实时可见（审查发生在工具内部，轮内持续追加），
//!   因此不能只在轮开始时快照。
//! - 写入方为命令执行工具（exec / local_zsh / ssh_exec）；读取方为审查载荷
//!   组装（`tools/review_context.rs`）。
//! - 会话删除时调用 `forget_session` 同步清理，避免残留。

use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

use parking_lot::Mutex;

/// 单会话最多保留的历史条目数（超出从最旧开始丢弃）。
const MAX_ENTRIES_PER_SESSION: usize = 64;
/// 台账最多同时持有的会话数（超出按最久未写入淘汰）。
const MAX_SESSIONS: usize = 128;
/// 单条命令送审渲染的字符上限。
const MAX_COMMAND_CHARS: usize = 300;
/// 单条备注（结果摘要/拦截原因）送审渲染的字符上限。
const MAX_NOTE_CHARS: usize = 160;
/// 渲染进审查 prompt 的最大条目数（取最新）。
const MAX_RENDER_ENTRIES: usize = 24;
/// 渲染进审查 prompt 的总字符上限（自最新向前累计，超限丢弃较旧条目）。
const MAX_RENDER_CHARS: usize = 4000;

/// 命令在审查门禁视角下的结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandHistoryStatus {
    /// 审查通过并已执行（成败细节在 note 中）。
    Executed,
    /// 被安全审查判定拦截，未执行。
    Blocked,
}

/// 一条命令执行历史。字段在写入时即完成截断，读取侧无需再防超长。
#[derive(Debug, Clone)]
pub struct CommandHistoryEntry {
    /// 工具名（exec / local_zsh / ssh_exec）。
    pub tool: String,
    /// 执行目标的简短标签（工作区 / 本地 zsh / SSH server id 等）。
    pub target: String,
    pub command: String,
    pub status: CommandHistoryStatus,
    /// 结果摘要或拦截原因。
    pub note: String,
}

#[derive(Debug)]
struct SessionHistory {
    entries: VecDeque<CommandHistoryEntry>,
    last_write_ms: i64,
}

fn registry() -> &'static Mutex<HashMap<String, SessionHistory>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, SessionHistory>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut truncated: String = text.trim().chars().take(max_chars).collect();
    if text.trim().chars().count() > max_chars {
        truncated.push('…');
    }
    truncated
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 记录一条命令执行历史。任何情况下都不应失败或阻塞调用方——仅做有界内存写入。
pub fn record(
    workspace_id: &str,
    tool: &str,
    target: &str,
    command: &str,
    status: CommandHistoryStatus,
    note: &str,
) {
    let timestamp = now_ms();
    let entry = CommandHistoryEntry {
        tool: tool.to_string(),
        target: truncate_chars(target, 80),
        command: truncate_chars(command, MAX_COMMAND_CHARS),
        status,
        note: truncate_chars(note, MAX_NOTE_CHARS),
    };
    let mut map = registry().lock();
    // 容量治理：会话数超限时淘汰最久未写入的会话（持锁内仅做 O(n) 扫描，
    // n 有界且极小，不涉及 I/O）。
    if map.len() >= MAX_SESSIONS && !map.contains_key(workspace_id) {
        let oldest = map
            .iter()
            .min_by_key(|(_, history)| history.last_write_ms)
            .map(|(key, _)| key.clone());
        if let Some(key) = oldest {
            map.remove(&key);
        }
    }
    let history = map.entry(workspace_id.to_string()).or_insert_with(|| SessionHistory {
        entries: VecDeque::new(),
        last_write_ms: timestamp,
    });
    history.entries.push_back(entry);
    while history.entries.len() > MAX_ENTRIES_PER_SESSION {
        history.entries.pop_front();
    }
    history.last_write_ms = timestamp;
}

/// 渲染本会话的命令历史为审查上下文文本（时间正序）；无历史时返回 None。
pub fn render_for_review(workspace_id: &str) -> Option<String> {
    let recent: Vec<CommandHistoryEntry> = {
        let map = registry().lock();
        let history = map.get(workspace_id)?;
        history
            .entries
            .iter()
            .rev()
            .take(MAX_RENDER_ENTRIES)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    };
    if recent.is_empty() {
        return None;
    }
    let mut lines = Vec::with_capacity(recent.len());
    let mut total_chars = 0usize;
    // 自最新向前累计，超总字符上限则丢弃较旧条目，保证最近的命令一定可见。
    for (index, entry) in recent.iter().enumerate().rev() {
        let label = match entry.status {
            CommandHistoryStatus::Executed => "已执行",
            CommandHistoryStatus::Blocked => "已拦截",
        };
        let mut line = format!(
            "#{num} {tool}（{target}）{label}：{command}",
            num = index + 1,
            tool = entry.tool,
            target = entry.target,
            command = entry.command
        );
        if !entry.note.is_empty() {
            line.push_str(&format!("｜{note}", note = entry.note));
        }
        if total_chars + line.chars().count() > MAX_RENDER_CHARS && !lines.is_empty() {
            break;
        }
        total_chars += line.chars().count();
        lines.push(line);
    }
    lines.reverse();
    Some(lines.join("\n"))
}

/// 会话删除时清理其命令历史（会话资源清理规范：不得遗留会话绑定的状态）。
pub fn forget_session(workspace_id: &str) {
    registry().lock().remove(workspace_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(name: &str) -> String {
        format!("history-test-{name}-{}", uuid::Uuid::new_v4())
    }

    #[test]
    fn records_and_renders_in_chronological_order() {
        let ws = session("render");
        record(&ws, "exec", "工作区", "git status", CommandHistoryStatus::Executed, "exit=0");
        record(
            &ws,
            "ssh_exec",
            "prod-db",
            "drop table users",
            CommandHistoryStatus::Blocked,
            "清空数据库",
        );
        let rendered = render_for_review(&ws).expect("应有历史");
        assert!(rendered.contains("#1 exec（工作区）已执行：git status｜exit=0"));
        assert!(rendered.contains("#2 ssh_exec（prod-db）已拦截：drop table users｜清空数据库"));
        assert!(rendered.lines().next().unwrap().contains("#1"));
        forget_session(&ws);
        assert!(render_for_review(&ws).is_none());
    }

    #[test]
    fn truncates_long_command_and_note() {
        let ws = session("truncate");
        let long_command = "x".repeat(MAX_COMMAND_CHARS + 100);
        let long_note = "y".repeat(MAX_NOTE_CHARS + 100);
        record(&ws, "exec", "工作区", &long_command, CommandHistoryStatus::Executed, &long_note);
        let rendered = render_for_review(&ws).unwrap();
        // 截断后附省略号，且不包含完整原文长度
        assert!(rendered.contains(&"x".repeat(MAX_COMMAND_CHARS)));
        assert!(!rendered.contains(&"x".repeat(MAX_COMMAND_CHARS + 1)));
        assert!(rendered.contains(&"y".repeat(MAX_NOTE_CHARS)));
        forget_session(&ws);
    }

    #[test]
    fn prunes_beyond_per_session_limit() {
        let ws = session("prune");
        for i in 0..(MAX_ENTRIES_PER_SESSION + 10) {
            record(&ws, "exec", "工作区", &format!("cmd-{i}"), CommandHistoryStatus::Executed, "");
        }
        let map = registry().lock();
        let history = map.get(&ws).expect("会话应在册");
        assert_eq!(history.entries.len(), MAX_ENTRIES_PER_SESSION);
        // 最旧的 10 条被丢弃，最新一条仍在
        assert_eq!(history.entries.front().unwrap().command, "cmd-10");
        assert_eq!(
            history.entries.back().unwrap().command,
            format!("cmd-{}", MAX_ENTRIES_PER_SESSION + 9)
        );
        drop(map);
        forget_session(&ws);
    }

    #[test]
    fn empty_session_renders_none() {
        assert!(render_for_review("history-test-missing-session").is_none());
    }
}
