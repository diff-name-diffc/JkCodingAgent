use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::Utc;
use tauri::ipc::Channel;

use super::super::db::{
    ChecklistPlanItem, ChecklistPlanState, ChecklistStepStatus, DispatcherDb, DispatcherMode,
    DispatcherSessionRuntimeState, PlanInteraction, PlanQuestionOption,
};
use super::super::tools::{
    parse_ask_plan_question, parse_create_plan_document, parse_edit_plan_document,
    parse_present_plan, parse_replace_plan_document, parse_update_plan, UpdatePlanDraft,
};
use super::helpers::{
    is_implemented_plan_path, lexical_normalize_path, slugify_plan_title, string_arg_required,
};
use super::types::AgentEvent;

// ─── Internal types ───────────────────────────────────────────────────────────

#[derive(Debug)]
pub(super) enum PlanningToolOutcome {
    ToolResult(String),
    WaitForUser(String),
}

// ─── Mode guard ───────────────────────────────────────────────────────────────

/// 模式守卫：确保计划类工具只在正确的 DispatcherMode（Default/Plan）下使用。
/// 例如 update_plan 仅 Default 模式可用，present_plan 仅 Plan 模式可用。
pub(super) fn ensure_mode(
    actual: DispatcherMode,
    expected: DispatcherMode,
    tool_name: &str,
) -> std::result::Result<(), String> {
    if actual == expected {
        return Ok(());
    }
    let expected = match expected {
        DispatcherMode::Default => "Default",
        DispatcherMode::Plan => "Plan",
    };
    Err(format!("错误：{tool_name} 只能在 {expected} 模式下使用"))
}

// ─── Plan document operations ─────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub(crate) enum PlanPathAccess {
    Read,
    WriteExisting,
}

pub(crate) async fn resolve_plan_path_async(
    workspace: &Path,
    raw_path: &str,
    access: PlanPathAccess,
) -> std::result::Result<PathBuf, String> {
    let workspace = workspace.to_path_buf();
    let raw_path = raw_path.to_string();
    tokio::task::spawn_blocking(move || resolve_plan_path(&workspace, &raw_path, access))
        .await
        .map_err(|error| format!("计划路径解析任务失败：{error}"))?
}

pub(crate) async fn create_plan_document(
    workspace: &Path,
    title: &str,
    content: &str,
) -> std::result::Result<PathBuf, String> {
    let workspace = workspace.to_path_buf();
    let title = title.to_string();
    let content = content.to_string();
    tokio::task::spawn_blocking(move || {
        let root = ensure_plan_root(&workspace)?;
        let filename = format!(
            "{}-{}.md",
            Utc::now().format("%Y%m%d-%H%M%S"),
            slugify_plan_title(&title)
        );
        let path = root.join(filename);
        fs::write(&path, content).map_err(|error| format!("写入计划书失败：{error}"))?;
        Ok(path)
    })
    .await
    .map_err(|error| format!("创建计划书任务失败：{error}"))?
}

pub(crate) async fn read_plan_document(
    workspace: &Path,
    path: &str,
) -> std::result::Result<String, String> {
    let plan_path = resolve_plan_path_async(workspace, path, PlanPathAccess::Read).await?;
    tokio::task::spawn_blocking(move || {
        let mut content = String::new();
        fs::File::open(&plan_path)
            .map_err(|error| format!("打开计划书失败：{error}"))?
            .read_to_string(&mut content)
            .map_err(|error| format!("读取计划书失败：{error}"))?;
        Ok(content)
    })
    .await
    .map_err(|error| format!("读取计划书任务失败：{error}"))?
}

pub(crate) async fn replace_plan_document(
    workspace: &Path,
    path: &str,
    content: &str,
) -> std::result::Result<PathBuf, String> {
    let plan_path = resolve_plan_path_async(workspace, path, PlanPathAccess::WriteExisting).await?;
    let content = content.to_string();
    tokio::task::spawn_blocking(move || {
        fs::write(&plan_path, content).map_err(|error| format!("替换计划书失败：{error}"))?;
        Ok(plan_path)
    })
    .await
    .map_err(|error| format!("替换计划书任务失败：{error}"))?
}

pub(crate) async fn edit_plan_document(
    workspace: &Path,
    path: &str,
    old_text: &str,
    new_text: &str,
    replace_all: bool,
) -> std::result::Result<PathBuf, String> {
    let plan_path = resolve_plan_path_async(workspace, path, PlanPathAccess::WriteExisting).await?;
    let old_text = old_text.to_string();
    let new_text = new_text.to_string();
    tokio::task::spawn_blocking(move || {
        let content =
            fs::read_to_string(&plan_path).map_err(|error| format!("读取计划书失败：{error}"))?;
        if !content.contains(&old_text) {
            return Err("错误：计划书中未找到 old_text".to_string());
        }
        let match_count = content.matches(&old_text).count();
        if match_count > 1 && !replace_all {
            return Err(format!(
                "错误：old_text 命中 {match_count} 处，请补充上下文或设置 replace_all=true"
            ));
        }
        let updated = if replace_all {
            content.replace(&old_text, &new_text)
        } else {
            content.replacen(&old_text, &new_text, 1)
        };
        fs::write(&plan_path, updated).map_err(|error| format!("编辑计划书失败：{error}"))?;
        Ok(plan_path)
    })
    .await
    .map_err(|error| format!("编辑计划书任务失败：{error}"))?
}

pub(crate) async fn mark_plan_implemented(
    workspace: &Path,
    path: &str,
) -> std::result::Result<(PathBuf, PathBuf), String> {
    let plan_path = resolve_plan_path_async(workspace, path, PlanPathAccess::WriteExisting).await?;
    tokio::task::spawn_blocking(move || {
        if is_implemented_plan_path(&plan_path) {
            return Err("错误：该计划书已经带有 -已实现.md 标记".to_string());
        }
        let file_name = plan_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "错误：计划书文件名不是有效 UTF-8".to_string())?;
        let implemented_name = match file_name.strip_suffix(".md") {
            Some(stem) => format!("{stem}-已实现.md"),
            None => format!("{file_name}-已实现.md"),
        };
        let implemented_path = plan_path.with_file_name(implemented_name);
        if implemented_path.exists() {
            return Err(format!(
                "错误：目标已存在，拒绝覆盖：{}",
                implemented_path.display()
            ));
        }
        fs::rename(&plan_path, &implemented_path)
            .map_err(|error| format!("重命名计划书失败：{error}"))?;
        Ok((plan_path, implemented_path))
    })
    .await
    .map_err(|error| format!("标记计划已实现任务失败：{error}"))?
}

fn ensure_plan_root(workspace: &Path) -> std::result::Result<PathBuf, String> {
    let root = workspace.join(".jkcodingagent").join("plan");
    fs::create_dir_all(&root).map_err(|error| format!("创建计划目录失败：{error}"))?;
    root.canonicalize()
        .map_err(|error| format!("解析计划目录失败：{error}"))
}

pub(crate) fn resolve_plan_path(
    workspace: &Path,
    raw_path: &str,
    access: PlanPathAccess,
) -> std::result::Result<PathBuf, String> {
    let root = ensure_plan_root(workspace)?;
    let raw = PathBuf::from(raw_path);
    let candidate = if raw.is_absolute() {
        raw
    } else {
        workspace.join(raw)
    };
    let candidate = lexical_normalize_path(&candidate);
    let resolved = match access {
        PlanPathAccess::Read => candidate
            .canonicalize()
            .map_err(|error| format!("解析计划书路径失败：{error}"))?,
        PlanPathAccess::WriteExisting => {
            if is_implemented_plan_path(&candidate) {
                return Err("错误：禁止修改文件名包含 -已实现.md 的计划书".to_string());
            }
            candidate
                .canonicalize()
                .map_err(|error| format!("解析计划书路径失败：{error}"))?
        }
    };

    if !resolved.starts_with(&root) {
        return Err(format!(
            "错误：计划书路径必须位于项目计划目录内：{}",
            root.display()
        ));
    }
    if matches!(access, PlanPathAccess::WriteExisting) && is_implemented_plan_path(&resolved) {
        return Err("错误：禁止修改文件名包含 -已实现.md 的计划书".to_string());
    }
    Ok(resolved)
}

// ─── Checklist state building ─────────────────────────────────────────────────

pub(super) fn build_checklist_state(
    draft: UpdatePlanDraft,
    previous: Option<&ChecklistPlanState>,
) -> std::result::Result<ChecklistPlanState, String> {
    if draft.items.is_empty() {
        return Err("错误：plan 至少需要包含一个步骤".to_string());
    }

    let mut in_progress_count = 0usize;
    let items = draft
        .items
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let status = ChecklistStepStatus::from_wire(&item.status)
                .map_err(|error| format!("错误：{}", error))?;
            if status == ChecklistStepStatus::InProgress {
                in_progress_count += 1;
            }
            let previous_item = previous.and_then(|state| {
                state.items.iter().find(|candidate| {
                    item.id
                        .as_deref()
                        .is_some_and(|id| candidate.id.as_deref() == Some(id))
                        || candidate.step == item.step
                })
            });
            let agent = match item.agent {
                Some(agent) => {
                    let normalized = agent.trim().to_ascii_lowercase();
                    if !matches!(normalized.as_str(), "claude" | "codex") {
                        return Err(format!("错误：不支持的 checklist agent：{agent}"));
                    }
                    Some(normalized)
                }
                None => previous_item.and_then(|item| item.agent.clone()),
            };
            Ok(ChecklistPlanItem {
                id: item
                    .id
                    .or_else(|| previous_item.and_then(|item| item.id.clone()))
                    .or_else(|| Some(format!("step_{}", index + 1))),
                step: item.step,
                status,
                agent,
                dispatch_id: previous_item.and_then(|item| item.dispatch_id.clone()),
                subprocess_task_id: previous_item.and_then(|item| item.subprocess_task_id.clone()),
                detail: previous_item.and_then(|item| item.detail.clone()),
            })
        })
        .collect::<std::result::Result<Vec<_>, String>>()?;

    if in_progress_count > 1 {
        return Err(format!(
            "错误：同一时间最多只能有 1 个 in_progress 步骤，实际收到 {in_progress_count} 个"
        ));
    }

    Ok(ChecklistPlanState {
        explanation: draft.explanation,
        items,
        updated_at: Utc::now().to_rfc3339(),
    })
}

pub(super) fn empty_checklist_state() -> ChecklistPlanState {
    ChecklistPlanState {
        explanation: None,
        items: Vec::new(),
        updated_at: Utc::now().to_rfc3339(),
    }
}

// ─── Checklist dispatch lifecycle ─────────────────────────────────────────────
// Checklist 与子进程派生的状态机联动：
//   reserve（预留，dispatch 提案时）→ start（子进程真正开始，回流时）→
//   complete（子进程完成，回流时）
// 这套生命周期保证 checklist 步骤状态与实际子进程执行同步。

pub(super) async fn reserve_checklist_dispatch(
    db: &DispatcherDb,
    session_id: &str,
    dispatch_id: &str,
    agent: &str,
    description: &str,
) -> std::result::Result<Option<ChecklistPlanState>, String> {
    let mut state = db
        .get_session_runtime_state_async(session_id)
        .await
        .map_err(|error| error.to_string())?;
    let Some(mut checklist) = state.checklist.take() else {
        return Ok(None);
    };
    if checklist.items.is_empty() {
        return Ok(None);
    }

    if checklist.items.iter().any(|item| {
        item.status == ChecklistStepStatus::InProgress
            && item
                .dispatch_id
                .as_deref()
                .is_some_and(|existing| existing != dispatch_id)
    }) {
        return Err("错误：Checklist 当前已有运行中的子步骤，请等待该子步骤完成后再启动下一个子 Agent 任务。".to_string());
    }

    let item_index = checklist
        .items
        .iter()
        .position(|item| item.status == ChecklistStepStatus::InProgress)
        .or_else(|| {
            checklist.items.iter().position(|item| {
                item.status == ChecklistStepStatus::Pending
                    && item
                        .agent
                        .as_deref()
                        .is_none_or(|preferred| preferred == agent)
            })
        })
        .or_else(|| {
            checklist
                .items
                .iter()
                .position(|item| item.status == ChecklistStepStatus::Pending)
        });

    let Some(index) = item_index else {
        return Ok(None);
    };

    let item = &mut checklist.items[index];
    item.agent = Some(agent.to_string());
    item.dispatch_id = Some(dispatch_id.to_string());
    item.detail = Some(description.to_string());
    checklist.updated_at = Utc::now().to_rfc3339();

    let state = db
        .update_checklist_async(session_id, &checklist)
        .await
        .map_err(|error| error.to_string())?;
    Ok(state.checklist)
}

pub(super) async fn start_checklist_dispatch(
    db: &DispatcherDb,
    session_id: &str,
    dispatch_id: &str,
) -> std::result::Result<Option<ChecklistPlanState>, String> {
    let state = db
        .get_session_runtime_state_async(session_id)
        .await
        .map_err(|error| error.to_string())?;
    let Some(mut checklist) = state.checklist else {
        return Ok(None);
    };

    let Some(item) = checklist
        .items
        .iter_mut()
        .find(|item| item.dispatch_id.as_deref() == Some(dispatch_id))
    else {
        return Ok(None);
    };

    item.status = ChecklistStepStatus::InProgress;
    checklist.updated_at = Utc::now().to_rfc3339();
    let state = db
        .update_checklist_async(session_id, &checklist)
        .await
        .map_err(|error| error.to_string())?;
    Ok(state.checklist)
}

pub(super) async fn complete_checklist_dispatch(
    db: &DispatcherDb,
    session_id: &str,
    dispatch_id: &str,
) -> std::result::Result<Option<ChecklistPlanState>, String> {
    let state = db
        .get_session_runtime_state_async(session_id)
        .await
        .map_err(|error| error.to_string())?;
    let Some(mut checklist) = state.checklist else {
        return Ok(None);
    };

    let Some(item) = checklist
        .items
        .iter_mut()
        .find(|item| item.dispatch_id.as_deref() == Some(dispatch_id))
    else {
        return Ok(None);
    };

    item.status = ChecklistStepStatus::Completed;
    checklist.updated_at = Utc::now().to_rfc3339();
    let state = db
        .update_checklist_async(session_id, &checklist)
        .await
        .map_err(|error| error.to_string())?;
    Ok(state.checklist)
}

// ─── Planning tool execution (impl on DispatcherAgent) ────────────────────────

use super::DispatcherAgent;

    impl DispatcherAgent {
    /// 计划类工具统一分派入口（process_single_tool_call 的优先级 1）。
    /// 处理 update_plan / ask_plan_question / 计划文档 CRUD /
    /// present_plan / mark_plan_implemented，返回 ToolResult 或 WaitForUser。
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn execute_planning_tool(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &Path,
        on_event: &Channel<AgentEvent>,
        tool_call: &super::super::llm::RequestedToolCall,
        runtime_state: &DispatcherSessionRuntimeState,
    ) -> Result<Option<PlanningToolOutcome>, String> {
        match tool_call.name.as_str() {
            "update_plan" => {
                ensure_mode(runtime_state.mode, DispatcherMode::Default, "update_plan")?;
                let draft = parse_update_plan(&tool_call.arguments)?;
                let latest_state = db
                    .get_session_runtime_state_async(workspace_id)
                    .await
                    .map_err(|error| error.to_string())?;
                let checklist = build_checklist_state(draft, latest_state.checklist.as_ref())?;
                db.update_checklist_async(workspace_id, &checklist)
                    .await
                    .map_err(|error| error.to_string())?;
                super::helpers::emit(
                    on_event,
                    AgentEvent::ChecklistPlanUpdated {
                        state: checklist.clone(),
                    },
                );
                Ok(Some(PlanningToolOutcome::ToolResult(format!(
                    "Checklist 已更新：{} 个步骤",
                    checklist.items.len()
                ))))
            }
            "ask_plan_question" => {
                ensure_mode(
                    runtime_state.mode,
                    DispatcherMode::Plan,
                    "ask_plan_question",
                )?;
                let draft = parse_ask_plan_question(&tool_call.arguments)?;
                let interaction = PlanInteraction::Question {
                    id: uuid::Uuid::new_v4().to_string(),
                    question: draft.question,
                    options: draft
                        .options
                        .into_iter()
                        .enumerate()
                        .map(|(index, option)| PlanQuestionOption {
                            id: option.id.unwrap_or_else(|| format!("option_{}", index + 1)),
                            label: option.label,
                            description: option.description,
                        })
                        .collect(),
                };
                db.set_plan_interaction_async(workspace_id, Some(&interaction))
                    .await
                    .map_err(|error| error.to_string())?;
                super::helpers::emit(
                    on_event,
                    AgentEvent::PlanQuestionRequested {
                        interaction: interaction.clone(),
                    },
                );
                Ok(Some(PlanningToolOutcome::WaitForUser(
                    "规划信息不足，已向用户提出一个问题。".to_string(),
                )))
            }
            "create_plan_document" => {
                ensure_mode(
                    runtime_state.mode,
                    DispatcherMode::Plan,
                    "create_plan_document",
                )?;
                let (title, content) = parse_create_plan_document(&tool_call.arguments)?;
                let plan_path = create_plan_document(workspace, &title, &content).await?;
                let plan_path = plan_path.to_string_lossy().to_string();
                db.set_active_plan_path_async(workspace_id, Some(&plan_path))
                    .await
                    .map_err(|error| error.to_string())?;
                super::helpers::emit(
                    on_event,
                    AgentEvent::PlanDocumentOpened {
                        plan_path: plan_path.clone(),
                    },
                );
                Ok(Some(PlanningToolOutcome::ToolResult(format!(
                    "计划书已创建：{plan_path}"
                ))))
            }
            "read_plan_document" => {
                ensure_mode(
                    runtime_state.mode,
                    DispatcherMode::Plan,
                    "read_plan_document",
                )?;
                let path = string_arg_required(&tool_call.arguments, "path")?;
                let content = read_plan_document(workspace, &path).await?;
                Ok(Some(PlanningToolOutcome::ToolResult(content)))
            }
            "replace_plan_document" => {
                ensure_mode(
                    runtime_state.mode,
                    DispatcherMode::Plan,
                    "replace_plan_document",
                )?;
                let (path, content) = parse_replace_plan_document(&tool_call.arguments)?;
                let plan_path = replace_plan_document(workspace, &path, &content).await?;
                let plan_path = plan_path.to_string_lossy().to_string();
                db.set_active_plan_path_async(workspace_id, Some(&plan_path))
                    .await
                    .map_err(|error| error.to_string())?;
                super::helpers::emit(
                    on_event,
                    AgentEvent::PlanDocumentOpened {
                        plan_path: plan_path.clone(),
                    },
                );
                Ok(Some(PlanningToolOutcome::ToolResult(format!(
                    "计划书已替换：{plan_path}"
                ))))
            }
            "edit_plan_document" => {
                ensure_mode(
                    runtime_state.mode,
                    DispatcherMode::Plan,
                    "edit_plan_document",
                )?;
                let (path, old_text, new_text, replace_all) =
                    parse_edit_plan_document(&tool_call.arguments)?;
                let plan_path =
                    edit_plan_document(workspace, &path, &old_text, &new_text, replace_all).await?;
                let plan_path = plan_path.to_string_lossy().to_string();
                db.set_active_plan_path_async(workspace_id, Some(&plan_path))
                    .await
                    .map_err(|error| error.to_string())?;
                super::helpers::emit(
                    on_event,
                    AgentEvent::PlanDocumentOpened {
                        plan_path: plan_path.clone(),
                    },
                );
                Ok(Some(PlanningToolOutcome::ToolResult(format!(
                    "计划书已编辑：{plan_path}"
                ))))
            }
            "present_plan" => {
                ensure_mode(runtime_state.mode, DispatcherMode::Plan, "present_plan")?;
                let (path, title, summary) = parse_present_plan(&tool_call.arguments)?;
                let plan_path =
                    resolve_plan_path_async(workspace, &path, PlanPathAccess::Read).await?;
                let plan_path = plan_path.to_string_lossy().to_string();
                let interaction = PlanInteraction::Ready {
                    plan_path: plan_path.clone(),
                    title,
                    summary,
                };
                db.set_active_plan_path_async(workspace_id, Some(&plan_path))
                    .await
                    .map_err(|error| error.to_string())?;
                db.set_plan_interaction_async(workspace_id, Some(&interaction))
                    .await
                    .map_err(|error| error.to_string())?;
                super::helpers::emit(
                    on_event,
                    AgentEvent::PlanReady {
                        interaction: interaction.clone(),
                    },
                );
                Ok(Some(PlanningToolOutcome::WaitForUser(
                    "计划书已完成，等待用户选择实施方式。".to_string(),
                )))
            }
            "mark_plan_implemented" => {
                ensure_mode(
                    runtime_state.mode,
                    DispatcherMode::Default,
                    "mark_plan_implemented",
                )?;
                let path = string_arg_required(&tool_call.arguments, "path")?;
                let summary = string_arg_required(&tool_call.arguments, "summary")?;
                let (original, implemented) = mark_plan_implemented(workspace, &path).await?;
                let original = original.to_string_lossy().to_string();
                let implemented = implemented.to_string_lossy().to_string();
                db.set_active_plan_path_async(workspace_id, Some(&implemented))
                    .await
                    .map_err(|error| error.to_string())?;
                db.set_plan_interaction_async(workspace_id, None)
                    .await
                    .map_err(|error| error.to_string())?;
                super::helpers::emit(
                    on_event,
                    AgentEvent::PlanImplemented {
                        plan_path: original.clone(),
                        implemented_path: implemented.clone(),
                        summary: summary.clone(),
                    },
                );
                Ok(Some(PlanningToolOutcome::ToolResult(format!(
                    "计划已标记为已实现：{implemented}\n实施摘要：{summary}"
                ))))
            }
            _ => Ok(None),
        }
    }
}
