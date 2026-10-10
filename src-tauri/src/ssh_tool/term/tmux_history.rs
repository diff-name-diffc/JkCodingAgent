//! 独立只读通道读取 tmux 当前活动 pane 的历史，不改变交互客户端的 copy-mode。

use std::time::Duration;

use russh::ChannelMsg;
use tokio::time::{timeout_at, Instant};

use super::screen::{captured_history, HistoryPage, NEW_LINES_MAX_ROWS};
use crate::ssh_tool::SshConnection;

const READ_TIMEOUT: Duration = Duration::from_secs(5);
const OUTPUT_LIMIT: usize = 1_048_576;
const ERROR_LIMIT: usize = 2_048;

pub(super) async fn read(
    connection: &SshConnection,
    name: &str,
    offset: usize,
    limit: usize,
    ansi: bool,
    cancel: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<HistoryPage, String> {
    if cancel.as_ref().is_some_and(|rx| *rx.borrow()) {
        return Err("读取 tmux 历史已取消，本次未发起远端查询".into());
    }
    super::validate_tmux_session_name(name)?;
    let limit = limit.clamp(1, NEW_LINES_MAX_ROWS);
    let command = capture_command(name, offset, limit, ansi);
    let deadline = Instant::now() + READ_TIMEOUT;
    let mut channel = timeout_at(deadline, connection.handle.channel_open_session())
        .await
        .map_err(|_| "读取 tmux 历史：创建 channel 超时".to_string())?
        .map_err(|error| format!("读取 tmux 历史：创建 channel 失败：{error}"))?;
    let request = async {
        channel
            .exec(true, command.as_bytes())
            .await
            .map_err(|error| format!("读取 tmux 历史：exec 失败：{error}"))?;
        let mut output = Vec::new();
        let mut errors = Vec::new();
        let mut status = None;
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Data { data }) => {
                    if output.len() + data.len() > OUTPUT_LIMIT {
                        return Err(
                            "tmux 历史超过 1 MiB 传输上限；请减小 history_lines 后重试".into()
                        );
                    }
                    output.extend_from_slice(&data);
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    let remaining = ERROR_LIMIT.saturating_sub(errors.len());
                    errors.extend_from_slice(&data[..data.len().min(remaining)]);
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => status = Some(exit_status),
                // EOF 只表示输出结束；服务端仍可随后发送退出状态。
                Some(ChannelMsg::Eof | ChannelMsg::Success) => {}
                Some(ChannelMsg::Failure) => return Err("SSH 服务端拒绝读取 tmux 历史".into()),
                Some(ChannelMsg::Close) | None => {
                    return match status {
                        Some(0) => parse_capture(&output, offset, limit, ansi),
                        Some(code) => Err(format!(
                            "读取 tmux 历史失败（exit_code={code}）：{}",
                            String::from_utf8_lossy(&errors).trim()
                        )),
                        None => Err("读取 tmux 历史：channel 已关闭，但未取得退出码".into()),
                    };
                }
                Some(_) => {}
            }
        }
    };
    let result = tokio::select! {
        biased;
        _ = crate::shared::cancel::wait_for_cancel(cancel) => Err("读取 tmux 历史已取消".into()),
        result = timeout_at(deadline, request) => result.unwrap_or_else(|_| Err("读取 tmux 历史超时".to_string())),
    };
    let _ = tokio::time::timeout(Duration::from_millis(250), channel.close()).await;
    result
}

fn capture_command(name: &str, offset: usize, limit: usize, ansi: bool) -> String {
    // tmux 坐标是有符号整数；超大 offset 仍查询元数据，由 parse_capture 判为空。
    let start = offset.saturating_add(limit).min(i32::MAX as usize);
    let end = offset.saturating_add(1).min(i32::MAX as usize);
    let escape = if ansi { " -e" } else { "" };
    // target-pane 需要冒号才能把 =name 解释为精确会话名。两个同步 tmux 命令
    // 同处一条队列，避免 SSH 往返期间切换活动 pane 导致元数据与内容错配。
    format!(
        "tmux display-message -p -t ={name}: '#{{history_size}}' \\; capture-pane -p{escape} -t ={name}: -S -{start} -E -{end}"
    )
}

fn parse_capture(
    output: &[u8],
    offset: usize,
    limit: usize,
    ansi: bool,
) -> Result<HistoryPage, String> {
    let text = std::str::from_utf8(output)
        .map_err(|_| "读取 tmux 历史：输出不是有效 UTF-8".to_string())?;
    let (header, body) = text
        .split_once('\n')
        .ok_or_else(|| "读取 tmux 历史：缺少 history_size 元数据".to_string())?;
    let available_lines = header
        .trim_end_matches('\r')
        .parse::<usize>()
        .map_err(|_| "读取 tmux 历史：history_size 元数据无效".to_string())?;
    let expected = available_lines.saturating_sub(offset).min(limit);
    // tmux 会将越界负坐标夹到首行；无历史或 offset 越界时不能把视口/首行当历史。
    if expected == 0 {
        return Ok(captured_history(&[], available_lines, offset, ansi));
    }
    let lines: Vec<_> = body.split_terminator('\n').collect();
    if lines.len() != expected {
        return Err(format!(
            "读取 tmux 历史：预期 {expected} 行，实际 {} 行；请重试读取",
            lines.len()
        ));
    }
    Ok(captured_history(&lines, available_lines, offset, ansi))
}

#[cfg(test)]
#[path = "tmux_history_tests.rs"]
mod tests;
