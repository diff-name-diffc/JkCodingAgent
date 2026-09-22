use super::*;

pub(super) async fn run_python_agent(
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
    let (spec, message_context, paths) = {
        let db = db.clone();
        let root_dir = root_dir.to_path_buf();
        let workspace_id = record.workspace_id.clone();
        let message_id = record.message_id.clone();
        let run_id = record.run_id.clone();
        let code = record.code.clone();
        tokio::task::spawn_blocking(move || -> Result<_> {
            let spec = resolve_summary_spec(&db)?;
            let message_context = db
                .get_visible_message_content(&workspace_id, &message_id)?
                .unwrap_or(code);
            let paths = prepare_paths(&root_dir, &run_id)?;
            Ok((spec, message_context, paths))
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
        record.explanation_markdown = explain_result(&spec, record, None).await?;
        record.status = "done".to_string();
        record.updated_at = Utc::now().to_rfc3339();
        upsert_run_record(db, record).await?;
        emit_run_event(app, record, "final", json!({ "record": record.clone() }));
        return Ok(());
    }

    let tool_definitions = rig_tool_definitions();
    let model = match completions_model(&spec) {
        Ok(model) => model,
        Err(error) => {
            record.status = "failed".to_string();
            record.error_reason = Some(format!("初始化 Python 教学 agent 模型失败：{error}"));
            record.explanation_markdown = explain_result(
                &spec,
                record,
                Some("代码执行失败，请解释错误原因和修复建议。"),
            )
            .await?;
            record.updated_at = Utc::now().to_rfc3339();
            upsert_run_record(db, record).await?;
            emit_run_event(app, record, "final", json!({ "record": record.clone() }));
            return Ok(());
        }
    };
    let mut messages = vec![
        Message::user(build_initial_agent_user_prompt(record, &message_context)),
    ];

    for _ in 0..MAX_AGENT_ITERATIONS {
        if cancellation_requested(&stop_rx) {
            mark_stopped(db, app, record).await?;
            return Ok(());
        }

        let request = build_completion_request(
            Some(build_python_agent_system_prompt()),
            messages.clone(),
            tool_definitions.clone(),
            spec.max_tokens,
            spec.temperature,
            spec.enable_thinking,
        );
        let response = model
            .completion(request)
            .await
            .context("调用 Python 教学 agent 失败")?;
        let (response_text, response_tool_calls) = split_python_response(&response.choice);

        if response_tool_calls.is_empty() {
            record.status = "failed".to_string();
            record.explanation_markdown = response_text.trim().to_string();
            if record.explanation_markdown.is_empty() {
                record.explanation_markdown = explain_result(
                    &spec,
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

        messages.push(python_assistant_turn(&response_text, &response_tool_calls));

        for tool_call in response_tool_calls {
            let tool_name = tool_call.function.name.clone();
            let tool_arguments = tool_call.function.arguments.clone();
            emit_run_event(
                app,
                record,
                "toolStarted",
                json!({ "name": tool_name.clone(), "arguments": tool_arguments.clone() }),
            );
            let tool_result = execute_python_tool(
                &paths,
                &tool_name,
                &tool_arguments,
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
                name: tool_name.clone(),
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
                json!({ "name": tool_name.clone(), "result": result_text.clone(), "record": record.clone() }),
            );

            messages.push(python_tool_result_turn(&tool_call, result_text));

            if record.status == "stopped" {
                mark_stopped(db, app, record).await?;
                return Ok(());
            }
            if record.status == "done" {
                record.explanation_markdown = explain_result(&spec, record, None).await?;
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
        &spec,
        record,
        Some("自动修复依赖后仍未完成，请解释当前错误和下一步建议。"),
    )
    .await?;
    record.updated_at = Utc::now().to_rfc3339();
    upsert_run_record(db, record).await?;
    emit_run_event(app, record, "final", json!({ "record": record.clone() }));
    Ok(())
}

/// 旧 `ToolDefinition`（kind/function）→ rig 工具定义。
fn rig_tool_definitions() -> Vec<rig::completion::ToolDefinition> {
    python_tool_definitions()
        .into_iter()
        .map(|definition| rig::completion::ToolDefinition {
            name: definition.function.name,
            description: definition.function.description,
            parameters: definition.function.parameters,
        })
        .collect()
}

/// 拆分模型回复：可见正文 + 工具调用。
fn split_python_response(
    choice: &[rig::message::AssistantContent],
) -> (String, Vec<rig::message::ToolCall>) {
    let mut text = String::new();
    let mut calls = Vec::new();
    for item in choice {
        match item {
            rig::message::AssistantContent::Text(content) => text.push_str(&content.text),
            rig::message::AssistantContent::ToolCall(call) => calls.push(call.clone()),
            _ => {}
        }
    }
    (text, calls)
}

/// 组装追加进历史的 assistant 消息（正文 + 工具调用）。
fn python_assistant_turn(
    text: &str,
    calls: &[rig::message::ToolCall],
) -> Message {
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(rig::message::AssistantContent::text(text.to_string()));
    }
    for call in calls {
        content.push(rig::message::AssistantContent::ToolCall(call.clone()));
    }
    Message::Assistant { id: None, content }
}

/// 组装工具结果消息（回灌给模型）。
fn python_tool_result_turn(
    call: &rig::message::ToolCall,
    result: String,
) -> Message {
    Message::User {
        content: vec![rig::message::UserContent::ToolResult(
            rig::message::ToolResult {
                call: call.id.clone(),
                provider: call.provider.clone(),
                name: call.function.name.clone(),
                content: vec![rig::message::ToolResultContent::text(result)],
            },
        )],
    }
}
