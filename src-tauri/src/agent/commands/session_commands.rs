use super::*;

// ── v6: Chat Sessions (paginated) ─────────────────────────────

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

#[tauri::command]
pub async fn chat_create_session(
    state: tauri::State<'_, DispatcherState>,
    app: AppHandle,
    title: String,
    category: Option<String>,
) -> Result<ChatSessionRecord, String> {
    let db = state.db().clone();
    let session = run_dispatcher_db("chat_create_session", move || {
        db.create_chat_session(&title, category.as_deref())
    })
    .await?;
    let _ = app.emit("dispatcher-session-updated", session.clone());
    Ok(session)
}

#[tauri::command]
pub async fn chat_delete_session(
    state: tauri::State<'_, DispatcherState>,
    session_id: String,
) -> Result<(), String> {
    let db = state.db().clone();
    run_dispatcher_db("chat_delete_session", move || {
        db.delete_chat_session(&session_id)
    })
    .await
}
#[tauri::command]
pub async fn chat_set_session_category_v6(
    state: tauri::State<'_, DispatcherState>,
    session_id: String,
    category_id: String,
) -> Result<(), String> {
    let db = state.db().clone();
    run_dispatcher_db("chat_set_session_category_v6", move || {
        db.set_chat_session_category(&session_id, &category_id)
    })
    .await
}

// ── v6: Project Sessions (paginated) ──────────────────────────

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
pub async fn project_create_session(
    state: tauri::State<'_, DispatcherState>,
    app: AppHandle,
    project_id: String,
    title: String,
) -> Result<ProjectSessionRecord, String> {
    let db = state.db().clone();
    let session = run_dispatcher_db("project_create_session", move || {
        db.create_project_session(&project_id, &title)
    })
    .await?;
    let _ = app.emit("dispatcher-session-updated", session.clone());
    Ok(session)
}

#[tauri::command]
pub async fn project_delete_session(
    state: tauri::State<'_, DispatcherState>,
    session_id: String,
) -> Result<(), String> {
    let db = state.db().clone();
    run_dispatcher_db("project_delete_session", move || {
        db.delete_project_session(&session_id)
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
