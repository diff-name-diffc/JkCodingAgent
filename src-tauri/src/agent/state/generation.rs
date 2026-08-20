use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// "最新一代胜出"模式，用于标题/关键字等异步生成的竞态保护。
/// 用代际号自增数保证只有最后一次（最新代）生成的结果才会被提交，
/// 先前的过期任务在 finish_latest 时被识别并丢弃，避免旧标题覆盖新标题。
///
/// G11-13：代际分配与 active 登记在**同一把锁内原子完成**——旧实现先用
/// AtomicU64 分配再单独加锁写入，两次 begin 的「分配顺序」与「登记顺序」
/// 可能不一致，导致 map 中留下较旧代际、让过期结果误判为最新代胜出。
#[derive(Clone, Default)]
pub(super) struct GenerationGate {
    inner: Arc<Mutex<GenerationData>>,
}

#[derive(Default)]
struct GenerationData {
    active: HashMap<String, u64>,
    next_generation: u64,
}

impl GenerationGate {
    /// 开始新一代生成：分配代际号并登记 active 条目（同锁原子）。
    /// 返回的守卫在被丢弃时自动清理 active 条目——即使生成任务
    /// 提前 return / abort 而没走到显式 finish，也不会泄漏条目。
    pub(super) fn begin(&self, workspace_id: &str) -> GenerationGuard {
        let mut data = self.inner.lock();
        let generation = data.next_generation;
        data.next_generation = data.next_generation.wrapping_add(1);
        data.active.insert(workspace_id.to_string(), generation);
        GenerationGuard {
            gate: self.clone(),
            key: workspace_id.to_string(),
            generation,
            finished: false,
        }
    }

    /// 提交结果时校验代际：仅当 generation 与当前最新代一致才提交并返回 true，
    /// 否则说明期间已有更新的生成任务发起，当前结果过期，返回 false 由调用方丢弃。
    ///
    /// 注意：校验通过到调用方真正写库之间仍存在窗口（期间可能有更新代 begin），
    /// 但各代写库按完成顺序落盘，最终收敛为"最后完成的一代写入值"，不会造成
    /// 旧代覆盖新代——本门禁只负责丢弃确定过期的结果，不负责写库串行化。
    fn finish_latest(&self, workspace_id: &str, generation: u64) -> bool {
        let mut data = self.inner.lock();
        if data
            .active
            .get(workspace_id)
            .is_some_and(|current| *current == generation)
        {
            data.active.remove(workspace_id);
            return true;
        }
        false
    }
}

/// 一次生成任务的代际守卫。
///
/// - `finish()`：显式校验"是否仍为最新代"，返回 true 才允许调用方提交结果；
/// - `Drop`：任何路径下（含提前 return、任务 abort）都会结算 active 条目，
///   保证 map 不残留——条目按 workspace_id 为键虽有上界，残留仍会让
///   后续 finish_latest 的代际比较基于过期值。
pub struct GenerationGuard {
    gate: GenerationGate,
    key: String,
    generation: u64,
    finished: bool,
}

impl GenerationGuard {
    /// 校验本代是否仍为最新代（消费守卫）。true 表示可以提交结果。
    pub(crate) fn finish(mut self) -> bool {
        self.finished = true;
        self.gate.finish_latest(&self.key, self.generation)
    }
}

impl Drop for GenerationGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.gate.finish_latest(&self.key, self.generation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GenerationGate;

    #[test]
    fn latest_generation_wins_and_older_ones_are_rejected() {
        let gate = GenerationGate::default();
        let first = gate.begin("ws-1");
        let second = gate.begin("ws-1");

        // 后一代登记后，前一代立即过期。
        assert!(!first.finish());
        assert!(second.finish());
    }

    #[test]
    fn dropped_guard_cleans_active_entry_without_blocking_later_begin() {
        let gate = GenerationGate::default();
        let abandoned = gate.begin("ws-1");
        drop(abandoned); // 模拟任务提前结束未调用 finish

        // 条目已随 drop 结算：新一代正常登记并可胜出。
        let next = gate.begin("ws-1");
        assert!(next.finish());
    }

    #[test]
    fn per_workspace_generations_are_independent() {
        let gate = GenerationGate::default();
        let a = gate.begin("ws-a");
        let b = gate.begin("ws-b");

        assert!(a.finish());
        assert!(b.finish());
    }

    #[test]
    fn begin_and_register_are_atomic_across_threads() {
        use std::sync::Arc;
        use std::thread;

        let gate = Arc::new(GenerationGate::default());
        let mut handles = Vec::new();
        for _ in 0..8 {
            let gate = Arc::clone(&gate);
            handles.push(thread::spawn(move || {
                let mut guards = Vec::new();
                for _ in 0..200 {
                    guards.push(gate.begin("ws-shared"));
                }
                guards
            }));
        }
        let all: Vec<_> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        // 只有最后登记的一代可以胜出；无论线程间 begin 顺序如何交错，
        // 恰好一个 finish 返回 true（分配与登记同锁，不存在两个"最新代"）。
        let winners = all.into_iter().filter(|guard| guard.clone_finish()).count();
        assert_eq!(winners, 1);
    }
}

#[cfg(test)]
impl GenerationGuard {
    /// 测试辅助：不消费守卫地探测胜负（Drop 仍会正常结算）。
    fn clone_finish(&self) -> bool {
        self.gate.finish_latest(&self.key, self.generation)
    }
}
