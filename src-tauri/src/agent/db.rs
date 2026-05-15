use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, types::Type, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::config::DEFAULT_SUMMARY_MODEL;
use super::llm::{ChatMessage, LlmUsage, OutboundToolCall};
use super::summary::ToolArtifactDraft;

const MAX_LLM_DIALOGUES: usize = 5;
const MAX_DIALOGUE_QUERY_LIMIT: usize = 50;
const DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS: u64 = 1_000_000;
pub(crate) const TOOL_RETRY_CONTEXT_PREFIX: &str = "[工具调用失败，已交回模型修正重试]";

// ── Content Segments ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ContentSegment {
    Text {
        id: String,
        text: String,
    },
    Image {
        id: String,
        image_id: String,
        path: String,
        alt: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        mime_type: Option<String>,
        source: String,
        generation_prompt: Option<String>,
    },
    File {
        id: String,
        file_id: String,
        path: String,
        file_name: String,
        mime_type: String,
        size: u64,
    },
}

impl ContentSegment {
    pub fn to_markdown(&self) -> String {
        match self {
            ContentSegment::Text { text, .. } => text.clone(),
            ContentSegment::Image { alt, path, .. } => {
                format!("![{}]({})", alt.as_deref().unwrap_or("image"), path)
            }
            ContentSegment::File { .. } => String::new(),
        }
    }
}

pub fn segments_to_markdown(segments: &[ContentSegment]) -> String {
    segments
        .iter()
        .map(|s| s.to_markdown())
        .collect::<Vec<_>>()
        .join("\n")
}

fn content_to_segments_json(content: &str) -> String {
    let segments = vec![ContentSegment::Text {
        id: Uuid::new_v4().to_string(),
        text: content.to_string(),
    }];
    serde_json::to_string(&segments).unwrap_or_else(|_| "[]".to_string())
}

fn parse_segments_json(segments_json: &str) -> Vec<ContentSegment> {
    serde_json::from_str(segments_json).unwrap_or_default()
}

// ── Records ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSessionRecord {
    pub id: String,
    pub project_id: String,
    pub kind: DispatcherSessionKind,
    pub title: String,
    pub mode: DispatcherMode,
    pub active_plan_path: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSettingsRecord {
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    pub summary_model: String,
    pub vision_model: String,
    pub asr_api_key: String,
    pub asr_websocket_url: String,
    pub auto_approve_dispatch: bool,
    pub context_debug: bool,
    pub image_model_url: String,
    pub image_model_api_key: String,
    pub image_model: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DispatcherMode {
    Default,
    Plan,
}

impl DispatcherMode {
    pub fn from_wire(value: &str) -> Result<Self> {
        match value.trim() {
            "" | "default" => Ok(Self::Default),
            "plan" => Ok(Self::Plan),
            other => anyhow::bail!("invalid dispatcher mode: {other}"),
        }
    }

    pub fn as_sql_value(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Plan => "plan",
        }
    }

    fn from_sql_value(value: String) -> Self {
        match value.as_str() {
            "plan" => Self::Plan,
            _ => Self::Default,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DispatcherSessionKind {
    Project,
    Chat,
}

impl DispatcherSessionKind {
    pub fn from_wire(value: &str) -> Result<Self> {
        match value.trim() {
            "" | "project" => Ok(Self::Project),
            "chat" => Ok(Self::Chat),
            other => anyhow::bail!("invalid dispatcher session kind: {other}"),
        }
    }

    pub fn as_sql_value(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Chat => "chat",
        }
    }

    fn from_sql_value(value: String) -> Self {
        match value.as_str() {
            "chat" => Self::Chat,
            _ => Self::Project,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistPlanState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    pub items: Vec<ChecklistPlanItem>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistPlanItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub step: String,
    pub status: ChecklistStepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispatch_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subprocess_task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ChecklistStepStatus {
    Pending,
    InProgress,
    Completed,
}

impl ChecklistStepStatus {
    pub fn from_wire(value: &str) -> Result<Self> {
        match value.trim() {
            "pending" => Ok(Self::Pending),
            "in_progress" => Ok(Self::InProgress),
            "completed" => Ok(Self::Completed),
            other => anyhow::bail!("invalid checklist step status: {other}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanQuestionOption {
    pub id: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PlanInteraction {
    Question {
        id: String,
        question: String,
        options: Vec<PlanQuestionOption>,
    },
    Ready {
        plan_path: String,
        title: String,
        summary: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSessionRuntimeState {
    pub mode: DispatcherMode,
    pub checklist: Option<ChecklistPlanState>,
    pub plan_interaction: Option<PlanInteraction>,
    pub active_plan_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherMessageUsageStats {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherMessageRecord {
    pub id: String,
    pub workspace_id: String,
    pub role: String,
    pub content: String,
    pub segments_json: String,
    pub thinking_content: Option<String>,
    pub thinking_elapsed_ms: Option<u64>,
    #[serde(skip_serializing)]
    pub context_payload: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_result_mode: Option<String>,
    pub tool_artifacts: Vec<DispatcherToolArtifactRef>,
    pub tool_calls_json: Option<String>,
    pub usage_stats: Option<DispatcherMessageUsageStats>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherToolArtifactRef {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub preview: String,
    pub char_count: usize,
    pub line_count: usize,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherToolArtifactRecord {
    pub id: String,
    pub workspace_id: String,
    pub message_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub title: String,
    pub kind: String,
    pub preview: String,
    pub content: String,
    pub char_count: usize,
    pub line_count: usize,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DispatcherSessionTokenUsageSource {
    Primary,
    Summary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSessionTokenUsageRecord {
    pub workspace_id: String,
    pub model: String,
    pub source_kind: DispatcherSessionTokenUsageSource,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cached_tokens: u64,
    pub context_window_tokens: u64,
    pub context_window_capacity: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct DispatcherDb {
    path: PathBuf,
}

struct NewDispatcherMessage<'a> {
    workspace_id: &'a str,
    role: &'a str,
    content: &'a str,
    segments_json: Option<String>,
    thinking_content: Option<&'a str>,
    thinking_elapsed_ms: u64,
    context_payload: Option<&'a str>,
    tool_call_id: Option<&'a str>,
    tool_name: Option<&'a str>,
    tool_result_mode: Option<&'a str>,
    tool_calls: Option<&'a [OutboundToolCall]>,
    tool_artifacts: &'a [ToolArtifactDraft],
    usage_stats: Option<&'a DispatcherMessageUsageStats>,
    visible: bool,
}

impl DispatcherDb {
    pub fn new(path: PathBuf) -> Result<Self> {
        let db = Self { path };
        db.init()?;
        Ok(db)
    }

    // ── Settings ──────────────────────────────────────────────

    pub fn get_settings(&self) -> Result<Option<DispatcherSettingsRecord>> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT api_base, api_key, model, summary_model, vision_model, asr_api_key, asr_websocket_url, auto_approve_dispatch, context_debug, image_model_url, image_model_api_key, image_model FROM dispatcher_settings WHERE id = 'default'",
            [],
            |row| {
                Ok(DispatcherSettingsRecord {
                    api_base: row.get(0)?,
                    api_key: row.get(1)?,
                    model: row.get(2)?,
                    summary_model: row.get(3)?,
                    vision_model: row.get(4)?,
                    asr_api_key: row.get(5)?,
                    asr_websocket_url: row.get(6)?,
                    auto_approve_dispatch: row.get::<_, i32>(7)? != 0,
                    context_debug: row.get::<_, i32>(8)? != 0,
                    image_model_url: row.get(9)?,
                    image_model_api_key: row.get(10)?,
                    image_model: row.get(11)?,
                })
            },
        )
        .optional()
        .context("load dispatcher settings")
    }

    pub fn save_settings(
        &self,
        api_base: &str,
        api_key: &str,
        model: &str,
        summary_model: &str,
        vision_model: &str,
        asr_api_key: &str,
        asr_websocket_url: &str,
        auto_approve_dispatch: bool,
        context_debug: bool,
        image_model_url: &str,
        image_model_api_key: &str,
        image_model: &str,
    ) -> Result<DispatcherSettingsRecord> {
        let conn = self.connect()?;
        let auto_approve_int = if auto_approve_dispatch { 1 } else { 0 };
        let context_debug_int = if context_debug { 1 } else { 0 };
        let normalized_summary_model = normalize_summary_model(summary_model);
        conn.execute(
            "INSERT INTO dispatcher_settings (id, api_base, api_key, model, summary_model, vision_model, asr_api_key, asr_websocket_url, auto_approve_dispatch, context_debug, image_model_url, image_model_api_key, image_model)
             VALUES ('default', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id) DO UPDATE SET api_base = ?1, api_key = ?2, model = ?3, summary_model = ?4, vision_model = ?5, asr_api_key = ?6, asr_websocket_url = ?7, auto_approve_dispatch = ?8, context_debug = ?9, image_model_url = ?10, image_model_api_key = ?11, image_model = ?12",
            params![
                api_base.trim(),
                api_key.trim(),
                model.trim(),
                &normalized_summary_model,
                vision_model.trim(),
                asr_api_key.trim(),
                asr_websocket_url.trim(),
                auto_approve_int,
                context_debug_int,
                image_model_url.trim(),
                image_model_api_key.trim(),
                image_model.trim()
            ],
        )
        .context("save dispatcher settings")?;
        Ok(DispatcherSettingsRecord {
            api_base: api_base.trim().to_string(),
            api_key: api_key.trim().to_string(),
            model: model.trim().to_string(),
            summary_model: normalized_summary_model,
            vision_model: vision_model.trim().to_string(),
            asr_api_key: asr_api_key.trim().to_string(),
            asr_websocket_url: asr_websocket_url.trim().to_string(),
            auto_approve_dispatch,
            context_debug,
            image_model_url: image_model_url.trim().to_string(),
            image_model_api_key: image_model_api_key.trim().to_string(),
            image_model: image_model.trim().to_string(),
        })
    }

    pub fn set_auto_approve_dispatch(
        &self,
        auto_approve_dispatch: bool,
    ) -> Result<DispatcherSettingsRecord> {
        let conn = self.connect()?;
        let auto_approve_int = if auto_approve_dispatch { 1 } else { 0 };
        conn.execute(
            "INSERT INTO dispatcher_settings (id, auto_approve_dispatch)
             VALUES ('default', ?1)
             ON CONFLICT(id) DO UPDATE SET auto_approve_dispatch = ?1",
            params![auto_approve_int],
        )
        .context("save dispatcher auto-approve setting")?;
        self.get_settings()?
            .context("load dispatcher settings after auto-approve update")
    }

    // ── Sessions ──────────────────────────────────────────────

    pub fn list_sessions(
        &self,
        project_id: &str,
        kind: DispatcherSessionKind,
    ) -> Result<Vec<DispatcherSessionRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, project_id, kind, title, mode, active_plan_path, created_at, updated_at
             FROM dispatcher_sessions
             WHERE project_id = ?1 AND kind = ?2
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![project_id, kind.as_sql_value()], |row| {
            Ok(DispatcherSessionRecord {
                id: row.get(0)?,
                project_id: row.get(1)?,
                kind: DispatcherSessionKind::from_sql_value(row.get(2)?),
                title: row.get(3)?,
                mode: DispatcherMode::from_sql_value(row.get(4)?),
                active_plan_path: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list dispatcher sessions")
    }

    pub fn create_session(
        &self,
        project_id: &str,
        title: &str,
        kind: DispatcherSessionKind,
        mode: DispatcherMode,
        active_plan_path: Option<&str>,
    ) -> Result<DispatcherSessionRecord> {
        let record = DispatcherSessionRecord {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            kind,
            title: title.to_string(),
            mode,
            active_plan_path: active_plan_path.map(str::to_string),
            created_at: now(),
            updated_at: now(),
        };

        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO dispatcher_sessions (id, project_id, kind, title, mode, active_plan_path, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                record.id,
                record.project_id,
                record.kind.as_sql_value(),
                record.title,
                record.mode.as_sql_value(),
                record.active_plan_path,
                record.created_at,
                record.updated_at
            ],
        )
        .context("insert dispatcher session")?;

        Ok(record)
    }

    pub fn update_session_title(
        &self,
        session_id: &str,
        title: &str,
    ) -> Result<Option<DispatcherSessionRecord>> {
        let updated_at = now();
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET title = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![title.trim(), &updated_at, session_id],
            )
            .context("update dispatcher session title")?;

        if changed == 0 {
            return Ok(None);
        }

        conn.query_row(
            "SELECT id, project_id, kind, title, mode, active_plan_path, created_at, updated_at
             FROM dispatcher_sessions
             WHERE id = ?1",
            params![session_id],
            |row| {
                Ok(DispatcherSessionRecord {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    kind: DispatcherSessionKind::from_sql_value(row.get(2)?),
                    title: row.get(3)?,
                    mode: DispatcherMode::from_sql_value(row.get(4)?),
                    active_plan_path: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            },
        )
        .optional()
        .context("load dispatcher session after title update")
    }

    pub fn delete_session(&self, session_id: &str) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM dispatcher_tool_artifacts WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_session_token_usage WHERE workspace_id = ?1",
            params![session_id],
        )?;
        // Delete associated image files before removing database records
        {
            let mut stmt = tx.prepare(
                "SELECT path FROM chat_images WHERE workspace_id = ?1",
            )?;
            let paths: Vec<String> = stmt
                .query_map(params![session_id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for path in paths {
                let _ = std::fs::remove_file(&path);
            }
        }
        tx.execute(
            "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_sessions WHERE id = ?1",
            params![session_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_session_runtime_state(
        &self,
        session_id: &str,
    ) -> Result<DispatcherSessionRuntimeState> {
        let conn = self.connect()?;
        let (mode, active_plan_path, checklist_json, plan_interaction_json) = conn
            .query_row(
                "SELECT mode, active_plan_path, checklist_json, plan_interaction_json
                 FROM dispatcher_sessions
                 WHERE id = ?1",
                params![session_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()
            .context("load dispatcher session runtime state")?
            .with_context(|| format!("dispatcher session not found: {session_id}"))?;

        Ok(DispatcherSessionRuntimeState {
            mode: DispatcherMode::from_sql_value(mode),
            active_plan_path,
            checklist: parse_optional_json(checklist_json, "checklist_json")?,
            plan_interaction: parse_optional_json(plan_interaction_json, "plan_interaction_json")?,
        })
    }

    pub fn set_session_mode(
        &self,
        session_id: &str,
        mode: DispatcherMode,
    ) -> Result<DispatcherSessionRuntimeState> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET mode = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![mode.as_sql_value(), now(), session_id],
            )
            .context("set dispatcher session mode")?;
        if changed == 0 {
            anyhow::bail!("dispatcher session not found: {session_id}");
        }
        self.get_session_runtime_state(session_id)
    }

    pub fn update_checklist(
        &self,
        session_id: &str,
        checklist: &ChecklistPlanState,
    ) -> Result<DispatcherSessionRuntimeState> {
        let checklist_json = serde_json::to_string(checklist).context("serialize checklist")?;
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET checklist_json = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![checklist_json, now(), session_id],
            )
            .context("update dispatcher checklist")?;
        if changed == 0 {
            anyhow::bail!("dispatcher session not found: {session_id}");
        }
        self.get_session_runtime_state(session_id)
    }

    pub fn clear_checklist(&self, session_id: &str) -> Result<DispatcherSessionRuntimeState> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET checklist_json = NULL, updated_at = ?1
                 WHERE id = ?2",
                params![now(), session_id],
            )
            .context("clear dispatcher checklist")?;
        if changed == 0 {
            anyhow::bail!("dispatcher session not found: {session_id}");
        }
        self.get_session_runtime_state(session_id)
    }

    pub fn attach_checklist_subprocess(
        &self,
        session_id: &str,
        dispatch_id: &str,
        task_id: &str,
    ) -> Result<DispatcherSessionRuntimeState> {
        let mut state = self.get_session_runtime_state(session_id)?;
        let Some(mut checklist) = state.checklist.take() else {
            return Ok(state);
        };

        if let Some(item) = checklist
            .items
            .iter_mut()
            .find(|item| item.dispatch_id.as_deref() == Some(dispatch_id))
        {
            item.status = ChecklistStepStatus::InProgress;
            item.subprocess_task_id = Some(task_id.to_string());
            checklist.updated_at = now();
            return self.update_checklist(session_id, &checklist);
        }

        Ok(DispatcherSessionRuntimeState {
            checklist: Some(checklist),
            ..state
        })
    }

    pub fn clear_checklist_dispatch(
        &self,
        session_id: &str,
        dispatch_id: &str,
    ) -> Result<DispatcherSessionRuntimeState> {
        let mut state = self.get_session_runtime_state(session_id)?;
        let Some(mut checklist) = state.checklist.take() else {
            return Ok(state);
        };

        let mut changed = false;
        for item in &mut checklist.items {
            if item.dispatch_id.as_deref() == Some(dispatch_id) {
                item.dispatch_id = None;
                item.subprocess_task_id = None;
                item.agent = None;
                item.detail = None;
                if item.status == ChecklistStepStatus::InProgress {
                    item.status = ChecklistStepStatus::Pending;
                }
                changed = true;
            }
        }

        if changed {
            checklist.updated_at = now();
            return self.update_checklist(session_id, &checklist);
        }

        Ok(DispatcherSessionRuntimeState {
            checklist: Some(checklist),
            ..state
        })
    }

    pub fn set_plan_interaction(
        &self,
        session_id: &str,
        interaction: Option<&PlanInteraction>,
    ) -> Result<DispatcherSessionRuntimeState> {
        let interaction_json = interaction
            .map(serde_json::to_string)
            .transpose()
            .context("serialize plan interaction")?;
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET plan_interaction_json = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![interaction_json, now(), session_id],
            )
            .context("update dispatcher plan interaction")?;
        if changed == 0 {
            anyhow::bail!("dispatcher session not found: {session_id}");
        }
        self.get_session_runtime_state(session_id)
    }

    pub fn set_active_plan_path(
        &self,
        session_id: &str,
        plan_path: Option<&str>,
    ) -> Result<DispatcherSessionRuntimeState> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET active_plan_path = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![plan_path, now(), session_id],
            )
            .context("update dispatcher active plan path")?;
        if changed == 0 {
            anyhow::bail!("dispatcher session not found: {session_id}");
        }
        self.get_session_runtime_state(session_id)
    }

    // ── Messages ──────────────────────────────────────────────

    pub fn add_visible_message(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        segments_json: Option<String>,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            segments_json,
            thinking_content: None,
            thinking_elapsed_ms: 0,
            context_payload: None,
            tool_call_id: None,
            tool_name: None,
            tool_result_mode: None,
            tool_calls: None,
            tool_artifacts: &[],
            usage_stats: None,
            visible: true,
        })
    }

    pub fn add_visible_message_with_usage(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        usage_stats: &DispatcherMessageUsageStats,
    ) -> Result<DispatcherMessageRecord> {
        self.add_visible_message_with_usage_and_thinking(
            workspace_id,
            role,
            content,
            usage_stats,
            None,
            0,
        )
    }

    pub fn add_visible_message_with_usage_and_thinking(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        usage_stats: &DispatcherMessageUsageStats,
        thinking_content: Option<&str>,
        thinking_elapsed_ms: u64,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            segments_json: None,
            thinking_content,
            thinking_elapsed_ms,
            context_payload: None,
            tool_call_id: None,
            tool_name: None,
            tool_result_mode: None,
            tool_calls: None,
            tool_artifacts: &[],
            usage_stats: Some(usage_stats),
            visible: true,
        })
    }

    pub fn add_visible_message_with_tools(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
    ) -> Result<DispatcherMessageRecord> {
        self.add_visible_message_with_tools_and_thinking(
            workspace_id,
            role,
            content,
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls,
            None,
            0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_visible_message_with_tools_and_thinking(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
        thinking_content: Option<&str>,
        thinking_elapsed_ms: u64,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            segments_json: None,
            thinking_content,
            thinking_elapsed_ms,
            context_payload: None,
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls,
            tool_artifacts: &[],
            usage_stats: None,
            visible: true,
        })
    }

    pub fn add_visible_tool_result(
        &self,
        workspace_id: &str,
        content: &str,
        context_payload: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_artifacts: &[ToolArtifactDraft],
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role: "tool",
            content,
            segments_json: None,
            thinking_content: None,
            thinking_elapsed_ms: 0,
            context_payload: Some(context_payload),
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls: None,
            tool_artifacts,
            usage_stats: None,
            visible: true,
        })
    }

    pub fn compact_successful_tool_retry(
        &self,
        workspace_id: &str,
        tool_name: &str,
        current_tool_call_id: &str,
    ) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        let cutoff_rowid = latest_user_message_rowid(&tx, workspace_id)?;
        let retry_context_pattern = format!("{TOOL_RETRY_CONTEXT_PREFIX}%");
        let retry_messages = {
            let mut stmt = tx.prepare(
                "SELECT id, tool_call_id
                 FROM dispatcher_messages
                 WHERE workspace_id = ?1
                   AND role = 'tool'
                   AND tool_name = ?2
                   AND rowid >= ?3
                   AND tool_call_id IS NOT NULL
                   AND tool_call_id <> ?4
                   AND context_payload LIKE ?5",
            )?;
            let rows = stmt.query_map(
                params![
                    workspace_id,
                    tool_name,
                    cutoff_rowid,
                    current_tool_call_id,
                    retry_context_pattern
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        if retry_messages.is_empty() {
            tx.commit()
                .context("commit empty dispatcher retry compaction")?;
            return Ok(());
        }

        let failed_tool_call_ids = retry_messages
            .iter()
            .map(|(_, tool_call_id)| tool_call_id.clone())
            .collect::<HashSet<_>>();

        for (message_id, _) in &retry_messages {
            tx.execute(
                "DELETE FROM dispatcher_tool_artifacts WHERE message_id = ?1",
                params![message_id],
            )
            .context("delete compacted retry tool artifacts")?;
            tx.execute(
                "DELETE FROM dispatcher_messages WHERE id = ?1",
                params![message_id],
            )
            .context("delete compacted retry tool message")?;
        }

        let assistant_messages = {
            let mut stmt = tx.prepare(
                "SELECT id, segments_json, tool_calls_json
                 FROM dispatcher_messages
                 WHERE workspace_id = ?1
                   AND role = 'assistant'
                   AND rowid >= ?2
                   AND tool_calls_json IS NOT NULL
                 ORDER BY rowid ASC",
            )?;
            let rows = stmt.query_map(params![workspace_id, cutoff_rowid], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        for (message_id, segments_json, tool_calls_json) in assistant_messages {
            let Ok(mut tool_calls) =
                serde_json::from_str::<Vec<OutboundToolCall>>(&tool_calls_json)
            else {
                continue;
            };
            let original_len = tool_calls.len();
            tool_calls.retain(|call| !failed_tool_call_ids.contains(&call.id));
            if tool_calls.len() == original_len {
                continue;
            }

            let content = segments_to_markdown(&parse_segments_json(&segments_json));
            if tool_calls.is_empty() && content.trim().is_empty() {
                tx.execute(
                    "DELETE FROM dispatcher_tool_artifacts WHERE message_id = ?1",
                    params![&message_id],
                )
                .context("delete compacted retry assistant artifacts")?;
                tx.execute(
                    "DELETE FROM dispatcher_messages WHERE id = ?1",
                    params![&message_id],
                )
                .context("delete compacted retry assistant message")?;
            } else {
                let next_tool_calls_json = if tool_calls.is_empty() {
                    None
                } else {
                    Some(
                        serde_json::to_string(&tool_calls)
                            .context("serialize compacted assistant tool calls")?,
                    )
                };
                tx.execute(
                    "UPDATE dispatcher_messages SET tool_calls_json = ?1 WHERE id = ?2",
                    params![next_tool_calls_json, &message_id],
                )
                .context("update compacted assistant tool calls")?;
            }
        }

        tx.commit()
            .context("commit dispatcher successful retry compaction")
    }

    pub fn add_hidden_message(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            segments_json: None,
            thinking_content: None,
            thinking_elapsed_ms: 0,
            context_payload: None,
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls,
            tool_artifacts: &[],
            usage_stats: None,
            visible: false,
        })
    }

    pub fn list_visible_messages(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<DispatcherMessageRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, role, segments_json, thinking_content, thinking_elapsed_ms, context_payload, tool_call_id, tool_name, tool_result_mode, tool_artifacts_json, tool_calls_json, usage_stats_json, created_at
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND visible = 1
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id], map_dispatcher_message_record)?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("load visible dispatcher messages")
    }

    /// Load recent complete visible dialogues for session title generation.
    ///
    /// The cutoff is based on user-started turns, so the latest user message and its
    /// following assistant/tool messages stay together instead of being clipped by a
    /// raw message count.
    pub fn list_recent_visible_dialogue_messages(
        &self,
        workspace_id: &str,
        max_dialogues: usize,
    ) -> Result<Vec<DispatcherMessageRecord>> {
        let conn = self.connect()?;
        let cutoff_rowid = self.find_dialogue_cutoff_rowid(&conn, workspace_id, max_dialogues)?;
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, role, segments_json, thinking_content, thinking_elapsed_ms, context_payload, tool_call_id, tool_name, tool_result_mode, tool_artifacts_json, tool_calls_json, usage_stats_json, created_at
             FROM dispatcher_messages
             WHERE workspace_id = ?1
               AND visible = 1
               AND context_cleared = 0
               AND rowid >= ?2
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(
            params![workspace_id, cutoff_rowid],
            map_dispatcher_message_record,
        )?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("load recent visible dispatcher dialogue messages")
    }

    /// Load only the recent dialogue window for one dispatcher session.
    ///
    /// Note:
    /// - `workspace_id` here is the dispatcher session id used by the frontend.
    /// - One project can have multiple dispatcher sessions; history is isolated by session id.
    /// - Only the most recent `MAX_LLM_DIALOGUES` user-started dialogues are injected into the LLM.
    pub fn load_llm_history(&self, workspace_id: &str) -> Result<Vec<ChatMessage>> {
        let conn = self.connect()?;
        let cutoff_rowid =
            self.find_dialogue_cutoff_rowid(&conn, workspace_id, MAX_LLM_DIALOGUES)?;

        let mut stmt = conn.prepare(
            "SELECT role, segments_json, context_payload, tool_call_id, tool_name, tool_calls_json
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND rowid >= ?2 AND context_cleared = 0
             ORDER BY rowid ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id, cutoff_rowid], |row| {
            let role: String = row.get(0)?;
            let segments_json: String = row.get(1)?;
            let context_payload: Option<String> = row.get(2)?;
            let tool_call_id: Option<String> = row.get(3)?;
            let tool_name: Option<String> = row.get(4)?;
            let tool_calls_json: Option<String> = row.get(5)?;

            let content = if let Some(payload) = context_payload {
                payload
            } else {
                let segments = parse_segments_json(&segments_json);
                segments_to_markdown(&segments)
            };

            let tool_calls = tool_calls_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<Vec<OutboundToolCall>>(json).ok());

            Ok(ChatMessage {
                role,
                content,
                tool_call_id,
                name: tool_name,
                tool_calls,
            })
        })?;

        let mut messages = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("load dispatcher llm history")?;
        messages.retain(should_keep_llm_message);

        while matches!(messages.first().map(|m| m.role.as_str()), Some("tool")) {
            messages.remove(0);
        }

        Ok(messages)
    }

    pub fn clear_context_messages(&self, workspace_id: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE dispatcher_messages
             SET context_cleared = 1
             WHERE workspace_id = ?1 AND context_cleared = 0",
            params![workspace_id],
        )
        .context("logically clear dispatcher messages")?;
        Ok(())
    }

    pub fn clear_messages(&self, workspace_id: &str) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM dispatcher_tool_artifacts WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher tool artifacts")?;
        tx.execute(
            "DELETE FROM dispatcher_session_token_usage WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher session token usage")?;
        // Delete associated image files before removing database records
        {
            let mut stmt = tx.prepare(
                "SELECT path FROM chat_images WHERE workspace_id = ?1",
            )?;
            let paths: Vec<String> = stmt
                .query_map(params![workspace_id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for path in paths {
                let _ = std::fs::remove_file(&path);
            }
        }
        tx.execute(
            "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher messages")?;
        tx.execute(
            "UPDATE dispatcher_sessions
             SET checklist_json = NULL,
                 plan_interaction_json = NULL,
                 active_plan_path = NULL,
                 updated_at = ?1
             WHERE id = ?2",
            params![now(), workspace_id],
        )
        .context("clear dispatcher planning state")?;
        tx.commit().context("commit dispatcher message cleanup")?;
        Ok(())
    }

    pub fn upsert_session_token_usage(
        &self,
        workspace_id: &str,
        model: &str,
        source_kind: DispatcherSessionTokenUsageSource,
        usage: &LlmUsage,
    ) -> Result<DispatcherSessionTokenUsageRecord> {
        let updated_at = now();
        let record = DispatcherSessionTokenUsageRecord {
            workspace_id: workspace_id.to_string(),
            model: model.to_string(),
            source_kind,
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage_total_tokens(usage),
            cached_tokens: usage.cached_tokens(),
            context_window_tokens: usage.prompt_tokens,
            context_window_capacity: default_context_window_capacity(model),
            updated_at,
        };
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO dispatcher_session_token_usage (
                workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
                cached_tokens, context_window_tokens, context_window_capacity, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(workspace_id, model, source_kind) DO UPDATE SET
                prompt_tokens = dispatcher_session_token_usage.prompt_tokens + excluded.prompt_tokens,
                completion_tokens = dispatcher_session_token_usage.completion_tokens + excluded.completion_tokens,
                total_tokens = dispatcher_session_token_usage.total_tokens + excluded.total_tokens,
                cached_tokens = dispatcher_session_token_usage.cached_tokens + excluded.cached_tokens,
                context_window_tokens = excluded.context_window_tokens,
                context_window_capacity = excluded.context_window_capacity,
                updated_at = excluded.updated_at",
            params![
                &record.workspace_id,
                &record.model,
                source_kind.as_sql_value(),
                record.prompt_tokens as i64,
                record.completion_tokens as i64,
                record.total_tokens as i64,
                record.cached_tokens as i64,
                record.context_window_tokens as i64,
                record.context_window_capacity as i64,
                &record.updated_at,
            ],
        )
        .context("upsert dispatcher session token usage")?;
        self.get_session_token_usage_record(&conn, workspace_id, model, source_kind)
    }

    fn get_session_token_usage_record(
        &self,
        conn: &Connection,
        workspace_id: &str,
        model: &str,
        source_kind: DispatcherSessionTokenUsageSource,
    ) -> Result<DispatcherSessionTokenUsageRecord> {
        conn.query_row(
            "SELECT workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
                    cached_tokens, context_window_tokens, context_window_capacity, updated_at
             FROM dispatcher_session_token_usage
             WHERE workspace_id = ?1 AND model = ?2 AND source_kind = ?3",
            params![workspace_id, model, source_kind.as_sql_value()],
            |row| {
                Ok(DispatcherSessionTokenUsageRecord {
                    workspace_id: row.get(0)?,
                    model: row.get(1)?,
                    source_kind: DispatcherSessionTokenUsageSource::from_sql_value(row.get(2)?),
                    prompt_tokens: row.get::<_, i64>(3)? as u64,
                    completion_tokens: row.get::<_, i64>(4)? as u64,
                    total_tokens: row.get::<_, i64>(5)? as u64,
                    cached_tokens: row.get::<_, i64>(6)? as u64,
                    context_window_tokens: row.get::<_, i64>(7)? as u64,
                    context_window_capacity: row.get::<_, i64>(8)? as u64,
                    updated_at: row.get(9)?,
                })
            },
        )
        .context("load dispatcher session token usage after upsert")
    }

    pub fn list_session_token_usage(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<DispatcherSessionTokenUsageRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
                    cached_tokens, context_window_tokens, context_window_capacity, updated_at
             FROM dispatcher_session_token_usage
             WHERE workspace_id = ?1
             ORDER BY updated_at DESC, model ASC, source_kind ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id], |row| {
            Ok(DispatcherSessionTokenUsageRecord {
                workspace_id: row.get(0)?,
                model: row.get(1)?,
                source_kind: DispatcherSessionTokenUsageSource::from_sql_value(row.get(2)?),
                prompt_tokens: row.get::<_, i64>(3)? as u64,
                completion_tokens: row.get::<_, i64>(4)? as u64,
                total_tokens: row.get::<_, i64>(5)? as u64,
                cached_tokens: row.get::<_, i64>(6)? as u64,
                context_window_tokens: row.get::<_, i64>(7)? as u64,
                context_window_capacity: row.get::<_, i64>(8)? as u64,
                updated_at: row.get(9)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list dispatcher session token usage")
    }

    pub fn get_tool_artifact(
        &self,
        workspace_id: &str,
        artifact_id: &str,
    ) -> Result<DispatcherToolArtifactRecord> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT id, workspace_id, message_id, tool_call_id, tool_name, title, kind, preview, content, char_count, line_count, created_at
             FROM dispatcher_tool_artifacts
             WHERE id = ?1 AND workspace_id = ?2",
            params![artifact_id, workspace_id],
            |row| {
                Ok(DispatcherToolArtifactRecord {
                    id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    message_id: row.get(2)?,
                    tool_call_id: row.get(3)?,
                    tool_name: row.get(4)?,
                    title: row.get(5)?,
                    kind: row.get(6)?,
                    preview: row.get(7)?,
                    content: row.get(8)?,
                    char_count: row.get::<_, i64>(9)? as usize,
                    line_count: row.get::<_, i64>(10)? as usize,
                    created_at: row.get(11)?,
                })
            },
        )
        .optional()
        .context("load dispatcher tool artifact")?
        .with_context(|| format!("dispatcher tool artifact not found: {artifact_id}"))
    }

    fn add_message(&self, params: NewDispatcherMessage<'_>) -> Result<DispatcherMessageRecord> {
        let tool_calls_json = params
            .tool_calls
            .map(serde_json::to_string)
            .transpose()
            .context("serialize tool calls")?;
        let usage_stats_json = params
            .usage_stats
            .map(serde_json::to_string)
            .transpose()
            .context("serialize dispatcher message usage stats")?;

        let segments_json = params
            .segments_json
            .unwrap_or_else(|| content_to_segments_json(params.content));
        let content = segments_to_markdown(&parse_segments_json(&segments_json));

        let mut record = DispatcherMessageRecord {
            id: Uuid::new_v4().to_string(),
            workspace_id: params.workspace_id.to_string(),
            role: params.role.to_string(),
            content,
            segments_json,
            thinking_content: params
                .thinking_content
                .filter(|content| !content.trim().is_empty())
                .map(|s| s.to_string()),
            thinking_elapsed_ms: params
                .thinking_content
                .filter(|content| !content.trim().is_empty())
                .map(|_| params.thinking_elapsed_ms),
            context_payload: params.context_payload.map(|s| s.to_string()),
            tool_call_id: params.tool_call_id.map(|s| s.to_string()),
            tool_name: params.tool_name.map(|s| s.to_string()),
            tool_result_mode: params.tool_result_mode.map(|s| s.to_string()),
            tool_artifacts: Vec::new(),
            tool_calls_json: tool_calls_json.clone(),
            usage_stats: params.usage_stats.cloned(),
            created_at: now(),
        };

        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE dispatcher_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&record.created_at, &record.workspace_id],
        )?;

        tx.execute(
            "INSERT INTO dispatcher_messages (
                id, workspace_id, role, segments_json, thinking_content, thinking_elapsed_ms, context_payload, tool_call_id, tool_name, tool_result_mode, tool_artifacts_json, tool_calls_json, usage_stats_json, visible, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                &record.id,
                &record.workspace_id,
                &record.role,
                &record.segments_json,
                &record.thinking_content,
                &record.thinking_elapsed_ms,
                &record.context_payload,
                &record.tool_call_id,
                &record.tool_name,
                &record.tool_result_mode,
                Option::<String>::None,
                &record.tool_calls_json,
                &usage_stats_json,
                if params.visible { 1 } else { 0 },
                &record.created_at
            ],
        )
        .context("insert dispatcher message")?;

        if !params.tool_artifacts.is_empty() {
            let artifacts = self.insert_tool_artifacts(
                &tx,
                &record.workspace_id,
                &record.id,
                record.tool_call_id.as_deref(),
                record.tool_name.as_deref(),
                params.tool_artifacts,
                &record.created_at,
            )?;
            let artifacts_json =
                serde_json::to_string(&artifacts).context("serialize dispatcher tool artifacts")?;
            tx.execute(
                "UPDATE dispatcher_messages SET tool_artifacts_json = ?1 WHERE id = ?2",
                params![&artifacts_json, &record.id],
            )
            .context("attach dispatcher tool artifacts to message")?;
            record.tool_artifacts = artifacts;
        }

        tx.commit().context("commit dispatcher message insert")?;

        Ok(record)
    }

    fn insert_tool_artifacts(
        &self,
        tx: &rusqlite::Transaction<'_>,
        workspace_id: &str,
        message_id: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        drafts: &[ToolArtifactDraft],
        created_at: &str,
    ) -> Result<Vec<DispatcherToolArtifactRef>> {
        let mut refs = Vec::with_capacity(drafts.len());

        for draft in drafts {
            let artifact_id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO dispatcher_tool_artifacts (
                    id, workspace_id, message_id, tool_call_id, tool_name, title, kind, preview, content, char_count, line_count, created_at
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    &artifact_id,
                    workspace_id,
                    message_id,
                    tool_call_id,
                    tool_name,
                    &draft.title,
                    &draft.kind,
                    &draft.preview,
                    &draft.content,
                    draft.char_count as i64,
                    draft.line_count as i64,
                    created_at,
                ],
            )
            .context("insert dispatcher tool artifact")?;

            refs.push(DispatcherToolArtifactRef {
                id: artifact_id,
                title: draft.title.clone(),
                kind: draft.kind.clone(),
                preview: draft.preview.clone(),
                char_count: draft.char_count,
                line_count: draft.line_count,
                created_at: created_at.to_string(),
            });
        }

        Ok(refs)
    }

    fn init(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create db directory {}", parent.display()))?;
        }
        let mut conn = self.connect()?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS dispatcher_sessions (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                kind TEXT NOT NULL DEFAULT 'project',
                title TEXT NOT NULL,
                mode TEXT NOT NULL DEFAULT 'default',
                active_plan_path TEXT,
                checklist_json TEXT,
                plan_interaction_json TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_sessions_project
            ON dispatcher_sessions(project_id, updated_at DESC);

            CREATE TABLE IF NOT EXISTS dispatcher_messages (
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                role TEXT NOT NULL,
                segments_json TEXT NOT NULL DEFAULT '[]',
                thinking_content TEXT,
                thinking_elapsed_ms INTEGER,
                context_payload TEXT,
                tool_call_id TEXT,
                tool_name TEXT,
                tool_result_mode TEXT,
                tool_artifacts_json TEXT,
                tool_calls_json TEXT,
                usage_stats_json TEXT,
                visible INTEGER NOT NULL DEFAULT 1,
                context_cleared INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_messages_workspace_created
            ON dispatcher_messages(workspace_id, created_at);

            CREATE TABLE IF NOT EXISTS chat_images (
                id TEXT PRIMARY KEY,
                image_id TEXT NOT NULL UNIQUE,
                workspace_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                segment_index INTEGER NOT NULL,
                path TEXT NOT NULL,
                alt TEXT,
                width INTEGER,
                height INTEGER,
                mime_type TEXT,
                source TEXT,
                generation_prompt TEXT,
                vector_embedding_json TEXT,
                text_description TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY (message_id) REFERENCES dispatcher_messages(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_chat_images_workspace
            ON chat_images(workspace_id, created_at);

            CREATE INDEX IF NOT EXISTS idx_chat_images_message
            ON chat_images(message_id);

            CREATE TABLE IF NOT EXISTS dispatcher_tool_artifacts (
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                message_id TEXT,
                tool_call_id TEXT,
                tool_name TEXT,
                title TEXT NOT NULL,
                kind TEXT NOT NULL,
                preview TEXT NOT NULL DEFAULT '',
                content TEXT NOT NULL,
                char_count INTEGER NOT NULL DEFAULT 0,
                line_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_artifacts_workspace_created
            ON dispatcher_tool_artifacts(workspace_id, created_at);

            CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_artifacts_message
            ON dispatcher_tool_artifacts(message_id);

            CREATE TABLE IF NOT EXISTS dispatcher_settings (
                id TEXT PRIMARY KEY DEFAULT 'default',
                api_base TEXT NOT NULL DEFAULT '',
                api_key TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL DEFAULT '',
                summary_model TEXT NOT NULL DEFAULT 'deepseek-v4-flash',
                vision_model TEXT NOT NULL DEFAULT '',
                asr_api_key TEXT NOT NULL DEFAULT '',
                asr_websocket_url TEXT NOT NULL DEFAULT '',
                auto_approve_dispatch INTEGER NOT NULL DEFAULT 0,
                context_debug INTEGER NOT NULL DEFAULT 0,
                image_model_url TEXT NOT NULL DEFAULT 'https://dashscope.aliyuncs.com/api/v1',
                image_model_api_key TEXT NOT NULL DEFAULT '',
                image_model TEXT NOT NULL DEFAULT 'qwen-image-2.0-pro'
            );

            CREATE TABLE IF NOT EXISTS dispatcher_session_token_usage (
                workspace_id TEXT NOT NULL,
                model TEXT NOT NULL,
                source_kind TEXT NOT NULL DEFAULT 'primary',
                prompt_tokens INTEGER NOT NULL DEFAULT 0,
                completion_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                cached_tokens INTEGER NOT NULL DEFAULT 0,
                context_window_tokens INTEGER NOT NULL DEFAULT 0,
                context_window_capacity INTEGER NOT NULL DEFAULT 1000000,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (workspace_id, model, source_kind)
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_session_token_usage_workspace_updated
            ON dispatcher_session_token_usage(workspace_id, updated_at DESC);
            ",
        )
        .context("initialize dispatcher sqlite schema")?;
        migrate_session_token_usage_primary_key(&mut conn)?;
        ensure_column_exists(
            &conn,
            "dispatcher_settings",
            "summary_model",
            "TEXT NOT NULL DEFAULT 'deepseek-v4-flash'",
        )?;
        ensure_column_exists(
            &conn,
            "dispatcher_settings",
            "vision_model",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        ensure_column_exists(
            &conn,
            "dispatcher_settings",
            "asr_api_key",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        ensure_column_exists(
            &conn,
            "dispatcher_settings",
            "asr_websocket_url",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        ensure_column_exists(
            &conn,
            "dispatcher_settings",
            "context_debug",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column_exists(
            &conn,
            "dispatcher_settings",
            "image_model_url",
            "TEXT NOT NULL DEFAULT 'https://dashscope.aliyuncs.com/api/v1'",
        )?;
        ensure_column_exists(
            &conn,
            "dispatcher_settings",
            "image_model_api_key",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        ensure_column_exists(
            &conn,
            "dispatcher_settings",
            "image_model",
            "TEXT NOT NULL DEFAULT 'qwen-image-2.0-pro'",
        )?;
        ensure_column_exists(&conn, "dispatcher_messages", "context_payload", "TEXT")?;
        ensure_column_exists(&conn, "dispatcher_messages", "segments_json", "TEXT NOT NULL DEFAULT '[]'")?;
        ensure_column_exists(&conn, "dispatcher_messages", "thinking_content", "TEXT")?;
        ensure_column_exists(
            &conn,
            "dispatcher_messages",
            "thinking_elapsed_ms",
            "INTEGER",
        )?;
        ensure_column_exists(&conn, "dispatcher_messages", "tool_result_mode", "TEXT")?;
        ensure_column_exists(&conn, "dispatcher_messages", "tool_artifacts_json", "TEXT")?;
        ensure_column_exists(&conn, "dispatcher_messages", "usage_stats_json", "TEXT")?;
        ensure_column_exists(
            &conn,
            "dispatcher_messages",
            "context_cleared",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column_exists(
            &conn,
            "dispatcher_sessions",
            "kind",
            "TEXT NOT NULL DEFAULT 'project'",
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_dispatcher_sessions_project_kind
             ON dispatcher_sessions(project_id, kind, updated_at DESC)",
            [],
        )
        .context("create dispatcher session project kind index")?;
        ensure_column_exists(
            &conn,
            "dispatcher_sessions",
            "mode",
            "TEXT NOT NULL DEFAULT 'default'",
        )?;
        ensure_column_exists(&conn, "dispatcher_sessions", "active_plan_path", "TEXT")?;
        ensure_column_exists(&conn, "dispatcher_sessions", "checklist_json", "TEXT")?;
        ensure_column_exists(
            &conn,
            "dispatcher_sessions",
            "plan_interaction_json",
            "TEXT",
        )?;
        Ok(())
    }

    fn connect(&self) -> Result<Connection> {
        Connection::open(&self.path).with_context(|| format!("open {}", self.path.display()))
    }

    fn find_dialogue_cutoff_rowid(
        &self,
        conn: &Connection,
        workspace_id: &str,
        max_dialogues: usize,
    ) -> Result<i64> {
        let max_dialogues = max_dialogues.clamp(1, MAX_DIALOGUE_QUERY_LIMIT);
        let mut stmt = conn.prepare(
            "SELECT rowid
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND role = 'user' AND context_cleared = 0
             ORDER BY rowid DESC
             LIMIT ?2",
        )?;
        let rowids = stmt
            .query_map(params![workspace_id, max_dialogues as i64], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("load dispatcher dialogue boundaries")?;

        Ok(rowids.into_iter().min().unwrap_or(0))
    }
}

impl DispatcherSessionTokenUsageSource {
    fn as_sql_value(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Summary => "summary",
        }
    }

    fn from_sql_value(value: String) -> Self {
        match value.as_str() {
            "summary" => Self::Summary,
            _ => Self::Primary,
        }
    }
}

fn migrate_session_token_usage_primary_key(conn: &mut Connection) -> Result<()> {
    let primary_key_columns = table_primary_key_columns(conn, "dispatcher_session_token_usage")?;
    if primary_key_columns
        .iter()
        .map(String::as_str)
        .eq(["workspace_id", "model", "source_kind"])
    {
        return Ok(());
    }

    let tx = conn.transaction()?;
    tx.execute_batch(
        "
        ALTER TABLE dispatcher_session_token_usage RENAME TO dispatcher_session_token_usage_old;

        CREATE TABLE dispatcher_session_token_usage (
            workspace_id TEXT NOT NULL,
            model TEXT NOT NULL,
            source_kind TEXT NOT NULL DEFAULT 'primary',
            prompt_tokens INTEGER NOT NULL DEFAULT 0,
            completion_tokens INTEGER NOT NULL DEFAULT 0,
            total_tokens INTEGER NOT NULL DEFAULT 0,
            cached_tokens INTEGER NOT NULL DEFAULT 0,
            context_window_tokens INTEGER NOT NULL DEFAULT 0,
            context_window_capacity INTEGER NOT NULL DEFAULT 1000000,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, model, source_kind)
        );

        INSERT INTO dispatcher_session_token_usage (
            workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
            cached_tokens, context_window_tokens, context_window_capacity, updated_at
        )
        SELECT
            workspace_id,
            model,
            COALESCE(NULLIF(source_kind, ''), 'primary'),
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cached_tokens,
            context_window_tokens,
            context_window_capacity,
            updated_at
        FROM dispatcher_session_token_usage_old;

        DROP TABLE dispatcher_session_token_usage_old;

        CREATE INDEX IF NOT EXISTS idx_dispatcher_session_token_usage_workspace_updated
        ON dispatcher_session_token_usage(workspace_id, updated_at DESC);
        ",
    )
    .context("migrate dispatcher session token usage primary key")?;
    tx.commit()
        .context("commit dispatcher session token usage primary key migration")
}

fn table_primary_key_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .with_context(|| format!("read table info for {table}"))?;
    let columns = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(5)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .with_context(|| format!("collect table info for {table}"))?;

    let mut primary_key_columns = columns
        .into_iter()
        .filter(|(position, _)| *position > 0)
        .collect::<Vec<_>>();
    primary_key_columns.sort_by_key(|(position, _)| *position);
    Ok(primary_key_columns
        .into_iter()
        .map(|(_, name)| name)
        .collect())
}

fn ensure_column_exists(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    match conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    ) {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(_, Some(message)))
            if message.contains("duplicate column name") =>
        {
            Ok(())
        }
        Err(error) => {
            Err(error).with_context(|| format!("ensure column {column} exists on table {table}"))
        }
    }
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn default_context_window_capacity(_model: &str) -> u64 {
    DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS
}

fn usage_total_tokens(usage: &LlmUsage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.prompt_tokens + usage.completion_tokens
    }
}

fn normalize_summary_model(summary_model: &str) -> String {
    let trimmed = summary_model.trim();
    if trimmed.is_empty() {
        DEFAULT_SUMMARY_MODEL.to_string()
    } else {
        trimmed.to_string()
    }
}

fn latest_user_message_rowid(conn: &Connection, workspace_id: &str) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(rowid), 0)
         FROM dispatcher_messages
         WHERE workspace_id = ?1 AND role = 'user'",
        params![workspace_id],
        |row| row.get(0),
    )
    .context("load latest dispatcher user message rowid")
}

fn map_chat_message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatMessage> {
    let tool_calls_json: Option<String> = row.get(4)?;
    let tool_calls = tool_calls_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<Vec<OutboundToolCall>>(json).ok());

    Ok(ChatMessage {
        role: row.get(0)?,
        content: row.get(1)?,
        tool_call_id: row.get(2)?,
        name: row.get(3)?,
        tool_calls,
    })
}

fn map_dispatcher_message_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<DispatcherMessageRecord> {
    let segments_json: String = row.get(3)?;
    let content = segments_to_markdown(&parse_segments_json(&segments_json));
    Ok(DispatcherMessageRecord {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        role: row.get(2)?,
        content,
        segments_json,
        thinking_content: row.get(4)?,
        thinking_elapsed_ms: row.get(5)?,
        context_payload: row.get(6)?,
        tool_call_id: row.get(7)?,
        tool_name: row.get(8)?,
        tool_result_mode: row.get(9)?,
        tool_artifacts: parse_tool_artifact_refs(row.get::<_, Option<String>>(10)?),
        tool_calls_json: row.get(11)?,
        usage_stats: parse_message_usage_stats(row.get::<_, Option<String>>(12)?)?,
        created_at: row.get(13)?,
    })
}

fn parse_tool_artifact_refs(raw: Option<String>) -> Vec<DispatcherToolArtifactRef> {
    raw.as_deref()
        .and_then(|json| serde_json::from_str::<Vec<DispatcherToolArtifactRef>>(json).ok())
        .unwrap_or_default()
}

fn parse_message_usage_stats(
    raw: Option<String>,
) -> rusqlite::Result<Option<DispatcherMessageUsageStats>> {
    raw.as_deref()
        .filter(|json| !json.trim().is_empty())
        .map(|json| {
            serde_json::from_str::<DispatcherMessageUsageStats>(json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(10, Type::Text, Box::new(error))
            })
        })
        .transpose()
}

fn parse_optional_json<T>(raw: Option<String>, column: &str) -> Result<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    raw.as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            serde_json::from_str::<T>(value)
                .with_context(|| format!("parse dispatcher session {column}"))
        })
        .transpose()
}

fn should_keep_llm_message(message: &ChatMessage) -> bool {
    match message.role.as_str() {
        "assistant" => {
            !is_process_only_assistant_message(&message.content)
                && !is_process_only_assistant_tool_call(message)
        }
        "tool" => !message
            .name
            .as_deref()
            .is_some_and(is_dispatch_plumbing_tool_name),
        _ => true,
    }
}

fn is_process_only_assistant_message(content: &str) -> bool {
    let trimmed = content.trim();
    matches!(
        trimmed,
        "🔄 子任务当前轮次已完成"
            | "✅ 子任务进程已结束"
            | "⚠️ 子任务进程已失败退出"
            | "⏹️ 子任务进程已取消"
            | "🔄 子任务当前轮次已完成，执行结果已同步供后续分析。"
            | "✅ 子任务进程已结束，执行结果已同步供后续分析。"
            | "⚠️ 子任务进程已失败退出，执行结果已同步供后续分析。"
            | "⏹️ 子任务进程已取消，执行结果已同步供后续分析。"
    ) || trimmed.starts_with("📋 已自动批准 ")
        || content.starts_with("📋 已提交 ")
        || content.starts_with("📨 已向 ")
        || content.starts_with("⏹️ 已向 ")
}

fn is_process_only_assistant_tool_call(message: &ChatMessage) -> bool {
    message
        .tool_calls
        .as_ref()
        .is_some_and(|calls| !calls.is_empty() && calls.iter().all(is_dispatch_plumbing_tool_call))
}

fn is_dispatch_plumbing_tool_call(call: &OutboundToolCall) -> bool {
    is_dispatch_plumbing_tool_name(&call.function.name)
}

fn is_dispatch_plumbing_tool_name(name: &str) -> bool {
    matches!(
        name,
        "dispatch_claude"
            | "dispatch_codex"
            | "continue_claude_session"
            | "continue_codex_session"
            | "exit_claude_session"
            | "exit_codex_session"
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::{
        ChecklistPlanItem, ChecklistPlanState, ChecklistStepStatus, DispatcherDb,
        DispatcherMessageUsageStats, DispatcherMode, DispatcherSessionKind,
        DispatcherSessionTokenUsageSource, PlanInteraction, PlanQuestionOption, ToolArtifactDraft,
    };
    use crate::agent::llm::{LlmPromptTokensDetails, LlmUsage};

    fn create_test_db() -> (DispatcherDb, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("jkcodingagent-db-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create temp db root");
        let db = DispatcherDb::new(root.join("dispatcher.sqlite3")).expect("create dispatcher db");
        (db, root)
    }

    fn cleanup_test_db(root: PathBuf) {
        let _ = fs::remove_dir_all(root);
    }

    fn sample_artifact() -> ToolArtifactDraft {
        ToolArtifactDraft {
            kind: "tool_raw_output".to_string(),
            title: "exec 原始结果".to_string(),
            preview: "line 1 / line 2".to_string(),
            content: "line 1\nline 2".to_string(),
            char_count: 13,
            line_count: 2,
        }
    }

    fn sample_usage(prompt_tokens: u64, completion_tokens: u64, cached_tokens: u64) -> LlmUsage {
        LlmUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
            prompt_tokens_details: Some(LlmPromptTokensDetails { cached_tokens }),
        }
    }

    fn sample_message_usage_stats() -> DispatcherMessageUsageStats {
        DispatcherMessageUsageStats {
            prompt_tokens: 120,
            completion_tokens: 45,
            total_tokens: 165,
            elapsed_ms: 12_345,
        }
    }

    #[test]
    fn update_session_title_persists_latest_title() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "新会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
            )
            .unwrap();

        let updated = db
            .update_session_title(&session.id, "修复会话命名")
            .unwrap()
            .expect("session should exist");

        assert_eq!(updated.title, "修复会话命名");
        assert_eq!(updated.project_id, "project-1");
        assert!(updated.updated_at >= session.updated_at);
        assert_eq!(
            db.list_sessions("project-1", DispatcherSessionKind::Project)
                .unwrap()[0]
                .title,
            "修复会话命名"
        );

        cleanup_test_db(root);
    }

    #[test]
    fn session_kind_isolates_project_and_plain_chat_sessions() {
        let (db, root) = create_test_db();
        db.create_session(
            "project-1",
            "项目会话",
            DispatcherSessionKind::Project,
            DispatcherMode::Default,
            None,
        )
        .unwrap();
        db.create_session(
            "__global_chat__",
            "普通聊天",
            DispatcherSessionKind::Chat,
            DispatcherMode::Default,
            None,
        )
        .unwrap();

        let project_sessions = db
            .list_sessions("project-1", DispatcherSessionKind::Project)
            .unwrap();
        let chat_sessions = db
            .list_sessions("__global_chat__", DispatcherSessionKind::Chat)
            .unwrap();

        assert_eq!(project_sessions.len(), 1);
        assert_eq!(project_sessions[0].kind, DispatcherSessionKind::Project);
        assert_eq!(project_sessions[0].title, "项目会话");
        assert_eq!(chat_sessions.len(), 1);
        assert_eq!(chat_sessions[0].kind, DispatcherSessionKind::Chat);
        assert_eq!(chat_sessions[0].title, "普通聊天");
        assert!(db
            .list_sessions("project-1", DispatcherSessionKind::Chat)
            .unwrap()
            .is_empty());

        cleanup_test_db(root);
    }

    #[test]
    fn session_kind_migration_defaults_legacy_rows_to_project() {
        let root =
            std::env::temp_dir().join(format!("jkcodingagent-db-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create temp db root");
        let path = root.join("dispatcher.sqlite3");
        let conn = rusqlite::Connection::open(&path).expect("open legacy db");
        conn.execute_batch(
            "
            CREATE TABLE dispatcher_sessions (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                title TEXT NOT NULL,
                mode TEXT NOT NULL DEFAULT 'default',
                active_plan_path TEXT,
                checklist_json TEXT,
                plan_interaction_json TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            INSERT INTO dispatcher_sessions (
                id, project_id, title, mode, active_plan_path, created_at, updated_at
            ) VALUES (
                'legacy-session', 'project-1', '旧项目会话', 'default', NULL,
                '2026-05-09T00:00:00Z', '2026-05-09T00:00:00Z'
            );
            ",
        )
        .expect("create legacy dispatcher sessions table");
        drop(conn);

        let db = DispatcherDb::new(path).expect("migrate dispatcher db");
        let project_sessions = db
            .list_sessions("project-1", DispatcherSessionKind::Project)
            .unwrap();

        assert_eq!(project_sessions.len(), 1);
        assert_eq!(project_sessions[0].id, "legacy-session");
        assert_eq!(project_sessions[0].kind, DispatcherSessionKind::Project);
        assert!(db
            .list_sessions("project-1", DispatcherSessionKind::Chat)
            .unwrap()
            .is_empty());

        cleanup_test_db(root);
    }

    #[test]
    fn session_token_usage_accumulates_per_model_and_source() {
        let (db, root) = create_test_db();
        let workspace_id = "session-usage";

        db.upsert_session_token_usage(
            workspace_id,
            "deepseek-chat",
            DispatcherSessionTokenUsageSource::Primary,
            &sample_usage(100, 20, 12),
        )
        .unwrap();
        let primary = db
            .upsert_session_token_usage(
                workspace_id,
                "deepseek-chat",
                DispatcherSessionTokenUsageSource::Primary,
                &sample_usage(80, 15, 8),
            )
            .unwrap();
        db.upsert_session_token_usage(
            workspace_id,
            "deepseek-chat",
            DispatcherSessionTokenUsageSource::Summary,
            &sample_usage(30, 5, 3),
        )
        .unwrap();

        assert_eq!(primary.prompt_tokens, 180);
        assert_eq!(primary.completion_tokens, 35);
        assert_eq!(primary.total_tokens, 215);
        assert_eq!(primary.cached_tokens, 20);
        assert_eq!(primary.context_window_tokens, 80);

        let entries = db.list_session_token_usage(workspace_id).unwrap();
        assert_eq!(entries.len(), 2);
        let summary = entries
            .iter()
            .find(|entry| entry.source_kind == DispatcherSessionTokenUsageSource::Summary)
            .expect("summary usage should be tracked separately");
        assert_eq!(summary.prompt_tokens, 30);
        assert_eq!(summary.total_tokens, 35);

        cleanup_test_db(root);
    }

    #[test]
    fn session_token_usage_derives_total_when_provider_omits_it() {
        let (db, root) = create_test_db();
        let usage = LlmUsage {
            prompt_tokens: 12,
            completion_tokens: 3,
            total_tokens: 0,
            prompt_tokens_details: None,
        };

        let record = db
            .upsert_session_token_usage(
                "session-usage",
                "minimal-compat-model",
                DispatcherSessionTokenUsageSource::Primary,
                &usage,
            )
            .unwrap();

        assert_eq!(record.total_tokens, 15);

        cleanup_test_db(root);
    }

    #[test]
    fn session_token_usage_migration_preserves_existing_rows() {
        let root =
            std::env::temp_dir().join(format!("jkcodingagent-db-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create temp db root");
        let path = root.join("dispatcher.sqlite3");
        let conn = rusqlite::Connection::open(&path).expect("open legacy db");
        conn.execute_batch(
            "
            CREATE TABLE dispatcher_session_token_usage (
                workspace_id TEXT NOT NULL,
                model TEXT NOT NULL,
                source_kind TEXT NOT NULL DEFAULT 'primary',
                prompt_tokens INTEGER NOT NULL DEFAULT 0,
                completion_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                cached_tokens INTEGER NOT NULL DEFAULT 0,
                context_window_tokens INTEGER NOT NULL DEFAULT 0,
                context_window_capacity INTEGER NOT NULL DEFAULT 1000000,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (workspace_id, model)
            );
            INSERT INTO dispatcher_session_token_usage (
                workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
                cached_tokens, context_window_tokens, context_window_capacity, updated_at
            ) VALUES (
                'session-usage', 'same-model', 'primary', 100, 20, 120, 10, 100, 1000000,
                '2026-05-09T00:00:00Z'
            );
            ",
        )
        .expect("create legacy token usage table");
        drop(conn);

        let db = DispatcherDb::new(path).expect("migrate dispatcher db");
        db.upsert_session_token_usage(
            "session-usage",
            "same-model",
            DispatcherSessionTokenUsageSource::Summary,
            &sample_usage(30, 5, 3),
        )
        .unwrap();

        let entries = db.list_session_token_usage("session-usage").unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|entry| entry.source_kind
            == DispatcherSessionTokenUsageSource::Primary
            && entry.prompt_tokens == 100));
        assert!(entries.iter().any(|entry| entry.source_kind
            == DispatcherSessionTokenUsageSource::Summary
            && entry.prompt_tokens == 30));

        cleanup_test_db(root);
    }

    #[test]
    fn session_runtime_state_defaults_to_default_mode() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "新会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
            )
            .unwrap();

        let state = db.get_session_runtime_state(&session.id).unwrap();
        assert_eq!(state.mode, DispatcherMode::Default);
        assert!(state.checklist.is_none());
        assert!(state.plan_interaction.is_none());
        assert!(state.active_plan_path.is_none());

        let state = db
            .set_session_mode(&session.id, DispatcherMode::Plan)
            .unwrap();
        assert_eq!(state.mode, DispatcherMode::Plan);

        cleanup_test_db(root);
    }

    #[test]
    fn visible_message_usage_stats_round_trip() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "测试会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
            )
            .unwrap();
        let stats = sample_message_usage_stats();

        let message = db
            .add_visible_message_with_usage(&session.id, "assistant", "完成", &stats)
            .unwrap();

        assert_eq!(message.usage_stats, Some(stats.clone()));
        let visible_messages = db.list_visible_messages(&session.id).unwrap();
        assert_eq!(visible_messages.len(), 1);
        assert_eq!(visible_messages[0].usage_stats, Some(stats));

        cleanup_test_db(root);
    }

    #[test]
    fn tool_result_separates_display_content_from_context_payload() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "测试会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
            )
            .unwrap();
        db.add_visible_message(&session.id, "user", "检查工具结果")
            .unwrap();
        let message = db
            .add_visible_tool_result(
                &session.id,
                "展示摘要",
                "上下文压缩",
                Some("call-1"),
                Some("exec"),
                Some("conservative_summary"),
                &[sample_artifact()],
            )
            .unwrap();

        assert_eq!(message.content, "展示摘要");
        assert_eq!(message.tool_artifacts.len(), 1);

        let visible_messages = db.list_visible_messages(&session.id).unwrap();
        assert_eq!(visible_messages.last().unwrap().content, "展示摘要");
        assert_eq!(visible_messages.last().unwrap().tool_artifacts.len(), 1);

        let llm_history = db.load_llm_history(&session.id).unwrap();
        let tool_message = llm_history
            .iter()
            .find(|message| message.role == "tool")
            .expect("tool message should exist in llm history");
        assert_eq!(tool_message.content, "上下文压缩");

        let artifact = db
            .get_tool_artifact(&session.id, &message.tool_artifacts[0].id)
            .unwrap();
        assert_eq!(artifact.content, "line 1\nline 2");

        cleanup_test_db(root);
    }

    #[test]
    fn clear_messages_removes_tool_artifacts_for_session() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "测试会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
            )
            .unwrap();
        db.update_checklist(
            &session.id,
            &ChecklistPlanState {
                explanation: Some("规划中".to_string()),
                items: vec![ChecklistPlanItem {
                    id: Some("step_1".to_string()),
                    step: "实现状态机".to_string(),
                    status: ChecklistStepStatus::InProgress,
                    agent: Some("claude".to_string()),
                    dispatch_id: Some("dispatch-1".to_string()),
                    subprocess_task_id: Some("task-1".to_string()),
                    detail: Some("子任务".to_string()),
                }],
                updated_at: "2026-05-09T00:00:00Z".to_string(),
            },
        )
        .unwrap();
        db.set_active_plan_path(&session.id, Some("/repo/.jkcodingagent/plan/demo.md"))
            .unwrap();
        db.set_plan_interaction(
            &session.id,
            Some(&PlanInteraction::Question {
                id: "q1".to_string(),
                question: "怎么做？".to_string(),
                options: vec![PlanQuestionOption {
                    id: "a".to_string(),
                    label: "A".to_string(),
                    description: "方案 A".to_string(),
                }],
            }),
        )
        .unwrap();
        let message = db
            .add_visible_tool_result(
                &session.id,
                "展示摘要",
                "上下文压缩",
                Some("call-1"),
                Some("exec"),
                Some("conservative_summary"),
                &[sample_artifact()],
            )
            .unwrap();

        db.clear_messages(&session.id).unwrap();

        assert!(db.list_visible_messages(&session.id).unwrap().is_empty());
        assert!(db
            .get_tool_artifact(&session.id, &message.tool_artifacts[0].id)
            .is_err());
        let state = db.get_session_runtime_state(&session.id).unwrap();
        assert!(state.checklist.is_none());
        assert!(state.plan_interaction.is_none());
        assert!(state.active_plan_path.is_none());

        cleanup_test_db(root);
    }

    #[test]
    fn clear_context_keeps_visible_messages_but_excludes_them_from_llm_history() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "测试会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
            )
            .unwrap();

        db.add_visible_message(&session.id, "user", "旧需求")
            .unwrap();
        db.add_visible_message(&session.id, "assistant", "旧回复")
            .unwrap();
        db.clear_context_messages(&session.id).unwrap();
        db.add_visible_message(&session.id, "user", "新需求")
            .unwrap();

        let visible_messages = db.list_visible_messages(&session.id).unwrap();
        assert_eq!(visible_messages.len(), 3);
        assert_eq!(visible_messages[0].content, "旧需求");
        assert_eq!(visible_messages[2].content, "新需求");

        let llm_history = db.load_llm_history(&session.id).unwrap();
        assert_eq!(llm_history.len(), 1);
        assert_eq!(llm_history[0].role, "user");
        assert_eq!(llm_history[0].content, "新需求");

        cleanup_test_db(root);
    }

    #[test]
    fn recent_visible_dialogue_messages_keep_complete_latest_turns() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "测试会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
            )
            .unwrap();

        db.add_visible_message(&session.id, "user", "第一轮需求")
            .unwrap();
        db.add_visible_message(&session.id, "assistant", "第一轮回复")
            .unwrap();
        db.add_visible_message(&session.id, "user", "第二轮需求")
            .unwrap();
        db.add_visible_tool_result(
            &session.id,
            "第二轮工具摘要",
            "第二轮工具上下文",
            Some("call-1"),
            Some("exec"),
            Some("summary"),
            &[],
        )
        .unwrap();
        db.add_visible_message(&session.id, "assistant", "第二轮回复")
            .unwrap();
        db.add_visible_message(&session.id, "user", "第三轮需求")
            .unwrap();

        let recent = db
            .list_recent_visible_dialogue_messages(&session.id, 2)
            .unwrap();
        let contents = recent
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            contents,
            vec!["第二轮需求", "第二轮工具摘要", "第二轮回复", "第三轮需求"]
        );

        cleanup_test_db(root);
    }

    #[test]
    fn clear_checklist_removes_only_step_plan_state() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "测试会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
            )
            .unwrap();
        db.update_checklist(
            &session.id,
            &ChecklistPlanState {
                explanation: Some("旧规划".to_string()),
                items: vec![ChecklistPlanItem {
                    id: Some("step_1".to_string()),
                    step: "旧步骤".to_string(),
                    status: ChecklistStepStatus::Completed,
                    agent: None,
                    dispatch_id: None,
                    subprocess_task_id: None,
                    detail: None,
                }],
                updated_at: "2026-05-09T00:00:00Z".to_string(),
            },
        )
        .unwrap();
        db.set_active_plan_path(&session.id, Some("/repo/.jkcodingagent/plan/demo.md"))
            .unwrap();
        db.set_plan_interaction(
            &session.id,
            Some(&PlanInteraction::Ready {
                plan_path: "/repo/.jkcodingagent/plan/demo.md".to_string(),
                title: "Demo".to_string(),
                summary: "ready".to_string(),
            }),
        )
        .unwrap();

        let state = db.clear_checklist(&session.id).unwrap();

        assert!(state.checklist.is_none());
        assert!(state.active_plan_path.is_some());
        assert!(state.plan_interaction.is_some());

        cleanup_test_db(root);
    }

    #[test]
    fn delete_session_removes_tool_artifacts_with_session() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "测试会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
            )
            .unwrap();
        let message = db
            .add_visible_tool_result(
                &session.id,
                "展示摘要",
                "上下文压缩",
                Some("call-1"),
                Some("exec"),
                Some("conservative_summary"),
                &[sample_artifact()],
            )
            .unwrap();

        db.delete_session(&session.id).unwrap();

        assert!(db
            .list_sessions("project-1", DispatcherSessionKind::Project)
            .unwrap()
            .is_empty());
        assert!(db
            .get_tool_artifact(&session.id, &message.tool_artifacts[0].id)
            .is_err());

        cleanup_test_db(root);
    }
}
