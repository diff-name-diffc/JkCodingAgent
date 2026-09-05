use super::*;

impl DispatcherDb {
    pub async fn add_visible_message_from_segments_async(
        &self,
        workspace_id: &str,
        role: &str,
        segments_json: String,
    ) -> Result<DispatcherMessageRecord> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let role = role.to_string();
        tokio::task::spawn_blocking(move || {
            db.add_visible_message_from_segments(&wid, &role, segments_json)
        })
        .await
        .context("add_visible_message_from_segments spawn_blocking")?
    }

    pub async fn add_visible_message_with_usage_and_thinking_async(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        usage_stats: &DispatcherMessageUsageStats,
        thinking_content: Option<&str>,
        thinking_elapsed_ms: u64,
    ) -> Result<DispatcherMessageRecord> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        let usage = usage_stats.clone();
        let thinking = thinking_content.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            db.add_visible_message_with_usage_and_thinking(
                &wid,
                &role,
                &content,
                &usage,
                thinking.as_deref(),
                thinking_elapsed_ms,
            )
        })
        .await
        .context("add_visible_message_with_usage_and_thinking spawn_blocking")?
    }

    pub async fn load_llm_history_async(&self, workspace_id: &str) -> Result<Vec<ChatMessage>> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.load_llm_history(&wid))
            .await
            .context("load_llm_history spawn_blocking")?
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
        let db = self.clone();
        let wid = workspace_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        let tool_call_id = tool_call_id.map(str::to_string);
        let tool_name = tool_name.map(str::to_string);
        let tool_result_mode = tool_result_mode.map(str::to_string);
        let tool_calls = tool_calls.map(|c| c.to_vec());
        let thinking = thinking_content.map(str::to_string);
        tokio::task::spawn_blocking(move || {
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
        })
        .await
        .context("add_visible_message_with_tools_and_thinking spawn_blocking")?
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
        let db = self.clone();
        let wid = workspace_id.to_string();
        let content = content.to_string();
        let context_payload = context_payload.to_string();
        let tool_call_id = tool_call_id.map(str::to_string);
        let tool_name = tool_name.map(str::to_string);
        let tool_result_mode = tool_result_mode.map(str::to_string);
        let artifacts = tool_artifacts.to_vec();
        tokio::task::spawn_blocking(move || {
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
        .context("add_visible_tool_result spawn_blocking")?
    }

    pub async fn count_visible_messages_async(&self, workspace_id: &str) -> Result<usize> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.count_visible_messages(&wid))
            .await
            .context("count_visible_messages spawn_blocking")?
    }

    /// 发送前校验（segments_json 解析在阻塞线程外，磁盘 I/O 在阻塞线程）：
    /// 见 `validate_chat_image_segments`。
    pub async fn validate_chat_image_segments_async(&self, segments_json: &str) -> Result<()> {
        let segments = try_parse_segments_json(segments_json)?;
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.validate_chat_image_segments(&segments))
            .await
            .context("validate_chat_image_segments spawn_blocking")?
    }

    pub async fn add_visible_message_with_usage_async(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        usage_stats: &DispatcherMessageUsageStats,
    ) -> Result<DispatcherMessageRecord> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        let usage_stats = usage_stats.clone();
        tokio::task::spawn_blocking(move || {
            db.add_visible_message_with_usage(&wid, &role, &content, &usage_stats)
        })
        .await
        .context("add_visible_message_with_usage spawn_blocking")?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_visible_message_with_tools_async(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
    ) -> Result<DispatcherMessageRecord> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        let tool_call_id = tool_call_id.map(str::to_string);
        let tool_name = tool_name.map(str::to_string);
        let tool_result_mode = tool_result_mode.map(str::to_string);
        let tool_calls = tool_calls.map(|c| c.to_vec());
        tokio::task::spawn_blocking(move || {
            db.add_visible_message_with_tools(
                &wid,
                &role,
                &content,
                tool_call_id.as_deref(),
                tool_name.as_deref(),
                tool_result_mode.as_deref(),
                tool_calls.as_deref(),
            )
        })
        .await
        .context("add_visible_message_with_tools spawn_blocking")?
    }
    pub async fn get_latest_user_message_content_async(
        &self,
        workspace_id: &str,
    ) -> Result<Option<String>> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.get_latest_user_message_content(&wid))
            .await
            .context("get_latest_user_message_content spawn_blocking")?
    }

    pub async fn get_recent_review_dialogue_async(
        &self,
        workspace_id: &str,
        max_messages: usize,
    ) -> Result<Vec<(String, String)>> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.get_recent_review_dialogue(&wid, max_messages))
            .await
            .context("get_recent_review_dialogue spawn_blocking")?
    }
}
