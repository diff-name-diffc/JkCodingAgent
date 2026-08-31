use super::*;

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
