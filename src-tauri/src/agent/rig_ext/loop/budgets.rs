//! 整棵 run 的准入与摘要预算；子 scope 通过根 run ID 共享。
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, Weak},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// 每根 run 的未交付叶子任务准入上限。调度器登记台账的 sequence 编码
/// （`dispatch.rs` 的 `round * LEAF_ADMISSION_CAP + index`）依赖同一数值保证
/// 跨轮唯一——两处必须引用本常量，任何调整都不得再引入字面量副本。
pub(super) const LEAF_ADMISSION_CAP: usize = 32;

pub(super) struct RunBudgets {
    leaves: Arc<Semaphore>,
    composites: Arc<Semaphore>,
    pub summaries: Arc<Semaphore>,
}

impl RunBudgets {
    pub fn shared(run: &str) -> Arc<Self> {
        static RUNS: OnceLock<Mutex<HashMap<String, Weak<RunBudgets>>>> = OnceLock::new();
        let mut runs = RUNS.get_or_init(Default::default).lock();
        runs.retain(|_, budget| budget.strong_count() > 0);
        if let Some(budget) = runs.get(run).and_then(Weak::upgrade) {
            return budget;
        }
        let budget = Arc::new(Self {
            leaves: Arc::new(Semaphore::new(LEAF_ADMISSION_CAP)),
            composites: Arc::new(Semaphore::new(4)),
            summaries: Arc::new(Semaphore::new(2)),
        });
        runs.insert(run.into(), Arc::downgrade(&budget));
        budget
    }

    pub fn reserve(&self, composite: bool) -> anyhow::Result<OwnedSemaphorePermit> {
        let budget = if composite {
            &self.composites
        } else {
            &self.leaves
        };
        budget.clone().try_acquire_owned().map_err(|_| {
            anyhow::Error::new(AdmissionError(format!(
                "调度容量不足：每个根 run 最多 {LEAF_ADMISSION_CAP} 个尚未交付叶子任务、4 个活动复合任务"
            )))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn root_scopes_share_capacity_without_composites_consuming_leaf_slots() {
        let root = RunBudgets::shared("budget-test");
        let child = RunBudgets::shared("budget-test");
        let leaves = (0..32)
            .map(|_| child.reserve(false).unwrap())
            .collect::<Vec<_>>();
        assert!(root.reserve(false).is_err());
        let composites = (0..4)
            .map(|_| root.reserve(true).unwrap())
            .collect::<Vec<_>>();
        assert!(child.reserve(true).is_err());
        assert!(RunBudgets::shared("independent-budget-test")
            .reserve(false)
            .is_ok());
        drop(leaves);
        assert!(root.reserve(false).is_ok());
        drop(composites);
        assert!(child.reserve(true).is_ok());
    }
}

/// 仅用于副作用和台账登记之前的可恢复准入拒绝。
#[derive(Debug)]
pub(super) struct AdmissionError(pub String);
impl std::fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for AdmissionError {}
