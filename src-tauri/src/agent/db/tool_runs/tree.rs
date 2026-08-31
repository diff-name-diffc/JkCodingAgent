//! 工具调用树：父子关系列举、外层消息绑定与未绑定树清理。
//!
//! ToolProgram 子调用不生成伪造的 LLM tool message，执行期间 message_id 为空；
//! 外层结果落库后在同一事务中统一补挂整棵树，保证按消息截断时不遗留孤儿记录。

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension, TransactionBehavior};

use super::{map_tool_run, DispatcherToolRunRecord, TOOL_RUN_SELECT_COLUMNS};
use crate::agent::db::util::now;
use crate::agent::db::DispatcherDb;

impl DispatcherDb {
    /// 将外层 LLM tool 结果消息绑定到整棵内部调用树及其产物。
    pub fn attach_tool_run_tree_message(
        &self,
        root_run_id: &str,
        message_id: &str,
    ) -> Result<Vec<DispatcherToolRunRecord>> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin attach dispatcher tool run tree message transaction")?;
        let workspace_id = tx
            .query_row(
                "SELECT workspace_id FROM dispatcher_tool_runs WHERE id = ?1",
                params![root_run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("load dispatcher tool run root before message attach")?
            .with_context(|| format!("dispatcher tool run root not found: {root_run_id}"))?;
        let message_workspace_id = tx
            .query_row(
                "SELECT workspace_id FROM dispatcher_messages WHERE id = ?1",
                params![message_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("load dispatcher message before tool run attach")?
            .with_context(|| format!("dispatcher message not found: {message_id}"))?;
        if message_workspace_id != workspace_id {
            anyhow::bail!(
                "dispatcher message {message_id} belongs to workspace {message_workspace_id}, not {workspace_id}"
            );
        }

        let timestamp = now();
        tx.execute(
            "WITH RECURSIVE tool_run_tree(id) AS (
                 SELECT id FROM dispatcher_tool_runs
                 WHERE id = ?1 AND workspace_id = ?2
                 UNION ALL
                 SELECT child.id
                 FROM dispatcher_tool_runs child
                 INNER JOIN tool_run_tree tree ON child.parent_run_id = tree.id
                 WHERE child.workspace_id = ?2
             )
             UPDATE dispatcher_tool_runs
             SET message_id = ?3, updated_at = ?4
             WHERE id IN (SELECT id FROM tool_run_tree)",
            params![root_run_id, &workspace_id, message_id, &timestamp],
        )
        .context("attach dispatcher tool run tree message")?;
        tx.execute(
            "WITH RECURSIVE tool_run_tree(id) AS (
                 SELECT id FROM dispatcher_tool_runs
                 WHERE id = ?1 AND workspace_id = ?2
                 UNION ALL
                 SELECT child.id
                 FROM dispatcher_tool_runs child
                 INNER JOIN tool_run_tree tree ON child.parent_run_id = tree.id
                 WHERE child.workspace_id = ?2
             )
             UPDATE dispatcher_tool_artifacts
             SET message_id = ?3
             WHERE workspace_id = ?2
               AND tool_run_id IN (SELECT id FROM tool_run_tree)",
            params![root_run_id, &workspace_id, message_id],
        )
        .context("attach dispatcher tool run tree artifacts to message")?;
        tx.commit()
            .context("commit attach dispatcher tool run tree message transaction")?;
        self.list_tool_run_tree(&workspace_id, root_run_id)
    }

    /// 外层 tool message 持久化失败时删除尚未绑定消息的整棵运行树。
    /// parent_run_id 与 artifact.tool_run_id 均为 ON DELETE CASCADE，因此只需
    /// 精确删除根记录；workspace 条件防止跨会话误删。
    pub fn delete_unattached_tool_run_tree(
        &self,
        workspace_id: &str,
        root_run_id: &str,
    ) -> Result<()> {
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "DELETE FROM dispatcher_tool_runs
                 WHERE id = ?1 AND workspace_id = ?2 AND message_id IS NULL",
                params![root_run_id, workspace_id],
            )
            .context("delete unattached dispatcher tool run tree")?;
        if changed == 0 {
            anyhow::bail!(
                "unattached dispatcher tool run root not found: {root_run_id} in {workspace_id}"
            );
        }
        Ok(())
    }

    /// 按外层模型工具调用定位完整运行树。`root_run_id` 可用于实时卡片精确命中；
    /// 历史消息没有该字段时，使用同一 tool_call_id 的最新根运行。
    pub fn list_tool_run_tree_for_call(
        &self,
        workspace_id: &str,
        tool_call_id: &str,
        root_run_id: Option<&str>,
    ) -> Result<Vec<DispatcherToolRunRecord>> {
        let conn = self.conn()?;
        let root_id = conn
            .query_row(
                "SELECT id
                 FROM dispatcher_tool_runs
                 WHERE workspace_id = ?1
                   AND parent_run_id IS NULL
                   AND (
                       (?3 IS NOT NULL AND id = ?3)
                       OR (?3 IS NULL AND tool_call_id = ?2)
                   )
                 ORDER BY created_at DESC, rowid DESC
                 LIMIT 1",
                params![workspace_id, tool_call_id, root_run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("locate dispatcher tool run tree root")?;
        let Some(root_id) = root_id else {
            return Ok(Vec::new());
        };
        drop(conn);
        self.list_tool_run_tree(workspace_id, &root_id)
    }

    /// 按父子关系返回一棵工具调用树，结果为稳定的深度优先顺序。
    pub fn list_tool_run_tree(
        &self,
        workspace_id: &str,
        root_run_id: &str,
    ) -> Result<Vec<DispatcherToolRunRecord>> {
        let conn = self.conn()?;
        let sql = format!(
            "WITH RECURSIVE tool_run_tree(id, sort_path) AS (
                 SELECT id, printf('%s', id)
                 FROM dispatcher_tool_runs
                 WHERE id = ?2 AND workspace_id = ?1
                 UNION ALL
                 SELECT child.id,
                        tool_run_tree.sort_path || printf('/%020d-%s', child.sequence, child.id)
                 FROM dispatcher_tool_runs child
                 INNER JOIN tool_run_tree ON child.parent_run_id = tool_run_tree.id
                 WHERE child.workspace_id = ?1
             )
             SELECT {TOOL_RUN_SELECT_COLUMNS}
             FROM dispatcher_tool_runs r
             INNER JOIN tool_run_tree tree ON tree.id = r.id
             ORDER BY tree.sort_path"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![workspace_id, root_run_id], map_tool_run)?;
        let runs = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("list dispatcher tool run tree")?;
        if runs.is_empty() {
            anyhow::bail!(
                "dispatcher tool run root not found in workspace {workspace_id}: {root_run_id}"
            );
        }
        Ok(runs)
    }
}
