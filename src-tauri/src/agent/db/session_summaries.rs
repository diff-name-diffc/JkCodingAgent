//! 会话级滚动摘要持久化（`dispatcher_session_summaries`，schema v6）。
//!
//! 历史级压缩（`rig_ext::context::compact_history`）的跨 run 延续：运行循环
//! 把被裁中段折叠为滚动摘要后 upsert 到本表；下一次运行装配历史时
//! （`rig_ext::message::apply_stored_session_summary`）读回并插到窗口历史
//! 之前，进入滚动合并链路。锚点（`covered_through_message_id`）是摘要覆盖
//! 范围内最近一条已知消息 id——锚点被截断/删除即摘要失效，读取路径即读
//! 即删防脏读。
//!
//! 级联清理：会话删除/清空走 `purge::purge_session_resources_tx`；消息截断
//! （regenerate/编辑重发）在 `cleanup::truncate_messages_from` 内精确删除
//! 锚点落在被删范围的行（锚点早于截断点的摘要仍然有效，有意保留）。

use anyhow::{Context, Result};
use rusqlite::params;
use rusqlite::OptionalExtension;

use super::util::now;

/// 会话滚动摘要记录（同会话只保留最新一条，锚点单调前进）。
#[derive(Debug, Clone)]
pub struct SessionSummaryRecord {
    pub summary: String,
    pub covered_through_message_id: String,
}

impl super::DispatcherDb {
    /// 读取有效摘要：锚点消息仍存在才返回；锚点已失（截断/删除）即删行并
    /// 返回 None。单连接内完成「读 + 校验 + 失效清理」，调用方无需三步。
    pub fn valid_session_summary(
        &self,
        workspace_id: &str,
    ) -> Result<Option<SessionSummaryRecord>> {
        let conn = self.conn()?;
        let record = conn
            .query_row(
                "SELECT summary, covered_through_message_id
                 FROM dispatcher_session_summaries WHERE workspace_id = ?1",
                params![workspace_id],
                |row| {
                    Ok(SessionSummaryRecord {
                        summary: row.get(0)?,
                        covered_through_message_id: row.get(1)?,
                    })
                },
            )
            .optional()
            .context("load session summary")?;
        let Some(record) = record else {
            return Ok(None);
        };
        let anchor_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM dispatcher_messages
                 WHERE workspace_id = ?1 AND id = ?2)",
                params![workspace_id, record.covered_through_message_id],
                |row| row.get(0),
            )
            .context("check session summary anchor")?;
        if anchor_exists {
            return Ok(Some(record));
        }
        conn.execute(
            "DELETE FROM dispatcher_session_summaries WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("delete stale session summary")?;
        Ok(None)
    }

    /// upsert 滚动摘要（workspace_id 主键冲突即整行更新）。
    pub fn upsert_session_summary(
        &self,
        workspace_id: &str,
        summary: &str,
        covered_through_message_id: &str,
    ) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO dispatcher_session_summaries
                 (workspace_id, summary, covered_through_message_id, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(workspace_id) DO UPDATE SET
                 summary = excluded.summary,
                 covered_through_message_id = excluded.covered_through_message_id,
                 updated_at = excluded.updated_at",
            params![workspace_id, summary, covered_through_message_id, now()],
        )
        .context("upsert session summary")?;
        Ok(())
    }

    pub async fn valid_session_summary_async(
        &self,
        workspace_id: &str,
    ) -> Result<Option<SessionSummaryRecord>> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.valid_session_summary(&wid))
            .await
            .context("valid_session_summary spawn_blocking")?
    }

    pub async fn upsert_session_summary_async(
        &self,
        workspace_id: &str,
        summary: &str,
        covered_through_message_id: &str,
    ) -> Result<()> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let summary = summary.to_string();
        let anchor = covered_through_message_id.to_string();
        tokio::task::spawn_blocking(move || db.upsert_session_summary(&wid, &summary, &anchor))
            .await
            .context("upsert_session_summary spawn_blocking")?
    }
}
