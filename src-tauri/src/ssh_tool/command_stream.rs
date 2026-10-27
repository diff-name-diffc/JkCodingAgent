use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use russh::client::Msg as ClientMsg;
use russh::{Channel, ChannelMsg};
use tokio::sync::{mpsc, watch};

use super::command_exec::build_sudo_wrapped_command;
use super::{CommandFailure, CommandFailureKind, SshConnection, SshExecResult, SshServerConfig};
use crate::ssh_tool::SshStreamResult;

const STDERR_LIMIT: usize = 64 * 1024;
const UNKNOWN_EXIT_CODE: i32 = -2;
const TIMEOUT_EXIT_CODE: i32 = 124;
const OUTPUT_FAILURE_EXIT_CODE: i32 = -4;

#[derive(Clone, Copy, Eq, PartialEq)]
enum StreamOutcome {
    Completed,
    Cancelled,
    TimedOut,
    OutputFailed,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_command_to_file_on_connection(
    connection: &Arc<SshConnection>,
    server: &SshServerConfig,
    server_id: &str,
    session_id: &str,
    command: &str,
    destination: PathBuf,
    max_bytes: u64,
    timeout_secs: u64,
    started: Instant,
    cancel: Option<watch::Receiver<bool>>,
    elevate: bool,
    written_counter: Arc<AtomicU64>,
) -> Result<SshStreamResult, CommandFailure> {
    if cancel
        .as_ref()
        .is_some_and(|rx| *rx.borrow() || rx.has_changed().is_err())
    {
        return Err(CommandFailure {
            message: "日志同步已取消，未发送远程命令".into(),
            stale: false,
            kind: CommandFailureKind::CancelledNotSent,
        });
    }
    let wire_command = if elevate {
        build_sudo_wrapped_command(command)
    } else {
        command.to_string()
    };
    let mut channel = connection
        .handle
        .channel_open_session()
        .await
        .map_err(|error| {
            CommandFailure::other(format!("创建日志同步 SSH channel 失败：{error}"))
        })?;
    channel
        .exec(true, wire_command.as_bytes())
        .await
        .map_err(|error| CommandFailure::other(format!("远程日志读取请求失败：{error}")))?;
    if elevate {
        let input = format!("{}\n", server.password);
        if let Err(error) = channel.data(input.as_bytes()).await {
            let _ = channel.close().await;
            return Err(CommandFailure::other(format!(
                "写入 sudo 口令失败：{error}"
            )));
        }
    }
    let _ = channel.eof().await;

    let (sender, receiver) = mpsc::channel::<Vec<u8>>(32);
    let writer_counter = Arc::clone(&written_counter);
    let writer = tokio::task::spawn_blocking(move || {
        write_output(receiver, destination, max_bytes, writer_counter)
    });
    let deadline = started + Duration::from_secs(timeout_secs);
    let (outcome, stderr, stderr_capped, exit_status) = drain_channel(
        &mut channel,
        sender,
        deadline,
        cancel,
        server.password.as_str(),
    )
    .await;
    if outcome != StreamOutcome::Completed {
        let _ = tokio::time::timeout(Duration::from_secs(5), channel.close()).await;
    }
    // 写盘线程的失败不提前 return：归并进 outcome 收口，保留 exit_code/stderr
    // 与 OUTPUT_FAILURE 语义（提前 return 会把这层信息全部丢成一条连接错误）。
    let writer_result = writer
        .await
        .map_err(|error| format!("日志文件写入任务失败：{error}"))
        .and_then(|result| result);
    let (bytes_written, mut write_failure) = match writer_result {
        Ok(bytes) => (bytes, None),
        Err(error) => (0, Some(error)),
    };

    let mut stderr = stderr;
    let mut external_state_unknown = false;
    let cancelled = outcome == StreamOutcome::Cancelled;
    let exit_code = match outcome {
        StreamOutcome::Completed if write_failure.is_some() => {
            stderr.push_str(&format!(
                "\n[本地日志写入失败（{}），远程 channel 已关闭。]",
                write_failure.take().expect("上方刚判过 Some")
            ));
            OUTPUT_FAILURE_EXIT_CODE
        }
        StreamOutcome::Completed => exit_status.map(|status| status as i32).unwrap_or_else(|| {
            external_state_unknown = true;
            stderr.push_str("\n[连接在日志读取结束前关闭，未取得退出码（按未知外部状态处理）。]");
            UNKNOWN_EXIT_CODE
        }),
        StreamOutcome::Cancelled => {
            external_state_unknown = true;
            stderr.push_str("\n[日志同步已取消，当前暂存文件不会发布。]");
            -3
        }
        StreamOutcome::TimedOut => {
            external_state_unknown = true;
            stderr.push_str(&format!(
                "\n[远程日志读取超过 {timeout_secs}s，已中止，当前暂存文件不会发布。]"
            ));
            TIMEOUT_EXIT_CODE
        }
        StreamOutcome::OutputFailed => {
            if let Some(error) = write_failure.take() {
                stderr.push_str(&format!("\n[{error}]"));
            }
            stderr.push_str("\n[本地日志写入失败，远程 channel 已关闭。]");
            OUTPUT_FAILURE_EXIT_CODE
        }
    };
    if stderr_capped {
        stderr.push_str("\n[stderr 已截断]");
    }
    connection.touch();
    Ok(SshStreamResult {
        execution: SshExecResult {
            cancelled,
            external_state_unknown,
            server_id: server_id.to_string(),
            session_id: session_id.to_string(),
            exit_code,
            stdout: String::new(),
            stderr,
            duration_ms: started.elapsed().as_millis(),
            truncated: stderr_capped,
            interactive_blocked: false,
            elevated: elevate,
        },
        bytes_written,
    })
}

fn write_output(
    mut receiver: mpsc::Receiver<Vec<u8>>,
    destination: PathBuf,
    max_bytes: u64,
    written_counter: Arc<AtomicU64>,
) -> Result<u64, String> {
    use std::io::Write;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&destination)
        .map_err(|error| format!("创建日志暂存文件失败 {}：{error}", destination.display()))?;
    let mut written = 0u64;
    while let Some(data) = receiver.blocking_recv() {
        let next = written
            .checked_add(data.len() as u64)
            .ok_or_else(|| "日志字节计数溢出".to_string())?;
        if next > max_bytes {
            return Err(format!("日志超过本次同步字节上限 {max_bytes}，已停止写入"));
        }
        file.write_all(&data)
            .map_err(|error| format!("写入日志暂存文件失败：{error}"))?;
        written = next;
        written_counter.fetch_add(data.len() as u64, Ordering::Relaxed);
    }
    file.flush()
        .map_err(|error| format!("刷新日志暂存文件失败：{error}"))?;
    Ok(written)
}

/// 退出码（或 EOF）单独到达后，等待对端补发另一侧的宽限时长：与
/// `command_exec::EXIT_STATUS_GRACE_SECS` 同口径，避免对端只发一边时
/// 一直阻塞到 deadline。
const CHANNEL_GRACE_SECS: u64 = 3;

async fn drain_channel(
    channel: &mut Channel<ClientMsg>,
    sender: mpsc::Sender<Vec<u8>>,
    deadline: Instant,
    cancel: Option<watch::Receiver<bool>>,
    sudo_password: &str,
) -> (StreamOutcome, String, bool, Option<u32>) {
    let mut stderr_bytes = Vec::new();
    let mut stderr_capped = false;
    let mut exit_status = None;
    let mut eof_seen = false;
    let mut outcome = StreamOutcome::Completed;
    // ExitStatus / Eof 任一单独到达的时间点：宽限满即按完成收口。
    let mut half_close_at: Option<Instant> = None;
    loop {
        if eof_seen && exit_status.is_some() {
            break;
        }
        if let Some(seen) = half_close_at {
            if seen.elapsed() >= Duration::from_secs(CHANNEL_GRACE_SECS) {
                break;
            }
        }
        let now = Instant::now();
        if now >= deadline {
            outcome = StreamOutcome::TimedOut;
            break;
        }
        let wait_for = deadline.saturating_duration_since(now);
        let next = tokio::select! {
            message = tokio::time::timeout(wait_for, channel.wait()) => message,
            _ = crate::shared::cancel::wait_for_cancel(cancel.clone()) => {
                outcome = StreamOutcome::Cancelled;
                break;
            }
        };
        match next {
            Ok(Some(ChannelMsg::Data { data })) => {
                // send 也必须受超时与取消约束：本地写盘卡住（网络盘/磁盘抖动）
                // 撑满 mpsc 背压后，这里不能无限阻塞而放弃预算与取消。
                tokio::select! {
                    sent = sender.send(data.to_vec()) => {
                        if sent.is_err() {
                            outcome = StreamOutcome::OutputFailed;
                            break;
                        }
                    }
                    _ = crate::shared::cancel::wait_for_cancel(cancel.clone()) => {
                        outcome = StreamOutcome::Cancelled;
                        break;
                    }
                    _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                        outcome = StreamOutcome::TimedOut;
                        break;
                    }
                }
            }
            Ok(Some(ChannelMsg::ExtendedData { data, ext })) => {
                if ext == 1 {
                    append_limited(&mut stderr_bytes, &data, &mut stderr_capped);
                }
            }
            Ok(Some(ChannelMsg::ExitStatus {
                exit_status: status,
            })) => {
                exit_status = Some(status);
                if half_close_at.is_none() {
                    half_close_at = Some(Instant::now());
                }
            }
            Ok(Some(ChannelMsg::Eof)) => {
                eof_seen = true;
                if half_close_at.is_none() {
                    half_close_at = Some(Instant::now());
                }
            }
            Ok(Some(ChannelMsg::Close)) | Ok(None) => break,
            Ok(Some(_)) => {}
            Err(_) => {
                outcome = StreamOutcome::TimedOut;
                break;
            }
        }
    }
    drop(sender);
    let mut stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
    if !sudo_password.is_empty() {
        stderr = stderr.replace(sudo_password, "[已掩码]");
    }
    (outcome, stderr, stderr_capped, exit_status)
}

fn append_limited(target: &mut Vec<u8>, data: &[u8], capped: &mut bool) {
    let remaining = STDERR_LIMIT.saturating_sub(target.len());
    let take = data.len().min(remaining);
    target.extend_from_slice(&data[..take]);
    *capped |= take < data.len();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_writer_streams_and_enforces_budget() {
        let dir = crate::test_util::TempDirGuard::new("ssh-log-stream-writer");
        let path = dir.path().join("output.log");
        let counter = Arc::new(AtomicU64::new(0));
        let (sender, receiver) = mpsc::channel(2);
        let writer_counter = Arc::clone(&counter);
        let writer_path = path.clone();
        let writer =
            std::thread::spawn(move || write_output(receiver, writer_path, 6, writer_counter));
        sender.blocking_send(b"abc".to_vec()).unwrap();
        sender.blocking_send(b"def".to_vec()).unwrap();
        drop(sender);
        assert_eq!(writer.join().unwrap().unwrap(), 6);
        assert_eq!(std::fs::read(path).unwrap(), b"abcdef");
        assert_eq!(counter.load(Ordering::Relaxed), 6);
    }

    #[test]
    fn output_writer_rejects_bytes_above_budget() {
        let dir = crate::test_util::TempDirGuard::new("ssh-log-stream-budget");
        let path = dir.path().join("output.log");
        let counter = Arc::new(AtomicU64::new(0));
        let (sender, receiver) = mpsc::channel(1);
        let writer_counter = Arc::clone(&counter);
        let writer_path = path.clone();
        let writer =
            std::thread::spawn(move || write_output(receiver, writer_path, 2, writer_counter));
        sender.blocking_send(b"abc".to_vec()).unwrap();
        drop(sender);
        let error = writer.join().unwrap().unwrap_err();
        assert!(error.contains("上限"));
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }
}
