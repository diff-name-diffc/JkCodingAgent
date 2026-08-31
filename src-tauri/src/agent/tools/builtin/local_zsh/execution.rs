use super::*;

pub(super) async fn capture_command_output(
    child: &mut tokio::process::Child,
    timeout_secs: u64,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> std::io::Result<CapturedCommandOutput> {
    let stdout_reader = child.stdout.take();
    let stderr_reader = child.stderr.take();
    let stdout_task = tokio::spawn(async move { read_limited(stdout_reader).await });
    let stderr_task = tokio::spawn(async move { read_limited(stderr_reader).await });

    let wait_outcome = {
        let wait = child.wait();
        tokio::pin!(wait);
        let deadline = sleep(Duration::from_secs(timeout_secs));
        tokio::pin!(deadline);
        let cancellation = wait_for_cancellation(cancel_rx);
        tokio::pin!(cancellation);

        tokio::select! {
            biased;
            status = &mut wait => CommandWaitOutcome::Exited(status),
            _ = &mut cancellation => CommandWaitOutcome::Cancelled,
            _ = &mut deadline => CommandWaitOutcome::TimedOut,
        }
    };

    let (status, timed_out, cancelled) = match wait_outcome {
        CommandWaitOutcome::Exited(status) => (status?, false, false),
        CommandWaitOutcome::TimedOut => {
            // 超时：杀整个进程组（含派生的孙进程），确保管道写端全部关闭、
            // reader 能读到 EOF，不会永久阻塞。
            kill_process_group(child);
            (child.wait().await?, true, false)
        }
        CommandWaitOutcome::Cancelled => {
            kill_process_group(child);
            (child.wait().await?, false, true)
        }
    };

    let (stdout, stdout_read) = stdout_task.await.map_err(std::io::Error::other)?;
    let (stderr, stderr_read) = stderr_task.await.map_err(std::io::Error::other)?;

    Ok(CapturedCommandOutput {
        output: Output {
            status,
            stdout,
            stderr,
        },
        total_bytes_read: stdout_read + stderr_read,
        timed_out,
        cancelled,
    })
}

enum CommandWaitOutcome {
    Exited(std::io::Result<std::process::ExitStatus>),
    TimedOut,
    Cancelled,
}

async fn wait_for_cancellation(mut cancel_rx: Option<watch::Receiver<bool>>) {
    let Some(cancel_rx) = cancel_rx.as_mut() else {
        std::future::pending::<()>().await;
        return;
    };
    if *cancel_rx.borrow() {
        return;
    }
    loop {
        match cancel_rx.changed().await {
            Ok(()) if *cancel_rx.borrow() => return,
            Ok(()) => {}
            Err(_) => return,
        }
    }
}

async fn read_limited<R>(reader: Option<R>) -> (Vec<u8>, usize)
where
    R: AsyncRead + Unpin,
{
    let Some(mut reader) = reader else {
        return (Vec::new(), 0);
    };

    let mut retained = Vec::new();
    let mut total_read = 0;
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                total_read += n;
                let remaining = MAX_OUTPUT_BYTES.saturating_sub(retained.len());
                if remaining > 0 {
                    retained.extend_from_slice(&chunk[..n.min(remaining)]);
                }
            }
            Err(_) => break,
        }
    }

    (retained, total_read)
}

/// 终止子进程所在的整个进程组。spawn 时设置了 `process_group(0)`，
/// 子进程 pid 即进程组 id；组杀失败时兜底单杀直接子进程。
#[cfg(unix)]
fn kill_process_group(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

#[cfg(not(unix))]
fn kill_process_group(child: &mut tokio::process::Child) {
    let _ = child.start_kill();
}
