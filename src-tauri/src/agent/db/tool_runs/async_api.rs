//! 工具运行台账的 `spawn_blocking` 异步包装。
//!
//! 同步实现见 `lifecycle` / `tree`；此处只做线程池移交与错误上下文，
//! 保持「Tauri async 命令内禁止直接阻塞」约束。

use anyhow::{Context, Result};

use super::{DispatcherToolRunRecord, FinishToolRun, NewToolRun, ToolRunTraceContext};
use crate::agent::db::DispatcherDb;

impl DispatcherDb {
    #[allow(dead_code)]
    pub async fn create_tool_run_with_trace_async(
        &self,
        run: NewToolRun,
        trace: ToolRunTraceContext,
    ) -> Result<DispatcherToolRunRecord> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.create_tool_run_with_trace(run, trace))
            .await
            .context("create_tool_run_with_trace spawn_blocking")?
    }

    pub async fn mark_tool_run_started_async(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let db = self.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || db.mark_tool_run_started(&id))
            .await
            .context("mark_tool_run_started spawn_blocking")?
    }

    pub async fn load_tool_run_async(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let db = self.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || db.load_tool_run(&id))
            .await
            .context("load_tool_run spawn_blocking")?
    }

    pub async fn finish_tool_run_async(
        &self,
        id: &str,
        finish: FinishToolRun,
    ) -> Result<DispatcherToolRunRecord> {
        let db = self.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || db.finish_tool_run(&id, finish))
            .await
            .context("finish_tool_run spawn_blocking")?
    }

    pub async fn attach_tool_run_tree_message_async(
        &self,
        root_run_id: &str,
        message_id: &str,
    ) -> Result<Vec<DispatcherToolRunRecord>> {
        let db = self.clone();
        let root_run_id = root_run_id.to_string();
        let message_id = message_id.to_string();
        tokio::task::spawn_blocking(move || {
            db.attach_tool_run_tree_message(&root_run_id, &message_id)
        })
        .await
        .context("attach_tool_run_tree_message spawn_blocking")?
    }

    pub async fn delete_unattached_tool_run_tree_async(
        &self,
        workspace_id: &str,
        root_run_id: &str,
    ) -> Result<()> {
        let db = self.clone();
        let workspace_id = workspace_id.to_string();
        let root_run_id = root_run_id.to_string();
        tokio::task::spawn_blocking(move || {
            db.delete_unattached_tool_run_tree(&workspace_id, &root_run_id)
        })
        .await
        .context("delete_unattached_tool_run_tree spawn_blocking")?
    }
}
