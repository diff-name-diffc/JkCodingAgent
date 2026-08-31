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
