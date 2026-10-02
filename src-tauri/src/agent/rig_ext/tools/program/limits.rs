//! 一次 `run_tool_program` 的执行限额。模型可见的字符上限只有
//! [`TOOL_RESULT_INLINE_MAX_CHARS_PROGRAM`]；这里的捕获上限只防止程序把进程内存写满。

use std::time::Duration;

use crate::agent::rig_ext::tool_result::TOOL_RESULT_INLINE_MAX_CHARS_PROGRAM;

/// 单次程序最多启动的绑定调用。超出后该次调用抛 `ToolCallError`，程序还可以捕获。
pub(super) const MAX_CALLS: usize = 32;

/// 可并行只读绑定的重叠上限。
pub(super) const MAX_CONCURRENCY: usize = 4;

/// 程序自己的 wall-time。外层策略表的 900 秒结算上限是另一层兜底，不在这里重复。
pub(super) const WALL_TIME: Duration = Duration::from_secs(120);

/// `console` 与返回值在交给外层截断之前的内部上限。
pub(super) const MAX_CAPTURED_CHARS: usize = 1_000_000;

/// QuickJS 堆上限。程序没有宿主 I/O，这个限额只约束解释器自己的分配。
pub(super) const MAX_MEMORY_BYTES: usize = 16 * 1024 * 1024;

pub(super) const MODEL_VISIBLE_CHARS: usize = TOOL_RESULT_INLINE_MAX_CHARS_PROGRAM;

#[derive(Debug, Clone, Copy)]
pub(super) struct ProgramLimits {
    pub max_calls: usize,
    pub max_concurrency: usize,
    pub wall_time: Duration,
    pub max_captured_chars: usize,
    pub max_memory_bytes: usize,
}

impl Default for ProgramLimits {
    fn default() -> Self {
        Self {
            max_calls: MAX_CALLS,
            max_concurrency: MAX_CONCURRENCY,
            wall_time: WALL_TIME,
            max_captured_chars: MAX_CAPTURED_CHARS,
            max_memory_bytes: MAX_MEMORY_BYTES,
        }
    }
}
