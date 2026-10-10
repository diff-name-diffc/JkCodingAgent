//! SSH 请求握手与 tmux 启动。EOF 只结束数据流，不能代替退出状态。

use std::time::Duration;

use russh::{Channel, ChannelMsg};
use tokio::time::{timeout, timeout_at, Instant};

use super::registry::{TermError, TermOpenParams, TmuxPreference};
use super::session::TERM_TYPE;
use crate::ssh_tool::SshConnection;

/// 启动有总上限，各次握手/模板命令另有完整预算，避免前序往返挤占 attach。
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
const OUTPUT_LIMIT: usize = 8_192;

pub(super) struct StartedTerminal {
    pub(super) channel: Channel<russh::client::Msg>,
    pub(super) tmux_session: Option<String>,
    pub(super) note: Option<String>,
    pub(super) initial_output: Vec<u8>,
    pub(super) command_started: bool,
}

pub(super) struct TemplateResult {
    pub(super) exit_status: u32,
    output: Vec<u8>,
}

impl TemplateResult {
    pub(super) fn ensure_success(&self, operation: &str) -> Result<(), String> {
        if self.exit_status == 0 {
            Ok(())
        } else {
            Err(format!(
                "{operation}失败（exit_code={}）：{}",
                self.exit_status,
                String::from_utf8_lossy(&self.output).trim(),
            ))
        }
    }
}

pub(super) async fn start_terminal(
    connection: &SshConnection,
    params: &TermOpenParams,
    cols: usize,
    rows: usize,
) -> Result<StartedTerminal, TermError> {
    let startup_deadline = Instant::now() + STARTUP_TIMEOUT;
    let mut channel = timeout_at(
        request_deadline(startup_deadline),
        connection.handle.channel_open_session(),
    )
    .await
    .map_err(|_| TermError::Open("创建 SSH channel 超时".to_string()))?
    .map_err(|error| TermError::Open(format!("创建 SSH channel 失败：{error}")))?;
    let mut initial_output = Vec::new();
    let mut command_session = None;
    let mut bare_command_sent = false;
    let result: Result<_, String> = async {
        let deadline = request_deadline(startup_deadline);
        timeout_at(
            deadline,
            channel.request_pty(true, TERM_TYPE, cols as u32, rows as u32, 0, 0, &[]),
        )
        .await
        .map_err(|_| "请求 PTY 超时".to_string())?
        .map_err(|error| format!("请求 PTY 失败：{error}"))?;
        await_request_reply(&mut channel, deadline, "请求 PTY", &mut initial_output).await?;

        let (tmux_session, note) =
            prepare_tmux(connection, params, startup_deadline, &mut command_session).await?;
        let deadline = request_deadline(startup_deadline);
        if let Some(name) = &tmux_session {
            let command = format!("tmux attach-session -t ={name}");
            // attach 请求失败与后续确认失败同口径：附会话名与「可能仍在」提示，
            // 避免模型在状态未知时误判需要重建现场。
            let attach_hint =
                |error: String| format!("{error}；远端 tmux 会话 {name} 可能仍在，请先查询状态");
            timeout_at(deadline, channel.exec(true, command.as_bytes()))
                .await
                .map_err(|_| attach_hint("请求 tmux attach 超时".to_string()))?
                .map_err(|error| attach_hint(format!("请求 tmux attach 失败：{error}")))?;
        } else if let Some(command) = &params.command {
            bare_command_sent = true;
            timeout_at(deadline, channel.exec(true, command.as_bytes()))
                .await
                .map_err(|_| "请求交互命令超时".to_string())?
                .map_err(|error| format!("请求交互命令失败：{error}"))?;
        } else {
            timeout_at(deadline, channel.request_shell(true))
                .await
                .map_err(|_| "请求 shell 超时".to_string())?
                .map_err(|error| format!("请求 shell 失败：{error}"))?;
        }
        await_request_reply(&mut channel, deadline, "启动终端", &mut initial_output)
            .await
            .map_err(|error| match &tmux_session {
                Some(name) => {
                    format!("{error}；远端 tmux 会话 {name} 可能仍在，请先查询状态")
                }
                None => error,
            })?;
        Ok((tmux_session, note))
    }
    .await;
    match result {
        Ok((tmux_session, note)) => Ok(StartedTerminal {
            channel,
            tmux_session,
            note,
            initial_output,
            command_started: command_session.is_some(),
        }),
        Err(error) => {
            // 启动预算已耗尽时，清理仍须获得发送 Close 请求的机会。
            let _ = timeout(CLOSE_TIMEOUT, channel.close()).await;
            Err(match command_session {
                Some(name) => command_state_unknown(&name, &error),
                None if bare_command_sent => TermError::ExternalStateUnknown(format!(
                    "{error}；远端 command 可能已执行，结果未知，禁止自动重跑。当前为裸 PTY，无法恢复现场，请先用 ssh_exec 核实任务结果"
                )),
                None => TermError::Open(error),
            })
        }
    }
}

fn request_deadline(startup_deadline: Instant) -> Instant {
    (Instant::now() + REQUEST_TIMEOUT).min(startup_deadline)
}

async fn prepare_tmux(
    connection: &SshConnection,
    params: &TermOpenParams,
    startup_deadline: Instant,
    command_session: &mut Option<String>,
) -> Result<(Option<String>, Option<String>), String> {
    if matches!(params.tmux, TmuxPreference::Off) {
        return Ok((None, Some(bare_terminal_note("已指定 tmux=off"))));
    }
    let probe = run_template_until(
        connection,
        "command -v tmux",
        request_deadline(startup_deadline),
    )
    .await?;
    if probe.exit_status == 1 {
        return if params.tmux_session.is_some() || matches!(params.tmux, TmuxPreference::Required) {
            Err(format!(
                "远端未找到 tmux；本次要求保活，未打开替代裸终端。{}",
                tmux_install_hint()
            ))
        } else {
            Ok((None, Some(bare_terminal_note("远端未安装 tmux"))))
        };
    }
    probe.ensure_success("探测 tmux")?;
    let name = params.tmux_session.clone().unwrap_or_else(|| {
        format!(
            "jkagent-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        )
    });
    let existing = run_template_until(
        connection,
        &format!("tmux has-session -t ={name}"),
        request_deadline(startup_deadline),
    )
    .await?;
    match existing.exit_status {
        0 => {
            let note = if params.command.is_some() {
                format!("已恢复现有 tmux 会话 {name}；command 仅在新建会话时执行，本次未重复运行")
            } else {
                format!("已恢复现有 tmux 会话 {name}")
            };
            Ok((Some(name), Some(note)))
        }
        1 => {
            let mut command = format!("tmux new-session -d -s {name}");
            if let Some(initial_command) = &params.command {
                // 外层 shell 只把审查后的命令作为一个参数传给 tmux；tmux 内层 shell 执行原文。
                command.push(' ');
                command.push_str(&quote_shell_argument(initial_command));
                // 从发送创建请求起就可能已有副作用；任何后续失败都不能建议重跑。
                *command_session = Some(name.clone());
            }
            run_template_until(connection, &command, request_deadline(startup_deadline))
                .await?
                .ensure_success(&format!("创建 tmux 会话 {name}"))?;
            let note = format!("已新建 tmux 会话 {name}；当前名称此前不存在，没有旧现场可恢复");
            Ok((Some(name), Some(note)))
        }
        _ => Err(existing
            .ensure_success("检查 tmux 会话")
            .expect_err("非零退出状态必须报错")),
    }
}

pub(super) fn command_state_unknown(name: &str, detail: &str) -> TermError {
    TermError::ExternalStateUnknown(format!(
        "{detail}；远端 command 可能已执行或已结束，结果未知，禁止自动重跑。tmuxSession={name}；请先用 ssh_exec 查询该会话及任务结果，仍存活时使用同名 ssh_term_open 并省略 command 恢复。会话不存在也不表示命令未执行"
    ))
}

fn bare_terminal_note(reason: &str) -> String {
    format!("{reason}；当前为裸终端，断连或关闭可能终止远端进程，不支持保活、现场恢复或共屏。{}；必须保活的任务请使用 tmux=required，避免落入裸 PTY", tmux_install_hint())
}

fn tmux_install_hint() -> &'static str {
    "可显式调用 ssh_tmux_install（支持 apt-get/dnf，经 ssh_exec 同一命令审查；需要提权时 sudo=true），成功后用 tmux=required 重开；其他系统请用 ssh_exec 检查 /etc/os-release 后自行安装。打开终端不会自动安装或注入 nohup/setsid"
}

fn quote_shell_argument(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// exec/shell/request_pty 返回仅表示写入 SSH 请求；Success 才是服务端确认。
async fn await_request_reply(
    channel: &mut Channel<russh::client::Msg>,
    deadline: Instant,
    operation: &str,
    initial_output: &mut Vec<u8>,
) -> Result<(), String> {
    loop {
        match timeout_at(deadline, channel.wait())
            .await
            .map_err(|_| format!("{operation}超时，服务端未确认请求"))?
        {
            Some(ChannelMsg::Success) => return Ok(()),
            Some(ChannelMsg::Failure) => return Err(format!("{operation}被 SSH 服务端拒绝")),
            Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                if initial_output.len() + data.len() > OUTPUT_LIMIT {
                    return Err(format!(
                        "{operation}未确认且启动输出超过 {OUTPUT_LIMIT} 字节"
                    ));
                }
                initial_output.extend_from_slice(&data);
            }
            Some(ChannelMsg::Eof | ChannelMsg::Close | ChannelMsg::ExitStatus { .. }) | None => {
                return Err(format!("{operation}未确认，SSH channel 已终止"));
            }
            Some(_) => {}
        }
    }
}

pub(super) async fn run_template_command(
    connection: &SshConnection,
    command: &str,
) -> Result<TemplateResult, String> {
    run_template_until(connection, command, Instant::now() + REQUEST_TIMEOUT).await
}

async fn run_template_until(
    connection: &SshConnection,
    command: &str,
    deadline: Instant,
) -> Result<TemplateResult, String> {
    let mut channel = timeout_at(deadline, connection.handle.channel_open_session())
        .await
        .map_err(|_| "模板命令创建 channel 超时".to_string())?
        .map_err(|error| format!("模板命令创建 channel 失败：{error}"))?;
    let result = async {
        timeout_at(deadline, channel.exec(true, command.as_bytes()))
            .await
            .map_err(|_| "模板命令 exec 超时".to_string())?
            .map_err(|error| format!("模板命令 exec 失败：{error}"))?;
        let mut output = Vec::new();
        let mut exit_status = None;
        loop {
            match timeout_at(deadline, channel.wait())
                .await
                .map_err(|_| "模板命令超时，未确认最终退出状态".to_string())?
            {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    let remaining = OUTPUT_LIMIT.saturating_sub(output.len());
                    output.extend_from_slice(&data[..data.len().min(remaining)]);
                }
                Some(ChannelMsg::ExitStatus { exit_status: code }) => exit_status = Some(code),
                // RFC 4254 的 EOF 仅表示不再有输出；ExitStatus 可以随后抵达。
                Some(ChannelMsg::Eof | ChannelMsg::Success) => {}
                Some(ChannelMsg::Failure) => return Err("SSH 服务端拒绝模板命令".into()),
                Some(ChannelMsg::Close) | None => {
                    return exit_status
                        .map(|exit_status| TemplateResult {
                            exit_status,
                            output,
                        })
                        .ok_or_else(|| "模板命令 channel 已关闭，但未取得退出码".into());
                }
                Some(_) => {}
            }
        }
    }
    .await;
    let _ = timeout(CLOSE_TIMEOUT, channel.close()).await;
    result
}

#[cfg(test)]
mod tests {
    use super::quote_shell_argument;

    #[test]
    fn reviewed_command_stays_one_outer_shell_argument() {
        assert_eq!(
            quote_shell_argument("printf '%s' \"$HOME\""),
            "'printf '\\''%s'\\'' \"$HOME\"'"
        );
    }
}
