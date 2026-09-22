//! ACP 执行器子进程的自有 spawn 与生命周期管理。
//!
//! 不经过 crate 的 AcpAgent transport（其 spawn 全量继承父进程 env、
//! stdout 行读取无上限），改为：
//! - `env_clear` + launcher 的白名单 env（防父进程环境密钥泄漏）；
//! - Unix 上子进程自成进程组组长，守卫 drop 时 SIGKILL 整组（覆盖
//!   `node` 派生的孙进程；语义与 crate ChildGuard 一致）；
//! - stdout 经 `BoundedLineReader` 包装：单行超 1 MiB 即 fail-closed
//!   报错中止连接（防失控/被篡改执行器用无换行字符流撑爆宿主内存）；
//! - stderr 独立引流并保留尾部，供失败诊断。

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use parking_lot::Mutex;
use tokio::io::AsyncReadExt;
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};

use super::launcher::LaunchPlan;

/// ACP 单行 JSONL 的字节上限：正常响应在 KB 量级，1 MiB 已非常宽裕。
/// 超限即报错触发连接中止（fail-closed）。
const MAX_ACP_LINE_BYTES: usize = 1024 * 1024;

/// stderr 诊断保留的字节上限（只留尾部）。
const STDERR_TAIL_BYTES: usize = 8 * 1024;

/// Windows：隐藏控制台窗口（CREATE_NO_WINDOW）。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// spawn 产物：管道句柄 + 进程组守卫（守卫随节点会话存活，drop 杀整组）。
pub(super) struct SpawnedAgent {
    pub guard: ChildGuard,
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub stderr: ChildStderr,
}

/// 以 LaunchPlan spawn 执行器子进程：env_clear + 白名单 env + 进程组隔离。
pub(super) fn spawn(plan: &LaunchPlan) -> Result<SpawnedAgent, String> {
    let mut command = std::process::Command::new(&plan.program);
    command.args(&plan.args);
    command.env_clear();
    command.envs(
        plan.envs
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // 子进程自成进程组组长：守卫 kill 整组时覆盖孙进程。
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = tokio::process::Command::from(command)
        .spawn()
        .map_err(|error| format!("启动 ACP 执行器失败（{}）：{error}", plan.program.display()))?;
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    match (stdin, stdout, stderr) {
        (Some(stdin), Some(stdout), Some(stderr)) => Ok(SpawnedAgent {
            guard: ChildGuard(child),
            stdin,
            stdout,
            stderr,
        }),
        _ => Err("启动 ACP 执行器失败：未能接管 stdio 管道".into()),
    }
}

/// 进程守卫：drop 时终止子进程及其进程组（Unix）。
/// 与 crate ChildGuard 同语义：先杀组（覆盖 wrapper 派生的孙进程），
/// 再补杀直接子进程兜底。
pub(super) struct ChildGuard(tokio::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0.id() {
            // 组 ID 即子进程 PID（process_group(0)）。组不存在时 ESRCH，
            // 属正常收尾。组消亡后 PGID 复用窗口极小（spawn 到此处的时间
            // 短），与 crate 的做法对齐，接受该残余风险。
            unsafe {
                libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
            }
        }
        let _ = self.0.start_kill();
    }
}

/// stderr 引流：持续消费防管道阻塞，保留尾部供失败诊断。
/// 返回（共享尾部缓冲, 引流任务）；任务在 stderr EOF（子进程退出）后结束。
pub(super) fn drain_stderr(mut stderr: ChildStderr) -> (StderrTail, tokio::task::JoinHandle<()>) {
    let tail = StderrTail::default();
    let writer = tail.clone();
    let task = tokio::spawn(async move {
        let mut buffer = [0u8; 8 * 1024];
        loop {
            match stderr.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(n) => writer.push(&buffer[..n]),
            }
        }
    });
    (tail, task)
}

/// stderr 尾部缓冲（环形语义：只保留最后 STDERR_TAIL_BYTES）。
#[derive(Clone, Default)]
pub(super) struct StderrTail(Arc<Mutex<Vec<u8>>>);

impl StderrTail {
    fn push(&self, bytes: &[u8]) {
        let mut buffer = self.0.lock();
        buffer.extend_from_slice(bytes);
        if buffer.len() > STDERR_TAIL_BYTES {
            let overflow = buffer.len() - STDERR_TAIL_BYTES;
            buffer.drain(..overflow);
        }
    }

    pub(super) fn take_string(&self) -> String {
        let bytes = std::mem::take(&mut *self.0.lock());
        String::from_utf8_lossy(&bytes).trim().to_string()
    }
}

/// futures AsyncRead 包装：对单行字节数做上限，超限 fail-closed 报错。
/// 跨 read 边界累计「距上一个换行的字节数」，遇 `\n` 归零。
pub(super) struct BoundedLineReader<R> {
    inner: R,
    since_newline: usize,
    violated: bool,
}

impl<R> BoundedLineReader<R> {
    pub(super) fn new(inner: R) -> Self {
        Self {
            inner,
            since_newline: 0,
            violated: false,
        }
    }
}

impl<R: futures::AsyncRead + Unpin> futures::AsyncRead for BoundedLineReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if self.violated {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ACP 执行器输出了超限单行（>1MiB），连接已中止",
            )));
        }
        let read = match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(n)) => n,
            other => return other,
        };
        for &byte in &buf[..read] {
            if byte == b'\n' {
                self.since_newline = 0;
            } else {
                self.since_newline += 1;
                if self.since_newline > MAX_ACP_LINE_BYTES {
                    self.violated = true;
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "ACP 执行器输出了超限单行（>1MiB），连接已中止",
                    )));
                }
            }
        }
        Poll::Ready(Ok(read))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::AsyncReadExt as _;

    #[tokio::test]
    async fn bounded_reader_passes_normal_lines() {
        let data = b"{\"jsonrpc\":\"2.0\"}\nsecond line\n".to_vec();
        let mut reader = BoundedLineReader::new(futures::io::Cursor::new(data.clone()));
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, data);
    }

    #[tokio::test]
    async fn bounded_reader_fails_closed_on_overlong_line() {
        let mut data = vec![b'x'; MAX_ACP_LINE_BYTES + 1];
        data.push(b'\n');
        let mut reader = BoundedLineReader::new(futures::io::Cursor::new(data));
        let mut out = Vec::new();
        let result = reader.read_to_end(&mut out).await;
        assert!(result.is_err(), "超限单行必须报错中止");
    }

    #[tokio::test]
    async fn bounded_reader_counts_across_read_boundaries() {
        // 无换行内容分两次喂入（模拟跨 poll_read 累计），合计超限即报错。
        let chunk = vec![b'y'; MAX_ACP_LINE_BYTES / 2 + 1];
        let mut data = chunk.clone();
        data.extend_from_slice(&chunk);
        let mut reader = BoundedLineReader::new(futures::io::Cursor::new(data));
        let mut out = Vec::new();
        assert!(reader.read_to_end(&mut out).await.is_err());
    }

    #[tokio::test]
    async fn bounded_reader_newline_resets_counter() {
        // 多行各不超限时放行，即使总量远超上限。
        let line = vec![b'z'; 1024];
        let mut data = Vec::new();
        for _ in 0..2048 {
            data.extend_from_slice(&line);
            data.push(b'\n');
        }
        let expected = data.len();
        let mut reader = BoundedLineReader::new(futures::io::Cursor::new(data));
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        assert_eq!(out.len(), expected);
    }

    #[test]
    fn stderr_tail_keeps_last_bytes() {
        let tail = StderrTail::default();
        tail.push(b"first ");
        tail.push(&vec![b'x'; STDERR_TAIL_BYTES]);
        tail.push(b"last");
        let text = tail.take_string();
        assert!(text.ends_with("last"));
        assert!(!text.contains("first"));
        assert!(text.len() <= STDERR_TAIL_BYTES);
    }
}
