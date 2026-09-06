use super::*;

impl DispatcherDb {
    pub fn clear_messages(&self, workspace_id: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        // 级联清单与删会话共享单一出处（db/purge.rs）；本命令只额外刷新
        // 会话表的 updated_at（会话行本身保留）。
        let image_dir = crate::agent::db::purge::purge_session_resources_tx(&tx, workspace_id)?;
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
    /// 截断即重发，被删轮次的副作用一并回收：
    /// - 子智能体 trace（`sub_agent_run_traces`）按被删工具运行的 `tool_call_id`
    ///   精确匹配删除，不留孤儿行；
    /// - 图编排产物按计划创建时间截断（`graph_plans.created_at` 为 epoch 毫秒，
    ///   与目标消息 `created_at` 换算比较），`graph_runs` / `graph_node_runs` /
    ///   `graph_node_activities` 随外键级联删除。
    ///
    /// 有意保留（决策在账）：
    /// - **token 用量**（`dispatcher_session_token_usage`）是真实消耗记录，
    ///   回退会让用量分析失真；
    /// - **会话关键字**（`session_keywords`）为聚合权重、无消息关联，无法精确
    ///   回退，重发后新一轮抽取自然覆盖；
    /// - **图片文件**：chat_images 记录随消息删除由外键级联清掉，但文件有意保留——
    ///   重发（regenerate / 编辑重发）会复用同一批 image_id，若在此处删文件，
    ///   重发消息将引用不存在的文件（曾因此出现「未找到图片」）。文件的生命
    ///   周期由会话删除/清空消息兜底回收。
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

        // 在删除工具运行之前收集其 tool_call_id，用于精确清理子智能体 trace。
        // WHERE 条件与下方 dispatcher_tool_runs 的删除范围保持一致。
        let mut trace_call_ids: Vec<String> = Vec::new();
        {
            let mut stmt = tx
                .prepare(
                    "SELECT DISTINCT tool_call_id FROM dispatcher_tool_runs
                     WHERE workspace_id = ?1 AND (
                         message_id IN (
                             SELECT id FROM dispatcher_messages
                             WHERE workspace_id = ?1 AND rowid >= ?2
                         )
                         OR (message_id IS NULL AND created_at >= ?3)
                     )",
                )
                .context("prepare truncated tool call id lookup")?;
            let mut rows = stmt
                .query(params![workspace_id, target_rowid, target_created_at])
                .context("query truncated tool call ids")?;
            while let Some(row) = rows.next().context("advance tool call id cursor")? {
                trace_call_ids.push(row.get(0)?);
            }
        }

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

        // 子智能体 trace 无消息外键（主键 workspace_id+tool_call_id），按上面
        // 收集的 tool_call_id 精确回收；逐条删除走索引，避免超长 IN 占位符列表。
        if !trace_call_ids.is_empty() {
            let mut stmt = tx
                .prepare(
                    "DELETE FROM sub_agent_run_traces
                     WHERE workspace_id = ?1 AND tool_call_id = ?2",
                )
                .context("prepare sub-agent trace cleanup")?;
            for call_id in &trace_call_ids {
                stmt.execute(params![workspace_id, call_id])
                    .context("delete truncated sub-agent run trace")?;
            }
        }

        // 图编排产物无消息关联，按计划创建时间截断（graph_plans.created_at 为
        // epoch 毫秒）。graph_runs / graph_node_runs / graph_node_activities
        // 由外键 ON DELETE CASCADE 级联回收。解析失败即中止（事务回滚），
        // 不做静默跳过——留痕优于泄漏。
        let target_epoch_ms = chrono::DateTime::parse_from_rfc3339(&target_created_at)
            .with_context(|| {
                format!("parse truncated message created_at {target_created_at}")
            })?
            .timestamp_millis();
        tx.execute(
            "DELETE FROM graph_plans WHERE workspace_id = ?1 AND created_at >= ?2",
            params![workspace_id, target_epoch_ms],
        )
        .context("delete graph plans created at or after truncated message")?;

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
