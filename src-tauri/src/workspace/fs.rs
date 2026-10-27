use anyhow::Context;
use base64::Engine;
use std::path::Path;

use super::validate_path_within;
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
    #[error("没有要复制的文件")]
    CopyNoSources,
    #[error("源路径不存在：{path}")]
    SourceNotFound { path: String },
    #[error("无法确定源条目名称：{path}")]
    SourceHasNoFileName { path: String },
    #[error("目标位置不是目录：{path}")]
    DestinationNotDirectory { path: String },
    #[error("不能把目录复制到它自己的子目录里（{path}）")]
    CopyIntoOwnSubtree { path: String },
    #[error("同名副本过多，无法生成新的副本名称：{name}")]
    CopyNameExhausted { name: String },
    #[error("{action} 失败（{path}）：{source}")]
    Io {
        action: &'static str,
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("后台文件任务失败：{0}")]
    TauriJoin(#[from] tauri::Error),
}

impl crate::shared::io_error::PathIoError for FsError {
    fn path_io(action: &'static str, path: std::path::PathBuf, source: std::io::Error) -> Self {
        FsError::Io {
            action,
            path,
            source,
        }
    }
}

/// io 错误闭包的类型钉住适配器：构造逻辑在 `shared::io_error`，此处固定
/// 目标错误类型（`?` 经 From 转换的调用点无法唯一推断泛型 E）。
fn io_error(
    action: &'static str,
    path: impl Into<std::path::PathBuf>,
) -> impl FnOnce(std::io::Error) -> FsError {
    crate::shared::io_error::io_error(action, path)
}

impl From<super::PathValidationError> for FsError {
    fn from(error: super::PathValidationError) -> Self {
        match error {
            super::PathValidationError::NotAbsolute => FsError::PathNotAbsolute,
            super::PathValidationError::OutsideAllowed => FsError::OutsideAllowedDirectory,
            super::PathValidationError::Io {
                action,
                path,
                source,
            } => FsError::Io {
                action,
                path,
                source,
            },
        }
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

/// 小文件读写路径的尺寸上限：与 `read_file_content` 的 2MB 门控对齐
///（≥2MB 的大文件走 rope 会话虚拟化编辑，不经本命令）。
const MAX_WRITE_FILE_BYTES: usize = 2 * 1024 * 1024;

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
    validate_path_within(path, project_path)?;
    let path = path.to_string();
    let result = tauri::async_runtime::spawn_blocking(move || -> FsResult<Vec<FsEntry>> {
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
    result
}

#[tauri::command]
pub async fn read_file_content(path: String, project_path: String) -> CommandResult<String> {
    read_file_content_impl(&path, &project_path)
        .await
        .with_context(|| format!("读取文件失败（{}）", path))
        .into_command_result()
}

async fn read_file_content_impl(path: &str, project_path: &str) -> FsResult<String> {
    let validated_path = validate_path_within(path, project_path)?;

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
    let validated_path = validate_path_within(path, project_path)?;

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
    let validated_path = validate_path_within(path, project_path)?;
    if content.len() > MAX_WRITE_FILE_BYTES {
        return Err(FsError::FileTooLarge {
            mb: content.len() as f64 / (1024.0 * 1024.0),
        });
    }

    // 原子写（临时文件 + rename）：编辑器防抖保存中途崩溃/掉电不会留下
    // 半截文件；与 rope_save（大文件路径）及 project::storage 的写入策略一致。
    tauri::async_runtime::spawn_blocking(move || -> FsResult<()> {
        crate::project::storage::atomic_write(&validated_path, &content).map_err(|error| {
            FsError::Io {
                action: "原子写入文件",
                path: validated_path.clone(),
                source: std::io::Error::other(error.to_string()),
            }
        })
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
    let validated_source = validate_path_within(source_path, project_path)?;
    ensure_not_project_root(&validated_source, project_path)?;
    let validated_destination = validate_new_path_within(destination_path, project_path)?;

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
    let validated_path = validate_path_within(path, project_path)?;
    ensure_not_project_root(&validated_path, project_path)?;

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

// ─── Copy（文件树 ⌘C / ⌘V 落盘） ─────────────────────────────────────────────

/// 重名时的副本后缀：`名称 副本.ext` → 名称 副本 2.ext` → ……
const COPY_SUFFIX: &str = " 副本";

/// 生成副本名的尝试上限（防御性上界，正常场景一到两次即可命中）。
const MAX_COPY_NAME_ATTEMPTS: usize = 999;

/// 把 `source_paths` 复制进工作区内的 `destination_dir`，返回实际创建的绝对路径
/// （重名时带「副本」后缀）。
///
/// 语义与 Finder 粘贴一致：
/// - `destination_dir` 必须经 [`validate_path_within`] 校验在工作区内；
/// - 源允许在工作区外（用户从访达复制文件粘进树里），但必须是绝对路径且存在；
/// - 目录递归复制；重名不覆盖，改为「名称 副本.ext」「名称 副本 2.ext」……
/// - 禁止把目录复制进它自己的子树（无限递归）；
/// - 符号链接不跟随：链到普通文件时复制其目标内容，链到目录/悬空时跳过并记日志
///   （见 [`copy_dir_recursive`]）。
#[tauri::command]
pub async fn copy_fs_entry(
    source_paths: Vec<String>,
    destination_dir: String,
    project_path: String,
) -> CommandResult<Vec<String>> {
    copy_fs_entry_impl(&source_paths, &destination_dir, &project_path)
        .await
        .with_context(|| {
            format!(
                "复制文件系统条目失败（{} -> {}）",
                source_paths.join("、"),
                destination_dir
            )
        })
        .into_command_result()
}

async fn copy_fs_entry_impl(
    source_paths: &[String],
    destination_dir: &str,
    project_path: &str,
) -> FsResult<Vec<String>> {
    if source_paths.is_empty() {
        return Err(FsError::CopyNoSources);
    }
    // 目标目录必须存在且位于工作区内；canonicalize 后的路径用于子树判定。
    let destination_dir = validate_path_within(destination_dir, project_path)?;
    let sources: Vec<std::path::PathBuf> = source_paths.iter().map(Into::into).collect();

    tauri::async_runtime::spawn_blocking(move || -> FsResult<Vec<String>> {
        if !destination_dir.is_dir() {
            return Err(FsError::DestinationNotDirectory {
                path: destination_dir.to_string_lossy().into_owned(),
            });
        }

        let mut created = Vec::with_capacity(sources.len());
        for source in &sources {
            let target = copy_entry(source, &destination_dir)?;
            created.push(target.to_string_lossy().into_owned());
        }
        Ok(created)
    })
    .await?
}

/// 复制单个条目（顶层源）：跟随自身的符号链接判断类型（用户选中的「文件」若是
/// 链接，应当拷出内容而不是建一条指向原目标的链接），递归内部再按不跟随策略
/// 逐项处理。
fn copy_entry(source: &Path, destination_dir: &Path) -> FsResult<std::path::PathBuf> {
    if !source.is_absolute() {
        return Err(FsError::PathNotAbsolute);
    }
    let metadata = std::fs::metadata(source).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            FsError::SourceNotFound {
                path: source.to_string_lossy().into_owned(),
            }
        } else {
            FsError::Io {
                action: "读取源条目元信息",
                path: source.to_path_buf(),
                source: error,
            }
        }
    })?;
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| FsError::SourceHasNoFileName {
            path: source.to_string_lossy().into_owned(),
        })?;
    let target = unique_copy_target(destination_dir, &name, metadata.is_dir())?;

    if metadata.is_dir() {
        let canonical_source = source
            .canonicalize()
            .map_err(io_error("解析源目录", source))?;
        // destination_dir 已是 canonicalize 结果：等号与子路径都算「复制进自身」。
        if destination_dir.starts_with(&canonical_source) {
            return Err(FsError::CopyIntoOwnSubtree {
                path: canonical_source.to_string_lossy().into_owned(),
            });
        }
        copy_dir_recursive(source, &target)?;
    } else {
        // std::fs::copy 沿用权限位并跟随符号链接语义。
        std::fs::copy(source, &target).map_err(io_error("复制文件", source))?;
    }

    Ok(target)
}

/// 递归复制目录内容（`target` 由调用方保证不存在）。
fn copy_dir_recursive(source: &Path, target: &Path) -> FsResult<()> {
    std::fs::create_dir(target).map_err(io_error("创建目录", target))?;
    // create_dir 不继承源目录权限位（默认 0777 & umask）：按源目录回设一次，
    // 避免 0700 的私有目录复制进工作区后被放宽成 0755。
    if let Ok(source_metadata) = std::fs::metadata(source) {
        let _ = std::fs::set_permissions(target, source_metadata.permissions());
    }

    let entries = std::fs::read_dir(source).map_err(io_error("读取源目录", source))?;
    for entry in entries {
        let entry = entry.map_err(io_error("读取源目录条目", source))?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        let metadata =
            std::fs::symlink_metadata(&from).map_err(io_error("读取源条目元信息", &from))?;

        if metadata.file_type().is_symlink() {
            // 符号链接不跟随：跟随目录链接会变成递归环，也可能把工作区外的整棵
            // 子树搬进来。链到普通文件时按内容复制（user 看到的是文件），其余跳过。
            match std::fs::metadata(&from) {
                Ok(resolved) if resolved.is_file() => {
                    std::fs::copy(&from, &to).map_err(io_error("复制符号链接指向的文件", &from))?;
                }
                _ => {
                    eprintln!("[fs] 复制目录时跳过符号链接（不跟随）：{}", from.display());
                }
            }
            continue;
        }

        if metadata.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if metadata.is_file() {
            std::fs::copy(&from, &to).map_err(io_error("复制文件", &from))?;
        } else {
            // FIFO / socket / 设备文件：fs::copy 打开读端会永久阻塞，直接跳过。
            eprintln!("[fs] 复制目录时跳过非普通文件：{}", from.display());
        }
    }

    Ok(())
}

/// `symlink_metadata` 探测占用：悬空软链接也算占用，避免覆盖既有条目。
fn path_is_occupied(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// 目标目录下的可用目标名：无同名直接用原名，否则依次尝试「名称 副本.ext」、
/// 「名称 副本 2.ext」……（目录无扩展名时后缀追加在名称末尾）。
fn unique_copy_target(
    destination_dir: &Path,
    name: &str,
    is_dir: bool,
) -> FsResult<std::path::PathBuf> {
    let direct = destination_dir.join(name);
    if !path_is_occupied(&direct) {
        return Ok(direct);
    }

    // 目录名没有「扩展名」概念（`v1.2`/`my.app` 是完整名字）：整体追加副本
    // 后缀，与 Finder 一致；只有文件才按 stem/extension 拆分。
    let (stem, extension) = if is_dir {
        (name.to_string(), None)
    } else {
        (
            Path::new(name)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| name.to_string()),
            Path::new(name)
                .extension()
                .map(|extension| extension.to_string_lossy().into_owned()),
        )
    };

    for index in 1..=MAX_COPY_NAME_ATTEMPTS {
        let suffix = if index == 1 {
            COPY_SUFFIX.to_string()
        } else {
            format!("{COPY_SUFFIX} {index}")
        };
        let candidate_name = match &extension {
            Some(extension) => format!("{stem}{suffix}.{extension}"),
            None => format!("{stem}{suffix}"),
        };
        let candidate = destination_dir.join(candidate_name);
        if !path_is_occupied(&candidate) {
            return Ok(candidate);
        }
    }

    Err(FsError::CopyNameExhausted {
        name: name.to_string(),
    })
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
    let validated_path = validate_path_within(path, project_path)?;

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

        // 行数语义对齐 ropey 的 len_lines()：非空文件在换行符计数上 +1，
        // 空文件为 0 行（前端 FileMeta/rope 视图的行号基准保持一致）。
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDirGuard;
    use std::fs;

    /// 同步跑异步实现：`copy_fs_entry_impl` 内层是 spawn_blocking，测试里直接
    /// 用 tauri 的全局运行时 block_on 即可，无需构造 App handle。
    fn copy_sync(sources: &[&Path], destination: &Path, workspace: &Path) -> FsResult<Vec<String>> {
        let sources: Vec<String> = sources
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        tauri::async_runtime::block_on(copy_fs_entry_impl(
            &sources,
            &destination.to_string_lossy(),
            &workspace.to_string_lossy(),
        ))
    }

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn copies_file_from_outside_workspace_into_destination() {
        let dir = TempDirGuard::new("aha-fs-copy-file");
        let workspace = dir.path().join("workspace");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&workspace).unwrap();
        let source = outside.join("notes.txt");
        write_file(&source, "hello");

        let created = copy_sync(&[&source], &workspace, &workspace).unwrap();

        assert_eq!(created.len(), 1);
        assert_eq!(fs::read_to_string(&created[0]).unwrap(), "hello");
        assert!(Path::new(&created[0]).ends_with("notes.txt"));
    }

    #[test]
    fn suffixes_duplicate_names_instead_of_overwriting() {
        let dir = TempDirGuard::new("aha-fs-copy-dupe");
        let workspace = dir.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let source = dir.path().join("report.md");
        write_file(&source, "v1");

        let first = copy_sync(&[&source], &workspace, &workspace).unwrap();
        let second = copy_sync(&[&source], &workspace, &workspace).unwrap();
        let third = copy_sync(&[&source], &workspace, &workspace).unwrap();

        assert!(first[0].ends_with("report.md"));
        assert!(second[0].ends_with("report 副本.md"), "{}", second[0]);
        assert!(third[0].ends_with("report 副本 2.md"), "{}", third[0]);
        // 原文件内容不被覆盖
        assert_eq!(fs::read_to_string(&first[0]).unwrap(), "v1");
    }

    #[test]
    fn copies_directory_recursively() {
        let dir = TempDirGuard::new("aha-fs-copy-dir");
        let workspace = dir.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let source = dir.path().join("assets");
        write_file(&source.join("a.txt"), "a");
        write_file(&source.join("nested/b.txt"), "b");

        let created = copy_sync(&[&source], &workspace, &workspace).unwrap();

        let copied_root = Path::new(&created[0]);
        assert!(copied_root.is_dir());
        assert_eq!(fs::read_to_string(copied_root.join("a.txt")).unwrap(), "a");
        assert_eq!(
            fs::read_to_string(copied_root.join("nested/b.txt")).unwrap(),
            "b"
        );
    }

    #[test]
    fn copies_multiple_sources_in_one_call() {
        let dir = TempDirGuard::new("aha-fs-copy-multi");
        let workspace = dir.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let first = dir.path().join("one.txt");
        let second = dir.path().join("two.txt");
        write_file(&first, "1");
        write_file(&second, "2");

        let created = copy_sync(&[&first, &second], &workspace, &workspace).unwrap();

        assert_eq!(created.len(), 2);
        assert!(Path::new(&created[0]).ends_with("one.txt"));
        assert!(Path::new(&created[1]).ends_with("two.txt"));
    }

    #[test]
    fn rejects_copying_directory_into_its_own_subtree() {
        let dir = TempDirGuard::new("aha-fs-copy-self");
        let workspace = dir.path().join("workspace");
        let source = workspace.join("docs");
        let inner = source.join("inner");
        write_file(&inner.join("keep.txt"), "x");

        let error = copy_sync(&[&source], &inner, &workspace).unwrap_err();

        assert!(
            matches!(error, FsError::CopyIntoOwnSubtree { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn rejects_destination_outside_workspace() {
        let dir = TempDirGuard::new("aha-fs-copy-outside");
        let workspace = dir.path().join("workspace");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let source = dir.path().join("file.txt");
        write_file(&source, "x");

        let error = copy_sync(&[&source], &outside, &workspace).unwrap_err();

        assert!(
            matches!(error, FsError::OutsideAllowedDirectory),
            "{error:?}"
        );
    }

    #[test]
    fn rejects_missing_source() {
        let dir = TempDirGuard::new("aha-fs-copy-missing");
        let workspace = dir.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let missing = dir.path().join("nope.txt");

        let error = copy_sync(&[&missing], &workspace, &workspace).unwrap_err();

        assert!(matches!(error, FsError::SourceNotFound { .. }), "{error:?}");
    }
}
