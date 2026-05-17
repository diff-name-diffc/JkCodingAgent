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

const IGNORED_FILES: &[&str] = &[".DS_Store"];

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

fn validate_new_path_within(
    target: &str,
    allowed_root: &str,
) -> Result<std::path::PathBuf, String> {
    let target = Path::new(target);
    let root = Path::new(allowed_root);

    if !target.is_absolute() {
        return Err("Path must be absolute".to_string());
    }

    let parent = target
        .parent()
        .ok_or_else(|| "Path must have a parent directory".to_string())?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|e| format!("Cannot resolve target parent directory: {}", e))?;
    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("Cannot resolve root directory: {}", e))?;

    if !canonical_parent.starts_with(&canonical_root) {
        return Err("Path is outside the allowed directory".to_string());
    }

    Ok(target.to_path_buf())
}

fn ensure_not_project_root(target: &Path, allowed_root: &str) -> Result<(), String> {
    let canonical_target = target
        .canonicalize()
        .map_err(|e| format!("Cannot resolve path: {}", e))?;
    let canonical_root = Path::new(allowed_root)
        .canonicalize()
        .map_err(|e| format!("Cannot resolve root directory: {}", e))?;

    if canonical_target == canonical_root {
        return Err("Cannot modify the project root".to_string());
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

fn should_ignore_project_file(relative_path: &str) -> bool {
    Path::new(relative_path)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| should_ignore_entry_name(name, false))
}

#[tauri::command]
pub async fn read_dir_entries(path: String, project_path: String) -> Result<Vec<FsEntry>, String> {
    validate_path_within(&path, &project_path)?;
    let entries = std::fs::read_dir(&path).map_err(|e| e.to_string())?;
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
pub async fn move_fs_entry(
    source_path: String,
    destination_path: String,
    project_path: String,
) -> Result<(), String> {
    let validated_source = validate_path_within(&source_path, &project_path)?;
    ensure_not_project_root(&validated_source, &project_path)?;
    let validated_destination = validate_new_path_within(&destination_path, &project_path)?;

    tauri::async_runtime::spawn_blocking(move || {
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
                return Err("A file or folder with the same name already exists".to_string());
            }
        }

        std::fs::rename(&validated_source, &validated_destination).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn delete_fs_entry(path: String, project_path: String) -> Result<(), String> {
    let validated_path = validate_path_within(&path, &project_path)?;
    ensure_not_project_root(&validated_path, &project_path)?;

    tauri::async_runtime::spawn_blocking(move || {
        let metadata = std::fs::symlink_metadata(&validated_path).map_err(|e| e.to_string())?;
        if metadata.is_dir() {
            std::fs::remove_dir_all(&validated_path).map_err(|e| e.to_string())
        } else {
            std::fs::remove_file(&validated_path).map_err(|e| e.to_string())
        }
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
            .filter(|line| !line.is_empty() && !should_ignore_project_file(line))
            .map(|l| l.to_string())
            .collect();

        files.sort();
        files.dedup();
        Ok(files)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_test_dir(prefix: &str) -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
        std::env::temp_dir().join(format!("nezha_fs_{}_{}", prefix, id))
    }

    /// Helper: create a unique temp directory.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let path = unique_test_dir(prefix);
            let _ = fs::create_dir_all(&path);
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    // ── should_ignore_entry_name tests ────────────────────────────────────

    #[test]
    fn ignores_ds_store_in_directory_entries() {
        assert!(should_ignore_entry_name(".DS_Store", false));
        assert!(!should_ignore_entry_name("README.md", false));
    }

    #[test]
    fn ignores_ds_store_anywhere_in_project_file_list() {
        assert!(should_ignore_project_file(".DS_Store"));
        assert!(should_ignore_project_file("src/.DS_Store"));
        assert!(!should_ignore_project_file("src/main.ts"));
    }

    #[test]
    fn ignores_common_dirs() {
        for &dir in IGNORED_DIRS {
            assert!(should_ignore_entry_name(dir, true), "should ignore dir: {}", dir);
        }
    }

    #[test]
    fn does_not_ignore_normal_dirs() {
        assert!(!should_ignore_entry_name("src", true));
        assert!(!should_ignore_entry_name("lib", true));
        assert!(!should_ignore_entry_name("components", true));
    }

    #[test]
    fn ignores_only_files_not_dirs_for_ds_store() {
        assert!(should_ignore_entry_name(".DS_Store", false));
        // is_dir=false with a directory name should not trigger dir ignores
        assert!(!should_ignore_entry_name("node_modules", false));
    }

    #[test]
    fn does_not_ignore_normal_files() {
        assert!(!should_ignore_entry_name("main.rs", false));
        assert!(!should_ignore_entry_name("index.ts", false));
        assert!(!should_ignore_entry_name("package.json", false));
    }

    // ── should_ignore_project_file tests ──────────────────────────────────

    #[test]
    fn ignores_ds_store_at_various_depths() {
        assert!(should_ignore_project_file(".DS_Store"));
        assert!(should_ignore_project_file("src/.DS_Store"));
        assert!(should_ignore_project_file("a/b/c/.DS_Store"));
    }

    #[test]
    fn does_not_ignore_normal_project_files() {
        assert!(!should_ignore_project_file("src/main.ts"));
        assert!(!should_ignore_project_file("README.md"));
        assert!(!should_ignore_project_file("Cargo.toml"));
    }

    // ── previewable_image_mime_type tests ─────────────────────────────────

    #[test]
    fn recognizes_png() {
        assert_eq!(
            previewable_image_mime_type(Path::new("photo.png")),
            Some("image/png")
        );
    }

    #[test]
    fn recognizes_jpg() {
        assert_eq!(
            previewable_image_mime_type(Path::new("photo.jpg")),
            Some("image/jpeg")
        );
    }

    #[test]
    fn recognizes_jpeg() {
        assert_eq!(
            previewable_image_mime_type(Path::new("photo.jpeg")),
            Some("image/jpeg")
        );
    }

    #[test]
    fn recognizes_gif() {
        assert_eq!(
            previewable_image_mime_type(Path::new("anim.gif")),
            Some("image/gif")
        );
    }

    #[test]
    fn recognizes_webp() {
        assert_eq!(
            previewable_image_mime_type(Path::new("img.webp")),
            Some("image/webp")
        );
    }

    #[test]
    fn recognizes_bmp() {
        assert_eq!(
            previewable_image_mime_type(Path::new("img.bmp")),
            Some("image/bmp")
        );
    }

    #[test]
    fn recognizes_svg() {
        assert_eq!(
            previewable_image_mime_type(Path::new("icon.svg")),
            Some("image/svg+xml")
        );
    }

    #[test]
    fn returns_none_for_text_file() {
        assert_eq!(previewable_image_mime_type(Path::new("doc.txt")), None);
    }

    #[test]
    fn returns_none_for_no_extension() {
        assert_eq!(previewable_image_mime_type(Path::new("Makefile")), None);
    }

    #[test]
    fn returns_none_for_non_image_extension() {
        assert_eq!(previewable_image_mime_type(Path::new("file.rs")), None);
        assert_eq!(previewable_image_mime_type(Path::new("file.pdf")), None);
    }

    #[test]
    fn recognizes_uppercase_extension() {
        assert_eq!(
            previewable_image_mime_type(Path::new("photo.PNG")),
            Some("image/png")
        );
        assert_eq!(
            previewable_image_mime_type(Path::new("photo.Jpeg")),
            Some("image/jpeg")
        );
    }

    // ── validate_path_within tests ────────────────────────────────────────

    #[test]
    fn validate_rejects_relative_path() {
        let result = validate_path_within("relative/path", "/some/root");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("absolute"));
    }

    #[test]
    fn validate_rejects_nonexistent_path() {
        let result = validate_path_within("/nonexistent/file.txt", "/nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("resolve"));
    }

    #[test]
    fn validate_accepts_file_within_root() {
        let tmp = TempDir::new("fs_validate");
        let root = tmp.path();
        let file = root.join("test.txt");
        fs::write(&file, "hi").expect("write");

        let result = validate_path_within(file.to_str().unwrap(), root.to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn validate_rejects_path_outside_root() {
        let tmp1 = TempDir::new("fs_validate_out1");
        let tmp2 = TempDir::new("fs_validate_out2");
        let file = tmp2.path().join("secret.txt");
        fs::write(&file, "hi").expect("write");

        let result = validate_path_within(file.to_str().unwrap(), tmp1.path().to_str().unwrap());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("outside"));
    }

    #[test]
    fn validate_accepts_deeply_nested_file() {
        let tmp = TempDir::new("fs_validate_deep");
        let root = tmp.path();
        let deep = root.join("a/b/c");
        fs::create_dir_all(&deep).expect("mkdir");
        let file = deep.join("file.txt");
        fs::write(&file, "hi").expect("write");

        let result = validate_path_within(file.to_str().unwrap(), root.to_str().unwrap());
        assert!(result.is_ok());
    }

    // ── validate_new_path_within tests ────────────────────────────────────

    #[test]
    fn validate_new_rejects_relative_path() {
        let result = validate_new_path_within("relative/path", "/some/root");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("absolute"));
    }

    #[test]
    fn validate_new_rejects_nonexistent_parent() {
        let result = validate_new_path_within("/nonexistent/newfile.txt", "/nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn validate_new_accepts_new_file_in_existing_dir() {
        let tmp = TempDir::new("fs_validate_new");
        let root = tmp.path();
        let existing = root.join("existing.txt");
        fs::write(&existing, "hi").expect("write");
        let new_file = root.join("newfile.txt");

        let result = validate_new_path_within(new_file.to_str().unwrap(), root.to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn validate_new_rejects_outside_root() {
        let tmp1 = TempDir::new("fs_validate_new1");
        let tmp2 = TempDir::new("fs_validate_new2");
        let existing = tmp2.path().join("file.txt");
        fs::write(&existing, "hi").expect("write");

        let new_file = tmp2.path().join("new.txt");
        let result = validate_new_path_within(new_file.to_str().unwrap(), tmp1.path().to_str().unwrap());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("outside"));
    }

    // ── ensure_not_project_root tests ─────────────────────────────────────

    #[test]
    fn ensure_not_project_root_rejects_root_itself() {
        let tmp = TempDir::new("fs_root");
        let root = tmp.path();
        fs::write(root.join(".gitkeep"), "").expect("write");

        let result = ensure_not_project_root(root, root.to_str().unwrap());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Cannot modify the project root"));
    }

    #[test]
    fn ensure_not_project_root_allows_subdirectory() {
        let tmp = TempDir::new("fs_root_sub");
        let root = tmp.path();
        let sub = root.join("subdir");
        fs::create_dir_all(&sub).expect("mkdir");

        let result = ensure_not_project_root(&sub, root.to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn ensure_not_project_root_allows_file() {
        let tmp = TempDir::new("fs_root_file");
        let root = tmp.path();
        let file = root.join("file.txt");
        fs::write(&file, "hi").expect("write");

        let result = ensure_not_project_root(&file, root.to_str().unwrap());
        assert!(result.is_ok());
    }

    // ── FsEntry serialization tests ───────────────────────────────────────

    #[test]
    fn fs_entry_serializes_correctly() {
        let entry = FsEntry {
            name: "test.rs".to_string(),
            path: "/project/test.rs".to_string(),
            is_dir: false,
            extension: Some("rs".to_string()),
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        assert!(json.contains("\"name\""));
        assert!(json.contains("\"is_dir\":false"));
        assert!(json.contains("\"extension\":\"rs\""));
    }

    #[test]
    fn fs_entry_dir_serializes_with_true() {
        let entry = FsEntry {
            name: "src".to_string(),
            path: "/project/src".to_string(),
            is_dir: true,
            extension: None,
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        assert!(json.contains("\"is_dir\":true"));
        assert!(json.contains("\"extension\":null"));
    }

    // ── ImagePreviewData serialization tests ──────────────────────────────

    #[test]
    fn image_preview_data_serializes_camel_case() {
        let data = ImagePreviewData {
            data_url: "data:image/png;base64,abc".to_string(),
            mime_type: "image/png".to_string(),
            byte_length: 12345,
        };
        let json = serde_json::to_string(&data).expect("serialize");
        assert!(json.contains("\"dataUrl\""));
        assert!(json.contains("\"mimeType\""));
        assert!(json.contains("\"byteLength\":12345"));
    }

    // ── FileMeta serialization tests ──────────────────────────────────────

    #[test]
    fn file_meta_serializes_camel_case() {
        let meta = FileMeta {
            size_bytes: 1024,
            line_count: 42,
            is_text: true,
        };
        let json = serde_json::to_string(&meta).expect("serialize");
        assert!(json.contains("\"sizeBytes\":1024"));
        assert!(json.contains("\"lineCount\":42"));
        assert!(json.contains("\"isText\":true"));
    }

    // ── IGNORED_DIRS and IGNORED_FILES constants ──────────────────────────

    #[test]
    fn ignored_dirs_contains_expected_entries() {
        assert!(IGNORED_DIRS.contains(&".git"));
        assert!(IGNORED_DIRS.contains(&"node_modules"));
        assert!(IGNORED_DIRS.contains(&"target"));
        assert!(IGNORED_DIRS.contains(&"dist"));
        assert!(IGNORED_DIRS.contains(&"__pycache__"));
        assert!(IGNORED_DIRS.contains(&".venv"));
    }

    #[test]
    fn ignored_files_contains_ds_store() {
        assert!(IGNORED_FILES.contains(&".DS_Store"));
    }

    #[test]
    fn max_image_preview_is_10mb() {
        assert_eq!(MAX_IMAGE_PREVIEW_BYTES, 10 * 1024 * 1024);
    }
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
