//! 工具运行台账的 `spawn_blocking` 异步包装。
//!
//! 同步实现见 `lifecycle` / `tree`；此处经 `DispatcherDb::blocking` 统一底座
//! 做线程池移交与错误上下文，保持「Tauri async 命令内禁止直接阻塞」约束。

use anyhow::Result;

use super::{DispatcherToolRunRecord, FinishToolRun, NewToolRun, ToolRunTraceContext};
use crate::agent::db::DispatcherDb;

impl DispatcherDb {
    pub async fn create_tool_run_with_trace_async(
        &self,
        run: NewToolRun,
        trace: ToolRunTraceContext,
    ) -> Result<DispatcherToolRunRecord> {
        self.blocking(
            "create_tool_run_with_trace spawn_blocking",
            move |db| db.create_tool_run_with_trace(run, trace),
        )
        .await
    }

    pub async fn mark_tool_run_started_async(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let id = id.to_string();
        self.blocking("mark_tool_run_started spawn_blocking", move |db| {
            db.mark_tool_run_started(&id)
        })
        .await
    }

    pub async fn finish_tool_run_async(
        &self,
        id: &str,
        finish: FinishToolRun,
    ) -> Result<DispatcherToolRunRecord> {
        let id = id.to_string();
        self.blocking("finish_tool_run spawn_blocking", move |db| {
            db.finish_tool_run(&id, finish)
        })
        .await
    }

    pub async fn attach_tool_run_tree_message_async(
        &self,
        root_run_id: &str,
        message_id: &str,
    ) -> Result<Vec<DispatcherToolRunRecord>> {
        let root_run_id = root_run_id.to_string();
        let message_id = message_id.to_string();
        self.blocking(
            "attach_tool_run_tree_message spawn_blocking",
            move |db| db.attach_tool_run_tree_message(&root_run_id, &message_id),
        )
        .await
    }
}
