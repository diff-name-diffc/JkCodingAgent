use super::*;

fn run_paths(root_dir: &Path, run_id: &str, chat_workspace: Option<&ChatWorkspace>) -> RunPaths {
    let root = root_dir.join("python-runner");
    let venv = root.join("venv");
    let run_dir = match chat_workspace {
        Some(workspace) => workspace.session_root.join("python-runs").join(run_id),
        None => root.join("runs").join(run_id),
    };
    RunPaths {
        root,
        venv,
        main_py: run_dir.join("main.py"),
        temp_dir: chat_workspace.map(|_| run_dir.join("tmp")),
        run_dir,
    }
}

pub(super) fn prepare_paths(
    root_dir: &Path,
    run_id: &str,
    chat_workspace: Option<&ChatWorkspace>,
) -> Result<RunPaths> {
    let mut paths = run_paths(root_dir, run_id, chat_workspace);
    if let Some(workspace) = chat_workspace {
        // 生产调用在取得会话写租约并重查目标消息后到达这里。
        std::fs::create_dir_all(&workspace.session_root).with_context(|| {
            format!(
                "create Python session directory {}",
                workspace.session_root.display()
            )
        })?;
        // 已有目录/文件可能是符号链接；临时运行文件只能解析到会话内。
        let resolve = |path: &Path| {
            crate::agent::rig_ext::tools::common::resolve_path(
                &workspace.session_root,
                true,
                &[],
                &path.to_string_lossy(),
            )
            .map_err(anyhow::Error::msg)
        };
        paths.run_dir = resolve(&paths.run_dir)?;
        paths.main_py = resolve(&paths.main_py)?;
        paths.temp_dir = paths.temp_dir.as_deref().map(resolve).transpose()?;
    }
    std::fs::create_dir_all(&paths.run_dir)
        .with_context(|| format!("create {}", paths.run_dir.display()))?;
    if let Some(temp_dir) = &paths.temp_dir {
        std::fs::create_dir_all(temp_dir)
            .with_context(|| format!("create {}", temp_dir.display()))?;
    }
    Ok(paths)
}

pub(super) async fn ensure_uv_available(
    paths: &RunPaths,
    stop_rx: &mut watch::Receiver<bool>,
) -> Result<()> {
    let output = run_command(
        "uv",
        &["--version"],
        None,
        paths.temp_dir.as_deref(),
        Duration::from_secs(10),
        stop_rx,
    )
    .await?;
    if command_succeeded(&output) {
        Ok(())
    } else {
        Err(anyhow!(
            "未找到可用的 uv。请先安装 uv 后再运行 Python 代码。"
        ))
    }
}

pub(super) async fn ensure_venv(
    paths: &RunPaths,
    stop_rx: &mut watch::Receiver<bool>,
) -> Result<()> {
    if venv_python(&paths.venv).exists() {
        return Ok(());
    }
    tokio::fs::create_dir_all(&paths.root).await?;
    let venv_arg = paths.venv.to_string_lossy().to_string();
    let output = run_command(
        "uv",
        &["venv", &venv_arg],
        Some(&paths.root),
        paths.temp_dir.as_deref(),
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
pub(super) async fn run_python_file_streaming(
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
    configure_temp_environment(&mut command, paths.temp_dir.as_deref());
    if paths.temp_dir.is_some() {
        command.env("PYTHONPYCACHEPREFIX", paths.run_dir.join("pycache"));
    }

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
        _ = crate::shared::cancel::wait_for_cancel(Some(stop_rx.clone())) => {
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
pub(super) struct PythonRunEventCtx {
    pub(super) run_id: String,
    pub(super) workspace_id: String,
    pub(super) message_id: String,
    pub(super) code_block_index: u32,
}

pub(super) async fn install_packages(
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
        // 安装目标经 --python 显式指定；cwd 固定在应用托管目录：run_dir 是
        // 脚本可写目录，uv 会从 cwd 向上发现 pyproject.toml/uv.toml（如
        // index-url），不能让脚本生成的配置被依赖安装继承。
        Some(&paths.root),
        paths.temp_dir.as_deref(),
        Duration::from_secs(INSTALL_TIMEOUT_SECS),
        stop_rx,
    )
    .await
}

pub(super) async fn run_command(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    temp_dir: Option<&Path>,
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
    configure_temp_environment(&mut command, temp_dir);
    let mut child = command
        .spawn()
        .with_context(|| format!("启动命令失败：{program}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_task = tokio::spawn(async move { read_limited(stdout).await });
    let stderr_task = tokio::spawn(async move { read_limited(stderr).await });

    let (status_code, timed_out, cancelled) = tokio::select! {
        _ = crate::shared::cancel::wait_for_cancel(Some(stop_rx.clone())) => {
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

fn configure_temp_environment(command: &mut Command, temp_dir: Option<&Path>) {
    if let Some(temp_dir) = temp_dir {
        for name in ["TMPDIR", "TMP", "TEMP"] {
            command.env(name, temp_dir);
        }
    }
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

pub(super) fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    }
}

pub(super) fn code_hash(code: &str) -> String {
    let digest = Sha256::digest(code.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn cancellation_requested(stop_rx: &watch::Receiver<bool>) -> bool {
    *stop_rx.borrow()
}

#[cfg(test)]
mod tests;
