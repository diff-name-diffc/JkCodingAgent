use std::process::{Output, Stdio};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::watch;
use tokio::time::{sleep, Duration};

use super::common::{is_dangerous, string_arg, with_compression_parameters};
use crate::agent::ssh_review::{review_shell_command, CommandReviewPayload, CommandReviewTarget};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;
use crate::agent::tools::{ToolAction, ToolResult};
use crate::ssh_tool::SshAuditReview;

pub(super) fn exec_tool() -> Box<dyn AgentTool> {
    Box::new(ExecTool)
}

pub(super) fn message_tool() -> Box<dyn AgentTool> {
    Box::new(MessageTool)
}

struct ExecTool;
struct MessageTool;

const MAX_OUTPUT_BYTES: usize = 64 * 1024;

struct CapturedCommandOutput {
    output: Output,
    total_bytes_read: usize,
    timed_out: bool,
    cancelled: bool,
}

#[async_trait]
impl AgentTool for ExecTool {
    fn name(&self) -> &'static str {
        "exec"
    }

    fn description(&self) -> &'static str {
        "在当前工作区执行 shell 命令并返回输出。适合搜索、构建、测试、查看 git 信息；优先使用只读命令。命令执行前会经过安全审查（未配置审查模型时拒绝执行）。默认开启压缩（compress=true）并需填写 compress_intent；需要保留原始报错原文时设 compress=false。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "要执行的 shell 命令" }
                },
                "required": ["command"]
            }),
            true,
            "命令输出噪声通常较大，推荐开启压缩并在 compress_intent 中说明想看什么（例如'确认 pnpm build 是否成功'）；要保留原始报错、测试明细时设 compress=false。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        let mut command_cancelled = false;
        let result = async {
            let Some(command) = string_arg(args, "command") else {
                return "错误：缺少必填参数 command".to_string();
            };
            if is_dangerous(&command) {
                return format!("错误：基于安全策略已拦截命令：{command}");
            }
            // 安全审查门禁（fail-closed）：与 local_zsh/ssh_exec 同链路。
            // 未配置审查、审查异常或判定不通过一律拒绝执行。
            match review_exec_command(args, context, &command).await {
                Ok(review) if review.allowed => {}
                Ok(review) => return format!("错误：命令已被安全审查拦截：{}", review.reason),
                Err(error) => return format!("错误：{error}"),
            }
            let timeout_secs = context.exec_timeout_secs.max(1);

            let mut cmd = Command::new("sh");
            cmd.arg("-lc")
                .arg(&command)
                .current_dir(&context.workspace)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            #[cfg(unix)]
            cmd.process_group(0); // 独立进程组：超时可按组终止全部派生进程
            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(error) => return format!("错误：执行命令失败：{error}"),
            };

            let captured =
                match capture_command_output(&mut child, timeout_secs, context.cancel_rx.clone())
                    .await
                {
                    Ok(output) => output,
                    Err(error) => return format!("错误：执行命令失败：{error}"),
                };
            command_cancelled = captured.cancelled;

            let stdout = String::from_utf8_lossy(&captured.output.stdout)
                .trim_end()
                .to_string();
            let stderr = String::from_utf8_lossy(&captured.output.stderr)
                .trim_end()
                .to_string();
            let mut result = String::new();
            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.is_empty() {
                    result.push_str("\n【标准错误】\n");
                }
                result.push_str(&stderr);
            }
            let retained_bytes = captured.output.stdout.len() + captured.output.stderr.len();
            if captured.total_bytes_read > retained_bytes {
                result.push_str(&format!(
                    "\n\n[...输出已截断，共 {} 字节，仅保留 stdout/stderr 各前 {} 字节...]",
                    captured.total_bytes_read, MAX_OUTPUT_BYTES
                ));
            }
            if captured.timed_out {
                result.push_str(&format!("\n\n[命令执行超时（{timeout_secs} 秒），已终止]"));
            } else if captured.cancelled {
                result.push_str("\n\n[命令已取消，进程组及其派生进程已终止]");
            } else if !captured.output.status.success() {
                result.push_str(&format!("\n\n[退出状态：{}]", captured.output.status));
            }
            if result.is_empty() {
                "[命令已完成，无输出]".to_string()
            } else {
                result
            }
        }
        .await;

        if command_cancelled {
            ToolResult::cancelled(result)
        } else {
            ToolResult::from_text(result)
        }
    }
}

/// exec 的安全审查：复用 ssh_review 链路。未配置审查模型时返回 Err（调用方 fail-closed 拦截）。
/// 审计：拦截结果作为工具返回持久化进会话消息记录（exec 运行在用户项目工作区内，
/// 不在项目目录中额外落审计文件，避免污染用户仓库）。
async fn review_exec_command(
    args: &Value,
    context: &ToolContext,
    command: &str,
) -> Result<SshAuditReview, String> {
    let Some(review_config) = context.ssh_review.as_ref() else {
        return Err(
            "未配置安全审查，已拒绝执行命令。请先在应用设置中配置安全审查模型。".to_string(),
        );
    };
    let intent = string_arg(args, "compress_intent")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| context.session_title.clone());
    let payload = CommandReviewPayload {
        intent,
        task: context.user_task.clone().unwrap_or_default(),
        target: CommandReviewTarget::WorkspaceShell {
            workspace_path: context.workspace.display().to_string(),
        },
        command: command.to_string(),
        stdin: None,
    };
    review_shell_command(review_config, &payload)
        .await
        .map(|verdict| SshAuditReview {
            allowed: verdict.allowed,
            reason: verdict.reason,
        })
}

async fn capture_command_output(
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
            // 取消与超时具有相同的资源收敛要求：不能只 drop wait future，
            // 否则 shell 派生的后台进程仍会继续执行副作用。
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
            // 发送端消失意味着所属运行已结束；按取消处理，避免遗留子进程。
            Err(_) => return,
        }
    }
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

#[async_trait]
impl AgentTool for MessageTool {
    fn name(&self) -> &'static str {
        "message"
    }

    fn description(&self) -> &'static str {
        "向用户发送最终回复。通常在调查完成、结果整理完成或协调结束后使用。通常保持默认 compress=false 即可。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "要发送给用户的内容" }
                },
                "required": ["content"]
            }),
            false,
            "消息工具一般只返回简短确认信息，默认关闭压缩。",
        )
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> ToolResult {
        match string_arg(args, "content") {
            Some(content) => {
                ToolResult::success_text(format!("消息已发送（{} 字符）", content.len()))
                    .with_action(ToolAction::FinalMessage { content })
            }
            None => ToolResult::recoverable_error("错误：缺少必填参数 content"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use super::{capture_command_output, AgentTool, ExecTool};
    use tokio::process::Command;

    #[test]
    fn exec_schema_does_not_expose_runtime_timeout() {
        let parameters = ExecTool.parameters();

        assert!(parameters["properties"].get("command").is_some());
        assert!(parameters["properties"].get("timeout").is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_kills_the_shell_process_group() {
        let mut command = Command::new("sh");
        command
            .arg("-lc")
            .arg("sleep 30 & wait")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = command.spawn().expect("spawn shell");
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        cancel_tx.send(true).expect("request cancellation");

        let captured = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            capture_command_output(&mut child, 30, Some(cancel_rx)),
        )
        .await
        .expect("process group should settle")
        .expect("capture output");

        assert!(captured.cancelled);
        assert!(!captured.timed_out);
    }
}
