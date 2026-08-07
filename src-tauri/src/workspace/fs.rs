use anyhow::Context;
use base64::Engine;
use std::path::Path;

use crate::shared::error::{CommandResult, IntoCommandResult};

type FsResult<T> = std::result::Result<T, FsError>;

#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("路径必须是绝对路径")]
    PathNotAbsolute,
    #[error("路径必须有父目录")]
    MissingParent,
    #[error("路径不在允许目录内")]
    OutsideAllowedDirectory,
    #[error("不能修改项目根目录")]
    ProjectRootModification,
    #[error("不支持的图片格式")]
    UnsupportedImageFormat,
    #[error("文件过大（{mb:.1} MB）")]
    FileTooLarge { mb: f64 },
    #[error("图片过大（{mb:.1} MB）")]
    ImageTooLarge { mb: f64 },
    #[error("目标位置已存在同名文件或目录")]
    DestinationExists,
    #[error("{action} 失败（{path}）：{source}")]
    Io {
        action: &'static str,
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("后台文件任务失败：{0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("后台文件任务失败：{0}")]
    TauriJoin(#[from] tauri::Error),
}

fn io_error(
    action: &'static str,
    path: impl Into<std::path::PathBuf>,
) -> impl FnOnce(std::io::Error) -> FsError {
    move |source| FsError::Io {
        action,
        path: path.into(),
        source,
    }
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct FsEntry {
    name: String,
    path: String,
    is_dir: bool,
    extension: Option<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImagePreviewData {
    data_url: String,
    mime_type: String,
    byte_length: u64,
}

pub(crate) const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    ".next",
    ".nuxt",
    "dist",
    "build",
    "target",
    "__pycache__",
    ".cache",
    "coverage",
    ".turbo",
    ".expo",
    "out",
    ".output",
    ".venv",
    "venv",
    ".tox",
];

const IGNORED_FILES: &[&str] = &[".DS_Store"];

const MAX_IMAGE_PREVIEW_BYTES: u64 = 10 * 1024 * 1024;

/// Validate that `target` is an absolute path within `allowed_root` (prevents directory traversal).
fn validate_path_within(target: &str, allowed_root: &str) -> FsResult<std::path::PathBuf> {
    let target = Path::new(target);
    let root = Path::new(allowed_root);

    if !target.is_absolute() {
        return Err(FsError::PathNotAbsolute);
    }

    let canonical_target = target
        .canonicalize()
        .map_err(io_error("解析目标路径", target))?;
    let canonical_root = root
        .canonicalize()
        .map_err(io_error("解析项目根目录", root))?;

    if !canonical_target.starts_with(&canonical_root) {
        return Err(FsError::OutsideAllowedDirectory);
    }

    Ok(canonical_target)
}

fn validate_new_path_within(target: &str, allowed_root: &str) -> FsResult<std::path::PathBuf> {
    let target = Path::new(target);
    let root = Path::new(allowed_root);

    if !target.is_absolute() {
        return Err(FsError::PathNotAbsolute);
    }

    let parent = target.parent().ok_or(FsError::MissingParent)?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(io_error("解析目标父目录", parent))?;
    let canonical_root = root
        .canonicalize()
        .map_err(io_error("解析项目根目录", root))?;

    if !canonical_parent.starts_with(&canonical_root) {
        return Err(FsError::OutsideAllowedDirectory);
    }

    Ok(target.to_path_buf())
}

fn ensure_not_project_root(target: &Path, allowed_root: &str) -> FsResult<()> {
    let canonical_target = target
        .canonicalize()
        .map_err(io_error("解析目标路径", target))?;
    let canonical_root = Path::new(allowed_root)
        .canonicalize()
        .map_err(io_error("解析项目根目录", allowed_root))?;

    if canonical_target == canonical_root {
        return Err(FsError::ProjectRootModification);
    }

    Ok(())
}

fn previewable_image_mime_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

fn should_ignore_entry_name(name: &str, is_dir: bool) -> bool {
    if is_dir {
        return IGNORED_DIRS.contains(&name);
    }

    IGNORED_FILES.contains(&name)
}

#[tauri::command]
pub async fn read_dir_entries(path: String, project_path: String) -> CommandResult<Vec<FsEntry>> {
    read_dir_entries_impl(&path, &project_path)
        .await
        .with_context(|| format!("读取目录失败（{}）", path))
        .into_command_result()
}

async fn read_dir_entries_impl(path: &str, project_path: &str) -> FsResult<Vec<FsEntry>> {
    validate_path_within(&path, &project_path)?;
    let path = path.to_string();
    let result =
        tauri::async_runtime::spawn_blocking(move || -> FsResult<Vec<FsEntry>> {
            let entries = std::fs::read_dir(&path).map_err(io_error("读取目录", &path))?;
            let mut result: Vec<FsEntry> = entries
                .flatten()
                .filter(|entry| {
                    let p = entry.path();
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    !should_ignore_entry_name(name_str.as_ref(), p.is_dir())
                })
                .map(|entry| {
                    let p = entry.path();
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let is_dir = p.is_dir();
                    let extension = p
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|s| s.to_lowercase());
                    FsEntry {
                        name,
                        path: p.to_string_lossy().into_owned(),
                        is_dir,
                        extension,
                    }
                })
                .collect();
            result.sort_by(|a, b| match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            });
            Ok(result)
        })
        .await?;
    Ok(result?)
}

#[tauri::command]
pub async fn read_file_content(path: String, project_path: String) -> CommandResult<String> {
    read_file_content_impl(&path, &project_path)
        .await
        .with_context(|| format!("读取文件失败（{}）", path))
        .into_command_result()
}

async fn read_file_content_impl(path: &str, project_path: &str) -> FsResult<String> {
    let validated_path = validate_path_within(&path, &project_path)?;

    tauri::async_runtime::spawn_blocking(move || -> FsResult<String> {
        use std::io::Read;
        let file =
            std::fs::File::open(&validated_path).map_err(io_error("打开文件", &validated_path))?;
        let meta = file
            .metadata()
            .map_err(io_error("读取文件元信息", &validated_path))?;
        if meta.len() > 2 * 1024 * 1024 {
            return Err(FsError::FileTooLarge {
                mb: meta.len() as f64 / 1024.0 / 1024.0,
            });
        }
        let mut buf = String::with_capacity(meta.len() as usize);
        std::io::BufReader::new(file)
            .read_to_string(&mut buf)
            .map_err(io_error("读取文件内容", &validated_path))?;
        Ok(buf)
    })
    .await?
}

#[tauri::command]
pub async fn read_image_preview(
    path: String,
    project_path: String,
) -> CommandResult<ImagePreviewData> {
    read_image_preview_impl(&path, &project_path)
        .await
        .with_context(|| format!("读取图片预览失败（{}）", path))
        .into_command_result()
}

async fn read_image_preview_impl(path: &str, project_path: &str) -> FsResult<ImagePreviewData> {
    let validated_path = validate_path_within(&path, &project_path)?;

    tauri::async_runtime::spawn_blocking(move || -> FsResult<ImagePreviewData> {
        use std::io::Read;

        let mime_type =
            previewable_image_mime_type(&validated_path).ok_or(FsError::UnsupportedImageFormat)?;

        let file =
            std::fs::File::open(&validated_path).map_err(io_error("打开图片", &validated_path))?;
        let meta = file
            .metadata()
            .map_err(io_error("读取图片元信息", &validated_path))?;
        if meta.len() > MAX_IMAGE_PREVIEW_BYTES {
            return Err(FsError::ImageTooLarge {
                mb: meta.len() as f64 / 1024.0 / 1024.0,
            });
        }

        let mut bytes = Vec::with_capacity(meta.len() as usize);
        std::io::BufReader::new(file)
            .read_to_end(&mut bytes)
            .map_err(io_error("读取图片内容", &validated_path))?;

        Ok(ImagePreviewData {
            data_url: format!(
                "data:{};base64,{}",
                mime_type,
                base64::engine::general_purpose::STANDARD.encode(bytes)
            ),
            mime_type: mime_type.to_string(),
            byte_length: meta.len(),
        })
    })
    .await?
}

#[tauri::command]
pub async fn write_file_content(
    path: String,
    content: String,
    project_path: String,
) -> CommandResult<()> {
    write_file_content_impl(&path, content, &project_path)
        .await
        .with_context(|| format!("写入文件失败（{}）", path))
        .into_command_result()
}

async fn write_file_content_impl(path: &str, content: String, project_path: &str) -> FsResult<()> {
    let validated_path = validate_path_within(&path, &project_path)?;

    tauri::async_runtime::spawn_blocking(move || -> FsResult<()> {
        std::fs::write(&validated_path, content).map_err(io_error("写入文件", &validated_path))
    })
    .await?
}

#[tauri::command]
pub async fn move_fs_entry(
    source_path: String,
    destination_path: String,
    project_path: String,
) -> CommandResult<()> {
    move_fs_entry_impl(&source_path, &destination_path, &project_path)
        .await
        .with_context(|| {
            format!(
                "移动文件系统条目失败（{} -> {}）",
                source_path, destination_path
            )
        })
        .into_command_result()
}

async fn move_fs_entry_impl(
    source_path: &str,
    destination_path: &str,
    project_path: &str,
) -> FsResult<()> {
    let validated_source = validate_path_within(&source_path, &project_path)?;
    ensure_not_project_root(&validated_source, &project_path)?;
    let validated_destination = validate_new_path_within(&destination_path, &project_path)?;

    tauri::async_runtime::spawn_blocking(move || -> FsResult<()> {
        if validated_source == validated_destination {
            return Ok(());
        }

        if validated_destination.exists() {
            let source_is_same_entry = validated_destination
                .canonicalize()
                .ok()
                .zip(validated_source.canonicalize().ok())
                .is_some_and(|(destination, source)| destination == source);

            if !source_is_same_entry {
                return Err(FsError::DestinationExists);
            }
        }

        std::fs::rename(&validated_source, &validated_destination)
            .map_err(io_error("移动文件系统条目", &validated_source))
    })
    .await?
}

#[tauri::command]
pub async fn delete_fs_entry(path: String, project_path: String) -> CommandResult<()> {
    delete_fs_entry_impl(&path, &project_path)
        .await
        .with_context(|| format!("删除文件系统条目失败（{}）", path))
        .into_command_result()
}

async fn delete_fs_entry_impl(path: &str, project_path: &str) -> FsResult<()> {
    let validated_path = validate_path_within(&path, &project_path)?;
    ensure_not_project_root(&validated_path, &project_path)?;

    tauri::async_runtime::spawn_blocking(move || -> FsResult<()> {
        let metadata = std::fs::symlink_metadata(&validated_path)
            .map_err(io_error("读取文件系统条目元信息", &validated_path))?;
        if metadata.is_dir() {
            std::fs::remove_dir_all(&validated_path).map_err(io_error("删除目录", &validated_path))
        } else {
            std::fs::remove_file(&validated_path).map_err(io_error("删除文件", &validated_path))
        }
    })
    .await?
}

// ─── Large-file support commands ────────────────────────────────────────────

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileMeta {
    size_bytes: u64,
    line_count: u64,
    is_text: bool,
}

/// Returns file size, line count, and whether the file is valid text.
/// Frontend uses this to decide which rendering path to take.
#[tauri::command]
pub async fn get_file_meta(path: String, project_path: String) -> CommandResult<FileMeta> {
    get_file_meta_impl(&path, &project_path)
        .await
        .with_context(|| format!("读取文件元信息失败（{}）", path))
        .into_command_result()
}

async fn get_file_meta_impl(path: &str, project_path: &str) -> FsResult<FileMeta> {
    let validated_path = validate_path_within(&path, &project_path)?;

    tauri::async_runtime::spawn_blocking(move || -> FsResult<FileMeta> {
        use std::io::Read;

        let file =
            std::fs::File::open(&validated_path).map_err(io_error("打开文件", &validated_path))?;
        let meta = file
            .metadata()
            .map_err(io_error("读取文件元信息", &validated_path))?;
        let size_bytes = meta.len();

        // Fast byte-level newline count — reads entire file but only scans for \n.
        // ~50ms for 28MB on modern hardware, much more reliable than sampling.
        let mut reader = std::io::BufReader::with_capacity(256 * 1024, file);
        let mut line_count: u64 = 0;
        let mut is_text = true;
        let mut total_bytes: u64 = 0;
        let mut buf = [0u8; 256 * 1024];

        loop {
            let n = reader
                .read(&mut buf)
                .map_err(io_error("扫描文件内容", &validated_path))?;
            if n == 0 {
                break;
            }

            let chunk = &buf[..n];

            // Check for binary content (NUL bytes) in the first 8KB
            if total_bytes < 8192 {
                let check_end = std::cmp::min(n, (8192 - total_bytes) as usize);
                if chunk[..check_end].contains(&0u8) {
                    is_text = false;
                    // Still count lines for display purposes, but mark as binary
                }
            }

            // Count newline bytes
            for &byte in chunk {
                if byte == b'\n' {
                    line_count += 1;
                }
            }

            total_bytes += n as u64;
        }

        // Account for last line if file doesn't end with newline
        if size_bytes > 0 {
            line_count += 1;
        }

        Ok(FileMeta {
            size_bytes,
            line_count,
            is_text,
        })
    })
    .await?
}
