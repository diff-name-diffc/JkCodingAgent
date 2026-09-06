use super::*;

// ── Chat Sessions (paginated) ─────────────────────────────

#[tauri::command]
pub async fn chat_list_sessions(
    state: tauri::State<'_, DispatcherState>,
    category: Option<String>,
    cursor: Option<String>,
    page_size: Option<i64>,
) -> Result<SessionPage<ChatSessionRecord>, String> {
    let db = state.db().clone();
    let size = page_size.unwrap_or(30).clamp(1, 100);
    run_dispatcher_db("chat_list_sessions", move || {
        db.list_chat_sessions_paginated(category.as_deref(), cursor.as_deref(), size)
    })
    .await
}

/// 统一建会话入口（原 chat_create_session + project_create_session 合并）。
/// kind=chat 时 category 缺省由 DB 层统一为 "tech"；kind=project 时必须
/// 传 project_id。返回载荷按 kind 序列化为对应子表记录形态。
#[tauri::command]
pub async fn session_create(
    state: tauri::State<'_, DispatcherState>,
    app: AppHandle,
    kind: DispatcherSessionKind,
    title: String,
    category: Option<String>,
    project_id: Option<String>,
) -> Result<SessionCreatedRecord, String> {
    let db = state.db().clone();
    let session = run_dispatcher_db("session_create", move || {
        db.create_session(
            kind,
            &title,
            category.as_deref(),
            project_id.as_deref(),
        )
    })
    .await?;
    let _ = app.emit("dispatcher-session-updated", session.clone());
    Ok(session)
}

/// 统一删会话入口（原 chat_delete_session + project_delete_session 合并）：
/// 查统一表 kind 分流子表删除，级联清理走共享 purge helper。
#[tauri::command]
pub async fn session_delete(
    state: tauri::State<'_, DispatcherState>,
    session_id: String,
) -> Result<(), String> {
    let db = state.db().clone();
    let session_for_cleanup = session_id.clone();
    let result =
        run_dispatcher_db("session_delete", move || db.delete_session(&session_id)).await;
    if result.is_ok() {
        // 会话资源清理规范：会话级内存状态（命令执行台账）同步回收。
        crate::agent::command_history::forget_session(&session_for_cleanup);
    }
    result
}

#[tauri::command]
pub async fn chat_set_session_category(
    state: tauri::State<'_, DispatcherState>,
    session_id: String,
    category_id: String,
) -> Result<(), String> {
    let db = state.db().clone();
    run_dispatcher_db("chat_set_session_category", move || {
        db.set_chat_session_category(&session_id, &category_id)
    })
    .await
}

// ── Project Sessions (paginated) ──────────────────────────

#[tauri::command]
pub async fn project_list_sessions(
    state: tauri::State<'_, DispatcherState>,
    project_id: String,
    offset: Option<i64>,
    page_size: Option<i64>,
) -> Result<SessionPage<ProjectSessionRecord>, String> {
    let db = state.db().clone();
    let off = offset.unwrap_or(0).max(0);
    let size = page_size.unwrap_or(30).clamp(1, 100);
    run_dispatcher_db("project_list_sessions", move || {
        db.list_project_sessions_paginated(&project_id, off, size)
    })
    .await
}

#[tauri::command]
pub async fn session_search_keywords(
    state: tauri::State<'_, DispatcherState>,
    query: String,
    kind: DispatcherSessionKind,
    project_id: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<SessionSearchResult>, String> {
    let db = state.db().clone();
    run_dispatcher_db("session_search_keywords", move || {
        db.search_sessions(&query, kind, project_id.as_deref(), limit.unwrap_or(20))
    })
    .await
}
