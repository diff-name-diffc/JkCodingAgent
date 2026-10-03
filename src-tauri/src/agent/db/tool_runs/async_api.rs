//! 工具运行台账的 `spawn_blocking` 异步包装。
//!
//! 同步实现见 `lifecycle`；此处经 `DispatcherDb::blocking` 统一底座
//! 做线程池移交与错误上下文，保持「Tauri async 命令内禁止直接阻塞」约束。

use anyhow::Result;

use super::DispatcherToolRunRecord;
use crate::agent::db::DispatcherDb;

impl DispatcherDb {
    pub async fn mark_tool_run_started_async(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let id = id.to_string();
        self.blocking("mark_tool_run_started spawn_blocking", move |db| {
            db.mark_tool_run_started(&id)
        })
        .await
    }
}
