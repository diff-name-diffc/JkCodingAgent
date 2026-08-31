//! 消息内容段落（ContentSegment）及其渲染、路径安全校验，以及图片文件的
//! 资源清理。资源清理函数被会话删除/清空逻辑调用（见 sessions / messages 模块）。

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ContentSegment {
    Text {
        id: String,
        text: String,
    },
    Image {
        id: String,
        image_id: String,
        alt: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        mime_type: Option<String>,
        source: String,
        generation_prompt: Option<String>,
    },
    File {
        id: String,
        file_id: String,
        path: String,
        file_name: String,
        mime_type: String,
        size: u64,
    },
}

pub fn segments_to_plain_text(segments: &[ContentSegment]) -> String {
    segments
        .iter()
        .filter_map(|segment| match segment {
            ContentSegment::Text { text, .. } => Some(text.as_str()),
            ContentSegment::Image { .. } | ContentSegment::File { .. } => None,
        })
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn content_to_segments_json(content: &str) -> String {
    let segments = vec![ContentSegment::Text {
        id: Uuid::new_v4().to_string(),
        text: content.to_string(),
    }];
    serde_json::to_string(&segments).unwrap_or_else(|_| "[]".to_string())
}

pub(super) fn parse_segments_json(segments_json: &str) -> Vec<ContentSegment> {
    match serde_json::from_str(segments_json) {
        Ok(segments) => segments,
        Err(e) => {
            let preview = if segments_json.len() > 500 {
                // 按字符边界安全截断：直接切第 500 字节在多字节 UTF-8 字符中间会 panic。
                let end = segments_json.floor_char_boundary(500);
                format!(
                    "{}...(truncated, {} bytes)",
                    &segments_json[..end],
                    segments_json.len()
                )
            } else {
                segments_json.to_string()
            };
            eprintln!("parse_segments_json failed: {e}\n  input: {preview}");
            Vec::new()
        }
    }
}

pub(crate) fn try_parse_segments_json(segments_json: &str) -> Result<Vec<ContentSegment>> {
    serde_json::from_str(segments_json).context("parse dispatcher message segments")
}

pub(super) fn insert_chat_images(
    tx: &rusqlite::Transaction<'_>,
    workspace_id: &str,
    message_id: &str,
    segments: &[ContentSegment],
    created_at: &str,
) -> Result<()> {
    for (index, segment) in segments.iter().enumerate() {
        let ContentSegment::Image {
            image_id,
            alt,
            width,
            height,
            mime_type,
            source,
            generation_prompt,
            ..
        } = segment
        else {
            continue;
        };

        // 段内寻址只有 chat-image://{image_id}，文件系统路径是 chat_images 的
        // 内部索引细节：由 image_id 反查得出，不再信任消息载荷里的路径。
        // 文件缺失时跳过登记并留痕——这类请求会被发送前校验拦截。
        let safe_path = match crate::chat_images::resolve_chat_image_id(image_id) {
            Ok(path) => path,
            Err(error) => {
                eprintln!("skip chat image registration ({image_id}): {error}");
                continue;
            }
        };
        tx.execute(
            "INSERT INTO chat_images (
                id, image_id, workspace_id, message_id, segment_index, path, alt, width, height, mime_type, source, generation_prompt, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(image_id) DO UPDATE SET
                workspace_id = excluded.workspace_id,
                message_id = excluded.message_id,
                segment_index = excluded.segment_index,
                path = excluded.path,
                alt = excluded.alt,
                width = excluded.width,
                height = excluded.height,
                mime_type = excluded.mime_type,
                source = excluded.source,
                generation_prompt = excluded.generation_prompt,
                created_at = excluded.created_at",
            params![
                Uuid::new_v4().to_string(),
                image_id,
                workspace_id,
                message_id,
                index as i64,
                safe_path.to_string_lossy().as_ref(),
                alt,
                width.map(i64::from),
                height.map(i64::from),
                mime_type,
                source,
                generation_prompt,
                created_at,
            ],
        )
        .context("insert chat image")?;
    }

    Ok(())
}

pub(super) fn delete_chat_image_resources(
    tx: &rusqlite::Transaction<'_>,
    workspace_id: &str,
) -> Result<Option<PathBuf>> {
    tx.execute(
        "DELETE FROM chat_images WHERE workspace_id = ?1",
        params![workspace_id],
    )
    .context("delete chat image records")?;

    // 图片按会话目录布局：整目录回收（含登记失败 best-effort 落盘的文件
    // 与暂存未发送的附件）。目录名非法（非 uuid 形态 workspace_id）时跳过
    // 并留痕，不阻断清理。
    match crate::chat_images::workspace_image_dir(workspace_id) {
        Ok(dir) => Ok(Some(dir)),
        Err(error) => {
            eprintln!("skip chat image dir cleanup ({workspace_id}): {error}");
            Ok(None)
        }
    }
}

/// 删除会话图片目录（事务提交后 best-effort 执行；NotFound 容忍）。
pub(super) fn remove_chat_image_dir(dir: &Path) -> Result<()> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("remove chat image dir {}", dir.display()))
        }
    }
}

impl super::DispatcherDb {
    /// 按 image_id 查索引行中的落盘路径（无行返回 None）。索引是图片文件
    /// 的唯一登记簿，`resolve_chat_image_by_id` 以此实现 O(1) 解析。
    pub(crate) fn chat_image_path(&self, image_id: &str) -> Result<Option<PathBuf>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT path FROM chat_images WHERE image_id = ?1")
            .context("prepare chat image path lookup")?;
        let mut rows = stmt
            .query(params![image_id])
            .context("query chat image path")?;
        match rows.next().context("advance chat image path cursor")? {
            Some(row) => Ok(Some(PathBuf::from(row.get::<_, String>(0)?))),
            None => Ok(None),
        }
    }

    /// 发送前校验：segments 中每个 Image 段引用的文件必须存在。缺失时返回
    /// 列出全部失效引用的错误（regenerate/edit 重发的引用可能已被手动清理，
    /// 半残消息入库只会让后续 LLM 请求反复触发缺图降级）。含磁盘 I/O，
    /// 调用方须在阻塞线程执行。
    pub(crate) fn validate_chat_image_segments(&self, segments: &[ContentSegment]) -> Result<()> {
        let mut missing: Vec<String> = Vec::new();
        for segment in segments {
            let ContentSegment::Image { image_id, .. } = segment else {
                continue;
            };
            let resolved = crate::chat_images::resolve_chat_image_by_id(Some(self), image_id);
            if !matches!(&resolved, Ok(path) if path.is_file()) {
                missing.push(format!("chat-image://{image_id}"));
            }
        }
        if !missing.is_empty() {
            anyhow::bail!("图片已失效，请重新粘贴上传：{}", missing.join("、"));
        }
        Ok(())
    }

    /// 保存即登记（统一入口见 `crate::chat_images::save_image`）：写入一行
    /// message_id 为 NULL 的索引记录，消息落库时由 `insert_chat_images`
    /// 重新绑定到具体消息。含同步 SQLite I/O，调用方须在阻塞线程执行。
    pub(crate) fn register_chat_image(
        &self,
        registration: &crate::chat_images::ChatImageRegistration,
        path: &Path,
    ) -> Result<()> {
        self.conn()?
            .execute(
                "INSERT INTO chat_images (
                    id, image_id, workspace_id, message_id, segment_index, path,
                    width, height, mime_type, source, generation_prompt, created_at
                 )
                 VALUES (?1, ?2, ?3, NULL, 0, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(image_id) DO UPDATE SET
                    workspace_id = excluded.workspace_id,
                    path = excluded.path,
                    width = excluded.width,
                    height = excluded.height,
                    mime_type = excluded.mime_type,
                    source = excluded.source,
                    generation_prompt = excluded.generation_prompt",
                params![
                    Uuid::new_v4().to_string(),
                    registration.image_id,
                    registration.workspace_id,
                    path.to_string_lossy(),
                    registration.width.map(i64::from),
                    registration.height.map(i64::from),
                    registration.mime_type,
                    registration.source,
                    registration.generation_prompt,
                    chrono::Utc::now().to_rfc3339(),
                ],
            )
            .context("register chat image")?;
        Ok(())
    }
}
