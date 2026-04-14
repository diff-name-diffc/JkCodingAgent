use base64::Engine;
use std::path::Path;

#[derive(serde::Serialize)]
pub(crate) struct FsEntry {
    name: String,
    path: String,
    is_dir: bool,
    extension: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImagePreviewData {
    data_url: String,
    mime_type: String,
    byte_length: u64,
}

const IGNORED_DIRS: &[&str] = &[
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

const MAX_IMAGE_PREVIEW_BYTES: u64 = 10 * 1024 * 1024;

/// Validate that `target` is an absolute path within `allowed_root` (prevents directory traversal).
fn validate_path_within(target: &str, allowed_root: &str) -> Result<std::path::PathBuf, String> {
    let target = Path::new(target);
    let root = Path::new(allowed_root);

    if !target.is_absolute() {
        return Err("Path must be absolute".to_string());
    }

    let canonical_target = target
        .canonicalize()
        .map_err(|e| format!("Cannot resolve path: {}", e))?;
    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("Cannot resolve root directory: {}", e))?;

    if !canonical_target.starts_with(&canonical_root) {
        return Err("Path is outside the allowed directory".to_string());
    }

    Ok(canonical_target)
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

#[tauri::command]
pub async fn read_dir_entries(path: String, project_path: String) -> Result<Vec<FsEntry>, String> {
    validate_path_within(&path, &project_path)?;
    let entries = std::fs::read_dir(&path).map_err(|e| e.to_string())?;
    let mut result: Vec<FsEntry> = entries
        .flatten()
        .filter(|entry| {
            let p = entry.path();
            if p.is_dir() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                !IGNORED_DIRS.contains(&name_str.as_ref())
            } else {
                true
            }
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
}

#[tauri::command]
pub async fn read_file_content(path: String, project_path: String) -> Result<String, String> {
    let validated_path = validate_path_within(&path, &project_path)?;

    tauri::async_runtime::spawn_blocking(move || {
        use std::io::Read;
        let file = std::fs::File::open(&validated_path).map_err(|e| e.to_string())?;
        let meta = file.metadata().map_err(|e| e.to_string())?;
        if meta.len() > 2 * 1024 * 1024 {
            return Err(format!(
                "File too large ({:.1} MB)",
                meta.len() as f64 / 1024.0 / 1024.0
            ));
        }
        let mut buf = String::with_capacity(meta.len() as usize);
        std::io::BufReader::new(file)
            .read_to_string(&mut buf)
            .map_err(|e| e.to_string())?;
        Ok(buf)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn read_image_preview(
    path: String,
    project_path: String,
) -> Result<ImagePreviewData, String> {
    let validated_path = validate_path_within(&path, &project_path)?;

    tauri::async_runtime::spawn_blocking(move || {
        use std::io::Read;

        let mime_type = previewable_image_mime_type(&validated_path)
            .ok_or_else(|| "Unsupported image format".to_string())?;

        let file = std::fs::File::open(&validated_path).map_err(|e| e.to_string())?;
        let meta = file.metadata().map_err(|e| e.to_string())?;
        if meta.len() > MAX_IMAGE_PREVIEW_BYTES {
            return Err(format!(
                "Image too large ({:.1} MB)",
                meta.len() as f64 / 1024.0 / 1024.0
            ));
        }

        let mut bytes = Vec::with_capacity(meta.len() as usize);
        std::io::BufReader::new(file)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;

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
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn write_file_content(
    path: String,
    content: String,
    project_path: String,
) -> Result<(), String> {
    let validated_path = validate_path_within(&path, &project_path)?;

    tauri::async_runtime::spawn_blocking(move || {
        std::fs::write(&validated_path, content).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn list_project_files(project_path: String) -> Result<Vec<String>, String> {
    let pp = project_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Merge tracked + untracked into a single git command (P7 perf fix)
        let output = std::process::Command::new("git")
            .args([
                "-c",
                "core.quotePath=false",
                "ls-files",
                "-c",
                "-o",
                "--exclude-standard",
            ])
            .current_dir(&pp)
            .output()
            .map_err(|e| e.to_string())?;

        let mut files: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect();

        files.sort();
        files.dedup();
        Ok(files)
    })
    .await
    .map_err(|e| e.to_string())?
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
pub async fn get_file_meta(path: String, project_path: String) -> Result<FileMeta, String> {
    let validated_path = validate_path_within(&path, &project_path)?;

    tauri::async_runtime::spawn_blocking(move || {
        use std::io::Read;

        let file = std::fs::File::open(&validated_path).map_err(|e| e.to_string())?;
        let meta = file.metadata().map_err(|e| e.to_string())?;
        let size_bytes = meta.len();

        // Fast byte-level newline count — reads entire file but only scans for \n.
        // ~50ms for 28MB on modern hardware, much more reliable than sampling.
        let mut reader = std::io::BufReader::with_capacity(256 * 1024, file);
        let mut line_count: u64 = 0;
        let mut is_text = true;
        let mut total_bytes: u64 = 0;
        let mut buf = [0u8; 256 * 1024];

        loop {
            let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
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
    .await
    .map_err(|e| e.to_string())?
}

/// Read a range of lines from a file (0-indexed start, exclusive end).
/// Used for incremental rendering of large files.
#[tauri::command]
pub async fn read_file_chunk(
    path: String,
    project_path: String,
    start_line: u64,
    max_lines: u64,
) -> Result<Vec<String>, String> {
    let validated_path = validate_path_within(&path, &project_path)?;

    tauri::async_runtime::spawn_blocking(move || {
        use std::io::{BufRead, BufReader};

        let file = std::fs::File::open(&validated_path).map_err(|e| e.to_string())?;
        let reader = BufReader::with_capacity(64 * 1024, file);
        let end_line = start_line + max_lines;

        let lines: Vec<String> = reader
            .lines()
            .skip(start_line as usize)
            .take((end_line - start_line) as usize)
            .map(|l| l.unwrap_or_else(|_| String::new()))
            .collect();

        Ok(lines)
    })
    .await
    .map_err(|e| e.to_string())?
}

