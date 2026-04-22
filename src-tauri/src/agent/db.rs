use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::cad::{
    CadBBox, CadEntityDetail, CadEntityEnvelope, CadEntityQueryFilters, CadEntityQueryResult,
    CadEntityRecord, CadPoint, CadReviewIssueRecord, CadReviewRunDetail, CadReviewRunRecord,
    CreateCadReviewRunInput, DispatcherAttachmentRecord, DispatcherAttachmentRef,
    DwgDocumentOverview, DwgDocumentRecord, DwgEntityPayloadRecord, DwgLayerDetail,
    DwgLayerListResult, DwgParseCacheRecord, DwgRegionInspectionResult, SaveDwgDocumentIndexInput,
    SaveDwgEntityPayloadsInput, SaveDwgParseCacheInput,
};
use super::llm::{ChatMessage, OutboundToolCall};
use super::summary::ToolArtifactDraft;

const MAX_LLM_DIALOGUES: usize = 5;

struct DwgDocumentWriteInput<'a> {
    project_path: &'a str,
    file_path: &'a str,
    file_size: u64,
    file_mtime: i64,
    parser_version: &'a str,
    summary: &'a super::cad::DwgParseSummary,
    created_at: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSessionRecord {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSettingsRecord {
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    pub auto_approve_dispatch: bool,
    pub context_debug: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherMessageRecord {
    pub id: String,
    pub workspace_id: String,
    pub role: String,
    pub content: String,
    #[serde(skip_serializing)]
    pub context_payload: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_result_mode: Option<String>,
    pub attachments: Vec<DispatcherAttachmentRef>,
    pub tool_artifacts: Vec<DispatcherToolArtifactRef>,
    pub tool_calls_json: Option<String>,
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

#[derive(Debug, Clone)]
pub struct DispatcherDb {
    path: PathBuf,
}

struct NewDispatcherMessage<'a> {
    workspace_id: &'a str,
    role: &'a str,
    content: &'a str,
    context_payload: Option<&'a str>,
    tool_call_id: Option<&'a str>,
    tool_name: Option<&'a str>,
    tool_result_mode: Option<&'a str>,
    tool_calls: Option<&'a [OutboundToolCall]>,
    attachment_ids: &'a [String],
    tool_artifacts: &'a [ToolArtifactDraft],
    visible: bool,
}

impl DispatcherDb {
    pub fn new(path: PathBuf) -> Result<Self> {
        let db = Self { path };
        db.init()?;
        Ok(db)
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    // ── Settings ──────────────────────────────────────────────

    pub fn get_settings(&self) -> Result<Option<DispatcherSettingsRecord>> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT api_base, api_key, model, auto_approve_dispatch, context_debug FROM dispatcher_settings WHERE id = 'default'",
            [],
            |row| {
                Ok(DispatcherSettingsRecord {
                    api_base: row.get(0)?,
                    api_key: row.get(1)?,
                    model: row.get(2)?,
                    auto_approve_dispatch: row.get::<_, i32>(3)? != 0,
                    context_debug: row.get::<_, i32>(4)? != 0,
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
        auto_approve_dispatch: bool,
        context_debug: bool,
    ) -> Result<DispatcherSettingsRecord> {
        let conn = self.connect()?;
        let auto_approve_int = if auto_approve_dispatch { 1 } else { 0 };
        let context_debug_int = if context_debug { 1 } else { 0 };
        conn.execute(
            "INSERT INTO dispatcher_settings (id, api_base, api_key, model, auto_approve_dispatch, context_debug)
             VALUES ('default', ?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET api_base = ?1, api_key = ?2, model = ?3, auto_approve_dispatch = ?4, context_debug = ?5",
            params![
                api_base.trim(),
                api_key.trim(),
                model.trim(),
                auto_approve_int,
                context_debug_int
            ],
        )
        .context("save dispatcher settings")?;
        Ok(DispatcherSettingsRecord {
            api_base: api_base.trim().to_string(),
            api_key: api_key.trim().to_string(),
            model: model.trim().to_string(),
            auto_approve_dispatch,
            context_debug,
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

    pub fn list_sessions(&self, project_id: &str) -> Result<Vec<DispatcherSessionRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, project_id, title, created_at, updated_at
             FROM dispatcher_sessions
             WHERE project_id = ?1
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(DispatcherSessionRecord {
                id: row.get(0)?,
                project_id: row.get(1)?,
                title: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list dispatcher sessions")
    }

    pub fn create_session(&self, project_id: &str, title: &str) -> Result<DispatcherSessionRecord> {
        let record = DispatcherSessionRecord {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            title: title.to_string(),
            created_at: now(),
            updated_at: now(),
        };

        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO dispatcher_sessions (id, project_id, title, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                record.id,
                record.project_id,
                record.title,
                record.created_at,
                record.updated_at
            ],
        )
        .context("insert dispatcher session")?;

        Ok(record)
    }

    pub fn delete_session(&self, session_id: &str) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM cad_review_issues WHERE run_id IN (
                SELECT id FROM cad_review_runs WHERE workspace_id = ?1
             )",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM cad_review_runs WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_attachments WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_tool_artifacts WHERE workspace_id = ?1",
            params![session_id],
        )?;
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

    // ── Messages ──────────────────────────────────────────────

    pub fn add_visible_message(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            context_payload: None,
            tool_call_id: None,
            tool_name: None,
            tool_result_mode: None,
            tool_calls: None,
            attachment_ids: &[],
            tool_artifacts: &[],
            visible: true,
        })
    }

    pub fn add_visible_user_message(
        &self,
        workspace_id: &str,
        content: &str,
        context_payload: Option<&str>,
        attachment_ids: &[String],
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role: "user",
            content,
            context_payload,
            tool_call_id: None,
            tool_name: None,
            tool_result_mode: None,
            tool_calls: None,
            attachment_ids,
            tool_artifacts: &[],
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
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            context_payload: None,
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls,
            attachment_ids: &[],
            tool_artifacts: &[],
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
            context_payload: Some(context_payload),
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls: None,
            attachment_ids: &[],
            tool_artifacts,
            visible: true,
        })
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
            context_payload: None,
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls,
            attachment_ids: &[],
            tool_artifacts: &[],
            visible: false,
        })
    }

    pub fn list_visible_messages(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<DispatcherMessageRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, role, content, context_payload, tool_call_id, tool_name, tool_result_mode, attachments_json, tool_artifacts_json, tool_calls_json, created_at
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND visible = 1
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id], |row| {
            Ok(DispatcherMessageRecord {
                id: row.get(0)?,
                workspace_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                context_payload: row.get(4)?,
                tool_call_id: row.get(5)?,
                tool_name: row.get(6)?,
                tool_result_mode: row.get(7)?,
                attachments: parse_attachment_refs(row.get::<_, Option<String>>(8)?),
                tool_artifacts: parse_tool_artifact_refs(row.get::<_, Option<String>>(9)?),
                tool_calls_json: row.get(10)?,
                created_at: row.get(11)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("load visible dispatcher messages")
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
            "SELECT role, COALESCE(context_payload, content), tool_call_id, tool_name, tool_calls_json
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND rowid >= ?2
             ORDER BY rowid ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id, cutoff_rowid], map_chat_message_row)?;

        let mut messages = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("load dispatcher llm history")?;
        messages.retain(should_keep_llm_message);

        while matches!(messages.first().map(|m| m.role.as_str()), Some("tool")) {
            messages.remove(0);
        }

        Ok(messages)
    }

    pub fn clear_messages(&self, workspace_id: &str) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM cad_review_issues WHERE run_id IN (
                SELECT id FROM cad_review_runs WHERE workspace_id = ?1
             )",
            params![workspace_id],
        )
        .context("clear cad review issues")?;
        tx.execute(
            "DELETE FROM cad_review_runs WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear cad review runs")?;
        tx.execute(
            "DELETE FROM dispatcher_attachments WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher attachments")?;
        tx.execute(
            "DELETE FROM dispatcher_tool_artifacts WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher tool artifacts")?;
        tx.execute(
            "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher messages")?;
        tx.commit().context("commit dispatcher message cleanup")?;
        Ok(())
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

    pub fn create_attachment(
        &self,
        workspace_id: &str,
        original_name: &str,
        stored_path: &str,
        mime_type: &str,
        size_bytes: u64,
    ) -> Result<DispatcherAttachmentRecord> {
        let record = DispatcherAttachmentRecord {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.to_string(),
            message_id: None,
            original_name: original_name.to_string(),
            stored_path: stored_path.to_string(),
            mime_type: mime_type.to_string(),
            size_bytes,
            created_at: now(),
        };
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO dispatcher_attachments (
                id, workspace_id, message_id, original_name, stored_path, mime_type, size_bytes, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                &record.id,
                &record.workspace_id,
                &record.message_id,
                &record.original_name,
                &record.stored_path,
                &record.mime_type,
                record.size_bytes as i64,
                &record.created_at
            ],
        )
        .context("insert dispatcher attachment")?;
        Ok(record)
    }

    pub fn list_pending_attachments(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<DispatcherAttachmentRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, message_id, original_name, stored_path, mime_type, size_bytes, created_at
             FROM dispatcher_attachments
             WHERE workspace_id = ?1 AND message_id IS NULL
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id], map_attachment_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list pending dispatcher attachments")
    }

    pub fn delete_pending_attachment(&self, workspace_id: &str, attachment_id: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "DELETE FROM dispatcher_attachments
             WHERE id = ?1 AND workspace_id = ?2 AND message_id IS NULL",
            params![attachment_id, workspace_id],
        )
        .context("delete pending dispatcher attachment")?;
        Ok(())
    }

    pub fn get_attachments_by_ids(
        &self,
        workspace_id: &str,
        attachment_ids: &[String],
    ) -> Result<Vec<DispatcherAttachmentRecord>> {
        if attachment_ids.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.connect()?;
        let mut refs = Vec::with_capacity(attachment_ids.len());
        for attachment_id in attachment_ids {
            let record = conn
                .query_row(
                    "SELECT id, workspace_id, message_id, original_name, stored_path, mime_type, size_bytes, created_at
                     FROM dispatcher_attachments
                     WHERE id = ?1 AND workspace_id = ?2",
                    params![attachment_id, workspace_id],
                    map_attachment_row,
                )
                .optional()
                .with_context(|| format!("load dispatcher attachment {attachment_id}"))?
                .with_context(|| format!("dispatcher attachment not found: {attachment_id}"))?;
            refs.push(record);
        }
        Ok(refs)
    }

    pub fn get_dwg_parse_cache(
        &self,
        project_path: &str,
        file_path: &str,
        file_size: u64,
        file_mtime: i64,
        parser_version: &str,
    ) -> Result<Option<DwgParseCacheRecord>> {
        let (project_path, file_path) = normalize_dwg_storage_keys(project_path, file_path);
        let Some(cache) = self.find_matching_dwg_parse_cache(
            &project_path,
            &file_path,
            file_size,
            file_mtime,
            parser_version,
        )?
        else {
            return Ok(None);
        };

        if self.dwg_cache_needs_materialization(&cache)? {
            return self.materialize_dwg_parse_cache(&cache).map(Some);
        }

        Ok(Some(cache))
    }

    pub fn save_dwg_parse_cache(
        &self,
        input: &SaveDwgParseCacheInput,
    ) -> Result<DwgParseCacheRecord> {
        let (project_path, file_path) =
            normalize_dwg_storage_keys(&input.project_path, &input.file_path);
        let mut summary = input.summary.clone();
        summary.file_path = file_path.clone();
        let mut record = DwgParseCacheRecord {
            id: Uuid::new_v4().to_string(),
            project_path,
            file_path,
            file_size: input.file_size,
            file_mtime: input.file_mtime,
            parser_version: input.parser_version.clone(),
            summary,
            document_id: None,
            entities: input.entities.clone(),
            created_at: now(),
        };
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        let document = upsert_dwg_document(
            &tx,
            &DwgDocumentWriteInput {
                project_path: &record.project_path,
                file_path: &record.file_path,
                file_size: record.file_size,
                file_mtime: record.file_mtime,
                parser_version: &record.parser_version,
                summary: &record.summary,
                created_at: &record.created_at,
            },
        )?;
        record.document_id = Some(document.id.clone());
        let summary_json =
            serde_json::to_string(&record.summary).context("serialize dwg parse summary")?;
        let entity_json =
            serde_json::to_string(&record.entities).context("serialize dwg parse entities")?;
        tx.execute(
            "DELETE FROM dwg_parse_cache
             WHERE project_path = ?1 AND file_path = ?2 AND parser_version = ?3",
            params![
                &record.project_path,
                &record.file_path,
                &record.parser_version
            ],
        )
        .context("cleanup stale dwg parse cache")?;
        tx.execute(
            "INSERT INTO dwg_parse_cache (
                id, project_path, file_path, file_size, file_mtime, parser_version, summary_json, document_id, entity_index_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &record.id,
                &record.project_path,
                &record.file_path,
                record.file_size as i64,
                record.file_mtime,
                &record.parser_version,
                summary_json,
                &record.document_id,
                entity_json,
                &record.created_at
            ],
        )
        .context("insert dwg parse cache")?;
        rebuild_dwg_entity_index(&tx, &document.id, &record.entities, &record.created_at)?;
        tx.commit().context("commit dwg parse cache save")?;
        Ok(record)
    }

    pub fn upsert_dwg_document_index(
        &self,
        input: &SaveDwgDocumentIndexInput,
    ) -> Result<DwgDocumentRecord> {
        let (project_path, file_path) =
            normalize_dwg_storage_keys(&input.project_path, &input.file_path);
        let mut summary = input.summary.clone();
        summary.file_path = file_path.clone();
        let created_at = now();
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        let document = upsert_dwg_document(
            &tx,
            &DwgDocumentWriteInput {
                project_path: &project_path,
                file_path: &file_path,
                file_size: input.file_size,
                file_mtime: input.file_mtime,
                parser_version: &input.parser_version,
                summary: &summary,
                created_at: &created_at,
            },
        )?;
        rebuild_dwg_entity_envelopes(&tx, &document.id, &input.envelopes, &created_at)?;
        tx.commit().context("commit dwg document index save")?;
        Ok(document)
    }

    pub fn upsert_dwg_entity_payloads(&self, input: &SaveDwgEntityPayloadsInput) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        let exists = tx
            .query_row(
                "SELECT 1 FROM dwg_documents WHERE id = ?1 LIMIT 1",
                params![&input.doc_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if !exists {
            return Ok(());
        }
        replace_dwg_entity_payloads(&tx, &input.doc_id, &input.payloads, &now())?;
        tx.commit().context("commit dwg entity payload save")
    }

    pub fn get_dwg_document(
        &self,
        project_path: &str,
        file_path: &str,
        file_size: u64,
        file_mtime: i64,
        parser_version: &str,
    ) -> Result<Option<DwgDocumentRecord>> {
        let (project_path, file_path) = normalize_dwg_storage_keys(project_path, file_path);
        if let Some(document) = self.find_matching_dwg_document(
            &project_path,
            &file_path,
            file_size,
            file_mtime,
            parser_version,
        )? {
            if self.dwg_document_has_materialized_index(&document.id)?
                || document.summary.total_entities == 0
            {
                return Ok(Some(document));
            }
        }

        let Some(cache) = self.find_matching_dwg_parse_cache(
            &project_path,
            &file_path,
            file_size,
            file_mtime,
            parser_version,
        )?
        else {
            return Ok(None);
        };
        let cache = self.materialize_dwg_parse_cache(&cache)?;
        let Some(document_id) = cache.document_id.as_deref() else {
            return Ok(None);
        };
        self.get_dwg_document_by_id(document_id)
    }

    pub fn get_dwg_document_by_id(&self, doc_id: &str) -> Result<Option<DwgDocumentRecord>> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT id, project_path, file_path, file_size, file_mtime, parser_version, summary_json, created_at, updated_at
             FROM dwg_documents
             WHERE id = ?1
             LIMIT 1",
            params![doc_id],
            map_dwg_document_row,
        )
        .optional()
        .context("load dwg document by id")
    }

    pub fn get_dwg_document_overview(
        &self,
        project_path: &str,
        file_path: &str,
        file_size: u64,
        file_mtime: i64,
        parser_version: &str,
    ) -> Result<Option<DwgDocumentOverview>> {
        self.get_dwg_document(
            project_path,
            file_path,
            file_size,
            file_mtime,
            parser_version,
        )?
        .map(|document| {
            let next_suggested_actions = vec![
                "先按图层收窄范围".to_string(),
                "如需定位区域，优先用 cad_inspect_dwg_region".to_string(),
                "只对少量目标调用 cad_get_dwg_entity_detail".to_string(),
            ];
            Ok(DwgDocumentOverview {
                document,
                next_suggested_actions,
            })
        })
        .transpose()
    }

    pub fn list_dwg_layers(
        &self,
        doc_id: &str,
        cursor: usize,
        limit: usize,
        sort_by: &str,
    ) -> Result<DwgLayerListResult> {
        let conn = self.connect()?;
        let order_clause = if sort_by == "name" {
            "name COLLATE NOCASE ASC"
        } else {
            "entity_count DESC, name COLLATE NOCASE ASC"
        };
        let total = conn.query_row(
            "SELECT COUNT(*) FROM (
                SELECT layer FROM dwg_entity_envelopes WHERE doc_id = ?1 GROUP BY layer
             )",
            params![doc_id],
            |row| row.get::<_, i64>(0),
        )? as usize;

        let sql = format!(
            "SELECT layer, COUNT(*) as entity_count
             FROM dwg_entity_envelopes
             WHERE doc_id = ?1
             GROUP BY layer
             ORDER BY {order_clause}
             LIMIT ?2 OFFSET ?3"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![doc_id, limit as i64, cursor as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?;

        let mut items = Vec::new();
        for row in rows {
            let (layer_name, entity_count) = row?;
            let mut type_stmt = conn.prepare(
                "SELECT entity_type, COUNT(*)
                 FROM dwg_entity_envelopes
                 WHERE doc_id = ?1 AND layer = ?2
                 GROUP BY entity_type
                 ORDER BY COUNT(*) DESC, entity_type COLLATE NOCASE ASC
                 LIMIT 8",
            )?;
            let type_rows = type_stmt.query_map(params![doc_id, &layer_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
            })?;
            let entity_types = type_rows
                .collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .collect::<BTreeMap<_, _>>();
            items.push(DwgLayerDetail {
                name: layer_name,
                entity_count,
                entity_types,
            });
        }

        let next_cursor = (cursor + items.len() < total).then_some(cursor + items.len());
        Ok(DwgLayerListResult {
            items,
            total,
            next_cursor,
        })
    }

    pub fn query_dwg_entities(
        &self,
        doc_id: &str,
        filters: &CadEntityQueryFilters,
        cursor: usize,
        limit: usize,
    ) -> Result<CadEntityQueryResult> {
        let conn = self.connect()?;
        let (join_clause, where_clause, params) = build_envelope_query(doc_id, filters);
        let count_sql = format!(
            "SELECT COUNT(*) FROM dwg_entity_envelopes e {join_clause} WHERE {where_clause}"
        );
        let total = query_count(&conn, &count_sql, &params)?;
        let query_sql = format!(
            "SELECT e.row_id, e.doc_id, e.entity_id, e.handle, e.entity_type, e.raw_type, e.layer, e.block_name,
                    e.text_excerpt, e.normalized_text, e.center_x, e.center_y, e.anchor_x, e.anchor_y,
                    e.bbox_min_x, e.bbox_min_y, e.bbox_max_x, e.bbox_max_y, e.layout, e.owner_block,
                    e.rotation_deg, e.scale_x, e.scale_y
             FROM dwg_entity_envelopes e
             {join_clause}
             WHERE {where_clause}
             ORDER BY e.sort_key ASC, e.row_id ASC
             LIMIT ? OFFSET ?"
        );
        let mut query_params = params.clone();
        query_params.push(SqlValue::Integer(limit as i64));
        query_params.push(SqlValue::Integer(cursor as i64));
        let mut stmt = conn.prepare(&query_sql)?;
        let rows = stmt.query_map(
            rusqlite::params_from_iter(query_params.iter()),
            map_envelope_row,
        )?;
        let items = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let next_cursor = (cursor + items.len() < total).then_some(cursor + items.len());
        Ok(CadEntityQueryResult {
            items,
            total,
            next_cursor,
            applied_filters: filters.clone(),
        })
    }

    pub fn get_dwg_entity_details(
        &self,
        doc_id: &str,
        entity_ids: &[String],
    ) -> Result<Vec<CadEntityDetail>> {
        if entity_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.connect()?;
        let placeholders = std::iter::repeat_n("?", entity_ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT e.row_id, e.doc_id, e.entity_id, e.handle, e.entity_type, e.raw_type, e.layer, e.block_name,
                    e.text_excerpt, e.normalized_text, e.center_x, e.center_y, e.anchor_x, e.anchor_y,
                    e.bbox_min_x, e.bbox_min_y, e.bbox_max_x, e.bbox_max_y, e.layout, e.owner_block,
                    e.rotation_deg, e.scale_x, e.scale_y
             FROM dwg_entity_envelopes e
             WHERE e.doc_id = ? AND e.entity_id IN ({placeholders})"
        );
        let mut params = vec![SqlValue::Text(doc_id.to_string())];
        params.extend(entity_ids.iter().cloned().map(SqlValue::Text));
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), map_envelope_row)?;
        let envelopes = rows.collect::<rusqlite::Result<Vec<_>>>()?;

        let payload_sql = format!(
            "SELECT entity_id, payload_json
             FROM dwg_entity_payloads
             WHERE doc_id = ? AND entity_id IN ({placeholders})"
        );
        let mut payload_stmt = conn.prepare(&payload_sql)?;
        let payload_rows =
            payload_stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    parse_json_column::<Value>(row.get(1)?)?,
                ))
            })?;
        let payloads = payload_rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .collect::<HashMap<_, _>>();
        let envelope_map = envelopes
            .into_iter()
            .map(|envelope| (envelope.id.clone(), envelope))
            .collect::<HashMap<_, _>>();

        Ok(entity_ids
            .iter()
            .filter_map(|entity_id| {
                envelope_map
                    .get(entity_id)
                    .cloned()
                    .map(|envelope| CadEntityDetail {
                        payload: payloads.get(entity_id).cloned(),
                        envelope,
                    })
            })
            .collect())
    }

    pub fn inspect_dwg_region(
        &self,
        doc_id: &str,
        bbox: &CadBBox,
        group_by: &str,
        sample_limit: usize,
    ) -> Result<DwgRegionInspectionResult> {
        let filters = CadEntityQueryFilters {
            bbox: Some(bbox.clone()),
            ..CadEntityQueryFilters::default()
        };
        let sample = self.query_dwg_entities(doc_id, &filters, 0, sample_limit)?;
        let conn = self.connect()?;
        let group_column = if group_by == "layer" {
            "layer"
        } else {
            "entity_type"
        };
        let sql = format!(
            "SELECT {group_column}, COUNT(*)
             FROM dwg_entity_envelopes
             WHERE doc_id = ?1
               AND bbox_min_x IS NOT NULL
               AND bbox_min_y IS NOT NULL
               AND bbox_max_x IS NOT NULL
               AND bbox_max_y IS NOT NULL
               AND bbox_min_x <= ?2
               AND bbox_max_x >= ?3
               AND bbox_min_y <= ?4
               AND bbox_max_y >= ?5
             GROUP BY {group_column}
             ORDER BY COUNT(*) DESC, {group_column} COLLATE NOCASE ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let group_rows = stmt.query_map(
            params![doc_id, bbox.max_x, bbox.min_x, bbox.max_y, bbox.min_y],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize)),
        )?;
        let group_counts = group_rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let mut text_stmt = conn.prepare(
            "SELECT DISTINCT text_excerpt
             FROM dwg_entity_envelopes
             WHERE doc_id = ?1
               AND text_excerpt IS NOT NULL
               AND bbox_min_x IS NOT NULL
               AND bbox_min_y IS NOT NULL
               AND bbox_max_x IS NOT NULL
               AND bbox_max_y IS NOT NULL
               AND bbox_min_x <= ?2
               AND bbox_max_x >= ?3
               AND bbox_min_y <= ?4
               AND bbox_max_y >= ?5
             LIMIT 12",
        )?;
        let text_samples = text_stmt
            .query_map(
                params![doc_id, bbox.max_x, bbox.min_x, bbox.max_y, bbox.min_y],
                |row| row.get::<_, Option<String>>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        Ok(DwgRegionInspectionResult {
            bbox: bbox.clone(),
            group_by: group_by.to_string(),
            group_counts,
            text_samples,
            items: sample.items,
            next_suggested_actions: vec![
                "如果区域仍然过大，继续缩小 bbox".to_string(),
                "对命中的实体再调用 cad_get_dwg_entity_detail".to_string(),
            ],
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn query_dwg_parse_entities(
        &self,
        project_path: &str,
        file_path: &str,
        file_size: u64,
        file_mtime: i64,
        parser_version: &str,
        filters: &CadEntityQueryFilters,
        cursor: usize,
        limit: usize,
    ) -> Result<Option<CadEntityQueryResult>> {
        let Some(cache) = self.get_dwg_parse_cache(
            project_path,
            file_path,
            file_size,
            file_mtime,
            parser_version,
        )?
        else {
            return Ok(None);
        };
        let Some(document_id) = cache.document_id.clone() else {
            return Ok(None);
        };
        Ok(Some(self.query_dwg_entities(
            &document_id,
            filters,
            cursor,
            limit,
        )?))
    }

    pub fn create_cad_review_run(
        &self,
        input: &CreateCadReviewRunInput,
    ) -> Result<CadReviewRunDetail> {
        let run = CadReviewRunRecord {
            id: Uuid::new_v4().to_string(),
            workspace_id: input.workspace_id.clone(),
            file_path: input.file_path.clone(),
            source_message_id: input.source_message_id.clone(),
            result_message_id: None,
            rule_attachment_ids: input.rule_attachment_ids.clone(),
            goal: input.goal.clone(),
            status: input.status.clone(),
            summary: input.summary.clone(),
            issue_count: input.issues.len(),
            created_at: now(),
        };

        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO cad_review_runs (
                id, workspace_id, file_path, source_message_id, result_message_id, rule_attachment_ids_json, goal, status, summary, issue_count, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                &run.id,
                &run.workspace_id,
                &run.file_path,
                &run.source_message_id,
                &run.result_message_id,
                serde_json::to_string(&run.rule_attachment_ids)?,
                &run.goal,
                &run.status,
                &run.summary,
                run.issue_count as i64,
                &run.created_at
            ],
        )
        .context("insert cad review run")?;

        let mut issues = Vec::with_capacity(input.issues.len());
        for issue in &input.issues {
            let record = CadReviewIssueRecord {
                id: Uuid::new_v4().to_string(),
                run_id: run.id.clone(),
                severity: issue.severity.clone(),
                title: issue.title.clone(),
                description: issue.description.clone(),
                layer: issue.layer.clone(),
                entity_refs: issue.entity_refs.clone(),
                anchor_point: issue.anchor_point.clone(),
                bbox: issue.bbox.clone(),
                viewport_hint: issue.viewport_hint.clone(),
                rule_ref: issue.rule_ref.clone(),
                created_at: run.created_at.clone(),
            };
            tx.execute(
                "INSERT INTO cad_review_issues (
                    id, run_id, severity, title, description, layer, entity_refs_json, anchor_point_json, bbox_json, viewport_hint_json, rule_ref, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    &record.id,
                    &record.run_id,
                    &record.severity,
                    &record.title,
                    &record.description,
                    &record.layer,
                    serde_json::to_string(&record.entity_refs)?,
                    json_string(&record.anchor_point)?,
                    json_string(&record.bbox)?,
                    json_string(&record.viewport_hint)?,
                    &record.rule_ref,
                    &record.created_at
                ],
            )
            .context("insert cad review issue")?;
            issues.push(record);
        }

        tx.commit().context("commit cad review run")?;
        Ok(CadReviewRunDetail { run, issues })
    }

    pub fn bind_cad_review_result_message(
        &self,
        run_id: &str,
        result_message_id: &str,
    ) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE cad_review_runs SET result_message_id = ?1 WHERE id = ?2",
            params![result_message_id, run_id],
        )
        .context("bind cad review result message")?;
        Ok(())
    }

    pub fn list_cad_review_runs(
        &self,
        workspace_id: &str,
        file_path: Option<&str>,
    ) -> Result<Vec<CadReviewRunRecord>> {
        let conn = self.connect()?;
        if let Some(file_path) = file_path {
            let mut stmt = conn.prepare(
                "SELECT id, workspace_id, file_path, source_message_id, result_message_id, rule_attachment_ids_json, goal, status, summary, issue_count, created_at
                 FROM cad_review_runs
                 WHERE workspace_id = ?1 AND file_path = ?2
                 ORDER BY created_at DESC, rowid DESC",
            )?;
            let rows = stmt.query_map(params![workspace_id, file_path], map_review_run_row)?;
            return rows
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("list cad review runs by file");
        }

        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, file_path, source_message_id, result_message_id, rule_attachment_ids_json, goal, status, summary, issue_count, created_at
             FROM cad_review_runs
             WHERE workspace_id = ?1
             ORDER BY created_at DESC, rowid DESC",
        )?;
        let rows = stmt.query_map(params![workspace_id], map_review_run_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list cad review runs")
    }

    pub fn get_cad_review_run_detail(
        &self,
        workspace_id: &str,
        run_id: &str,
    ) -> Result<CadReviewRunDetail> {
        let conn = self.connect()?;
        let run = conn
            .query_row(
                "SELECT id, workspace_id, file_path, source_message_id, result_message_id, rule_attachment_ids_json, goal, status, summary, issue_count, created_at
                 FROM cad_review_runs
                 WHERE workspace_id = ?1 AND id = ?2",
                params![workspace_id, run_id],
                map_review_run_row,
            )
            .optional()
            .context("load cad review run")?
            .with_context(|| format!("cad review run not found: {run_id}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, run_id, severity, title, description, layer, entity_refs_json, anchor_point_json, bbox_json, viewport_hint_json, rule_ref, created_at
             FROM cad_review_issues
             WHERE run_id = ?1
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(params![run_id], map_review_issue_row)?;
        let issues = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("load cad review issues")?;
        Ok(CadReviewRunDetail { run, issues })
    }

    fn add_message(&self, params: NewDispatcherMessage<'_>) -> Result<DispatcherMessageRecord> {
        let tool_calls_json = params
            .tool_calls
            .map(serde_json::to_string)
            .transpose()
            .context("serialize tool calls")?;

        let mut record = DispatcherMessageRecord {
            id: Uuid::new_v4().to_string(),
            workspace_id: params.workspace_id.to_string(),
            role: params.role.to_string(),
            content: params.content.to_string(),
            context_payload: params.context_payload.map(|s| s.to_string()),
            tool_call_id: params.tool_call_id.map(|s| s.to_string()),
            tool_name: params.tool_name.map(|s| s.to_string()),
            tool_result_mode: params.tool_result_mode.map(|s| s.to_string()),
            attachments: Vec::new(),
            tool_artifacts: Vec::new(),
            tool_calls_json: tool_calls_json.clone(),
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
                id, workspace_id, role, content, context_payload, tool_call_id, tool_name, tool_result_mode, attachments_json, tool_artifacts_json, tool_calls_json, visible, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                &record.id,
                &record.workspace_id,
                &record.role,
                &record.content,
                &record.context_payload,
                &record.tool_call_id,
                &record.tool_name,
                &record.tool_result_mode,
                Option::<String>::None,
                Option::<String>::None,
                &record.tool_calls_json,
                if params.visible { 1 } else { 0 },
                &record.created_at
            ],
        )
        .context("insert dispatcher message")?;

        if !params.attachment_ids.is_empty() {
            let attachments = self.bind_attachments_to_message(
                &tx,
                &record.workspace_id,
                &record.id,
                params.attachment_ids,
            )?;
            let attachments_json =
                serde_json::to_string(&attachments).context("serialize dispatcher attachments")?;
            tx.execute(
                "UPDATE dispatcher_messages SET attachments_json = ?1 WHERE id = ?2",
                params![&attachments_json, &record.id],
            )
            .context("attach dispatcher attachments to message")?;
            record.attachments = attachments;
        }

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

    fn bind_attachments_to_message(
        &self,
        tx: &rusqlite::Transaction<'_>,
        workspace_id: &str,
        message_id: &str,
        attachment_ids: &[String],
    ) -> Result<Vec<DispatcherAttachmentRef>> {
        let mut refs = Vec::with_capacity(attachment_ids.len());
        for attachment_id in attachment_ids {
            tx.execute(
                "UPDATE dispatcher_attachments
                 SET message_id = ?1
                 WHERE id = ?2 AND workspace_id = ?3",
                params![message_id, attachment_id, workspace_id],
            )
            .with_context(|| format!("bind dispatcher attachment {attachment_id}"))?;

            let record = tx
                .query_row(
                    "SELECT id, workspace_id, message_id, original_name, stored_path, mime_type, size_bytes, created_at
                     FROM dispatcher_attachments
                     WHERE id = ?1 AND workspace_id = ?2",
                    params![attachment_id, workspace_id],
                    map_attachment_row,
                )
                .optional()
                .with_context(|| format!("load bound dispatcher attachment {attachment_id}"))?
                .with_context(|| format!("dispatcher attachment not found: {attachment_id}"))?;

            refs.push(DispatcherAttachmentRef {
                id: record.id,
                original_name: record.original_name,
                stored_path: record.stored_path,
                mime_type: record.mime_type,
                size_bytes: record.size_bytes,
                created_at: record.created_at,
            });
        }

        Ok(refs)
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
        let conn = self.connect()?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS dispatcher_sessions (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                title TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_sessions_project
            ON dispatcher_sessions(project_id, updated_at DESC);

            CREATE TABLE IF NOT EXISTS dispatcher_messages (
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                context_payload TEXT,
                tool_call_id TEXT,
                tool_name TEXT,
                tool_result_mode TEXT,
                attachments_json TEXT,
                tool_artifacts_json TEXT,
                tool_calls_json TEXT,
                visible INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_messages_workspace_created
            ON dispatcher_messages(workspace_id, created_at);

            CREATE TABLE IF NOT EXISTS dispatcher_attachments (
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                message_id TEXT,
                original_name TEXT NOT NULL,
                stored_path TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                size_bytes INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_attachments_workspace_created
            ON dispatcher_attachments(workspace_id, created_at);

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

            CREATE TABLE IF NOT EXISTS dwg_parse_cache (
                id TEXT PRIMARY KEY,
                project_path TEXT NOT NULL,
                file_path TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                file_mtime INTEGER NOT NULL,
                parser_version TEXT NOT NULL,
                summary_json TEXT NOT NULL,
                document_id TEXT,
                entity_index_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dwg_parse_cache_lookup
            ON dwg_parse_cache(project_path, file_path, parser_version, created_at DESC);

            CREATE TABLE IF NOT EXISTS dwg_documents (
                id TEXT PRIMARY KEY,
                project_path TEXT NOT NULL,
                file_path TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                file_mtime INTEGER NOT NULL,
                parser_version TEXT NOT NULL,
                summary_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE UNIQUE INDEX IF NOT EXISTS idx_dwg_documents_lookup
            ON dwg_documents(project_path, file_path, file_size, file_mtime, parser_version);

            CREATE TABLE IF NOT EXISTS dwg_entity_envelopes (
                row_id INTEGER PRIMARY KEY AUTOINCREMENT,
                doc_id TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                handle TEXT NOT NULL,
                entity_type TEXT NOT NULL,
                raw_type TEXT NOT NULL,
                layer TEXT NOT NULL,
                block_name TEXT,
                text_excerpt TEXT,
                normalized_text TEXT,
                center_x REAL,
                center_y REAL,
                anchor_x REAL,
                anchor_y REAL,
                bbox_min_x REAL,
                bbox_min_y REAL,
                bbox_max_x REAL,
                bbox_max_y REAL,
                layout TEXT,
                owner_block TEXT,
                rotation_deg REAL,
                scale_x REAL,
                scale_y REAL,
                sort_key INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );

            CREATE UNIQUE INDEX IF NOT EXISTS idx_dwg_entity_envelopes_doc_entity
            ON dwg_entity_envelopes(doc_id, entity_id);

            CREATE INDEX IF NOT EXISTS idx_dwg_entity_envelopes_doc_sort
            ON dwg_entity_envelopes(doc_id, sort_key, row_id);

            CREATE INDEX IF NOT EXISTS idx_dwg_entity_envelopes_doc_layer_type
            ON dwg_entity_envelopes(doc_id, layer, entity_type);

            CREATE INDEX IF NOT EXISTS idx_dwg_entity_envelopes_doc_bbox
            ON dwg_entity_envelopes(doc_id, bbox_min_x, bbox_min_y, bbox_max_x, bbox_max_y);

            CREATE TABLE IF NOT EXISTS dwg_entity_payloads (
                doc_id TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (doc_id, entity_id)
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS dwg_entity_rtree USING rtree(
                row_id,
                min_x,
                max_x,
                min_y,
                max_y
            );

            CREATE TABLE IF NOT EXISTS cad_review_runs (
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                file_path TEXT NOT NULL,
                source_message_id TEXT NOT NULL,
                result_message_id TEXT,
                rule_attachment_ids_json TEXT NOT NULL DEFAULT '[]',
                goal TEXT NOT NULL,
                status TEXT NOT NULL,
                summary TEXT NOT NULL,
                issue_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_cad_review_runs_workspace_file
            ON cad_review_runs(workspace_id, file_path, created_at DESC);

            CREATE TABLE IF NOT EXISTS cad_review_issues (
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                severity TEXT NOT NULL,
                title TEXT NOT NULL,
                description TEXT NOT NULL,
                layer TEXT,
                entity_refs_json TEXT NOT NULL DEFAULT '[]',
                anchor_point_json TEXT,
                bbox_json TEXT,
                viewport_hint_json TEXT,
                rule_ref TEXT,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_cad_review_issues_run
            ON cad_review_issues(run_id, created_at);

            CREATE TABLE IF NOT EXISTS dispatcher_settings (
                id TEXT PRIMARY KEY DEFAULT 'default',
                api_base TEXT NOT NULL DEFAULT '',
                api_key TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL DEFAULT '',
                auto_approve_dispatch INTEGER NOT NULL DEFAULT 0,
                context_debug INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
        .context("initialize dispatcher sqlite schema")?;
        ensure_column_exists(
            &conn,
            "dispatcher_settings",
            "context_debug",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column_exists(&conn, "dispatcher_messages", "context_payload", "TEXT")?;
        ensure_column_exists(&conn, "dispatcher_messages", "tool_result_mode", "TEXT")?;
        ensure_column_exists(&conn, "dispatcher_messages", "attachments_json", "TEXT")?;
        ensure_column_exists(&conn, "dispatcher_messages", "tool_artifacts_json", "TEXT")?;
        ensure_column_exists(&conn, "dwg_parse_cache", "document_id", "TEXT")?;
        ensure_column_exists(&conn, "cad_review_issues", "viewport_hint_json", "TEXT")?;
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
        let mut stmt = conn.prepare(
            "SELECT rowid
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND role = 'user'
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

fn map_attachment_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DispatcherAttachmentRecord> {
    Ok(DispatcherAttachmentRecord {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        message_id: row.get(2)?,
        original_name: row.get(3)?,
        stored_path: row.get(4)?,
        mime_type: row.get(5)?,
        size_bytes: row.get::<_, i64>(6)? as u64,
        created_at: row.get(7)?,
    })
}

fn map_parse_cache_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DwgParseCacheRecord> {
    Ok(DwgParseCacheRecord {
        id: row.get(0)?,
        project_path: row.get(1)?,
        file_path: row.get(2)?,
        file_size: row.get::<_, i64>(3)? as u64,
        file_mtime: row.get(4)?,
        parser_version: row.get(5)?,
        summary: parse_json_column(row.get(6)?)?,
        document_id: row.get(7)?,
        entities: parse_json_column(row.get(8)?)?,
        created_at: row.get(9)?,
    })
}

fn map_review_run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CadReviewRunRecord> {
    Ok(CadReviewRunRecord {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        file_path: row.get(2)?,
        source_message_id: row.get(3)?,
        result_message_id: row.get(4)?,
        rule_attachment_ids: parse_json_column(row.get(5)?)?,
        goal: row.get(6)?,
        status: row.get(7)?,
        summary: row.get(8)?,
        issue_count: row.get::<_, i64>(9)? as usize,
        created_at: row.get(10)?,
    })
}

fn map_review_issue_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CadReviewIssueRecord> {
    Ok(CadReviewIssueRecord {
        id: row.get(0)?,
        run_id: row.get(1)?,
        severity: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        layer: row.get(5)?,
        entity_refs: parse_json_column(row.get(6)?)?,
        anchor_point: parse_optional_json_column(row.get(7)?)?,
        bbox: parse_optional_json_column(row.get(8)?)?,
        viewport_hint: parse_optional_json_column(row.get(9)?)?,
        rule_ref: row.get(10)?,
        created_at: row.get(11)?,
    })
}

fn parse_attachment_refs(raw: Option<String>) -> Vec<DispatcherAttachmentRef> {
    raw.and_then(|json| serde_json::from_str::<Vec<DispatcherAttachmentRef>>(&json).ok())
        .unwrap_or_default()
}

fn parse_json_column<T: for<'de> Deserialize<'de>>(raw: String) -> rusqlite::Result<T> {
    serde_json::from_str(&raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

fn parse_optional_json_column<T: for<'de> Deserialize<'de>>(
    raw: Option<String>,
) -> rusqlite::Result<Option<T>> {
    raw.map(parse_json_column).transpose()
}

fn map_dwg_document_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DwgDocumentRecord> {
    Ok(DwgDocumentRecord {
        id: row.get(0)?,
        project_path: row.get(1)?,
        file_path: row.get(2)?,
        file_size: row.get::<_, i64>(3)? as u64,
        file_mtime: row.get(4)?,
        parser_version: row.get(5)?,
        summary: parse_json_column(row.get(6)?)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn map_envelope_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CadEntityEnvelope> {
    Ok(CadEntityEnvelope {
        id: row.get(2)?,
        handle: row.get(3)?,
        entity_type: row.get(4)?,
        raw_type: row.get(5)?,
        layer: row.get(6)?,
        block_name: row.get(7)?,
        text_excerpt: row.get(8)?,
        normalized_text: row.get(9)?,
        center: match (
            row.get::<_, Option<f64>>(10)?,
            row.get::<_, Option<f64>>(11)?,
        ) {
            (Some(x), Some(y)) => Some(CadPoint { x, y }),
            _ => None,
        },
        anchor: match (
            row.get::<_, Option<f64>>(12)?,
            row.get::<_, Option<f64>>(13)?,
        ) {
            (Some(x), Some(y)) => Some(CadPoint { x, y }),
            _ => None,
        },
        bbox: match (
            row.get::<_, Option<f64>>(14)?,
            row.get::<_, Option<f64>>(15)?,
            row.get::<_, Option<f64>>(16)?,
            row.get::<_, Option<f64>>(17)?,
        ) {
            (Some(min_x), Some(min_y), Some(max_x), Some(max_y)) => Some(CadBBox {
                min_x,
                min_y,
                max_x,
                max_y,
            }),
            _ => None,
        },
        layout: row.get(18)?,
        owner_block: row.get(19)?,
        rotation_deg: row.get(20)?,
        scale_x: row.get(21)?,
        scale_y: row.get(22)?,
    })
}

fn upsert_dwg_document(
    tx: &rusqlite::Transaction<'_>,
    record: &DwgDocumentWriteInput<'_>,
) -> Result<DwgDocumentRecord> {
    let existing = tx
        .query_row(
            "SELECT id, project_path, file_path, file_size, file_mtime, parser_version, summary_json, created_at, updated_at
             FROM dwg_documents
             WHERE project_path = ?1 AND file_path = ?2 AND file_size = ?3 AND file_mtime = ?4 AND parser_version = ?5
             LIMIT 1",
            params![
                record.project_path,
                record.file_path,
                record.file_size as i64,
                record.file_mtime,
                record.parser_version
            ],
            map_dwg_document_row,
        )
        .optional()
        .context("query existing dwg document")?;
    let summary_json = serde_json::to_string(record.summary)?;
    let now = now();
    if let Some(mut document) = existing {
        tx.execute(
            "UPDATE dwg_documents
             SET summary_json = ?1, updated_at = ?2
             WHERE id = ?3",
            params![summary_json, &now, &document.id],
        )?;
        document.summary = record.summary.clone();
        document.updated_at = now;
        return Ok(document);
    }

    let document = DwgDocumentRecord {
        id: Uuid::new_v4().to_string(),
        project_path: record.project_path.to_string(),
        file_path: record.file_path.to_string(),
        file_size: record.file_size,
        file_mtime: record.file_mtime,
        parser_version: record.parser_version.to_string(),
        summary: record.summary.clone(),
        created_at: record.created_at.to_string(),
        updated_at: now,
    };
    tx.execute(
        "INSERT INTO dwg_documents (
            id, project_path, file_path, file_size, file_mtime, parser_version, summary_json, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            &document.id,
            &document.project_path,
            &document.file_path,
            document.file_size as i64,
            document.file_mtime,
            &document.parser_version,
            serde_json::to_string(&document.summary)?,
            &document.created_at,
            &document.updated_at
        ],
    )?;
    Ok(document)
}

fn normalize_dwg_storage_keys(project_path: &str, file_path: &str) -> (String, String) {
    (
        normalize_dwg_storage_path(project_path),
        normalize_dwg_storage_path(file_path),
    )
}

fn normalize_dwg_storage_path(path: &str) -> String {
    let candidate = PathBuf::from(path);
    let normalized = if candidate.exists() {
        candidate.canonicalize().unwrap_or(candidate)
    } else {
        lexical_normalize_path(&candidate)
    };
    normalized.to_string_lossy().into_owned()
}

fn lexical_normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

impl DispatcherDb {
    fn find_matching_dwg_parse_cache(
        &self,
        project_path: &str,
        file_path: &str,
        file_size: u64,
        file_mtime: i64,
        parser_version: &str,
    ) -> Result<Option<DwgParseCacheRecord>> {
        let conn = self.connect()?;
        if let Some(record) = conn
            .query_row(
                "SELECT id, project_path, file_path, file_size, file_mtime, parser_version, summary_json, document_id, entity_index_json, created_at
                 FROM dwg_parse_cache
                 WHERE project_path = ?1 AND file_path = ?2 AND file_size = ?3 AND file_mtime = ?4 AND parser_version = ?5
                 ORDER BY created_at DESC
                 LIMIT 1",
                params![
                    project_path,
                    file_path,
                    file_size as i64,
                    file_mtime,
                    parser_version
                ],
                map_parse_cache_row,
            )
            .optional()
            .context("load dwg parse cache")?
        {
            return Ok(Some(record));
        }

        let mut stmt = conn.prepare(
            "SELECT id, project_path, file_path, file_size, file_mtime, parser_version, summary_json, document_id, entity_index_json, created_at
             FROM dwg_parse_cache
             WHERE file_size = ?1 AND file_mtime = ?2 AND parser_version = ?3
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(
            params![file_size as i64, file_mtime, parser_version],
            map_parse_cache_row,
        )?;
        for row in rows {
            let record = row?;
            let (candidate_project_path, candidate_file_path) =
                normalize_dwg_storage_keys(&record.project_path, &record.file_path);
            if candidate_project_path == project_path && candidate_file_path == file_path {
                return Ok(Some(record));
            }
        }
        Ok(None)
    }

    fn find_matching_dwg_document(
        &self,
        project_path: &str,
        file_path: &str,
        file_size: u64,
        file_mtime: i64,
        parser_version: &str,
    ) -> Result<Option<DwgDocumentRecord>> {
        let conn = self.connect()?;
        if let Some(document) = conn
            .query_row(
                "SELECT id, project_path, file_path, file_size, file_mtime, parser_version, summary_json, created_at, updated_at
                 FROM dwg_documents
                 WHERE project_path = ?1 AND file_path = ?2 AND file_size = ?3 AND file_mtime = ?4 AND parser_version = ?5
                 LIMIT 1",
                params![
                    project_path,
                    file_path,
                    file_size as i64,
                    file_mtime,
                    parser_version
                ],
                map_dwg_document_row,
            )
            .optional()
            .context("load dwg document")?
        {
            return Ok(Some(document));
        }

        let mut stmt = conn.prepare(
            "SELECT id, project_path, file_path, file_size, file_mtime, parser_version, summary_json, created_at, updated_at
             FROM dwg_documents
             WHERE file_size = ?1 AND file_mtime = ?2 AND parser_version = ?3",
        )?;
        let rows = stmt.query_map(
            params![file_size as i64, file_mtime, parser_version],
            map_dwg_document_row,
        )?;
        for row in rows {
            let document = row?;
            let (candidate_project_path, candidate_file_path) =
                normalize_dwg_storage_keys(&document.project_path, &document.file_path);
            if candidate_project_path == project_path && candidate_file_path == file_path {
                return Ok(Some(document));
            }
        }
        Ok(None)
    }

    fn dwg_cache_needs_materialization(&self, cache: &DwgParseCacheRecord) -> Result<bool> {
        let Some(document_id) = cache.document_id.as_deref() else {
            return Ok(true);
        };
        if self.get_dwg_document_by_id(document_id)?.is_none() {
            return Ok(true);
        }
        if cache.entities.is_empty() {
            return Ok(false);
        }
        Ok(!self.dwg_document_has_materialized_index(document_id)?)
    }

    fn dwg_document_has_materialized_index(&self, doc_id: &str) -> Result<bool> {
        let conn = self.connect()?;
        let count = conn.query_row(
            "SELECT COUNT(*) FROM dwg_entity_envelopes WHERE doc_id = ?1",
            params![doc_id],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count > 0)
    }

    fn materialize_dwg_parse_cache(
        &self,
        cache: &DwgParseCacheRecord,
    ) -> Result<DwgParseCacheRecord> {
        let (project_path, file_path) =
            normalize_dwg_storage_keys(&cache.project_path, &cache.file_path);
        let mut normalized_cache = cache.clone();
        normalized_cache.project_path = project_path;
        normalized_cache.file_path = file_path.clone();
        normalized_cache.summary.file_path = file_path;

        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        let document = upsert_dwg_document(
            &tx,
            &DwgDocumentWriteInput {
                project_path: &normalized_cache.project_path,
                file_path: &normalized_cache.file_path,
                file_size: normalized_cache.file_size,
                file_mtime: normalized_cache.file_mtime,
                parser_version: &normalized_cache.parser_version,
                summary: &normalized_cache.summary,
                created_at: &normalized_cache.created_at,
            },
        )?;
        normalized_cache.document_id = Some(document.id.clone());
        rebuild_dwg_entity_index(
            &tx,
            &document.id,
            &normalized_cache.entities,
            &normalized_cache.created_at,
        )?;
        tx.execute(
            "UPDATE dwg_parse_cache
             SET project_path = ?1, file_path = ?2, summary_json = ?3, document_id = ?4
             WHERE id = ?5",
            params![
                &normalized_cache.project_path,
                &normalized_cache.file_path,
                serde_json::to_string(&normalized_cache.summary)?,
                &normalized_cache.document_id,
                &normalized_cache.id,
            ],
        )?;
        tx.commit().context("materialize dwg parse cache")?;
        Ok(normalized_cache)
    }
}

fn rebuild_dwg_entity_index(
    tx: &rusqlite::Transaction<'_>,
    doc_id: &str,
    entities: &[CadEntityRecord],
    created_at: &str,
) -> Result<()> {
    let envelopes = entities.iter().map(entity_to_envelope).collect::<Vec<_>>();
    let payloads = entities
        .iter()
        .map(|entity| DwgEntityPayloadRecord {
            entity_id: entity.id.clone(),
            payload: entity_to_payload(entity),
        })
        .collect::<Vec<_>>();
    rebuild_dwg_entity_envelopes(tx, doc_id, &envelopes, created_at)?;
    replace_dwg_entity_payloads(tx, doc_id, &payloads, created_at)
}

fn rebuild_dwg_entity_envelopes(
    tx: &rusqlite::Transaction<'_>,
    doc_id: &str,
    envelopes: &[CadEntityEnvelope],
    created_at: &str,
) -> Result<()> {
    tx.execute(
        "DELETE FROM dwg_entity_rtree WHERE row_id IN (
            SELECT row_id FROM dwg_entity_envelopes WHERE doc_id = ?1
         )",
        params![doc_id],
    )?;
    tx.execute(
        "DELETE FROM dwg_entity_envelopes WHERE doc_id = ?1",
        params![doc_id],
    )?;

    let mut envelope_stmt = tx.prepare(
        "INSERT INTO dwg_entity_envelopes (
            doc_id, entity_id, handle, entity_type, raw_type, layer, block_name, text_excerpt,
            normalized_text, center_x, center_y, anchor_x, anchor_y,
            bbox_min_x, bbox_min_y, bbox_max_x, bbox_max_y,
            layout, owner_block, rotation_deg, scale_x, scale_y, sort_key, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
    )?;
    let mut rtree_stmt = tx.prepare(
        "INSERT INTO dwg_entity_rtree (row_id, min_x, max_x, min_y, max_y)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;

    for (sort_key, envelope) in envelopes.iter().enumerate() {
        envelope_stmt.execute(params![
            doc_id,
            &envelope.id,
            &envelope.handle,
            &envelope.entity_type,
            &envelope.raw_type,
            &envelope.layer,
            &envelope.block_name,
            &envelope.text_excerpt,
            &envelope.normalized_text,
            envelope.center.as_ref().map(|value| value.x),
            envelope.center.as_ref().map(|value| value.y),
            envelope.anchor.as_ref().map(|value| value.x),
            envelope.anchor.as_ref().map(|value| value.y),
            envelope.bbox.as_ref().map(|value| value.min_x),
            envelope.bbox.as_ref().map(|value| value.min_y),
            envelope.bbox.as_ref().map(|value| value.max_x),
            envelope.bbox.as_ref().map(|value| value.max_y),
            &envelope.layout,
            &envelope.owner_block,
            envelope.rotation_deg,
            envelope.scale_x,
            envelope.scale_y,
            sort_key as i64,
            created_at,
        ])?;
        if let Some(bbox) = &envelope.bbox {
            let row_id = tx.last_insert_rowid();
            rtree_stmt.execute(params![
                row_id, bbox.min_x, bbox.max_x, bbox.min_y, bbox.max_y
            ])?;
        }
    }
    Ok(())
}

fn replace_dwg_entity_payloads(
    tx: &rusqlite::Transaction<'_>,
    doc_id: &str,
    payloads: &[DwgEntityPayloadRecord],
    created_at: &str,
) -> Result<()> {
    tx.execute(
        "DELETE FROM dwg_entity_payloads WHERE doc_id = ?1",
        params![doc_id],
    )?;
    let mut payload_stmt = tx.prepare(
        "INSERT INTO dwg_entity_payloads (doc_id, entity_id, payload_json, created_at)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for payload in payloads {
        payload_stmt.execute(params![
            doc_id,
            &payload.entity_id,
            serde_json::to_string(&payload.payload)?,
            created_at,
        ])?;
    }
    Ok(())
}

fn entity_to_envelope(entity: &CadEntityRecord) -> CadEntityEnvelope {
    let anchor = entity
        .center
        .clone()
        .or_else(|| entity.bbox.as_ref().map(bbox_center))
        .or_else(|| entity.vertices.first().cloned());
    CadEntityEnvelope {
        id: entity.id.clone(),
        handle: entity.handle.clone(),
        entity_type: entity.entity_type.clone(),
        raw_type: entity.raw_type.clone(),
        layer: entity.layer.clone(),
        block_name: entity.block_name.clone(),
        text_excerpt: entity.text.clone().map(|value| truncate_text(&value, 160)),
        normalized_text: entity.text.as_ref().map(|value| {
            value
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase()
        }),
        center: entity.center.clone(),
        anchor,
        bbox: entity.bbox.clone(),
        layout: None,
        owner_block: None,
        rotation_deg: None,
        scale_x: None,
        scale_y: None,
    }
}

fn entity_to_payload(entity: &CadEntityRecord) -> Value {
    json!({
        "id": entity.id,
        "handle": entity.handle,
        "entityType": entity.entity_type,
        "rawType": entity.raw_type,
        "layer": entity.layer,
        "color": entity.color,
        "lineType": entity.line_type,
        "text": entity.text,
        "blockName": entity.block_name,
        "center": entity.center,
        "radius": entity.radius,
        "vertices": entity.vertices,
        "bbox": entity.bbox,
    })
}

fn bbox_center(bbox: &CadBBox) -> CadPoint {
    CadPoint {
        x: (bbox.min_x + bbox.max_x) / 2.0,
        y: (bbox.min_y + bbox.max_y) / 2.0,
    }
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    normalized.chars().take(max_chars).collect()
}

fn build_envelope_query(
    doc_id: &str,
    filters: &CadEntityQueryFilters,
) -> (String, String, Vec<SqlValue>) {
    let mut join_clause = String::new();
    let mut where_parts = vec!["e.doc_id = ?".to_string()];
    let mut params = vec![SqlValue::Text(doc_id.to_string())];

    if !filters.layers.is_empty() {
        let placeholders = std::iter::repeat_n("?", filters.layers.len())
            .collect::<Vec<_>>()
            .join(", ");
        where_parts.push(format!("e.layer IN ({placeholders})"));
        params.extend(filters.layers.iter().cloned().map(SqlValue::Text));
    }
    if !filters.entity_types.is_empty() {
        let placeholders = std::iter::repeat_n("?", filters.entity_types.len())
            .collect::<Vec<_>>()
            .join(", ");
        where_parts.push(format!("e.entity_type IN ({placeholders})"));
        params.extend(filters.entity_types.iter().cloned().map(SqlValue::Text));
    }
    if let Some(text_query) = filters
        .text_query
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        where_parts.push("(LOWER(COALESCE(e.normalized_text, '')) LIKE ? OR LOWER(COALESCE(e.block_name, '')) LIKE ?)".to_string());
        let like = format!("%{}%", text_query.to_ascii_lowercase());
        params.push(SqlValue::Text(like.clone()));
        params.push(SqlValue::Text(like));
    }
    if let Some(block_name) = filters
        .block_name
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        where_parts.push("LOWER(COALESCE(e.block_name, '')) LIKE ?".to_string());
        params.push(SqlValue::Text(format!(
            "%{}%",
            block_name.to_ascii_lowercase()
        )));
    }
    if let Some(bbox) = &filters.bbox {
        join_clause.push_str(" JOIN dwg_entity_rtree r ON r.row_id = e.row_id");
        where_parts
            .push("r.min_x <= ? AND r.max_x >= ? AND r.min_y <= ? AND r.max_y >= ?".to_string());
        params.push(SqlValue::Real(bbox.max_x));
        params.push(SqlValue::Real(bbox.min_x));
        params.push(SqlValue::Real(bbox.max_y));
        params.push(SqlValue::Real(bbox.min_y));
    }

    (join_clause, where_parts.join(" AND "), params)
}

fn query_count(conn: &Connection, sql: &str, params: &[SqlValue]) -> Result<usize> {
    Ok(
        conn.query_row(sql, rusqlite::params_from_iter(params.iter()), |row| {
            row.get::<_, i64>(0)
        })? as usize,
    )
}

fn json_string<T: Serialize>(value: &Option<T>) -> Result<Option<String>> {
    value
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .context("serialize optional json payload")
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

fn parse_tool_artifact_refs(raw: Option<String>) -> Vec<DispatcherToolArtifactRef> {
    raw.as_deref()
        .and_then(|json| serde_json::from_str::<Vec<DispatcherToolArtifactRef>>(json).ok())
        .unwrap_or_default()
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

    use rusqlite::{params, Connection};

    use super::{entity_to_envelope, entity_to_payload, DispatcherDb, ToolArtifactDraft};
    use crate::agent::cad::{
        CadBBox, CadEntityQueryFilters, CadEntityRecord, CadPoint, CreateCadReviewIssueInput,
        CreateCadReviewRunInput, DwgBlockSummary, DwgEntityPayloadRecord, DwgLayerSummary,
        DwgParseSummary, SaveDwgDocumentIndexInput, SaveDwgEntityPayloadsInput,
        SaveDwgParseCacheInput,
    };

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

    fn sample_summary(file_path: &str) -> DwgParseSummary {
        DwgParseSummary {
            file_path: file_path.to_string(),
            parser_version: "dwg-worker-v1".to_string(),
            total_entities: 3,
            unknown_entity_count: 0,
            bounds: Some(CadBBox {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 20.0,
                max_y: 20.0,
            }),
            layers: vec![
                DwgLayerSummary {
                    name: "A-WALL".to_string(),
                    entity_count: 2,
                },
                DwgLayerSummary {
                    name: "A-TEXT".to_string(),
                    entity_count: 1,
                },
            ],
            entity_counts: [("LINE".to_string(), 2usize), ("TEXT".to_string(), 1usize)]
                .into_iter()
                .collect(),
            text_samples: vec!["房间".to_string()],
            blocks: vec![DwgBlockSummary {
                name: "ROOM_TAG".to_string(),
                count: 1,
            }],
        }
    }

    fn sample_entities() -> Vec<CadEntityRecord> {
        vec![
            CadEntityRecord {
                id: "L1".to_string(),
                handle: "L1".to_string(),
                entity_type: "LINE".to_string(),
                raw_type: "LINE".to_string(),
                layer: "A-WALL".to_string(),
                color: None,
                line_type: None,
                text: None,
                block_name: None,
                center: Some(CadPoint { x: 2.0, y: 2.0 }),
                radius: None,
                vertices: vec![CadPoint { x: 0.0, y: 0.0 }, CadPoint { x: 4.0, y: 4.0 }],
                bbox: Some(CadBBox {
                    min_x: 0.0,
                    min_y: 0.0,
                    max_x: 4.0,
                    max_y: 4.0,
                }),
            },
            CadEntityRecord {
                id: "L2".to_string(),
                handle: "L2".to_string(),
                entity_type: "LINE".to_string(),
                raw_type: "LINE".to_string(),
                layer: "A-WALL".to_string(),
                color: None,
                line_type: None,
                text: None,
                block_name: None,
                center: Some(CadPoint { x: 8.0, y: 8.0 }),
                radius: None,
                vertices: vec![CadPoint { x: 6.0, y: 6.0 }, CadPoint { x: 10.0, y: 10.0 }],
                bbox: Some(CadBBox {
                    min_x: 6.0,
                    min_y: 6.0,
                    max_x: 10.0,
                    max_y: 10.0,
                }),
            },
            CadEntityRecord {
                id: "T1".to_string(),
                handle: "T1".to_string(),
                entity_type: "TEXT".to_string(),
                raw_type: "TEXT".to_string(),
                layer: "A-TEXT".to_string(),
                color: None,
                line_type: None,
                text: Some("房间名称".to_string()),
                block_name: None,
                center: Some(CadPoint { x: 16.0, y: 16.0 }),
                radius: None,
                vertices: Vec::new(),
                bbox: Some(CadBBox {
                    min_x: 15.0,
                    min_y: 15.0,
                    max_x: 17.0,
                    max_y: 17.0,
                }),
            },
        ]
    }

    #[test]
    fn tool_result_separates_display_content_from_context_payload() {
        let (db, root) = create_test_db();
        let session = db.create_session("project-1", "测试会话").unwrap();
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
        let session = db.create_session("project-1", "测试会话").unwrap();
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

        cleanup_test_db(root);
    }

    #[test]
    fn delete_session_removes_tool_artifacts_with_session() {
        let (db, root) = create_test_db();
        let session = db.create_session("project-1", "测试会话").unwrap();
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

        assert!(db.list_sessions("project-1").unwrap().is_empty());
        assert!(db
            .get_tool_artifact(&session.id, &message.tool_artifacts[0].id)
            .is_err());

        cleanup_test_db(root);
    }

    #[test]
    fn init_migrates_existing_sqlite_schema() {
        let root =
            std::env::temp_dir().join(format!("jkcodingagent-db-migrate-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create temp db root");
        let db_path = root.join("dispatcher.sqlite3");
        let conn = Connection::open(&db_path).expect("open sqlite");
        conn.execute_batch(
            "
            CREATE TABLE dispatcher_messages (
              id TEXT PRIMARY KEY,
              workspace_id TEXT NOT NULL,
              role TEXT NOT NULL,
              content TEXT NOT NULL,
              created_at TEXT NOT NULL
            );
            CREATE TABLE dispatcher_settings (
              id TEXT PRIMARY KEY DEFAULT 'default',
              api_base TEXT NOT NULL DEFAULT '',
              api_key TEXT NOT NULL DEFAULT '',
              model TEXT NOT NULL DEFAULT '',
              auto_approve_dispatch INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
        .expect("seed legacy schema");
        drop(conn);

        let db = DispatcherDb::new(db_path.clone()).expect("migrate dispatcher db");
        let conn = Connection::open(db.path()).expect("open migrated db");
        let columns = conn
            .prepare("PRAGMA table_info(dispatcher_messages)")
            .expect("prepare pragma")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query columns")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect columns");
        assert!(columns.contains(&"context_payload".to_string()));
        assert!(columns.contains(&"attachments_json".to_string()));
        assert!(columns.contains(&"tool_artifacts_json".to_string()));
        assert!(conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'cad_review_runs'",
                [],
                |row| row.get::<_, String>(0),
            )
            .is_ok());

        drop(db);
        cleanup_test_db(root);
    }

    #[test]
    fn query_dwg_parse_entities_filters_and_paginates() {
        let (db, root) = create_test_db();
        let file_path = "/repo/sample.dwg";
        db.save_dwg_parse_cache(&SaveDwgParseCacheInput {
            project_path: "/repo".to_string(),
            file_path: file_path.to_string(),
            file_size: 128,
            file_mtime: 12345,
            parser_version: "dwg-worker-v1".to_string(),
            summary: sample_summary(file_path),
            entities: sample_entities(),
        })
        .expect("save parse cache");

        let filters = CadEntityQueryFilters {
            layers: vec!["A-WALL".to_string()],
            entity_types: vec!["LINE".to_string()],
            text_query: None,
            block_name: None,
            bbox: Some(CadBBox {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 12.0,
                max_y: 12.0,
            }),
        };
        let page1 = db
            .query_dwg_parse_entities(
                "/repo",
                file_path,
                128,
                12345,
                "dwg-worker-v1",
                &filters,
                0,
                1,
            )
            .expect("query page1")
            .expect("cache exists");
        assert_eq!(page1.total, 2);
        assert_eq!(page1.items.len(), 1);
        assert_eq!(page1.items[0].id, "L1");
        assert_eq!(page1.next_cursor, Some(1));

        let page2 = db
            .query_dwg_parse_entities(
                "/repo",
                file_path,
                128,
                12345,
                "dwg-worker-v1",
                &filters,
                1,
                1,
            )
            .expect("query page2")
            .expect("cache exists");
        assert_eq!(page2.items.len(), 1);
        assert_eq!(page2.items[0].id, "L2");
        assert_eq!(page2.next_cursor, None);

        let empty = db
            .query_dwg_parse_entities(
                "/repo",
                file_path,
                128,
                12345,
                "dwg-worker-v1",
                &filters,
                9,
                50,
            )
            .expect("query empty page")
            .expect("cache exists");
        assert!(empty.items.is_empty());

        cleanup_test_db(root);
    }

    #[test]
    fn legacy_dwg_parse_cache_is_materialized_on_lookup() {
        let (db, root) = create_test_db();
        let legacy_project_path = "/repo/./";
        let legacy_file_path = "/repo/plans/../sample.dwg";
        let summary = sample_summary(legacy_file_path);
        let entities = sample_entities();
        let conn = Connection::open(db.path()).expect("open db");
        conn.execute(
            "INSERT INTO dwg_parse_cache (
                id, project_path, file_path, file_size, file_mtime, parser_version, summary_json, document_id, entity_index_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9)",
            params![
                "legacy-cache-id",
                legacy_project_path,
                legacy_file_path,
                128i64,
                12345i64,
                "dwg-worker-v1",
                serde_json::to_string(&summary).expect("serialize summary"),
                serde_json::to_string(&entities).expect("serialize entities"),
                "2026-04-22T00:00:00Z",
            ],
        )
        .expect("insert legacy cache");
        drop(conn);

        let overview = db
            .get_dwg_document_overview("/repo", "/repo/sample.dwg", 128, 12345, "dwg-worker-v1")
            .expect("load overview")
            .expect("overview exists");
        assert_eq!(overview.document.project_path, "/repo");
        assert_eq!(overview.document.file_path, "/repo/sample.dwg");

        let materialized_cache = db
            .get_dwg_parse_cache("/repo", "/repo/sample.dwg", 128, 12345, "dwg-worker-v1")
            .expect("load cache")
            .expect("cache exists");
        assert!(materialized_cache.document_id.is_some());

        let entities = db
            .query_dwg_entities(
                &overview.document.id,
                &CadEntityQueryFilters::default(),
                0,
                10,
            )
            .expect("query entities");
        assert_eq!(entities.total, 3);
        assert_eq!(entities.items.len(), 3);

        cleanup_test_db(root);
    }

    #[test]
    fn upsert_dwg_document_index_and_payloads_split_hot_path_storage() {
        let (db, root) = create_test_db();
        let file_path = "/repo/sample.dwg";
        let entities = sample_entities();
        let envelopes = entities.iter().map(entity_to_envelope).collect::<Vec<_>>();
        let payloads = entities
            .iter()
            .map(|entity| DwgEntityPayloadRecord {
                entity_id: entity.id.clone(),
                payload: entity_to_payload(entity),
            })
            .collect::<Vec<_>>();

        let document = db
            .upsert_dwg_document_index(&SaveDwgDocumentIndexInput {
                project_path: "/repo".to_string(),
                file_path: file_path.to_string(),
                file_size: 128,
                file_mtime: 12345,
                parser_version: "dwg-worker-v1".to_string(),
                summary: sample_summary(file_path),
                envelopes,
            })
            .expect("save dwg document index");

        let conn = Connection::open(db.path()).expect("open db");
        let parse_cache_count = conn
            .query_row("SELECT COUNT(*) FROM dwg_parse_cache", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("count parse cache");
        assert_eq!(parse_cache_count, 0);
        drop(conn);

        let queried = db
            .query_dwg_entities(&document.id, &CadEntityQueryFilters::default(), 0, 10)
            .expect("query envelopes");
        assert_eq!(queried.total, 3);

        db.upsert_dwg_entity_payloads(&SaveDwgEntityPayloadsInput {
            doc_id: document.id.clone(),
            payloads,
        })
        .expect("save payloads");

        let details = db
            .get_dwg_entity_details(&document.id, &["T1".to_string()])
            .expect("load entity detail");
        assert_eq!(details.len(), 1);
        assert_eq!(
            details[0]
                .payload
                .as_ref()
                .and_then(|payload| payload.get("text"))
                .and_then(|value| value.as_str()),
            Some("房间名称")
        );

        cleanup_test_db(root);
    }

    #[test]
    fn create_cad_review_run_can_bind_result_message() {
        let (db, root) = create_test_db();
        let detail = db
            .create_cad_review_run(&CreateCadReviewRunInput {
                workspace_id: "ws-1".to_string(),
                file_path: "/repo/sample.dwg".to_string(),
                source_message_id: "msg-source".to_string(),
                rule_attachment_ids: vec!["att-1".to_string()],
                goal: "检查门洞尺寸".to_string(),
                status: "completed".to_string(),
                summary: "发现 1 个问题".to_string(),
                issues: vec![CreateCadReviewIssueInput {
                    severity: "high".to_string(),
                    title: "门洞过窄".to_string(),
                    description: "门洞净宽不足".to_string(),
                    layer: Some("A-DOOR".to_string()),
                    entity_refs: vec!["L1".to_string()],
                    anchor_point: Some(CadPoint { x: 2.0, y: 2.0 }),
                    bbox: None,
                    viewport_hint: None,
                    rule_ref: Some("规则 1".to_string()),
                }],
            })
            .expect("create cad review run");

        db.bind_cad_review_result_message(&detail.run.id, "msg-result")
            .expect("bind result message");

        let loaded = db
            .get_cad_review_run_detail("ws-1", &detail.run.id)
            .expect("load review detail");
        assert_eq!(loaded.run.result_message_id.as_deref(), Some("msg-result"));
        assert_eq!(loaded.issues.len(), 1);
        assert_eq!(loaded.issues[0].entity_refs, vec!["L1".to_string()]);

        cleanup_test_db(root);
    }
}
