use super::*;

impl DispatcherDb {
    pub fn list_visible_messages(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<DispatcherMessageRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, role, segments_json, thinking_content, thinking_elapsed_ms, context_payload, tool_call_id, tool_name, tool_result_mode, tool_artifacts_json, tool_calls_json, usage_stats_json, created_at
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND visible = 1
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id], map_dispatcher_message_record)?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("load visible dispatcher messages")
    }

    /// 统计可见消息条数（G7-11：Finished 事件轻量负载的对账依据）。
    pub fn count_visible_messages(&self, workspace_id: &str) -> Result<usize> {
        let conn = self.conn()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatcher_messages
                 WHERE workspace_id = ?1 AND visible = 1",
                params![workspace_id],
                |row| row.get(0),
            )
            .context("count visible dispatcher messages")?;
        usize::try_from(count).context("visible dispatcher message count out of range")
    }

    /// Load recent complete visible dialogues for session title generation.
    ///
    /// The cutoff is based on user-started turns, so the latest user message and its
    /// following assistant/tool messages stay together instead of being clipped by a
    /// raw message count.
    pub fn list_recent_visible_dialogue_messages(
        &self,
        workspace_id: &str,
        max_dialogues: usize,
    ) -> Result<Vec<DispatcherMessageRecord>> {
        let conn = self.conn()?;
        let cutoff_rowid = self.find_dialogue_cutoff_rowid(&conn, workspace_id, max_dialogues)?;
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, role, segments_json, thinking_content, thinking_elapsed_ms, context_payload, tool_call_id, tool_name, tool_result_mode, tool_artifacts_json, tool_calls_json, usage_stats_json, created_at
             FROM dispatcher_messages
             WHERE workspace_id = ?1
               AND visible = 1
               AND context_cleared = 0
               AND rowid >= ?2
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(
            params![workspace_id, cutoff_rowid],
            map_dispatcher_message_record,
        )?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("load recent visible dispatcher dialogue messages")
    }

    /// Load only the recent dialogue window for one dispatcher session.
    ///
    /// Note:
    /// - `workspace_id` here is the dispatcher session id used by the frontend.
    /// - One project can have multiple dispatcher sessions; history is isolated by session id.
    /// - Only the most recent `MAX_LLM_DIALOGUES` user-started dialogues are injected into the LLM.
    pub fn load_llm_history(&self, workspace_id: &str) -> Result<Vec<ChatMessage>> {
        let conn = self.conn()?;
        let cutoff_rowid =
            self.find_dialogue_cutoff_rowid(&conn, workspace_id, MAX_LLM_DIALOGUES)?;

        let mut stmt = conn.prepare(
            "SELECT role, segments_json, context_payload, tool_call_id, tool_name, tool_calls_json, thinking_content
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND rowid >= ?2 AND visible = 1 AND context_cleared = 0
             ORDER BY rowid ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id, cutoff_rowid], |row| {
            let role: String = row.get(0)?;
            let segments_json: String = row.get(1)?;
            let context_payload: Option<String> = row.get(2)?;
            let tool_call_id: Option<String> = row.get(3)?;
            let tool_name: Option<String> = row.get(4)?;
            let tool_calls_json: Option<String> = row.get(5)?;
            let thinking_content: Option<String> = row.get(6)?;

            let (content, content_parts) = if let Some(payload) = context_payload {
                (payload, Vec::new())
            } else {
                let segments = parse_segments_json(&segments_json);
                (
                    segments_to_plain_text(&segments),
                    segments_to_llm_content_parts(&role, &segments),
                )
            };

            let tool_calls = tool_calls_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<Vec<OutboundToolCall>>(json).ok());

            Ok(ChatMessage {
                reasoning_content: if role == "assistant" {
                    thinking_content.filter(|content| !content.trim().is_empty())
                } else {
                    None
                },
                role,
                content,
                content_parts,
                tool_call_id,
                name: tool_name,
                tool_calls,
            })
        })?;

        let mut messages = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("load dispatcher llm history")?;
        messages.retain(should_keep_llm_message);

        while matches!(messages.first().map(|m| m.role.as_str()), Some("tool")) {
            messages.remove(0);
        }

        Ok(messages)
    }

    /// Fetch only the content of the latest visible user message.
    ///
    /// Lighter than `load_llm_history` — skips parsing tool calls, context payloads,
    /// and dialogue cutoff calculations.
    pub fn get_latest_user_message_content(&self, workspace_id: &str) -> Result<Option<String>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT segments_json
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND role = 'user' AND visible = 1 AND context_cleared = 0
             ORDER BY rowid DESC
             LIMIT 1",
            params![workspace_id],
            |row| {
                let segments_json: String = row.get(0)?;
                Ok(segments_to_plain_text(&parse_segments_json(&segments_json)))
            },
        )
        .optional()
        .context("load latest dispatcher user message content")
    }

    pub fn get_visible_message_content(
        &self,
        workspace_id: &str,
        message_id: &str,
    ) -> Result<Option<String>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT segments_json
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND id = ?2 AND visible = 1
             LIMIT 1",
            params![workspace_id, message_id],
            |row| {
                let segments_json: String = row.get(0)?;
                Ok(segments_to_plain_text(&parse_segments_json(&segments_json)))
            },
        )
        .optional()
        .context("load dispatcher visible message content")
    }
}
