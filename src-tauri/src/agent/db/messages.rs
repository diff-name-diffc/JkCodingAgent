//! 消息（dispatcher_messages 表）的增删、历史加载、LLM 历史过滤，以及对应的
//! `spawn_blocking` 异步包装。私有核心插入逻辑（add_message / insert_tool_artifacts）
//! 亦在此处。

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::common::should_keep_llm_message;
use crate::agent::llm::{
    ChatMessage, ChatMessageContentPart, ChatMessageImageSource, OutboundToolCall,
};

use super::artifacts::{DispatcherToolArtifactRef, ToolArtifactDraft};
use super::content::{
    content_to_segments_json, delete_chat_image_resources, insert_chat_images, parse_segments_json,
    remove_chat_image_files, safe_absolute_image_path, segments_to_plain_text,
    try_parse_segments_json, ContentSegment,
};
use super::util::{map_dispatcher_message_record, now, MAX_LLM_DIALOGUES};
use super::DispatcherDb;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherMessageUsageStats {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub elapsed_ms: u64,
    #[serde(default)]
    pub paused: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherMessageRecord {
    pub id: String,
    pub workspace_id: String,
    pub role: String,
    pub content: String,
    pub segments_json: String,
    pub thinking_content: Option<String>,
    pub thinking_elapsed_ms: Option<u64>,
    #[serde(skip_serializing)]
    pub context_payload: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_result_mode: Option<String>,
    pub tool_artifacts: Vec<DispatcherToolArtifactRef>,
    pub tool_calls_json: Option<String>,
    pub usage_stats: Option<DispatcherMessageUsageStats>,
    pub created_at: String,
}

impl DispatcherMessageRecord {
    pub fn to_llm_message(&self) -> Option<ChatMessage> {
        let (content, content_parts) = if let Some(payload) = self.context_payload.clone() {
            (payload, Vec::new())
        } else {
            let segments = parse_segments_json(&self.segments_json);
            (
                segments_to_plain_text(&segments),
                segments_to_llm_content_parts(&self.role, &segments),
            )
        };
        let tool_calls = self
            .tool_calls_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<Vec<OutboundToolCall>>(json).ok());
        let message = ChatMessage {
            reasoning_content: if self.role == "assistant" {
                self.thinking_content
                    .clone()
                    .filter(|content| !content.trim().is_empty())
            } else {
                None
            },
            role: self.role.clone(),
            content,
            content_parts,
            tool_call_id: self.tool_call_id.clone(),
            name: self.tool_name.clone(),
            tool_calls,
        };
        should_keep_llm_message(&message).then_some(message)
    }
}

fn segments_to_llm_content_parts(
    role: &str,
    segments: &[ContentSegment],
) -> Vec<ChatMessageContentPart> {
    if role != "user" {
        return Vec::new();
    }

    segments
        .iter()
        .filter_map(|segment| match segment {
            ContentSegment::Text { text, .. } if !text.trim().is_empty() => {
                Some(ChatMessageContentPart::Text { text: text.clone() })
            }
            ContentSegment::Image { image_id, .. } => Some(ChatMessageContentPart::Image {
                source: ChatMessageImageSource::ChatImage {
                    image_id: image_id.clone(),
                },
            }),
            ContentSegment::Text { .. } | ContentSegment::File { .. } => None,
        })
        .collect()
}

struct NewDispatcherMessage<'a> {
    workspace_id: &'a str,
    role: &'a str,
    content: &'a str,
    segments_json: Option<String>,
    thinking_content: Option<&'a str>,
    thinking_elapsed_ms: u64,
    context_payload: Option<&'a str>,
    tool_call_id: Option<&'a str>,
    tool_name: Option<&'a str>,
    tool_result_mode: Option<&'a str>,
    tool_calls: Option<&'a [OutboundToolCall]>,
    tool_artifacts: &'a [ToolArtifactDraft],
    usage_stats: Option<&'a DispatcherMessageUsageStats>,
    visible: bool,
}

impl DispatcherDb {
    pub fn add_visible_message_from_segments(
        &self,
        workspace_id: &str,
        role: &str,
        segments_json: String,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content: "",
            segments_json: Some(segments_json),
            thinking_content: None,
            thinking_elapsed_ms: 0,
            context_payload: None,
            tool_call_id: None,
            tool_name: None,
            tool_result_mode: None,
            tool_calls: None,
            tool_artifacts: &[],
            usage_stats: None,
            visible: true,
        })
    }

    pub fn add_visible_message_with_usage(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        usage_stats: &DispatcherMessageUsageStats,
    ) -> Result<DispatcherMessageRecord> {
        self.add_visible_message_with_usage_and_thinking(
            workspace_id,
            role,
            content,
            usage_stats,
            None,
            0,
        )
    }

    pub fn add_visible_message_with_usage_and_thinking(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        usage_stats: &DispatcherMessageUsageStats,
        thinking_content: Option<&str>,
        thinking_elapsed_ms: u64,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            segments_json: None,
            thinking_content,
            thinking_elapsed_ms,
            context_payload: None,
            tool_call_id: None,
            tool_name: None,
            tool_result_mode: None,
            tool_calls: None,
            tool_artifacts: &[],
            usage_stats: Some(usage_stats),
            visible: true,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_visible_message_with_tools(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
    ) -> Result<DispatcherMessageRecord> {
        self.add_visible_message_with_tools_and_thinking(
            workspace_id,
            role,
            content,
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls,
            None,
            0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_visible_message_with_tools_and_thinking(
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
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            segments_json: None,
            thinking_content,
            thinking_elapsed_ms,
            context_payload: None,
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls,
            tool_artifacts: &[],
            usage_stats: None,
            visible: true,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_visible_tool_result(
        &self,
        workspace_id: &str,
        content: &str,
        context_payload: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_artifacts: &[ToolArtifactDraft],
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role: "tool",
            content,
            segments_json: None,
            thinking_content: None,
            thinking_elapsed_ms: 0,
            context_payload: Some(context_payload),
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls: None,
            tool_artifacts,
            usage_stats: None,
            visible: true,
        })
    }

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

    /// 当前会话实际绑定的聊天图片文件。运行时可把这些精确文件加入路径授权，
    /// 无需放行整个全局 chat-images 目录或其他会话的图片。
    pub fn list_chat_image_paths(&self, workspace_id: &str) -> Result<Vec<PathBuf>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT path FROM chat_images
                 WHERE workspace_id = ?1
                 GROUP BY path
                 ORDER BY MIN(created_at) ASC, path ASC",
            )
            .context("prepare chat image path list")?;
        let paths = stmt
            .query_map(params![workspace_id], |row| {
                row.get::<_, String>(0).map(PathBuf::from)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("collect chat image paths")?;
        Ok(paths)
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
        let image_paths = delete_chat_image_resources(&tx, workspace_id)?;
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
        // 数据库清空已提交，图片文件清理失败不应把清空误报为失败。改为 best-effort。
        if let Err(error) = remove_chat_image_files(&image_paths) {
            eprintln!("remove chat image files failed (clear messages {workspace_id}): {error:#}");
        }
        Ok(())
    }

    /// 删除指定消息及其之后的所有消息（含属于这些消息的工具产物与工具运行记录）。
    /// 用于「从该条用户消息重新生成」：先截断再重发，避免重复消息。
    /// 同时在事务内删除被删消息的 chat_images 记录，提交后清理不再被引用的图片
    /// 文件——一旦截断后未重发、或重发内容不含原图，也不会留下孤儿记录与文件泄漏。
    /// 重发复用同一 image_id 时，insert_chat_images 会为图片重新插入记录。
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

        // 先收集将被删消息引用的图片路径（chat_images 记录 + 消息内图片段落），
        // 必须在删除 dispatcher_messages 之前完成。
        let mut candidate_paths: HashSet<String> = HashSet::new();
        {
            let mut stmt = tx
                .prepare(
                    "SELECT path FROM chat_images
                     WHERE workspace_id = ?1 AND message_id IN (
                         SELECT id FROM dispatcher_messages
                         WHERE workspace_id = ?1 AND rowid >= ?2)",
                )
                .context("load truncated chat image paths")?;
            let indexed = stmt
                .query_map(params![workspace_id, target_rowid], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<std::result::Result<Vec<_>, _>>()
                .context("collect truncated chat image paths")?;
            candidate_paths.extend(indexed);
        }
        {
            let mut stmt = tx
                .prepare(
                    "SELECT segments_json FROM dispatcher_messages
                     WHERE workspace_id = ?1 AND rowid >= ?2",
                )
                .context("load truncated message segments for image cleanup")?;
            let segments_json = stmt
                .query_map(params![workspace_id, target_rowid], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<std::result::Result<Vec<_>, _>>()
                .context("collect truncated message segments for image cleanup")?;
            for json in segments_json {
                for segment in parse_segments_json(&json) {
                    if let ContentSegment::Image { path, .. } = segment {
                        candidate_paths.insert(path);
                    }
                }
            }
        }

        tx.execute(
            "DELETE FROM chat_images
             WHERE workspace_id = ?1 AND message_id IN (
                 SELECT id FROM dispatcher_messages
                 WHERE workspace_id = ?1 AND rowid >= ?2)",
            params![workspace_id, target_rowid],
        )
        .context("delete truncated chat images")?;
        let removed = tx
            .execute(
                "DELETE FROM dispatcher_messages WHERE workspace_id = ?1 AND rowid >= ?2",
                params![workspace_id, target_rowid],
            )
            .context("truncate dispatcher messages")?;

        // 删除后仍被幸存记录（更早消息的 chat_images 记录或消息段落）引用的图片
        // 不能删文件，只清理真正无引用的孤儿文件。
        let mut orphan_paths: Vec<PathBuf> = Vec::new();
        for path in candidate_paths {
            let still_referenced: i64 = tx
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM chat_images
                         WHERE workspace_id = ?1 AND path = ?2
                     ) OR EXISTS(
                         SELECT 1 FROM dispatcher_messages
                         WHERE workspace_id = ?1 AND instr(segments_json, ?2) > 0
                     )",
                    params![workspace_id, &path],
                    |row| row.get(0),
                )
                // 查询失败时保守处理：视为仍被引用，不删文件。
                .unwrap_or(1);
            if still_referenced != 0 {
                continue;
            }
            match safe_absolute_image_path(&path) {
                Ok(safe_path) => orphan_paths.push(safe_path),
                Err(error) => {
                    eprintln!("skip invalid chat image path {path:?}: {error:#}");
                }
            }
        }

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

        // 数据库截断已提交，图片文件清理失败不应把截断误报为失败。改为 best-effort。
        if let Err(error) = remove_chat_image_files(&orphan_paths) {
            eprintln!(
                "remove chat image files failed (truncate messages {workspace_id}): {error:#}"
            );
        }
        Ok(removed as u64)
    }
    fn add_message(&self, params: NewDispatcherMessage<'_>) -> Result<DispatcherMessageRecord> {
        let tool_calls_json = params
            .tool_calls
            .map(serde_json::to_string)
            .transpose()
            .context("serialize tool calls")?;
        let usage_stats_json = params
            .usage_stats
            .map(serde_json::to_string)
            .transpose()
            .context("serialize dispatcher message usage stats")?;

        let segments_json = params
            .segments_json
            .unwrap_or_else(|| content_to_segments_json(params.content));
        let segments = try_parse_segments_json(&segments_json)?;
        let content = segments_to_plain_text(&segments);

        let mut record = DispatcherMessageRecord {
            id: Uuid::new_v4().to_string(),
            workspace_id: params.workspace_id.to_string(),
            role: params.role.to_string(),
            content,
            segments_json,
            thinking_content: params
                .thinking_content
                .filter(|content| !content.trim().is_empty())
                .map(|s| s.to_string()),
            thinking_elapsed_ms: params
                .thinking_content
                .filter(|content| !content.trim().is_empty())
                .map(|_| params.thinking_elapsed_ms),
            context_payload: params.context_payload.map(|s| s.to_string()),
            tool_call_id: params.tool_call_id.map(|s| s.to_string()),
            tool_name: params.tool_name.map(|s| s.to_string()),
            tool_result_mode: params.tool_result_mode.map(|s| s.to_string()),
            tool_artifacts: Vec::new(),
            tool_calls_json: tool_calls_json.clone(),
            usage_stats: params.usage_stats.cloned(),
            created_at: now(),
        };

        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE dispatcher_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&record.created_at, &record.workspace_id],
        )?;
        tx.execute(
            "UPDATE chat_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&record.created_at, &record.workspace_id],
        )?;
        tx.execute(
            "UPDATE project_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&record.created_at, &record.workspace_id],
        )?;

        let thinking_elapsed_ms = record
            .thinking_elapsed_ms
            .map(i64::try_from)
            .transpose()
            .context("convert thinking elapsed milliseconds for sqlite")?;

        tx.execute(
            "INSERT INTO dispatcher_messages (
                id, workspace_id, role, segments_json, thinking_content, thinking_elapsed_ms, context_payload, tool_call_id, tool_name, tool_result_mode, tool_artifacts_json, tool_calls_json, usage_stats_json, visible, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                &record.id,
                &record.workspace_id,
                &record.role,
                &record.segments_json,
                &record.thinking_content,
                &thinking_elapsed_ms,
                &record.context_payload,
                &record.tool_call_id,
                &record.tool_name,
                &record.tool_result_mode,
                Option::<String>::None,
                &record.tool_calls_json,
                &usage_stats_json,
                if params.visible { 1 } else { 0 },
                &record.created_at
            ],
        )
        .context("insert dispatcher message")?;

        insert_chat_images(
            &tx,
            &record.workspace_id,
            &record.id,
            &segments,
            &record.created_at,
        )?;

        if !params.tool_artifacts.is_empty() {
            let artifacts = self.insert_tool_artifacts(
                &tx,
                &record.workspace_id,
                &record.id,
                record.tool_call_id.as_deref(),
                record.tool_name.as_deref(),
                params.tool_artifacts,
                &record.created_at,
            )?;
            let artifacts_json =
                serde_json::to_string(&artifacts).context("serialize dispatcher tool artifacts")?;
            tx.execute(
                "UPDATE dispatcher_messages SET tool_artifacts_json = ?1 WHERE id = ?2",
                params![&artifacts_json, &record.id],
            )
            .context("attach dispatcher tool artifacts to message")?;
            record.tool_artifacts = artifacts;
        }

        tx.commit().context("commit dispatcher message insert")?;

        Ok(record)
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_tool_artifacts(
        &self,
        tx: &rusqlite::Transaction<'_>,
        workspace_id: &str,
        message_id: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        drafts: &[ToolArtifactDraft],
        created_at: &str,
    ) -> Result<Vec<DispatcherToolArtifactRef>> {
        let mut refs = Vec::with_capacity(drafts.len());
        let tool_run_id = match tool_call_id {
            Some(tool_call_id) => tx
                .query_row(
                    "SELECT id FROM dispatcher_tool_runs
                     WHERE workspace_id = ?1 AND tool_call_id = ?2
                     ORDER BY created_at DESC, rowid DESC
                     LIMIT 1",
                    params![workspace_id, tool_call_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .context("resolve dispatcher tool run for artifact")?,
            None => None,
        };

        for draft in drafts {
            let artifact_id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO dispatcher_tool_artifacts (
                    id, workspace_id, message_id, tool_call_id, tool_run_id, tool_name,
                    title, kind, preview, content, char_count, line_count, created_at
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    &artifact_id,
                    workspace_id,
                    message_id,
                    tool_call_id,
                    &tool_run_id,
                    tool_name,
                    &draft.title,
                    &draft.kind,
                    &draft.preview,
                    &draft.content,
                    draft.char_count as i64,
                    draft.line_count as i64,
                    created_at,
                ],
            )
            .context("insert dispatcher tool artifact")?;

            refs.push(DispatcherToolArtifactRef {
                id: artifact_id,
                title: draft.title.clone(),
                kind: draft.kind.clone(),
                preview: draft.preview.clone(),
                char_count: draft.char_count,
                line_count: draft.line_count,
                created_at: created_at.to_string(),
            });
        }

        Ok(refs)
    }
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

    pub async fn list_chat_image_paths_async(&self, workspace_id: &str) -> Result<Vec<PathBuf>> {
        let db = self.clone();
        let workspace_id = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.list_chat_image_paths(&workspace_id))
            .await
            .context("list_chat_image_paths spawn_blocking")?
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
}

// G9-05：LLM 上下文过滤的唯一实现位于 `crate::agent::common::should_keep_llm_message`。
// 本文件的 `load_llm_history` 与 `DispatcherMessageRecord::to_llm_message` 直接委托，
// 不再维护同口径的第二份私有过滤函数，消除双实现漂移风险。

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rusqlite::params;

    use super::DispatcherDb;

    fn test_db() -> DispatcherDb {
        let path = std::env::temp_dir().join(format!(
            "jkcodingagent-messages-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        DispatcherDb::new(path).expect("create test dispatcher db")
    }

    fn add_text_message(db: &DispatcherDb, session_id: &str, role: &str, text: &str) {
        let segments_json = super::super::content::content_to_segments_json(text);
        db.add_visible_message_from_segments(session_id, role, segments_json)
            .expect("add message");
    }

    #[test]
    fn count_visible_messages_is_session_scoped() {
        let db = test_db();
        let session = db
            .create_chat_session("messages", Some("tech"))
            .expect("create chat session");
        assert_eq!(db.count_visible_messages(&session.id).expect("count"), 0);

        add_text_message(&db, &session.id, "user", "你好");
        add_text_message(&db, &session.id, "assistant", "有什么可以帮你？");
        assert_eq!(db.count_visible_messages(&session.id).expect("count"), 2);

        // 会话隔离：其他会话的消息不计入本会话计数。
        let other = db
            .create_chat_session("other", Some("tech"))
            .expect("create other chat session");
        add_text_message(&db, &other.id, "user", "另一会话的消息");
        assert_eq!(db.count_visible_messages(&session.id).expect("count"), 2);
        assert_eq!(db.count_visible_messages(&other.id).expect("count"), 1);
    }

    #[test]
    fn chat_image_path_list_is_session_scoped() {
        let db = test_db();
        let first = db
            .create_chat_session("first", Some("tech"))
            .expect("create first session");
        let second = db
            .create_chat_session("second", Some("tech"))
            .expect("create second session");
        let first_message = db
            .add_visible_message_from_segments(
                &first.id,
                "user",
                super::super::content::content_to_segments_json("first"),
            )
            .expect("add first message");
        let second_message = db
            .add_visible_message_from_segments(
                &second.id,
                "user",
                super::super::content::content_to_segments_json("second"),
            )
            .expect("add second message");
        let conn = db.conn().expect("db conn");
        for (index, (workspace_id, message_id, path)) in [
            (&first.id, &first_message.id, "/tmp/first.png"),
            (&second.id, &second_message.id, "/tmp/second.png"),
        ]
        .into_iter()
        .enumerate()
        {
            conn.execute(
                "INSERT INTO chat_images (
                    id, image_id, workspace_id, message_id, segment_index, path, created_at
                 ) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)",
                params![
                    format!("row-{index}"),
                    format!("image-{index}"),
                    workspace_id,
                    message_id,
                    path,
                    format!("2026-01-01T00:00:0{index}Z"),
                ],
            )
            .expect("insert chat image");
        }

        assert_eq!(
            db.list_chat_image_paths(&first.id)
                .expect("list first paths"),
            vec![PathBuf::from("/tmp/first.png")]
        );
    }
}
