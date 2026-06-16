//! 消息内容段落（ContentSegment）及其渲染、路径安全校验，以及图片/计划文件的
//! 资源清理。资源清理函数被会话删除/清空逻辑调用（见 sessions / messages 模块）。

use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

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
        path: String,
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

impl ContentSegment {
    pub fn to_markdown(&self) -> String {
        match self {
            ContentSegment::Text { text, .. } => text.clone(),
            ContentSegment::Image {
                alt,
                // `path` is intentionally not included in the rendered markdown
                // output so that raw filesystem paths never appear in the
                // content visible to the user or sent to the LLM.
                image_id,
                ..
            } => {
                format!(
                    "![{}]({})",
                    alt.as_deref().unwrap_or("image"),
                    format!("chat-image://{}", image_id)
                )
            }
            ContentSegment::File { .. } => String::new(),
        }
    }
}

pub fn segments_to_markdown(segments: &[ContentSegment]) -> String {
    segments
        .iter()
        .map(|s| s.to_markdown())
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
                format!(
                    "{}...(truncated, {} bytes)",
                    &segments_json[..500],
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

pub(super) fn safe_absolute_image_path(path: &str) -> Result<PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        anyhow::bail!("chat image path is empty");
    }

    let path = Path::new(trimmed);
    if !path.is_absolute() {
        anyhow::bail!("chat image path must be absolute: {trimmed}");
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        anyhow::bail!("chat image path must not contain parent traversal: {trimmed}");
    }

    Ok(path.to_path_buf())
}

pub(super) fn safe_absolute_plan_path(path: &str) -> Result<PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        anyhow::bail!("plan path is empty");
    }

    let path = Path::new(trimmed);
    if !path.is_absolute() {
        anyhow::bail!("plan path must be absolute: {trimmed}");
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        anyhow::bail!("plan path must not contain parent traversal: {trimmed}");
    }
    if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
        anyhow::bail!("plan path must be a Markdown file: {trimmed}");
    }

    Ok(path.to_path_buf())
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
            path,
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

        let safe_path = safe_absolute_image_path(path)?;
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
) -> Result<()> {
    let mut paths = HashSet::new();
    {
        let mut stmt = tx
            .prepare("SELECT path FROM chat_images WHERE workspace_id = ?1")
            .context("load chat image paths")?;
        let indexed_paths = stmt
            .query_map(params![workspace_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("collect chat image paths")?;
        paths.extend(indexed_paths);
    }
    {
        let mut stmt = tx
            .prepare("SELECT segments_json FROM dispatcher_messages WHERE workspace_id = ?1")
            .context("load dispatcher message segments for image cleanup")?;
        let segments_json = stmt
            .query_map(params![workspace_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("collect dispatcher message segments for image cleanup")?;
        for json in segments_json {
            for segment in parse_segments_json(&json) {
                if let ContentSegment::Image { path, .. } = segment {
                    paths.insert(path);
                }
            }
        }
    }

    for path in paths {
        let safe_path = safe_absolute_image_path(&path)?;
        match std::fs::remove_file(&safe_path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("remove chat image {}", safe_path.display()));
            }
        }
    }

    tx.execute(
        "DELETE FROM chat_images WHERE workspace_id = ?1",
        params![workspace_id],
    )
    .context("delete chat image records")?;

    Ok(())
}

pub(super) fn delete_plan_file_resources(
    tx: &rusqlite::Transaction<'_>,
    workspace_id: &str,
) -> Result<()> {
    let mut paths = HashSet::new();

    let mut stmt = tx
        .prepare("SELECT active_plan_path FROM dispatcher_sessions WHERE id = ?1 AND active_plan_path IS NOT NULL")
        .context("load dispatcher session plan path")?;
    let dispatcher_paths = stmt
        .query_map(params![workspace_id], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("collect dispatcher plan paths")?;
    paths.extend(dispatcher_paths);

    let mut stmt = tx
        .prepare("SELECT active_plan_path FROM project_sessions WHERE id = ?1 AND active_plan_path IS NOT NULL")
        .context("load project session plan path")?;
    let project_paths = stmt
        .query_map(params![workspace_id], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("collect project plan paths")?;
    paths.extend(project_paths);

    for path in paths {
        let safe_path = match safe_absolute_plan_path(&path) {
            Ok(p) => p,
            Err(error) => {
                eprintln!("skip invalid plan path: {error}");
                continue;
            }
        };
        match std::fs::remove_file(&safe_path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                eprintln!("remove plan file {} failed: {error}", safe_path.display());
            }
        }
    }

    Ok(())
}
