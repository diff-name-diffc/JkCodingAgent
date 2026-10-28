use std::time::Instant;

use rig::tool::ToolExecutionError;

use super::super::model::UpdateState;
use super::super::store::SnapshotStore;
use super::metrics::StageClock;
use super::{UpdateFailure, UpdateReporter};

pub(super) async fn set_stage(
    store: &SnapshotStore,
    reporter: &mut UpdateReporter,
    clock: &mut StageClock,
    state: UpdateState,
) -> Result<(), UpdateFailure> {
    reporter.set_state(state);
    clock.transition(state);
    reporter.set_metrics(clock.metrics.clone());
    let record = reporter.record();
    blocking({
        let store = store.clone();
        move || store.write_update(&record)
    })
    .await??;
    Ok(())
}

pub(super) fn check_work(
    reporter: &UpdateReporter,
    deadline: Instant,
) -> Result<(), UpdateFailure> {
    if *reporter.cancel.borrow() {
        return Err(UpdateFailure::cancelled("日志快照更新已取消"));
    }
    if Instant::now() >= deadline {
        return Err(UpdateFailure::failed("日志快照更新超过时间预算"));
    }
    Ok(())
}

pub(super) fn remaining_secs(deadline: Instant) -> u64 {
    deadline
        .saturating_duration_since(Instant::now())
        .as_secs()
        .max(1)
}

pub(super) fn tool_failure(error: ToolExecutionError) -> UpdateFailure {
    UpdateFailure {
        cancelled: error.kind() == rig::tool::ToolErrorKind::Cancelled,
        message: error
            .model_feedback()
            .map(str::to_string)
            .unwrap_or_else(|| error.message().to_string()),
    }
}

pub(crate) async fn blocking<T: Send + 'static>(
    task: impl FnOnce() -> T + Send + 'static,
) -> Result<T, UpdateFailure> {
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|error| UpdateFailure::failed(format!("日志后台任务失败：{error}")))
}
