use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;
use tokio::time::{timeout, Duration};
use uuid::Uuid;

use crate::agent::config::resolve_home_dir;
use crate::agent::db::{DispatcherDb, PythonCodeRunRecord};
use crate::agent::llm::{
    ChatMessage, OpenAiCompatProvider, OutboundToolCall, ToolDefinition, ToolFunctionDefinition,
};
use crate::agent::DispatcherState;
use crate::shared::truncate_for_display;

const RUN_TIMEOUT_SECS: u64 = 60;
const INSTALL_TIMEOUT_SECS: u64 = 120;
const MAX_AGENT_ITERATIONS: usize = 6;
const MAX_OUTPUT_CHARS: usize = 24_000;

#[derive(Default)]
pub struct PythonRunnerState {
    active_runs: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PythonRunToolEvent {
    pub kind: String,
    pub name: String,
    pub detail: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PythonRunEvent {
    pub event: String,
    pub run_id: String,
    pub workspace_id: String,
    pub message_id: String,
    pub code_block_index: u32,
    pub data: Value,
}

#[derive(Debug)]
struct CommandOutput {
    status_code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
    cancelled: bool,
}

struct RunPaths {
    root: PathBuf,
    venv: PathBuf,
    run_dir: PathBuf,
    main_py: PathBuf,
}

mod agent;
pub(crate) mod commands;
use agent::run_python_agent;
async fn execute_python_tool(
    paths: &RunPaths,
    name: &str,
    arguments: &Value,
    record: &mut PythonCodeRunRecord,
    installed_packages: &mut Vec<String>,
    stop_rx: &mut watch::Receiver<bool>,
    app: &AppHandle,
) -> Result<String> {
    let event_ctx = PythonRunEventCtx {
        run_id: record.run_id.clone(),
        workspace_id: record.workspace_id.clone(),
        message_id: record.message_id.clone(),
        code_block_index: record.code_block_index,
    };
    match name {
        "run_python" => {
            let output = run_python_file_streaming(paths, stop_rx, app, &event_ctx).await?;
            apply_python_output(record, &output);
            if output.cancelled {
                record.status = "stopped".to_string();
            } else if command_succeeded(&output) {
                record.status = "done".to_string();
            } else {
                record.status = "failed".to_string();
                record.error_reason = Some(first_non_empty(
                    &record.stderr,
                    "Python 进程以非 0 状态退出",
                ));
            }
            Ok(format_command_result(&output))
        }
        "install_packages" => {
            let packages = parse_packages(arguments)?;
            let output = install_packages(paths, &packages, stop_rx).await?;
            if command_succeeded(&output) {
                for package in packages {
                    if !installed_packages.contains(&package) {
                        installed_packages.push(package);
                    }
                }
            }
            Ok(format_command_result(&output))
        }
        "update_code" => {
            let code = parse_complete_code(arguments)?;
            tokio::fs::write(&paths.main_py, &code)
                .await
                .with_context(|| format!("write {}", paths.main_py.display()))?;
            record.code_hash = code_hash(&code);
            record.code = code;
            record.status = "running".to_string();
            record.error_reason = None;
            Ok("已用补全后的代码更新当前临时 main.py。请继续调用 run_python。".to_string())
        }
        other => Err(anyhow!("未知工具：{other}")),
    }
}

fn resolve_summary_provider(db: &DispatcherDb) -> Result<OpenAiCompatProvider> {
    let settings = db.get_settings_v2()?;
    let model_config = settings
        .project
        .summary_model_configs
        .iter()
        .find(|config| config.active)
        .or_else(|| settings.project.summary_model_configs.first())
        .cloned()
        .ok_or_else(|| anyhow!("摘要模型未配置。请先在 Aha 设置中配置项目摘要模型。"))?;
    if model_config.url.trim().is_empty()
        || model_config.api_key.trim().is_empty()
        || model_config.model.trim().is_empty()
    {
        anyhow::bail!("摘要模型未完整配置。Python 执行解释不会回退到主聊天模型。");
    }
    Ok(OpenAiCompatProvider::new(
        model_config.api_key,
        model_config.url,
        model_config.model,
        4096,
        0.1,
    ))
}

async fn explain_result(
    provider: &OpenAiCompatProvider,
    record: &PythonCodeRunRecord,
    extra_instruction: Option<&str>,
) -> Result<String> {
    let prompt = format!(
        "你是 Python 教学助理。请基于代码和运行结果，用简体中文给出简洁但有教学价值的解释。\n\
         必须包含：1) 运行结果说明；2) 关键代码解释；3) 如果失败，指出错误原因和修复建议。\n\
         不要编造未出现的输出。\n{}\n\n代码：\n```python\n{}\n```\n\nstdout:\n{}\n\nstderr:\n{}\n\n状态：{}",
        extra_instruction.unwrap_or(""),
        record.code,
        record.stdout,
        record.stderr,
        record.status,
    );
    let response = provider
        .chat_stream(&[ChatMessage::system(prompt)], &[], false, |_| {})
        .await
        .context("生成 Python 教学解释失败")?;
    Ok(response.content.trim().to_string())
}

fn build_python_agent_system_prompt() -> String {
    "你是一个只负责运行 Python Markdown 代码块的教学 agent。\
     你可以通过工具运行代码、安装依赖，或把 Markdown 中不完整的示例代码补全为可运行脚本。\
     你会看到完整消息上下文和被点击的代码块；补全代码必须忠实于消息意图，不要换题。\
     如果 stderr 显示缺失第三方包，请调用 install_packages 安装最小必要包，然后调用 run_python 重试。\
     如果代码块明显只是片段、伪代码、缺少数据/变量/函数定义，先调用 update_code 写入一个完整可运行的教学示例，再调用 run_python。\
     如果是语法错误、类型错误或逻辑错误，优先基于上下文用 update_code 修正成最小可运行版本；确实无法合理补全时再解释原因。\
     不要建议使用 shell；不要安装与错误无关的包；不要访问项目文件。"
        .to_string()
}

fn build_initial_agent_user_prompt(record: &PythonCodeRunRecord, message_context: &str) -> String {
    format!(
        "请帮助执行并解释被点击的 Python 代码块。初次运行已经失败，请结合完整消息上下文判断：\
         需要安装依赖、补全代码，还是直接解释错误。\n\n完整消息上下文：\n{}\n\n被点击的代码块：\n```python\n{}\n```\n\nstdout:\n{}\n\nstderr:\n{}",
        truncate_for_display(message_context, 12_000, "\n...[消息上下文已截断]"),
        record.code,
        record.stdout,
        record.stderr
    )
}

fn python_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            kind: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "run_python".to_string(),
                description: "运行当前 main.py，返回 stdout/stderr 和退出状态。".to_string(),
                parameters: json!({ "type": "object", "properties": {} }),
            },
        },
        ToolDefinition {
            kind: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "install_packages".to_string(),
                description: "在全应用共享 uv 虚拟环境中安装缺失的 Python 包。只安装必要依赖。"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "packages": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "要安装的包名列表，例如 [\"pandas\", \"matplotlib\"]"
                        }
                    },
                    "required": ["packages"]
                }),
            },
        },
        ToolDefinition {
            kind: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "update_code".to_string(),
                description: "用完整、可运行、忠实于消息上下文的 Python 脚本替换当前临时 main.py。用于补全片段、缺失变量、缺失示例数据或修正代码错误。".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "code": {
                            "type": "string",
                            "description": "完整 Python 文件内容，不要包含 Markdown 代码围栏。"
                        }
                    },
                    "required": ["code"]
                }),
            },
        },
    ]
}

fn parse_packages(arguments: &Value) -> Result<Vec<String>> {
    let packages = arguments
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("install_packages 缺少 packages 数组"))?;
    let mut result = Vec::new();
    for item in packages {
        let package = item
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("包名必须是非空字符串"))?;
        if !is_safe_package_spec(package) {
            anyhow::bail!("非法包名：{package}");
        }
        result.push(package.to_string());
    }
    if result.is_empty() {
        anyhow::bail!("packages 不能为空");
    }
    Ok(result)
}

fn parse_complete_code(arguments: &Value) -> Result<String> {
    let code = arguments
        .get("code")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("update_code 缺少非空 code"))?;
    if code.len() > 200_000 {
        anyhow::bail!("补全代码过长，已拒绝写入");
    }
    let code = code
        .strip_prefix("```python")
        .or_else(|| code.strip_prefix("```py"))
        .or_else(|| code.strip_prefix("```"))
        .unwrap_or(code)
        .trim();
    let code = code.strip_suffix("```").unwrap_or(code).trim();
    if code.is_empty() {
        anyhow::bail!("补全代码不能为空");
    }
    Ok(format!("{code}\n"))
}

fn is_safe_package_spec(package: &str) -> bool {
    package.len() <= 120
        && package.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '.' | '_' | '-' | '[' | ']' | '=' | '<' | '>' | '~' | '!')
        })
        && !package.starts_with('-')
}

fn apply_python_output(record: &mut PythonCodeRunRecord, output: &CommandOutput) {
    record.stdout = output.stdout.clone();
    record.stderr = output.stderr.clone();
    if output.timed_out {
        append_line(&mut record.stderr, "Python 执行超时，进程已终止。");
    }
    if output.cancelled {
        append_line(&mut record.stderr, "Python 执行已停止。");
    }
    record.updated_at = Utc::now().to_rfc3339();
}

fn format_command_result(output: &CommandOutput) -> String {
    format!(
        "exit_code: {:?}\ntimed_out: {}\ncancelled: {}\nstdout:\n{}\n\nstderr:\n{}",
        output.status_code, output.timed_out, output.cancelled, output.stdout, output.stderr
    )
}

fn command_succeeded(output: &CommandOutput) -> bool {
    output.status_code == Some(0) && !output.timed_out && !output.cancelled
}

fn first_non_empty(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        truncate_for_display(trimmed, 2000, "\n...[错误已截断]")
    }
}

fn append_line(target: &mut String, line: &str) {
    if !target.is_empty() && !target.ends_with('\n') {
        target.push('\n');
    }
    target.push_str(line);
}

/// 同步 DB upsert 放进 spawn_blocking 执行，避免阻塞异步任务线程。
async fn upsert_run_record(db: &DispatcherDb, record: &PythonCodeRunRecord) -> Result<()> {
    let db = db.clone();
    let record = record.clone();
    tokio::task::spawn_blocking(move || db.upsert_python_code_run(&record))
        .await
        .map_err(|error| anyhow!("spawn_blocking 失败: {error}"))?
}

async fn mark_stopped(
    db: &DispatcherDb,
    app: &AppHandle,
    record: &mut PythonCodeRunRecord,
) -> Result<()> {
    record.status = "stopped".to_string();
    record.updated_at = Utc::now().to_rfc3339();
    upsert_run_record(db, record).await?;
    emit_run_event(app, record, "stopped", json!({ "record": record.clone() }));
    Ok(())
}

async fn persist_and_emit(
    db: &DispatcherDb,
    app: &AppHandle,
    record: &PythonCodeRunRecord,
    event: &str,
    data: Value,
) -> Result<()> {
    upsert_run_record(db, record).await?;
    emit_run_event(app, record, event, data);
    Ok(())
}
fn emit_run_event(app: &AppHandle, record: &PythonCodeRunRecord, event: &str, data: Value) {
    let _ = app.emit(
        "python-run-event",
        PythonRunEvent {
            event: event.to_string(),
            run_id: record.run_id.clone(),
            workspace_id: record.workspace_id.clone(),
            message_id: record.message_id.clone(),
            code_block_index: record.code_block_index,
            data,
        },
    );
}

mod runtime;
use runtime::{
    cancellation_requested, code_hash, ensure_uv_available, ensure_venv, install_packages,
    prepare_paths, run_python_file_streaming, PythonRunEventCtx,
};
