use super::*;

impl DispatcherDb {
    pub async fn add_visible_message_from_segments_async(
        &self,
        workspace_id: &str,
        role: &str,
        segments_json: String,
    ) -> Result<DispatcherMessageRecord> {
        let wid = workspace_id.to_string();
        let role = role.to_string();
        self.blocking(
            "add_visible_message_from_segments spawn_blocking",
            move |db| db.add_visible_message_from_segments(&wid, &role, segments_json),
        )
        .await
    }

    pub async fn load_llm_history_async(&self, workspace_id: &str) -> Result<Vec<ChatMessage>> {
        let wid = workspace_id.to_string();
        self.blocking("load_llm_history spawn_blocking", move |db| {
            db.load_llm_history(&wid)
        })
        .await
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn add_visible_message_with_tools_and_thinking_async(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
        thinking_content: Option<&str>,
        thinking_elapsed_ms: u64,
    ) -> Result<DispatcherMessageRecord> {
        let wid = workspace_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        let tool_call_id = tool_call_id.map(str::to_string);
        let tool_name = tool_name.map(str::to_string);
        let tool_result_mode = tool_result_mode.map(str::to_string);
        let tool_calls = tool_calls.map(|c| c.to_vec());
        let thinking = thinking_content.map(str::to_string);
        self.blocking(
            "add_visible_message_with_tools_and_thinking spawn_blocking",
            move |db| {
                db.add_visible_message_with_tools_and_thinking(
                    &wid,
                    &role,
                    &content,
                    tool_call_id.as_deref(),
                    tool_name.as_deref(),
                    tool_result_mode.as_deref(),
                    tool_calls.as_deref(),
                    thinking.as_deref(),
                    thinking_elapsed_ms,
                )
            },
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_visible_tool_result_async(
        &self,
        workspace_id: &str,
        content: &str,
        context_payload: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_artifacts: &[ToolArtifactDraft],
    ) -> Result<DispatcherMessageRecord> {
        let wid = workspace_id.to_string();
        let content = content.to_string();
        let context_payload = context_payload.to_string();
        let tool_call_id = tool_call_id.map(str::to_string);
        let tool_name = tool_name.map(str::to_string);
        let tool_result_mode = tool_result_mode.map(str::to_string);
        let artifacts = tool_artifacts.to_vec();
        self.blocking("add_visible_tool_result spawn_blocking", move |db| {
            db.add_visible_tool_result(
                &wid,
                &content,
                &context_payload,
                tool_call_id.as_deref(),
                tool_name.as_deref(),
                tool_result_mode.as_deref(),
                &artifacts,
            )
        })
        .await
    }

    pub async fn count_visible_messages_async(&self, workspace_id: &str) -> Result<usize> {
        let wid = workspace_id.to_string();
        self.blocking("count_visible_messages spawn_blocking", move |db| {
            db.count_visible_messages(&wid)
        })
        .await
    }

    /// 发送前校验（segments_json 解析在阻塞线程外，磁盘 I/O 在阻塞线程）：
    /// 见 `validate_chat_image_segments`。
    pub async fn validate_chat_image_segments_async(&self, segments_json: &str) -> Result<()> {
        let segments = try_parse_segments_json(segments_json)?;
        self.blocking("validate_chat_image_segments spawn_blocking", move |db| {
            db.validate_chat_image_segments(&segments)
        })
        .await
    }

    pub async fn add_visible_message_with_usage_async(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        usage_stats: &DispatcherMessageUsageStats,
    ) -> Result<DispatcherMessageRecord> {
        let wid = workspace_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        let usage_stats = usage_stats.clone();
        self.blocking(
            "add_visible_message_with_usage spawn_blocking",
            move |db| db.add_visible_message_with_usage(&wid, &role, &content, &usage_stats),
        )
        .await
    }

    pub async fn get_latest_user_message_content_async(
        &self,
        workspace_id: &str,
    ) -> Result<Option<String>> {
        let wid = workspace_id.to_string();
        self.blocking(
            "get_latest_user_message_content spawn_blocking",
            move |db| db.get_latest_user_message_content(&wid),
        )
        .await
    }

    pub async fn get_recent_review_dialogue_async(
        &self,
        workspace_id: &str,
        max_messages: usize,
    ) -> Result<Vec<(String, String)>> {
        let wid = workspace_id.to_string();
        self.blocking("get_recent_review_dialogue spawn_blocking", move |db| {
            db.get_recent_review_dialogue(&wid, max_messages)
        })
        .await
    }
}
