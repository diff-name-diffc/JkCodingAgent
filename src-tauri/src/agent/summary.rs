use std::process::Stdio;

use serde_json::Value;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

const SUMMARY_MODE_THRESHOLD_CHARS: usize = 240;
const SUMMARY_MODE_THRESHOLD_LINES: usize = 24;
const AUTO_SUMMARY_THRESHOLD_CHARS: usize = 1200;
const AUTO_SUMMARY_THRESHOLD_LINES: usize = 120;
const FULL_RESULT_MAX_CHARS: usize = 12_000;
const FULL_RESULT_MAX_LINES: usize = 400;
const OLLAMA_MODEL: &str = "llama3.2:3b";
const OLLAMA_TIMEOUT_SECS: u64 = 20;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedToolResult {
    pub content: String,
    pub result_mode: &'static str,
}

pub async fn prepare_tool_result(
    tool_name: &str,
    args: &Value,
    raw_output: &str,
) -> Result<PreparedToolResult, String> {
    let trimmed = raw_output.trim();
    if trimmed.is_empty() {
        return Ok(PreparedToolResult {
            content: String::new(),
            result_mode: tool_result_mode_label(ToolResultAction::KeepRaw),
        });
    }

    match decide_tool_result_action(tool_name, args, trimmed) {
        ToolResultAction::KeepRaw => Ok(PreparedToolResult {
            content: trimmed.to_string(),
            result_mode: tool_result_mode_label(ToolResultAction::KeepRaw),
        }),
        ToolResultAction::Summarize => {
            summarize_with_ollama(build_tool_summary_prompt(tool_name, trimmed))
                .await
                .map(|content| PreparedToolResult {
                    content,
                    result_mode: tool_result_mode_label(ToolResultAction::Summarize),
                })
        }
        ToolResultAction::ConservativeSummarize => {
            summarize_with_ollama(build_conservative_tool_summary_prompt(tool_name, trimmed))
                .await
                .map(|content| PreparedToolResult {
                    content,
                    result_mode: tool_result_mode_label(ToolResultAction::ConservativeSummarize),
                })
        }
    }
}

pub async fn summarize_dispatch_result(dispatch_result: &str) -> Result<String, String> {
    let trimmed = dispatch_result.trim();
    if trimmed.is_empty() {
        return Ok(trimmed.to_string());
    }

    if !exceeds_limits(
        trimmed,
        SUMMARY_MODE_THRESHOLD_CHARS,
        SUMMARY_MODE_THRESHOLD_LINES,
    ) {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolResultMode {
    Auto,
    Full,
    Summary,
}

fn build_tool_summary_prompt(tool_name: &str, raw_output: &str) -> String {
    let focus = tool_summary_focus(tool_name);
    format!(
        "你是调度 Agent 的工具结果摘要器。请将下面的长工具输出压缩成便于继续决策的中文摘要。\n\
要求：\n\
- 只保留事实，不要猜测。\n\
- 优先保留错误、失败原因、退出码、关键文件路径、命令结果、数量统计。\n\
- 如果输出里包含明确完成项或下一步线索，也要保留。\n\
- 根据工具类型保留最重要的结构化信息：{focus}\n\
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

fn build_conservative_tool_summary_prompt(tool_name: &str, raw_output: &str) -> String {
    let focus = tool_summary_focus(tool_name);
    format!(
        "你在执行调度 Agent 的高保真压缩，不是激进摘要。目标是在明显缩短长度的同时，尽量保留后续推理需要的原始事实。\n\
要求：\n\
- 只保留原文里明确出现的事实，不要猜测。\n\
- 尽量保留原始顺序、关键实体名、文件路径、符号名、配置键、错误文本、数量、退出状态。\n\
- 如果内容是代码或配置，不要把不同段落混成一句话；应说明关键位置、职责和关系。\n\
- 如果内容是目录或文件列表，优先保留层级、关键文件名、数量和明显缺失项。\n\
- 如果内容是命令输出，优先保留命令结果、失败原因、关键日志、测试失败项和退出状态。\n\
- 需要压缩，但不要过度归纳；宁可稍长，也不要丢掉影响后续判断的细节。\n\
- {focus}\n\
- 使用下面结构输出，控制在 8-12 行：\n\
【工具结果保守压缩】\n\
- 工具：{tool_name}\n\
- 总体结论：...\n\
- 关键事实：逐条列出 3-6 条，尽量包含精确值或原始关键词\n\
- 重要细节：补充仍会影响后续决策的上下文；若无写“无”\n\
- 建议下一步：若原文没有明确建议，写“无”\n\n\
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolOutputKind {
    Exact,
    Command,
    Mutation,
    Message,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolResultAction {
    KeepRaw,
    Summarize,
    ConservativeSummarize,
}

fn tool_result_mode_label(action: ToolResultAction) -> &'static str {
    match action {
        ToolResultAction::KeepRaw => "raw",
        ToolResultAction::Summarize => "summary",
        ToolResultAction::ConservativeSummarize => "conservative_summary",
    }
}

fn decide_tool_result_action(tool_name: &str, args: &Value, raw_output: &str) -> ToolResultAction {
    let mode = requested_result_mode(args).unwrap_or_else(|| default_result_mode(tool_name));
    let kind = tool_output_kind(tool_name);

    match mode {
        ToolResultMode::Summary => {
            if exceeds_limits(
                raw_output,
                SUMMARY_MODE_THRESHOLD_CHARS,
                SUMMARY_MODE_THRESHOLD_LINES,
            ) {
                ToolResultAction::Summarize
            } else {
                ToolResultAction::KeepRaw
            }
        }
        ToolResultMode::Full => {
            if exceeds_limits(raw_output, FULL_RESULT_MAX_CHARS, FULL_RESULT_MAX_LINES) {
                ToolResultAction::ConservativeSummarize
            } else {
                ToolResultAction::KeepRaw
            }
        }
        ToolResultMode::Auto => match kind {
            ToolOutputKind::Exact => {
                if exceeds_limits(raw_output, FULL_RESULT_MAX_CHARS, FULL_RESULT_MAX_LINES) {
                    ToolResultAction::ConservativeSummarize
                } else {
                    ToolResultAction::KeepRaw
                }
            }
            ToolOutputKind::Command | ToolOutputKind::Other => {
                if exceeds_limits(
                    raw_output,
                    AUTO_SUMMARY_THRESHOLD_CHARS,
                    AUTO_SUMMARY_THRESHOLD_LINES,
                ) {
                    ToolResultAction::Summarize
                } else {
                    ToolResultAction::KeepRaw
                }
            }
            ToolOutputKind::Mutation | ToolOutputKind::Message => {
                if exceeds_limits(raw_output, FULL_RESULT_MAX_CHARS, FULL_RESULT_MAX_LINES) {
                    ToolResultAction::ConservativeSummarize
                } else {
                    ToolResultAction::KeepRaw
                }
            }
        },
    }
}

fn requested_result_mode(args: &Value) -> Option<ToolResultMode> {
    let raw = args.get("result_mode")?.as_str()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "auto" => Some(ToolResultMode::Auto),
        "full" => Some(ToolResultMode::Full),
        "summary" => Some(ToolResultMode::Summary),
        _ => None,
    }
}

fn default_result_mode(tool_name: &str) -> ToolResultMode {
    match tool_output_kind(tool_name) {
        ToolOutputKind::Exact => ToolResultMode::Full,
        ToolOutputKind::Command
        | ToolOutputKind::Mutation
        | ToolOutputKind::Message
        | ToolOutputKind::Other => ToolResultMode::Auto,
    }
}

fn tool_output_kind(tool_name: &str) -> ToolOutputKind {
    match tool_name {
        "read_file" | "list_dir" | "glob" => ToolOutputKind::Exact,
        "exec" => ToolOutputKind::Command,
        "write_file" | "edit_file" => ToolOutputKind::Mutation,
        "message" => ToolOutputKind::Message,
        _ => ToolOutputKind::Other,
    }
}

fn tool_summary_focus(tool_name: &str) -> &'static str {
    match tool_name {
        "read_file" => "保留关键文件路径、符号名、行号范围、配置键和能支持判断的核心实现细节",
        "list_dir" | "glob" => "保留目录层级、关键文件名、数量和显著的结构特征",
        "exec" => "保留命令结果、错误文本、失败项、退出状态、关键路径和数量统计",
        _ => "保留后续判断最依赖的事实、路径、标识符和数量信息",
    }
}

fn exceeds_limits(raw_output: &str, max_chars: usize, max_lines: usize) -> bool {
    raw_output.chars().count() > max_chars || raw_output.lines().count() > max_lines
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{build_ollama_install_message, decide_tool_result_action, ToolResultAction};

    #[test]
    fn exact_tools_keep_medium_sized_raw_output_by_default() {
        let action = decide_tool_result_action("read_file", &json!({}), &"a".repeat(2_000));
        assert_eq!(action, ToolResultAction::KeepRaw);
    }

    #[test]
    fn exec_auto_mode_summarizes_medium_output() {
        let action = decide_tool_result_action("exec", &json!({}), &"a".repeat(2_000));
        assert_eq!(action, ToolResultAction::Summarize);
    }

    #[test]
    fn explicit_full_mode_prevents_normal_summary_until_hard_limit() {
        let action = decide_tool_result_action(
            "exec",
            &json!({ "result_mode": "full" }),
            &"a".repeat(2_000),
        );
        assert_eq!(action, ToolResultAction::KeepRaw);
    }

    #[test]
    fn explicit_summary_mode_requests_summary() {
        let action = decide_tool_result_action(
            "read_file",
            &json!({ "result_mode": "summary" }),
            &"a".repeat(500),
        );
        assert_eq!(action, ToolResultAction::Summarize);
    }

    #[test]
    fn full_mode_falls_back_to_conservative_summary_for_oversized_output() {
        let action = decide_tool_result_action(
            "read_file",
            &json!({ "result_mode": "full" }),
            &"a".repeat(20_000),
        );
        assert_eq!(action, ToolResultAction::ConservativeSummarize);
    }

    #[test]
    fn install_message_mentions_model_and_pull_command() {
        let message = build_ollama_install_message("command not found");
        assert!(message.contains("ollama"));
        assert!(message.contains("llama3.2:3b"));
        assert!(message.contains("ollama pull llama3.2:3b"));
    }
}
