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

#[tauri::command]
pub async fn python_runner_list_results(
    state: State<'_, DispatcherState>,
    workspace_id: String,
    message_id: Option<String>,
) -> Result<Vec<PythonCodeRunRecord>, String> {
    let db = state.db().clone();
    tokio::task::spawn_blocking(move || {
        db.list_python_code_runs(&workspace_id, message_id.as_deref())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("spawn_blocking 失败: {error}"))?
}

#[tauri::command]
pub async fn python_runner_clear_result(
    state: State<'_, DispatcherState>,
    workspace_id: String,
    message_id: String,
    code_block_index: u32,
) -> Result<(), String> {
    let db = state.db().clone();
    tokio::task::spawn_blocking(move || {
        db.clear_python_code_run(&workspace_id, &message_id, code_block_index)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("spawn_blocking 失败: {error}"))?
}

#[tauri::command]
pub async fn python_runner_stop(
    state: State<'_, PythonRunnerState>,
    run_id: String,
) -> Result<(), String> {
    let sender = state.active_runs.lock().get(&run_id).cloned();
    if let Some(sender) = sender {
        let _ = sender.send(true);
    }
    Ok(())
}

#[tauri::command]
pub async fn python_runner_start(
    app: AppHandle,
    state: State<'_, PythonRunnerState>,
    dispatcher_state: State<'_, DispatcherState>,
    workspace_id: String,
    message_id: String,
    code_block_index: u32,
    code: String,
) -> Result<PythonCodeRunRecord, String> {
    let db = dispatcher_state.db().clone();
    let root_dir = resolve_home_dir().map_err(|error| error.to_string())?;
    let run_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let record = PythonCodeRunRecord {
        run_id: run_id.clone(),
        workspace_id: workspace_id.clone(),
        message_id: message_id.clone(),
        code_block_index,
        code_hash: code_hash(&code),
        code: code.clone(),
        status: "running".to_string(),
        stdout: String::new(),
        stderr: String::new(),
        installed_packages_json: "[]".to_string(),
        tool_events_json: "[]".to_string(),
        explanation_markdown: String::new(),
        error_reason: None,
        created_at: now.clone(),
        updated_at: now,
    };
    {
        let db = db.clone();
        let record = record.clone();
        tokio::task::spawn_blocking(move || db.upsert_python_code_run(&record))
            .await
            .map_err(|error| format!("spawn_blocking 失败: {error}"))?
            .map_err(|error| error.to_string())?;
    }

    let (stop_tx, stop_rx) = watch::channel(false);
    state.active_runs.lock().insert(run_id.clone(), stop_tx);

    emit_run_event(
        &app,
        &record,
        "started",
        json!({ "message": "Python 执行已启动" }),
    );

    let app_for_task = app.clone();
    let state_for_task = Arc::clone(&state.inner().active_runs);
    let task_record = record.clone();
    tokio::spawn(async move {
        let final_record =
            run_python_agent(db, root_dir, task_record, stop_rx, app_for_task.clone()).await;
        state_for_task.lock().remove(&run_id);
        if let Err(error) = final_record {
            eprintln!("python runner failed: {error:#}");
        }
    });

    Ok(record)
}

async fn run_python_agent(
    db: DispatcherDb,
    root_dir: PathBuf,
    mut record: PythonCodeRunRecord,
    stop_rx: watch::Receiver<bool>,
    app: AppHandle,
) -> Result<()> {
    let result = run_python_agent_inner(&db, &root_dir, &mut record, stop_rx.clone(), &app).await;
    match result {
        Ok(()) => {}
        Err(error) => {
            record.status = if cancellation_requested(&stop_rx) {
                "stopped".to_string()
            } else {
                "failed".to_string()
            };
            record.error_reason = Some(error.to_string());
            record.updated_at = Utc::now().to_rfc3339();
            upsert_run_record(&db, &record).await?;
            emit_run_event(
                &app,
                &record,
                if record.status == "stopped" {
                    "stopped"
                } else {
                    "failed"
                },
                json!({ "error": error.to_string() }),
            );
        }
    }
    Ok(())
}

async fn run_python_agent_inner(
    db: &DispatcherDb,
    root_dir: &Path,
    record: &mut PythonCodeRunRecord,
    mut stop_rx: watch::Receiver<bool>,
    app: &AppHandle,
) -> Result<()> {
    // DB 读取与目录创建是同步阻塞操作，统一放进 spawn_blocking，
    // 不在 tokio 任务线程上直接执行（项目规范：重操作不阻塞异步运行时）。
    let (provider, message_context, paths) = {
        let db = db.clone();
        let root_dir = root_dir.to_path_buf();
        let workspace_id = record.workspace_id.clone();
        let message_id = record.message_id.clone();
        let run_id = record.run_id.clone();
        let code = record.code.clone();
        tokio::task::spawn_blocking(move || -> Result<_> {
            let provider = resolve_summary_provider(&db)?;
            let message_context = db
                .get_visible_message_content(&workspace_id, &message_id)?
                .unwrap_or(code);
            let paths = prepare_paths(&root_dir, &run_id)?;
            Ok((provider, message_context, paths))
        })
        .await
        .map_err(|error| anyhow!("spawn_blocking 失败: {error}"))??
    };
    ensure_uv_available(&mut stop_rx).await?;
    ensure_venv(&paths, &mut stop_rx).await?;
    tokio::fs::write(&paths.main_py, &record.code)
        .await
        .with_context(|| format!("write {}", paths.main_py.display()))?;

    let event_ctx = PythonRunEventCtx {
        run_id: record.run_id.clone(),
        workspace_id: record.workspace_id.clone(),
        message_id: record.message_id.clone(),
        code_block_index: record.code_block_index,
    };
    let first = run_python_file_streaming(&paths, &mut stop_rx, app, &event_ctx).await?;
    apply_python_output(record, &first);
    persist_and_emit(
        db,
        app,
        record,
        "output",
        json!({ "stdout": record.stdout.clone(), "stderr": record.stderr.clone() }),
    )
    .await?;

    if first.cancelled {
        mark_stopped(db, app, record).await?;
        return Ok(());
    }

    let mut installed_packages = Vec::<String>::new();
    let mut tool_events = Vec::<PythonRunToolEvent>::new();
    if command_succeeded(&first) {
        record.explanation_markdown = explain_result(&provider, record, None).await?;
        record.status = "done".to_string();
        record.updated_at = Utc::now().to_rfc3339();
        upsert_run_record(db, record).await?;
        emit_run_event(app, record, "final", json!({ "record": record.clone() }));
        return Ok(());
    }

    let tools = python_tool_definitions();
    let mut messages = vec![
        ChatMessage::system(build_python_agent_system_prompt()),
        ChatMessage {
            role: "user".to_string(),
            content: build_initial_agent_user_prompt(record, &message_context),
            content_parts: Vec::new(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        },
    ];

    for _ in 0..MAX_AGENT_ITERATIONS {
        if cancellation_requested(&stop_rx) {
            mark_stopped(db, app, record).await?;
            return Ok(());
        }

        let response = provider
            .chat_stream(&messages, &tools, false, |_| {})
            .await
            .context("调用 Python 教学 agent 失败")?;

        if response.tool_calls.is_empty() {
            record.status = "failed".to_string();
            record.explanation_markdown = response.content.trim().to_string();
            if record.explanation_markdown.is_empty() {
                record.explanation_markdown = explain_result(
                    &provider,
                    record,
                    Some("代码执行失败，请解释错误原因和修复建议。"),
                )
                .await?;
            }
            record.error_reason = Some(first_non_empty(&record.stderr, "代码执行失败"));
            record.updated_at = Utc::now().to_rfc3339();
            upsert_run_record(db, record).await?;
            emit_run_event(app, record, "final", json!({ "record": record.clone() }));
            return Ok(());
        }

        let outbound_calls = response
            .tool_calls
            .iter()
            .map(|call| OutboundToolCall {
                id: call.id.clone(),
                kind: "function".to_string(),
                function: crate::agent::llm::FunctionCall {
                    name: call.name.clone(),
                    arguments: call.arguments.to_string(),
                },
            })
            .collect::<Vec<_>>();
        messages.push(ChatMessage {
            role: "assistant".to_string(),
            content: response.content,
            content_parts: Vec::new(),
            reasoning_content: (!response.thinking_content.trim().is_empty())
                .then_some(response.thinking_content),
            tool_calls: Some(outbound_calls),
            tool_call_id: None,
            name: None,
        });

        for tool_call in response.tool_calls {
            emit_run_event(
                app,
                record,
                "toolStarted",
                json!({ "name": tool_call.name.clone(), "arguments": tool_call.arguments.clone() }),
            );
            let tool_result = execute_python_tool(
                &paths,
                &tool_call.name,
                &tool_call.arguments,
                record,
                &mut installed_packages,
                &mut stop_rx,
                app,
            )
            .await;
            let result_text = match tool_result {
                Ok(text) => text,
                Err(error) => format!("工具执行失败：{error}"),
            };

            tool_events.push(PythonRunToolEvent {
                kind: "finished".to_string(),
                name: tool_call.name.clone(),
                detail: truncate_for_display(&result_text, 2000, "\n...[工具结果已截断]"),
                created_at: Utc::now().to_rfc3339(),
            });
            record.installed_packages_json = serde_json::to_string(&installed_packages)?;
            record.tool_events_json = serde_json::to_string(&tool_events)?;
            record.updated_at = Utc::now().to_rfc3339();
            upsert_run_record(db, record).await?;
            emit_run_event(
                app,
                record,
                "toolFinished",
                json!({ "name": tool_call.name.clone(), "result": result_text, "record": record.clone() }),
            );

            messages.push(ChatMessage {
                role: "tool".to_string(),
                content: result_text,
                content_parts: Vec::new(),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: Some(tool_call.id),
                name: Some(tool_call.name),
            });

            if record.status == "stopped" {
                mark_stopped(db, app, record).await?;
                return Ok(());
            }
            if record.status == "done" {
                record.explanation_markdown = explain_result(&provider, record, None).await?;
                record.updated_at = Utc::now().to_rfc3339();
                upsert_run_record(db, record).await?;
                emit_run_event(app, record, "final", json!({ "record": record.clone() }));
                return Ok(());
            }
        }
    }

    record.status = "failed".to_string();
    record.error_reason = Some("Python 教学 agent 达到最大工具迭代次数".to_string());
    record.explanation_markdown = explain_result(
        &provider,
        record,
        Some("自动修复依赖后仍未完成，请解释当前错误和下一步建议。"),
    )
    .await?;
    record.updated_at = Utc::now().to_rfc3339();
    upsert_run_record(db, record).await?;
    emit_run_event(app, record, "final", json!({ "record": record.clone() }));
    Ok(())
}

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

fn prepare_paths(root_dir: &Path, run_id: &str) -> Result<RunPaths> {
    let root = root_dir.join("python-runner");
    let venv = root.join("venv");
    let run_dir = root.join("runs").join(run_id);
    std::fs::create_dir_all(&run_dir).with_context(|| format!("create {}", run_dir.display()))?;
    Ok(RunPaths {
        root,
        venv,
        main_py: run_dir.join("main.py"),
        run_dir,
    })
}

async fn ensure_uv_available(stop_rx: &mut watch::Receiver<bool>) -> Result<()> {
    let output = run_command("uv", &["--version"], None, Duration::from_secs(10), stop_rx).await?;
    if command_succeeded(&output) {
        Ok(())
    } else {
        Err(anyhow!(
            "未找到可用的 uv。请先安装 uv 后再运行 Python 代码。"
        ))
    }
}

async fn ensure_venv(paths: &RunPaths, stop_rx: &mut watch::Receiver<bool>) -> Result<()> {
    if venv_python(&paths.venv).exists() {
        return Ok(());
    }
    tokio::fs::create_dir_all(&paths.root).await?;
    let venv_arg = paths.venv.to_string_lossy().to_string();
    let output = run_command(
        "uv",
        &["venv", &venv_arg],
        Some(&paths.root),
        Duration::from_secs(INSTALL_TIMEOUT_SECS),
        stop_rx,
    )
    .await?;
    if command_succeeded(&output) {
        Ok(())
    } else {
        Err(anyhow!(
            "创建 Python 虚拟环境失败：{}",
            format_command_result(&output)
        ))
    }
}

/// Run the Python file and stream stdout line-by-line via events.
async fn run_python_file_streaming(
    paths: &RunPaths,
    stop_rx: &mut watch::Receiver<bool>,
    app: &AppHandle,
    event_ctx: &PythonRunEventCtx,
) -> Result<CommandOutput> {
    let python = venv_python(&paths.venv).to_string_lossy().to_string();
    let main = paths.main_py.to_string_lossy().to_string();

    let mut command = Command::new(&python);
    command
        .arg(&*main)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(&paths.run_dir);

    let mut child = command
        .spawn()
        .with_context(|| format!("启动 Python 进程失败：{}", python))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Stream stdout line-by-line, collecting into full output
    let app_clone = app.clone();
    let evt = event_ctx.clone();
    let stdout_task = tokio::spawn(async move {
        let Some(reader) = stdout else {
            return Ok(String::new());
        };
        let mut lines = BufReader::new(reader).lines();
        let mut full = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            full.push_str(&line);
            full.push('\n');
            let _ = app_clone.emit(
                "python-run-event",
                PythonRunEvent {
                    event: "output".to_string(),
                    run_id: evt.run_id.clone(),
                    workspace_id: evt.workspace_id.clone(),
                    message_id: evt.message_id.clone(),
                    code_block_index: evt.code_block_index,
                    data: json!({ "stdout": format!("{}\n", line) }),
                },
            );
        }
        Ok::<String, anyhow::Error>(full)
    });

    // stderr collected all at once (no streaming needed for errors)
    let stderr_task = tokio::spawn(async move { read_limited(stderr).await });

    let (status_code, timed_out, cancelled) = tokio::select! {
        _ = wait_for_cancellation(stop_rx) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            (None, false, true)
        }
        result = timeout(Duration::from_secs(RUN_TIMEOUT_SECS), child.wait()) => {
            match result {
                Ok(status) => {
                    let status = status.context("等待 Python 进程退出失败")?;
                    (status.code(), false, false)
                }
                Err(_) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    (None, true, false)
                }
            }
        }
    };

    let stdout_text = stdout_task.await.context("读取 stdout 任务失败")??;
    let stderr_text = stderr_task.await.context("读取 stderr 任务失败")??;

    Ok(CommandOutput {
        status_code,
        stdout: truncate_for_display(&stdout_text, MAX_OUTPUT_CHARS, "\n...[输出已截断]"),
        stderr: stderr_text,
        timed_out,
        cancelled,
    })
}

/// Lightweight context for emitting run events from spawned tasks.
#[derive(Clone)]
struct PythonRunEventCtx {
    run_id: String,
    workspace_id: String,
    message_id: String,
    code_block_index: u32,
}

async fn install_packages(
    paths: &RunPaths,
    packages: &[String],
    stop_rx: &mut watch::Receiver<bool>,
) -> Result<CommandOutput> {
    let python = venv_python(&paths.venv).to_string_lossy().to_string();
    let mut args = vec![
        "pip".to_string(),
        "install".to_string(),
        "--python".to_string(),
        python,
    ];
    args.extend(packages.iter().cloned());
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    run_command(
        "uv",
        &refs,
        Some(&paths.root),
        Duration::from_secs(INSTALL_TIMEOUT_SECS),
        stop_rx,
    )
    .await
}

async fn run_command(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    duration: Duration,
    stop_rx: &mut watch::Receiver<bool>,
) -> Result<CommandOutput> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("启动命令失败：{program}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_task = tokio::spawn(async move { read_limited(stdout).await });
    let stderr_task = tokio::spawn(async move { read_limited(stderr).await });

    let (status_code, timed_out, cancelled) = tokio::select! {
        _ = wait_for_cancellation(stop_rx) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            (None, false, true)
        }
        result = timeout(duration, child.wait()) => {
            match result {
                Ok(status) => {
                    let status = status.context("等待命令退出失败")?;
                    (status.code(), false, false)
                }
                Err(_) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    (None, true, false)
                }
            }
        }
    };

    let stdout = stdout_task.await.context("读取 stdout 任务失败")??;
    let stderr = stderr_task.await.context("读取 stderr 任务失败")??;
    Ok(CommandOutput {
        status_code,
        stdout,
        stderr,
        timed_out,
        cancelled,
    })
}

async fn read_limited<R>(reader: Option<R>) -> Result<String>
where
    R: AsyncRead + Unpin,
{
    let Some(mut reader) = reader else {
        return Ok(String::new());
    };
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await?;
    let text = String::from_utf8_lossy(&bytes).to_string();
    Ok(truncate_for_display(
        &text,
        MAX_OUTPUT_CHARS,
        "\n...[输出已截断]",
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

fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    }
}

fn code_hash(code: &str) -> String {
    let digest = Sha256::digest(code.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn cancellation_requested(stop_rx: &watch::Receiver<bool>) -> bool {
    *stop_rx.borrow()
}

async fn wait_for_cancellation(stop_rx: &mut watch::Receiver<bool>) {
    if *stop_rx.borrow() {
        return;
    }
    let _ = stop_rx.changed().await;
}
