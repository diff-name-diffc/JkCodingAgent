use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::watch;

use super::{blocking, UpdateContext, UpdateFailure};
use crate::agent::log_snapshot::model::LogSource;
use crate::agent::rig_ext::tools::common::resolve_path;

pub(super) async fn resolve_local_sources(
    context: &UpdateContext,
    sources: Vec<LogSource>,
) -> Result<Vec<LogSource>, UpdateFailure> {
    let mut resolved = Vec::with_capacity(sources.len());
    for source in sources {
        match source {
            LogSource::LocalFile { path } => {
                let root = context.session_root.clone();
                let extra = context.extra_allowed_dirs.clone();
                let canonical = blocking(move || {
                    resolve_path(&root, true, &extra, &path).and_then(|path| {
                        let path = path
                            .canonicalize()
                            .map_err(|error| format!("解析本地日志路径失败：{error}"))?;
                        let path = resolve_path(&root, true, &extra, &path.to_string_lossy())?;
                        if !path.is_file() {
                            return Err(format!("本地日志不是普通文件：{}", path.display()));
                        }
                        Ok(path)
                    })
                })
                .await??;
                resolved.push(LogSource::LocalFile {
                    path: canonical.to_string_lossy().into_owned(),
                });
            }
            other => resolved.push(other),
        }
    }
    Ok(resolved)
}

pub(super) fn copy_local_file(
    source: &Path,
    destination: &Path,
    max_bytes: u64,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
    counter: Arc<AtomicU64>,
) -> Result<u64, UpdateFailure> {
    use std::io::{Read, Write};

    validate_text_file(source)?;
    let mut input = std::fs::File::open(source)
        .map_err(|error| UpdateFailure::failed(format!("打开本地日志失败：{error}")))?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options
        .open(destination)
        .map_err(|error| UpdateFailure::failed(format!("创建本地日志暂存文件失败：{error}")))?;
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut total = 0u64;
    loop {
        if *cancel.borrow() {
            return Err(UpdateFailure::cancelled("本地日志同步已取消"));
        }
        if Instant::now() >= deadline {
            return Err(UpdateFailure::failed("本地日志同步超过更新时间预算"));
        }
        let read = input
            .read(&mut buffer)
            .map_err(|error| UpdateFailure::failed(format!("读取本地日志失败：{error}")))?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > max_bytes {
            return Err(UpdateFailure::failed(format!(
                "本地日志超过剩余同步预算 {max_bytes}"
            )));
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| UpdateFailure::failed(format!("写入本地日志暂存文件失败：{error}")))?;
        counter.fetch_add(read as u64, Ordering::Relaxed);
    }
    output
        .flush()
        .map_err(|error| UpdateFailure::failed(format!("刷新本地日志暂存文件失败：{error}")))?;
    Ok(total)
}

pub(crate) fn validate_text_file(path: &Path) -> Result<(), UpdateFailure> {
    use std::io::Read;

    let lower = path.to_string_lossy().to_ascii_lowercase();
    if [".gz", ".zip", ".xz", ".bz2", ".zst"]
        .iter()
        .any(|extension| lower.ends_with(extension))
    {
        return Err(UpdateFailure::failed(format!(
            "暂不支持压缩日志，请先选择未压缩的当前日志：{}",
            path.display()
        )));
    }
    let file = std::fs::File::open(path)
        .map_err(|error| UpdateFailure::failed(format!("打开日志文件失败：{error}")))?;
    let mut buffer = [0u8; 8192];
    let read = file
        .take(buffer.len() as u64)
        .read(&mut buffer)
        .map_err(|error| UpdateFailure::failed(format!("读取日志文件头失败：{error}")))?;
    let bytes = &buffer[..read];
    let compressed = bytes.starts_with(&[0x1f, 0x8b])
        || bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00])
        || bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd])
        || bytes.starts_with(b"BZh");
    if compressed {
        return Err(UpdateFailure::failed("暂不支持压缩日志内容"));
    }
    if bytes.contains(&0) {
        return Err(UpdateFailure::failed(
            "检测到 NUL 字节，按二进制文件拒绝索引；请选择文本日志",
        ));
    }
    Ok(())
}
