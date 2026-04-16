use std::process::Stdio;

use tokio::process::Command;
use tokio::time::{timeout, Duration};

const SUMMARY_THRESHOLD_CHARS: usize = 100;
const OLLAMA_MODEL: &str = "llama3.2:3b";
const OLLAMA_TIMEOUT_SECS: u64 = 20;

pub async fn prepare_tool_result(tool_name: &str, raw_output: &str) -> Result<String, String> {
    if should_summarize_tool_result(raw_output) {
        summarize_with_ollama(build_tool_summary_prompt(tool_name, raw_output)).await
    } else {
        Ok(raw_output.trim().to_string())
    }
}

pub async fn summarize_dispatch_result(dispatch_result: &str) -> Result<String, String> {
    let trimmed = dispatch_result.trim();
    if trimmed.is_empty() {
        return Ok(trimmed.to_string());
    }

    if trimmed.chars().count() <= SUMMARY_THRESHOLD_CHARS {
        return Ok(trimmed.to_string());
    }

    summarize_with_ollama(build_dispatch_summary_prompt(trimmed)).await
}

pub fn build_ollama_install_message(error: &str) -> String {
    format!(
        "检测到摘要依赖 `ollama` 执行失败，当前轮次已停止。\n\n请先安装并确保以下命令可用后再重试：\n- `ollama`\n- `ollama pull {OLLAMA_MODEL}`\n\n如果已经安装，请确认 `ollama` 在 PATH 中且本地模型已拉取完成。\n\n错误详情：\n{error}"
    )
}

async fn summarize_with_ollama(prompt: String) -> Result<String, String> {
    let output = timeout(
        Duration::from_secs(OLLAMA_TIMEOUT_SECS),
        Command::new("ollama")
            .arg("run")
            .arg(OLLAMA_MODEL)
            .arg(prompt)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| format!("`ollama run {OLLAMA_MODEL}` 超时（>{OLLAMA_TIMEOUT_SECS}s）"))?
    .map_err(|error| format!("启动 `ollama` 失败：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("退出状态：{}", output.status)
        };
        return Err(format!("`ollama run {OLLAMA_MODEL}` 执行失败：{detail}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Err(format!("`ollama run {OLLAMA_MODEL}` 返回空结果"));
    }

    Ok(stdout)
}

fn should_summarize_tool_result(raw_output: &str) -> bool {
    raw_output.trim().chars().count() > SUMMARY_THRESHOLD_CHARS
}

fn build_tool_summary_prompt(tool_name: &str, raw_output: &str) -> String {
    format!(
        "你是调度 Agent 的工具结果摘要器。请将下面的长工具输出压缩成便于继续决策的中文摘要。\n\
要求：\n\
- 只保留事实，不要猜测。\n\
- 优先保留错误、失败原因、退出码、关键文件路径、命令结果、数量统计。\n\
- 如果输出里包含明确完成项或下一步线索，也要保留。\n\
- 使用下面结构输出，最多 6 行：\n\
【工具调用摘要】\n\
- 工具：{tool_name}\n\
- 结果：一句话概括\n\
- 关键事实：最多 3 条，单行内用 `；` 分隔\n\
- 下一步：若无明确建议写“无”\n\n\
工具原始输出如下：\n{}",
        raw_output.trim()
    )
}

fn build_dispatch_summary_prompt(dispatch_result: &str) -> String {
    format!(
        "你是调度 Agent 的子任务回流摘要器。请把下面的子智能体执行结果压缩成可直接注入主调度上下文的中文摘要。\n\
要求：\n\
- 只保留事实，不要臆测。\n\
- 必须保留：当前状态、已完成事项、失败/阻塞点、关键证据、建议下一步。\n\
- 如有测试、构建、lint、报错、修改文件、退出状态，优先保留。\n\
- 使用下面结构输出，最多 8 行：\n\
【子任务回流摘要】\n\
- 状态：...\n\
- 已完成：...\n\
- 阻塞/风险：...\n\
- 关键证据：...\n\
- 建议下一步：...\n\n\
子任务原始结果如下：\n{}",
        dispatch_result.trim()
    )
}

#[cfg(test)]
mod tests {
    use super::{build_ollama_install_message, should_summarize_tool_result};

    #[test]
    fn marks_all_long_outputs_for_summary() {
        assert!(should_summarize_tool_result(&"a".repeat(101)));
        assert!(!should_summarize_tool_result("short"));
    }

    #[test]
    fn install_message_mentions_model_and_pull_command() {
        let message = build_ollama_install_message("command not found");
        assert!(message.contains("ollama"));
        assert!(message.contains("llama3.2:3b"));
        assert!(message.contains("ollama pull llama3.2:3b"));
    }
}
