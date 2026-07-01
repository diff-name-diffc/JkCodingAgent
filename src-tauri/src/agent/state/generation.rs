use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// "最新一代胜出"模式，用于标题/关键字等异步生成的竞态保护。
///
/// 场景：用户连续发多条消息，每条都会异步触发一次标题生成。
/// 用代际号保证只有最后一次（最新代）生成的结果才会被提交，
/// 先前的过期任务在 finish_latest 时被识别并丢弃，避免旧标题覆盖新标题。
pub(super) struct GenerationGate {
    active: Mutex<HashMap<String, u64>>,
    next_generation: AtomicU64,
}

impl Default for GenerationGate {
    fn default() -> Self {
        Self {
            active: Mutex::new(HashMap::new()),
            next_generation: AtomicU64::new(1),
        }
    }
}

impl GenerationGate {
    pub(super) fn begin(&self, workspace_id: &str) -> u64 {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        self.active
            .lock()
            .insert(workspace_id.to_string(), generation);
        generation
    }

    /// 提交结果时校验代际：仅当 generation 与当前最新代一致才提交并返回 true，
    /// 否则说明期间已有更新的生成任务发起，当前结果过期，返回 false 由调用方丢弃。
    pub(super) fn finish_latest(&self, workspace_id: &str, generation: u64) -> bool {
        let mut active = self.active.lock();
        if active
            .get(workspace_id)
            .is_some_and(|current| *current == generation)
        {
            active.remove(workspace_id);
            return true;
        }
        false
    }
}
