use std::time::Instant;

use super::super::db::DispatcherMessageUsageStats;
use crate::agent::db::LlmUsage;

// ─── Usage Tracking ────────────────────────────────────────────────────────────

/// 总量归一：provider 未上报 total 时按 prompt+completion 补齐。
fn normalized_total_tokens(usage: &LlmUsage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.prompt_tokens + usage.completion_tokens
    }
}

/// 单轮 run 的用量累计与耗时（前端 `RunUsageUpdated` 的载荷来源）。
///
/// 说明：旧实现带「子智能体调用期间暂停计时」的 pause/resume 语义（避免子智能体
/// 墙钟时间稀释主模型 token 速度）；rig 运行时不停表，改为纯累计 + 真实墙钟，
/// 前端展示的 token/秒 在 `call_sub_agent` 期间会偏低（不再人为剔除）。
pub struct UsageTracker {
    pub started_at: Instant,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl UsageTracker {
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        }
    }

    pub fn record(&mut self, usage: &LlmUsage) -> DispatcherMessageUsageStats {
        self.prompt_tokens += usage.prompt_tokens;
        self.completion_tokens += usage.completion_tokens;
        self.total_tokens += normalized_total_tokens(usage);
        self.snapshot()
    }

    pub fn snapshot(&self) -> DispatcherMessageUsageStats {
        DispatcherMessageUsageStats {
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            total_tokens: self.total_tokens,
            elapsed_ms: self.started_at.elapsed().as_millis() as u64,
        }
    }
}

impl Default for UsageTracker {
    fn default() -> Self {
        Self::new()
    }
}
