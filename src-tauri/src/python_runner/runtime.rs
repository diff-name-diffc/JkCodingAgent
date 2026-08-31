use super::*;

pub(super) fn prepare_paths(root_dir: &Path, run_id: &str) -> Result<RunPaths> {
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

pub(super) async fn ensure_uv_available(stop_rx: &mut watch::Receiver<bool>) -> Result<()> {
    let output = run_command("uv", &["--version"], None, Duration::from_secs(10), stop_rx).await?;
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
        Some(&paths.root),
        Duration::from_secs(INSTALL_TIMEOUT_SECS),
        stop_rx,
    )
    .await
}

pub(super) async fn run_command(
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

pub(super) async fn wait_for_cancellation(stop_rx: &mut watch::Receiver<bool>) {
    if *stop_rx.borrow() {
        return;
    }
    let _ = stop_rx.changed().await;
}
