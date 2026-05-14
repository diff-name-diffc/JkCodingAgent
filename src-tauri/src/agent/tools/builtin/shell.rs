use std::process::{Output, Stdio};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::time::{timeout, Duration};

use super::common::{is_dangerous, string_arg, u64_arg, with_result_mode_parameter};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;

pub(super) fn exec_tool() -> Box<dyn AgentTool> {
    Box::new(ExecTool)
}

pub(super) fn message_tool() -> Box<dyn AgentTool> {
    Box::new(MessageTool)
}

struct ExecTool;
struct MessageTool;

const MAX_OUTPUT_BYTES: usize = 256 * 1024;

struct CapturedCommandOutput {
    output: Output,
    total_bytes_read: usize,
    timed_out: bool,
}

#[async_trait]
impl AgentTool for ExecTool {
    fn name(&self) -> &'static str {
        "exec"
    }

    fn description(&self) -> &'static str {
        "在当前工作区执行 shell 命令并返回输出。适合搜索、构建、测试、查看 git 信息；优先使用只读命令。默认自动判断；查精确报错用 result_mode=full，只看结论用 result_mode=summary。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "要执行的 shell 命令" },
                    "timeout": { "type": "integer", "description": "超时时间，单位秒，默认 60", "minimum": 1 }
                },
                "required": ["command"]
            }),
            "auto",
            "命令输出噪声通常较大：只看成败、统计或阶段性结论时选 summary；要保留原始报错、测试明细或精确文本时选 full。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(command) = string_arg(args, "command") else {
            return "错误：缺少必填参数 command".to_string();
        };
        if is_dangerous(&command) {
            return format!("错误：基于安全策略已拦截命令：{command}");
        }
        let timeout_secs = u64_arg(args, "timeout")
            .unwrap_or(context.exec_timeout_secs)
            .max(1);

        let mut child = match Command::new("sh")
            .arg("-lc")
            .arg(&command)
            .current_dir(&context.workspace)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(error) => return format!("执行命令失败：{error}"),
        };

        let captured = match capture_command_output(&mut child, timeout_secs).await {
            Ok(output) => output,
            Err(error) => return format!("执行命令失败：{error}"),
        };

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
        } else if !captured.output.status.success() {
            result.push_str(&format!("\n\n[退出状态：{}]", captured.output.status));
        }
        if result.is_empty() {
            "[命令已完成，无输出]".to_string()
        } else {
            result
        }
    }
}

async fn capture_command_output(
    child: &mut tokio::process::Child,
    timeout_secs: u64,
) -> std::io::Result<CapturedCommandOutput> {
    let stdout_reader = child.stdout.take();
    let stderr_reader = child.stderr.take();
    let stdout_task = tokio::spawn(async move { read_limited(stdout_reader).await });
    let stderr_task = tokio::spawn(async move { read_limited(stderr_reader).await });

    let (status, timed_out) = match timeout(Duration::from_secs(timeout_secs), child.wait()).await {
        Ok(status) => (status?, false),
        Err(_) => {
            child.kill().await?;
            (child.wait().await?, true)
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
    })
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
        "向用户发送最终回复。通常在调查完成、结果整理完成或协调结束后使用。通常保持默认 result_mode=auto 即可。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "要发送给用户的内容" }
                },
                "required": ["content"]
            }),
            "auto",
            "消息工具一般只返回简短确认信息，无需显式改动。",
        )
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> String {
        match string_arg(args, "content") {
            Some(content) => format!("消息已发送（{} 字符）", content.len()),
            None => "错误：缺少必填参数 content".to_string(),
        }
    }
}
