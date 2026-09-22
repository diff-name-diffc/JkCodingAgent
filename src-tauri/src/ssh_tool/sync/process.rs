use super::super::SshServerConfig;
use super::{transport::Transport, SyncDirectory, SyncProgress, SyncResult};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::sync::watch;

pub(super) fn arguments(request: &SyncDirectory, source: &Path, shell: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "--recursive",
        "--links",
        "--safe-links",
        "--times",
        "--perms",
        "--protect-args",
        "--info=progress2",
        "--stats",
        "--itemize-changes",
        // 应用配置可能携带凭据；接收端同样保留这些排除项，不使用 --delete-excluded。
        "--exclude=.jkcodingagent/",
        "--exclude=.git/",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if request.delete {
        args.push("--delete-delay".into());
    }
    if request.dry_run {
        args.push("--dry-run".into());
    }
    args.extend([
        "--rsh".into(),
        shell.into(),
        "--".into(),
        format!("{}/", source.to_string_lossy().trim_end_matches('/')),
        format!("aha-sync:{}/", request.destination.trim_end_matches('/')),
    ]);
    args
}

struct ProcessGuard(Child);
impl ProcessGuard {
    fn kill(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::killpg(self.0.id() as libc::pid_t, libc::SIGKILL);
        }
        let _ = self.0.kill();
    }
}
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        self.kill();
        let _ = self.0.wait();
    }
}

#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    pending: Vec<u8>,
    truncated: bool,
    progress: Option<SyncProgress>,
}
impl Capture {
    fn push(&mut self, bytes: &[u8], limit: usize) {
        let count = bytes.len().min(limit.saturating_sub(self.bytes.len()));
        self.bytes.extend_from_slice(&bytes[..count]);
        self.truncated |= count < bytes.len();
        for byte in bytes {
            if *byte == b'\r' || *byte == b'\n' {
                if let Some(progress) = parse_progress(&String::from_utf8_lossy(&self.pending)) {
                    self.progress = Some(progress);
                }
                self.pending.clear();
            } else if self.pending.len() < 4096 {
                self.pending.push(*byte);
            }
        }
    }
}

pub(super) fn parse_progress(line: &str) -> Option<SyncProgress> {
    let mut fields = line.split_whitespace();
    let transferred_bytes = fields.next()?.replace(',', "").parse().ok()?;
    let percent = fields.next()?.strip_suffix('%')?.parse::<u8>().ok()?;
    if percent > 100 {
        return None;
    }
    Some(SyncProgress {
        transferred_bytes,
        percent,
    })
}

#[cfg(unix)]
fn nonblocking(fd: &impl std::os::fd::AsRawFd) -> Result<(), String> {
    let fd = fd.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(format!(
            "配置 rsync 输出管道失败：{}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn drain(reader: &mut impl Read, capture: &mut Capture, limit: usize) -> Result<(), String> {
    let mut buffer = [0u8; 32768];
    // 每轮有界，连续大量输出也不能饿死取消与超时检查。
    for _ in 0..8 {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => capture.push(&buffer[..n], limit),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("读取 rsync 输出失败：{e}")),
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    server: &SshServerConfig,
    request: SyncDirectory,
    source: PathBuf,
    transport: Transport,
    cancel: Option<watch::Receiver<bool>>,
    abort: Arc<AtomicBool>,
    progress: Arc<dyn Fn(SyncProgress) + Send + Sync>,
) -> Result<SyncResult, String> {
    #[cfg(not(unix))]
    {
        let _ = (server, request, source, transport, cancel, abort, progress);
        Err("rsync 同步暂不支持此平台".into())
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let cancelled =
            || abort.load(Ordering::Acquire) || cancel.as_ref().is_some_and(|rx| *rx.borrow());
        if cancelled() {
            return Err("目录同步已取消，未执行".into());
        }
        let mut command = Command::new(&transport.binaries.rsync);
        command
            .args(arguments(&request, &source, &transport.shell))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        transport.configure(&mut command);
        let started = Instant::now();
        let mut child = ProcessGuard(
            command
                .spawn()
                .map_err(|e| format!("启动 rsync 失败：{e}"))?,
        );
        let mut stdout = child.0.stdout.take().ok_or("rsync stdout 管道缺失")?;
        let mut stderr = child.0.stderr.take().ok_or("rsync stderr 管道缺失")?;
        nonblocking(&stdout)?;
        nonblocking(&stderr)?;
        let (mut out, mut err) = (Capture::default(), Capture::default());
        let limit = server.max_output_bytes.clamp(1024, 1024 * 1024);
        let timeout = Duration::from_secs(server.default_timeout_secs.clamp(1, 300));
        let mut last_event = Instant::now();
        let mut was_cancelled = false;
        let mut timed_out = false;
        let status = loop {
            drain(&mut stdout, &mut out, limit)?;
            drain(&mut stderr, &mut err, limit)?;
            if last_event.elapsed() >= Duration::from_millis(250) {
                if let Some(p) = &out.progress {
                    progress(p.clone());
                }
                last_event = Instant::now();
            }
            if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())? {
                break status;
            }
            was_cancelled = cancelled();
            timed_out = started.elapsed() >= timeout;
            if was_cancelled || timed_out {
                child.kill();
                break child
                    .0
                    .wait()
                    .map_err(|e| format!("回收 rsync 进程失败：{e}"))?;
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        drain(&mut stdout, &mut out, limit)?;
        drain(&mut stderr, &mut err, limit)?;
        if let Some(p) = &out.progress {
            progress(p.clone());
        }
        let mut stderr = String::from_utf8_lossy(&err.bytes).into_owned();
        let mut stdout = String::from_utf8_lossy(&out.bytes).into_owned();
        for secret in [&server.password, &server.private_key_passphrase] {
            if !secret.is_empty() {
                stderr = stderr.replace(secret, "[redacted]");
                stdout = stdout.replace(secret, "[redacted]");
            }
        }
        Ok(SyncResult {
            ssh_profile: request.ssh_profile,
            source: request.source,
            destination: request.destination,
            delete: request.delete,
            dry_run: request.dry_run,
            exit_code: status.code(),
            cancelled: was_cancelled,
            timed_out,
            duration_ms: started.elapsed().as_millis() as u64,
            stdout,
            stderr,
            truncated: out.truncated || err.truncated,
            progress: out.progress,
            audit_error: None,
        })
    }
}
