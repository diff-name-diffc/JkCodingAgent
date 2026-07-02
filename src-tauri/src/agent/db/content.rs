//! 消息内容段落（ContentSegment）及其渲染、路径安全校验，以及图片文件的
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

pub(crate) fn try_parse_segments_json(segments_json: &str) -> Result<Vec<ContentSegment>> {
    serde_json::from_str(segments_json).context("parse dispatcher message segments")
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

    let normalized = lexical_normalize_path(path);
    let base = crate::chat_images::chat_images_dir().map_err(anyhow::Error::msg)?;
    let normalized_base = lexical_normalize_path(&base);
    if !normalized.starts_with(&normalized_base) {
        anyhow::bail!(
            "chat image path must be inside app-managed chat images directory: {}",
            normalized.display()
        );
    }

    Ok(normalized)
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
) -> Result<Vec<PathBuf>> {
    let mut paths: HashSet<String> = HashSet::new();
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

    let safe_paths = paths
        .into_iter()
        .map(|path| safe_absolute_image_path(&path))
        .collect::<Result<Vec<_>>>()?;

    tx.execute(
        "DELETE FROM chat_images WHERE workspace_id = ?1",
        params![workspace_id],
    )
    .context("delete chat image records")?;

    Ok(safe_paths)
}

pub(super) fn remove_chat_image_files(paths: &[PathBuf]) -> Result<()> {
    for safe_path in paths {
        match std::fs::remove_file(safe_path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("remove chat image {}", safe_path.display()));
            }
        }
    }

    Ok(())
}

fn lexical_normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}
