use super::*;

impl DispatcherDb {
    pub fn clear_messages(&self, workspace_id: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM dispatcher_tool_artifacts WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher tool artifacts")?;
        tx.execute(
            "DELETE FROM dispatcher_tool_runs WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher tool runs")?;
        tx.execute(
            "DELETE FROM sub_agent_run_traces WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear sub-agent run traces")?;
        // 图编排产物（graph_plans / graph_node_runs）随会话清空同步删除。
        tx.execute(
            "DELETE FROM graph_node_runs
             WHERE plan_id IN (SELECT id FROM graph_plans WHERE workspace_id = ?1)",
            params![workspace_id],
        )
        .context("clear graph node runs")?;
        tx.execute(
            "DELETE FROM graph_plans WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear graph plans")?;
        tx.execute(
            "DELETE FROM dispatcher_session_token_usage WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher session token usage")?;
        let image_dir = delete_chat_image_resources(&tx, workspace_id)?;
        tx.execute(
            "DELETE FROM session_keywords WHERE session_id = ?1",
            params![workspace_id],
        )
        .context("clear session keywords")?;
        tx.execute(
            "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher messages")?;
        tx.execute(
            "UPDATE dispatcher_sessions SET updated_at = ?1 WHERE id = ?2",
            params![now(), workspace_id],
        )
        .context("update dispatcher session after clear")?;
        tx.execute(
            "UPDATE chat_sessions SET updated_at = ?1 WHERE id = ?2",
            params![now(), workspace_id],
        )
        .context("update chat session updated_at after clear")?;
        tx.execute(
            "UPDATE project_sessions SET updated_at = ?1 WHERE id = ?2",
            params![now(), workspace_id],
        )
        .context("update project session updated_at after clear")?;
        tx.commit().context("commit dispatcher message cleanup")?;
        // 数据库清空已提交，图片目录清理失败不应把清空误报为失败。改为 best-effort。
        if let Some(dir) = image_dir {
            if let Err(error) = remove_chat_image_dir(&dir) {
                eprintln!(
                    "remove chat image dir failed (clear messages {workspace_id}): {error:#}"
                );
            }
        }
        Ok(())
    }

    /// 删除指定消息及其之后的所有消息（含属于这些消息的工具产物与工具运行记录）。
    /// 用于「从该条用户消息重新生成」：先截断再重发，避免重复消息。
    ///
    /// chat_images 记录随消息删除由外键级联清掉，但**图片文件有意保留**：
    /// 重发（regenerate / 编辑重发）会复用同一批 image_id，若在此处删文件，
    /// 重发消息将引用不存在的文件（曾因此出现「未找到图片」）。文件的生命
    /// 周期由会话删除/清空消息兜底回收。
    pub fn truncate_messages_from(&self, workspace_id: &str, message_id: &str) -> Result<u64> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let (target_rowid, target_created_at): (i64, String) = tx
            .query_row(
                "SELECT rowid, created_at FROM dispatcher_messages
                 WHERE workspace_id = ?1 AND id = ?2",
                params![workspace_id, message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .context("lookup dispatcher message rowid")?
            .ok_or_else(|| {
                anyhow::anyhow!("message {message_id} not found in workspace {workspace_id}")
            })?;

        tx.execute(
            "DELETE FROM dispatcher_tool_artifacts
             WHERE workspace_id = ?1 AND message_id IN (
                 SELECT id FROM dispatcher_messages
                 WHERE workspace_id = ?1 AND rowid >= ?2)",
            params![workspace_id, target_rowid],
        )
        .context("delete truncated dispatcher tool artifacts")?;
        tx.execute(
            "DELETE FROM dispatcher_tool_runs
             WHERE workspace_id = ?1 AND (
                 message_id IN (
                     SELECT id FROM dispatcher_messages
                     WHERE workspace_id = ?1 AND rowid >= ?2
                 )
                 OR (message_id IS NULL AND created_at >= ?3)
             )",
            params![workspace_id, target_rowid, target_created_at],
        )
        .context("delete truncated dispatcher tool runs")?;

        let removed = tx
            .execute(
                "DELETE FROM dispatcher_messages WHERE workspace_id = ?1 AND rowid >= ?2",
                params![workspace_id, target_rowid],
            )
            .context("truncate dispatcher messages")?;

        // 消息删除改变了会话状态，同步全部会话表的 updated_at，避免统一会话列表排序错乱。
        let updated_at = now();
        tx.execute(
            "UPDATE dispatcher_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&updated_at, workspace_id],
        )
        .context("update dispatcher session updated_at after truncate")?;
        tx.execute(
            "UPDATE chat_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&updated_at, workspace_id],
        )
        .context("update chat session updated_at after truncate")?;
        tx.execute(
            "UPDATE project_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&updated_at, workspace_id],
        )
        .context("update project session updated_at after truncate")?;

        tx.commit()
            .context("commit dispatcher message truncation")?;

        Ok(removed as u64)
    }
}
