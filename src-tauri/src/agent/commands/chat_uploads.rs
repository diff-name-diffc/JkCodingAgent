//! 普通聊天文件上传：拷贝进会话独占临时目录，返回绝对路径供消息文本引用，
//! 模型经文件工具按会话工作区路径读取。
//!
//! 写入持会话目录写租约：与 Python/Shell 执行、删除/清空回收共用同一互斥域；
//! 会话不存在或非普通聊天会话时 fail-closed，不在删除竞态中复活目录。

use super::*;
use crate::agent::rig_ext::chat_workspace::ChatWorkspace;
use crate::agent::rig_ext::r#loop::resources::{
    AcquireError, Claim, Resource, ResourceArbiter, ResourceLease,
};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::Instant;

/// 上传大小上限：IPC 经 base64 字符串传输，过大既慢又占内存。
const MAX_UPLOAD_BYTES: usize = 32 * 1024 * 1024;

/// 文件名长度上限（字节）：文件系统 NAME_MAX 常见 255，留余量给冲突后缀。
const MAX_FILE_NAME_BYTES: usize = 200;

/// 等待会话目录写租约的上限：Python/长命令运行期间目录被占，超时明确报错。
const LEASE_WAIT_SECS: u64 = 10;

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatUploadFileResult {
    /// 落盘后的绝对路径（canonical），供消息引用与文件工具读取。
    pub path: String,
}

/// 上传文件到普通聊天会话临时目录（回形针选择器的非图片文件走这里；
/// 图片仍走 chat_images 管线）。
#[tauri::command]
pub async fn chat_upload_file(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    file_name: String,
    data_base64: String,
) -> Result<ChatUploadFileResult, String> {
    let db = state.db().clone();
    let app_root = state.agent_config().root_dir;
    chat_upload_file_impl(db, app_root, workspace_id, file_name, data_base64).await
}

async fn chat_upload_file_impl(
    db: DispatcherDb,
    app_root: PathBuf,
    workspace_id: String,
    file_name: String,
    data_base64: String,
) -> Result<ChatUploadFileResult, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&data_base64)
        .map_err(|error| format!("上传文件内容解码失败：{error}"))?;
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(format!(
            "上传文件超过大小上限（{} MB），当前约 {} MB",
            MAX_UPLOAD_BYTES / (1024 * 1024),
            bytes.len().div_ceil(1024 * 1024)
        ));
    }
    let file_name = sanitize_file_name(&file_name)?;
    let session_root = ChatWorkspace::session_directory(&app_root, &workspace_id);
    // 运行方/回收方对 canonical 根互斥：会话目录是符号链接时锁真实目标；
    // 不存在时保留确定性路径身份（回收方同口径）。
    let lease_root = match std::fs::canonicalize(&session_root) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => session_root.clone(),
        Err(error) => return Err(format!("解析会话临时目录失败：{error}")),
    };
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let lease = ResourceArbiter::shared()
        .acquire(
            vec![Claim {
                resource: Resource::LocalFilesystem(lease_root),
                write: true,
            }],
            cancel_rx,
            Instant::now() + Duration::from_secs(LEASE_WAIT_SECS),
        )
        .await
        .map_err(|error| match error {
            AcquireError::QueueTimeout { .. } => {
                "会话临时目录正被 Python 或其他工具使用，请停止相关运行后重试上传".to_string()
            }
            AcquireError::Cancelled => "上传已取消".to_string(),
        })?;
    tokio::task::spawn_blocking(move || {
        // 租约护住整个写入；目录创建放在确认会话仍存在之后。
        let _lease_guard: ResourceLease = lease;
        let session = db
            .get_dispatcher_session(&workspace_id)
            .map_err(|error| format_anyhow_error(&error))?
            .ok_or_else(|| "会话不存在或已删除，无法上传文件".to_string())?;
        if session.kind != DispatcherSessionKind::Chat {
            return Err("仅普通聊天会话支持上传文件到会话临时目录".to_string());
        }
        std::fs::create_dir_all(&session_root)
            .map_err(|error| format!("创建会话临时目录失败：{error}"))?;
        let canonical_root = session_root
            .canonicalize()
            .map_err(|error| format!("解析会话临时目录失败：{error}"))?;
        let target = dedupe_target(&canonical_root, &file_name)?;
        std::fs::write(&target, &bytes).map_err(|error| format!("写入上传文件失败：{error}"))?;
        Ok(ChatUploadFileResult {
            path: target.to_string_lossy().into_owned(),
        })
    })
    .await
    .map_err(|error| format!("上传文件任务失败：{error}"))?
}

/// 只保留文件名本身（剥掉任何路径成分）：路径穿越形态（`../x`、绝对路径）
/// 收敛为最终文件名；空名、过长名与控制字符拒绝。
fn sanitize_file_name(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("上传文件名为空".to_string());
    }
    let name = Path::new(trimmed)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "上传文件名无效".to_string())?;
    // 浏览器 File.name 本就不含分隔符；反斜杠在 unix 上不是分隔符，这里一并剥掉。
    let name = name.rsplit('\\').next().unwrap_or(name).to_string();
    if name.is_empty() || name == "." || name == ".." {
        return Err("上传文件名无效".to_string());
    }
    if name.len() > MAX_FILE_NAME_BYTES {
        return Err(format!("上传文件名过长（上限 {MAX_FILE_NAME_BYTES} 字节）"));
    }
    if name.bytes().any(|b| b.is_ascii_control()) {
        return Err("上传文件名含非法控制字符".to_string());
    }
    Ok(name)
}

/// 同名冲突时不覆盖既有文件，在最后一个扩展名前加 `-2`、`-3`… 递增。
fn dedupe_target(directory: &Path, name: &str) -> Result<PathBuf, String> {
    let candidate = directory.join(name);
    if !candidate.exists() {
        return Ok(candidate);
    }
    // `.env` 之类的点文件整体视为 stem，不把 `env` 当扩展名。
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    };
    for index in 2..1000u32 {
        let next = match ext {
            Some(ext) => directory.join(format!("{stem}-{index}.{ext}")),
            None => directory.join(format!("{stem}-{index}")),
        };
        if !next.exists() {
            return Ok(next);
        }
    }
    Err("会话临时目录内同名上传文件过多，请更换文件名或先清理会话".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDirGuard;

    fn encoded(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[tokio::test]
    async fn uploads_into_session_directory_and_reports_canonical_path() {
        let guard = TempDirGuard::new("chat-upload-basic");
        let app_root = guard.path().join("app");
        let db = DispatcherDb::new(guard.path().join("test.sqlite3")).unwrap();
        let session = db.create_chat_session("first", None).unwrap();

        let result = chat_upload_file_impl(
            db.clone(),
            app_root.clone(),
            session.id.clone(),
            "report.pdf".to_string(),
            encoded(b"%PDF-1.4 hello"),
        )
        .await
        .unwrap();

        let expected_dir = ChatWorkspace::session_directory(&app_root, &session.id)
            .canonicalize()
            .unwrap();
        assert_eq!(Path::new(&result.path), expected_dir.join("report.pdf"));
        let written = std::fs::read(&result.path).unwrap();
        assert_eq!(written, b"%PDF-1.4 hello");
    }

    #[tokio::test]
    async fn rejects_missing_session_and_project_sessions() {
        let guard = TempDirGuard::new("chat-upload-gating");
        let app_root = guard.path().join("app");
        let db = DispatcherDb::new(guard.path().join("test.sqlite3")).unwrap();

        let missing = chat_upload_file_impl(
            db.clone(),
            app_root.clone(),
            "no-such-session".to_string(),
            "a.txt".to_string(),
            encoded(b"x"),
        )
        .await
        .unwrap_err();
        assert!(missing.contains("会话不存在"), "{missing}");

        let project = db.create_project_session("proj-1", "项目会话").unwrap();
        let wrong_kind = chat_upload_file_impl(
            db.clone(),
            app_root.clone(),
            project.id.clone(),
            "a.txt".to_string(),
            encoded(b"x"),
        )
        .await
        .unwrap_err();
        assert!(wrong_kind.contains("仅普通聊天"), "{wrong_kind}");
        // 非普通聊天会话不得留下会话临时目录。
        assert!(!ChatWorkspace::session_directory(&app_root, &project.id).exists());
    }

    #[tokio::test]
    async fn oversize_uploads_are_rejected_before_touching_disk() {
        let guard = TempDirGuard::new("chat-upload-oversize");
        let app_root = guard.path().join("app");
        let db = DispatcherDb::new(guard.path().join("test.sqlite3")).unwrap();
        let session = db.create_chat_session("first", None).unwrap();

        let payload = vec![0u8; MAX_UPLOAD_BYTES + 1];
        let error = chat_upload_file_impl(
            db,
            app_root.clone(),
            session.id.clone(),
            "big.bin".to_string(),
            encoded(&payload),
        )
        .await
        .unwrap_err();
        assert!(error.contains("大小上限"), "{error}");
        assert!(!ChatWorkspace::session_directory(&app_root, &session.id).exists());
    }

    // 真实时钟：spawn_blocking 的落盘线程不参与 paused 时钟的自动推进，
    // 暂停时钟会让等待 JoinHandle 的 timeout 被瞬间推到期。
    #[tokio::test]
    async fn concurrent_lease_holder_defers_upload_until_released() {
        let guard = TempDirGuard::new("chat-upload-lease");
        let app_root = guard.path().join("app");
        let db = DispatcherDb::new(guard.path().join("test.sqlite3")).unwrap();
        let session = db.create_chat_session("first", None).unwrap();
        let session_root = ChatWorkspace::session_directory(&app_root, &session.id);
        std::fs::create_dir_all(&session_root).unwrap();
        let canonical = session_root.canonicalize().unwrap();

        let (_tx, rx) = watch::channel(false);
        let holder = ResourceArbiter::shared()
            .acquire(
                vec![Claim {
                    resource: Resource::LocalFilesystem(canonical.clone()),
                    write: true,
                }],
                rx,
                Instant::now() + Duration::from_secs(60),
            )
            .await
            .unwrap();

        let mut upload = tokio::spawn(chat_upload_file_impl(
            db,
            app_root,
            session.id.clone(),
            "report.pdf".to_string(),
            encoded(b"data"),
        ));
        // 持租约期间上传不落盘；释放后排队写入完成（不因短暂占用直接失败）。
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut upload)
                .await
                .is_err()
        );
        assert!(!canonical.join("report.pdf").exists());
        drop(holder);
        let result = tokio::time::timeout(Duration::from_secs(5), upload)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read(&result.path).unwrap(), b"data");
    }

    #[test]
    fn sanitize_strips_path_components_and_rejects_bad_names() {
        assert_eq!(sanitize_file_name("../etc/passwd").unwrap(), "passwd");
        assert_eq!(sanitize_file_name("/abs/path/a.txt").unwrap(), "a.txt");
        assert_eq!(sanitize_file_name("C:\\dir\\win.txt").unwrap(), "win.txt");
        assert_eq!(sanitize_file_name("  spaced.txt ").unwrap(), "spaced.txt");
        assert_eq!(sanitize_file_name(".env").unwrap(), ".env");
        assert!(sanitize_file_name("   ").is_err());
        assert!(sanitize_file_name("..").is_err());
        assert!(sanitize_file_name(&"a".repeat(MAX_FILE_NAME_BYTES + 1)).is_err());
        assert!(sanitize_file_name("bad\nname").is_err());
    }

    #[test]
    fn dedupe_appends_counter_before_extension_without_overwriting() {
        let guard = TempDirGuard::new("chat-upload-dedupe");
        let dir = guard.path();
        std::fs::write(dir.join("report.pdf"), b"first").unwrap();
        std::fs::write(dir.join("report-2.pdf"), b"second").unwrap();
        std::fs::write(dir.join(".env"), b"secret").unwrap();

        let target = dedupe_target(dir, "report.pdf").unwrap();
        assert_eq!(target, dir.join("report-3.pdf"));
        std::fs::write(&target, b"third").unwrap();
        assert_eq!(std::fs::read(dir.join("report.pdf")).unwrap(), b"first");

        let dotfile = dedupe_target(dir, ".env").unwrap();
        assert_eq!(dotfile, dir.join(".env-2"));

        let plain = dedupe_target(dir, "noext").unwrap();
        assert_eq!(plain, dir.join("noext"));
    }
}
