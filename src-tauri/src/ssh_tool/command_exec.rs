use std::sync::Arc;
use std::time::{Duration, Instant};

use russh::client::Msg as ClientMsg;
use russh::{Channel, ChannelMsg};

use super::audit::{sanitize_error_text, sanitize_ssh_error};
use super::{SshConnection, SshExecResult};

// 交互阻塞检测分两档：
// - 有证据（输出末尾命中密码 / 确认 / 分页器等提示符）→ 最快 PROMPT_IDLE_SECS 秒中止；
// - 纯静默（sleep、慢查询、写文件的备份等）→ 阈值随 timeout_secs 放大（timeout / 4），
//   避免误杀安静的长命令，上限 MAX_SILENT_IDLE_SECS。
const PROMPT_IDLE_SECS: u64 = 8;
const MAX_SILENT_IDLE_SECS: u64 = 60;
/// 已拿到退出码后，等待对端补发 EOF/Close 的宽限秒数。
const EXIT_STATUS_GRACE_SECS: u64 = 3;
/// 交互提示符探测只扫描每路输出的末尾字符数（避免全缓冲拷贝）。
const PROMPT_SCAN_CHARS: usize = 512;
/// stdin 的执行上限，同时也是安全审查的送审上限（见 agent::ssh_review）：
/// 凡执行的内容必须完整送审，不允许「执行一大段、只审开头」的盲区。
pub const MAX_STDIN_CHARS: usize = 32_000;
const TIMEOUT_EXIT_CODE: i32 = 124;
const INTERACTIVE_EXIT_CODE: i32 = -1;
/// 命令正常结束但协议未带回退出码（极少见）；与交互阻塞的 -1 区分开。
const UNKNOWN_EXIT_CODE: i32 = -2;

/// 命令执行失败：message 为用户可读诊断；stale 表示连接已断，
/// 调用方应丢弃缓存连接并重连重试一次。
pub(super) struct CommandFailure {
    pub(super) message: String,
    pub(super) stale: bool,
}

impl CommandFailure {
    fn new(prefix: &str, error: russh::Error, connection: &SshConnection) -> Self {
        Self {
            message: sanitize_ssh_error(prefix, error),
            stale: connection.handle.is_closed(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_command_on_connection(
    connection: &Arc<SshConnection>,
    server_id: &str,
    session_id: &str,
    command: &str,
    stdin: Option<&str>,
    timeout_secs: u64,
    max_output_bytes: usize,
    started: Instant,
) -> Result<SshExecResult, CommandFailure> {
    let mut channel = connection
        .handle
        .channel_open_session()
        .await
        .map_err(|error| CommandFailure::new("创建 SSH channel 失败", error, connection))?;
    channel
        .exec(true, command.as_bytes())
        .await
        .map_err(|error| CommandFailure::new("执行远程命令失败", error, connection))?;

    // 写入 stdin（若有）并关闭输入端。写入失败（如命令很快退出、不再读输入）
    // 不判为命令失败，但要把原因带到 stderr，避免无声丢失。
    let mut stdin_write_note = None;
    if let Some(input) = stdin.filter(|input| !input.is_empty()) {
        if let Err(error) = channel.data(input.as_bytes()).await {
            stdin_write_note = Some(format!(
                "\n[写入 stdin 失败（命令可能未读取标准输入）：{}]",
                sanitize_error_text(&error.to_string())
            ));
        }
    }
    let _ = channel.eof().await;

    let (prompt_idle_secs, silent_idle_secs) = idle_thresholds(timeout_secs);
    let deadline = started + Duration::from_secs(timeout_secs);
    let (outcome, stdout_raw, stderr_raw, stdout_capped, stderr_capped, exit_status) =
        drain_channel(
            &mut channel,
            max_output_bytes,
            prompt_idle_secs,
            silent_idle_secs,
            deadline,
        )
        .await;

    // 通道已排空但未取得退出码、连接已关闭：命令大概率已在远端执行，重试会让
    // 非幂等命令（rm / git push / 数据库写入等）被重复执行。按「退出码未知」返回
    // 已收集的输出，不再触发 stale 重试——重试只保留给命令开始前的建链阶段失败。
    let disconnected_note = (matches!(outcome, DrainOutcome::Completed)
        && exit_status.is_none()
        && connection.handle.is_closed())
    .then(|| {
        format!("\n[SSH server {server_id} 的连接在命令执行期间断开，未取得退出码（按未知处理）]")
    });
    if !matches!(outcome, DrainOutcome::Completed) {
        // 超时 / 交互阻塞正是对端可能无响应的场景，close 要等对端确认，
        // 不加超时会无限期挂住命令任务（前端拿不到结果、连接无法归还池）。
        let _ = tokio::time::timeout(Duration::from_secs(5), channel.close()).await;
    }

    let stdout = finalize_output(&stdout_raw, stdout_capped);
    let (exit_code, mut stderr) = match outcome {
        DrainOutcome::Completed => (
            exit_status
                .map(|status| status as i32)
                .unwrap_or(UNKNOWN_EXIT_CODE),
            finalize_output(&stderr_raw, stderr_capped),
        ),
        DrainOutcome::TimedOut => {
            let mut text = finalize_output(&stderr_raw, stderr_capped);
            text.push_str(&format!(
                "\n[命令超过 {timeout_secs}s 仍未结束，已中止。若是长任务请提高 timeout_secs；若是交互阻塞请改用非交互形式。]"
            ));
            (TIMEOUT_EXIT_CODE, text)
        }
        DrainOutcome::InteractiveBlocked => {
            let mut text = finalize_output(&stderr_raw, stderr_capped);
            let hint =
                interactive_prompt_hint(&stdout_raw, &stderr_raw).unwrap_or("未匹配到明显提示符");
            text.push_str(&format!(
                "\n[工具检测到命令疑似在等待交互输入（连续 {silent_idle_secs}s 无输出且未退出；若命中密码/确认等提示符最快 {prompt_idle_secs}s 即中止。疑似：{hint}），已主动中止以免长时间挂起。最近输出结尾：{:?}。请改用非交互形式后重试：sudo→免密账号或 NOPASSWD；确认提示→加 -y/--yes；分页器→PAGER=cat、GIT_PAGER=cat；REPL→用 -e/-c 或通过 stdin 参数喂入。]",
                tail_snippet(&stdout_raw, &stderr_raw)
            ));
            (INTERACTIVE_EXIT_CODE, text)
        }
    };
    if let Some(note) = stdin_write_note {
        stderr.push_str(&note);
    }
    if let Some(note) = disconnected_note {
        stderr.push_str(&note);
    }
    connection.touch();

    Ok(SshExecResult {
        server_id: server_id.to_string(),
        session_id: session_id.to_string(),
        exit_code,
        stdout,
        stderr,
        duration_ms: started.elapsed().as_millis(),
        truncated: stdout_capped || stderr_capped,
        interactive_blocked: matches!(outcome, DrainOutcome::InteractiveBlocked),
    })
}

/// 静默阈值随命令超时放大：短命令保持灵敏（8s），长命令（如 300s 备份）放宽到
/// 最多 60s，避免误杀安静的长任务；命中提示符证据时始终用快速阈值。
fn idle_thresholds(timeout_secs: u64) -> (u64, u64) {
    let silent_idle_secs = (timeout_secs / 4).clamp(PROMPT_IDLE_SECS, MAX_SILENT_IDLE_SECS);
    let prompt_idle_secs = PROMPT_IDLE_SECS.min(silent_idle_secs);
    (prompt_idle_secs, silent_idle_secs)
}

/// 读取 channel 的 stdout 与 stderr 直至结束 / 超时 / 疑似交互阻塞。
#[allow(clippy::too_many_arguments)]
async fn drain_channel(
    channel: &mut Channel<ClientMsg>,
    max_output_bytes: usize,
    prompt_idle_secs: u64,
    silent_idle_secs: u64,
    deadline: Instant,
) -> (DrainOutcome, Vec<u8>, Vec<u8>, bool, bool, Option<u32>) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdout_capped = false;
    let mut stderr_capped = false;
    let mut exit_status = None;
    let mut eof_seen = false;
    let mut last_data_at = Instant::now();

    let outcome = loop {
        if eof_seen && exit_status.is_some() {
            break DrainOutcome::Completed;
        }
        let now = Instant::now();
        if now >= deadline {
            break DrainOutcome::TimedOut;
        }
        let idle_for = now.duration_since(last_data_at);
        if exit_status.is_some() && idle_for >= Duration::from_secs(EXIT_STATUS_GRACE_SECS) {
            break DrainOutcome::Completed;
        }
        let prompt_hit = interactive_prompt_hint(&stdout, &stderr).is_some();
        let idle_budget_secs = if prompt_hit {
            prompt_idle_secs
        } else {
            silent_idle_secs
        };
        if idle_for >= Duration::from_secs(idle_budget_secs) {
            break DrainOutcome::InteractiveBlocked;
        }
        let wait_for = Duration::from_secs(idle_budget_secs)
            .saturating_sub(idle_for)
            .min(deadline.saturating_duration_since(now));
        match tokio::time::timeout(wait_for, channel.wait()).await {
            Ok(Some(ChannelMsg::Data { data })) => {
                append_limited(&mut stdout, &data, max_output_bytes, &mut stdout_capped);
                last_data_at = Instant::now();
            }
            Ok(Some(ChannelMsg::ExtendedData { data, ext })) => {
                if ext == 1 {
                    append_limited(&mut stderr, &data, max_output_bytes, &mut stderr_capped);
                }
                last_data_at = Instant::now();
            }
            Ok(Some(ChannelMsg::ExitStatus {
                exit_status: status,
            })) => exit_status = Some(status),
            Ok(Some(ChannelMsg::Eof)) => eof_seen = true,
            Ok(Some(ChannelMsg::Close)) | Ok(None) => break DrainOutcome::Completed,
            Ok(Some(_)) | Err(_) => {}
        }
    };

    (
        outcome,
        stdout,
        stderr,
        stdout_capped,
        stderr_capped,
        exit_status,
    )
}

#[derive(Clone, Copy)]
enum DrainOutcome {
    Completed,
    InteractiveBlocked,
    TimedOut,
}

fn append_limited(out: &mut Vec<u8>, data: &[u8], max_bytes: usize, capped: &mut bool) {
    let remaining = max_bytes.saturating_sub(out.len());
    if remaining == 0 {
        if !data.is_empty() {
            *capped = true;
        }
        return;
    }
    let take = data.len().min(remaining);
    out.extend_from_slice(&data[..take]);
    if take < data.len() {
        *capped = true;
    }
}

fn finalize_output(buf: &[u8], capped: bool) -> String {
    let mut text = String::from_utf8_lossy(buf).into_owned();
    if capped {
        text.push_str("\n[输出已截断]");
    }
    text
}

fn interactive_prompt_hint(stdout: &[u8], stderr: &[u8]) -> Option<&'static str> {
    let mut combined = tail_chars(stdout, PROMPT_SCAN_CHARS);
    combined.push_str(&tail_chars(stderr, PROMPT_SCAN_CHARS));
    let lower = combined.to_ascii_lowercase();
    let patterns: &[(&str, &str)] = &[
        ("passphrase", "口令提示"),
        ("password", "密码提示"),
        ("密码", "密码提示"),
        ("[y/n]", "y/n 确认"),
        ("(y/n)", "y/n 确认"),
        ("[yes/no]", "yes/no 确认"),
        ("(yes/no)", "yes/no 确认"),
        ("y/n", "y/n 确认"),
        ("are you sure", "确认提示"),
        ("do you want to continue", "确认提示"),
        ("continue?", "确认提示"),
        ("是否", "确认提示"),
        ("确认", "确认提示"),
        ("press any key", "按键继续"),
        ("press enter", "回车继续"),
        ("verification code", "验证码"),
        ("one-time", "验证码"),
        ("otp", "验证码"),
    ];
    patterns
        .iter()
        .find_map(|(needle, label)| lower.contains(needle).then_some(*label))
}

fn tail_chars(buf: &[u8], max_chars: usize) -> String {
    let window = &buf[buf.len().saturating_sub(max_chars.saturating_mul(4))..];
    let text = String::from_utf8_lossy(window);
    let skip = text.chars().count().saturating_sub(max_chars);
    text.chars().skip(skip).collect()
}

fn tail_snippet(stdout: &[u8], stderr: &[u8]) -> String {
    let mut combined = String::from_utf8_lossy(stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(stderr));
    let from = combined
        .char_indices()
        .rev()
        .nth(200)
        .map(|(index, _)| index)
        .unwrap_or(0);
    combined[from..].trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_thresholds_scale_with_timeout() {
        assert_eq!(idle_thresholds(30), (8, 8));
        assert_eq!(idle_thresholds(120), (8, 30));
        assert_eq!(idle_thresholds(300), (8, 60));
        assert_eq!(idle_thresholds(600), (8, 60));
    }
}
