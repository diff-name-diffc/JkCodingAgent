use super::*;

/// 以硬上限并发读取 stdout/stderr。任一管道超过上限时立即终止子进程，
/// 但保留已经读取的完整前缀供调用方渲染并显式标记 truncated。
pub(super) async fn run_bounded_search_command(
    mut command: Command,
    stdout_limit: usize,
) -> io::Result<BoundedCommandOutput> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;

    match tokio::time::timeout(
        GREP_TIMEOUT,
        collect_bounded_output(&mut child, stdout_limit),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "grep 搜索超时（60 秒）",
            ))
        }
    }
}

pub(super) async fn collect_bounded_output(
    child: &mut tokio::process::Child,
    stdout_limit: usize,
) -> io::Result<BoundedCommandOutput> {
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("grep stdout 管道未创建"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("grep stderr 管道未创建"))?;
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut status = None;
    let mut truncated = false;
    let mut stdout_chunk = [0u8; 16 * 1024];
    let mut stderr_chunk = [0u8; 4 * 1024];

    enum Event {
        Stdout(io::Result<usize>),
        Stderr(io::Result<usize>),
        Exited(io::Result<ExitStatus>),
    }

    while status.is_none() || !stdout_done || !stderr_done {
        let event = tokio::select! {
            read = stdout.read(&mut stdout_chunk), if !stdout_done => Event::Stdout(read),
            read = stderr.read(&mut stderr_chunk), if !stderr_done => Event::Stderr(read),
            exited = child.wait(), if status.is_none() => Event::Exited(exited),
        };

        let exceeded = match event {
            Event::Stdout(Ok(0)) => {
                stdout_done = true;
                false
            }
            Event::Stdout(Ok(read)) => {
                append_capped(&mut stdout_bytes, &stdout_chunk[..read], stdout_limit)
            }
            Event::Stdout(Err(error)) => return Err(error),
            Event::Stderr(Ok(0)) => {
                stderr_done = true;
                false
            }
            Event::Stderr(Ok(read)) => append_capped(
                &mut stderr_bytes,
                &stderr_chunk[..read],
                MAX_GREP_STDERR_BYTES,
            ),
            Event::Stderr(Err(error)) => return Err(error),
            Event::Exited(result) => {
                status = Some(result?);
                false
            }
        };

        if exceeded && !truncated {
            truncated = true;
            child.start_kill()?;
        }
    }

    Ok(BoundedCommandOutput {
        status: status.ok_or_else(|| io::Error::other("grep 子进程未返回退出状态"))?,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
        truncated,
    })
}

/// 返回本次追加是否超过上限。目标缓冲区永远不会增长到 limit 之外。
pub(super) fn append_capped(target: &mut Vec<u8>, chunk: &[u8], limit: usize) -> bool {
    let remaining = limit.saturating_sub(target.len());
    target.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    chunk.len() > remaining
}

pub(super) fn search_status_is_success(output: &BoundedCommandOutput) -> bool {
    output.truncated || matches!(output.status.code(), Some(0 | 1))
}

pub(super) fn mark_backend_truncated(
    rendered: &mut GrepRendered,
    backend: &str,
    stdout_limit: usize,
) {
    rendered.truncated = true;
    let note = format!(
        "...（{backend} 原始输出达到 {stdout_limit} bytes 上限，结果为部分数据；请缩小路径或模式）"
    );
    if rendered.display.is_empty() {
        rendered.display = note;
    } else {
        rendered.display.push('\n');
        rendered.display.push_str(&note);
    }
}
