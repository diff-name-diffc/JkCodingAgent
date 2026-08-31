//! 工具产物（dispatcher_tool_artifacts 表）的读取，以及消息中引用产物所用的轻量类型。

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::util::now;
use super::DispatcherDb;

/// 单个原始工具产物允许持久化的最大字节数。工具输出可能来自外部进程或
/// MCP 服务，不能把未经约束的正文复制进 SQLite；截断发生在构造 draft 时，
/// 因而所有调用路径共享同一硬边界。
pub const MAX_RAW_TOOL_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolArtifactDraft {
    pub kind: String,
    pub title: String,
    pub preview: String,
    pub content: String,
    pub char_count: usize,
    pub line_count: usize,
}

impl ToolArtifactDraft {
    pub fn raw_tool_output(tool_name: &str, raw_output: &str) -> Self {
        let original_bytes = raw_output.len();
        let original_chars = raw_output.chars().count();
        let original_lines = raw_output.lines().count().max(1);
        let (content, truncated) = bounded_raw_artifact_content(raw_output);
        let mut preview = bounded_raw_artifact_preview(raw_output);
        if preview.is_empty() {
            preview = "原始结果为空白或仅包含空行".to_string()
        }
        if truncated {
            preview.push_str(&format!(
                " [已截断 truncated=true, originalBytes={original_bytes}, storedBytes={}]",
                content.len()
            ));
        }

        Self {
            kind: "tool_raw_output".to_string(),
            title: format!("{tool_name} 原始结果"),
            preview,
            content,
            // 计数描述原始输出，而非截断后的持久化前缀；调用方可据此判断
            // 产物覆盖程度，preview/content 尾标记同时给出 truncated 元信息。
            char_count: original_chars,
            line_count: original_lines,
        }
    }
}

fn bounded_raw_artifact_content(raw_output: &str) -> (String, bool) {
    if raw_output.len() <= MAX_RAW_TOOL_ARTIFACT_BYTES {
        return (raw_output.to_string(), false);
    }

    let marker = format!(
        "\n\n...（原始工具结果已截断；truncated=true, originalBytes={}, limitBytes={}）",
        raw_output.len(),
        MAX_RAW_TOOL_ARTIFACT_BYTES
    );
    let prefix_budget = MAX_RAW_TOOL_ARTIFACT_BYTES.saturating_sub(marker.len());
    let mut boundary = prefix_budget.min(raw_output.len());
    while !raw_output.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }

    let mut content = String::with_capacity(boundary + marker.len());
    content.push_str(&raw_output[..boundary]);
    content.push_str(&marker);
    debug_assert!(content.len() <= MAX_RAW_TOOL_ARTIFACT_BYTES);
    (content, true)
}

/// 预览从源字符串直接按字符流构建，不先 join 整行。外部工具可能输出一条
/// 数十 MiB 的无换行文本，先聚合三行再截预览会额外制造一次大内存复制。
fn bounded_raw_artifact_preview(raw_output: &str) -> String {
    const MAX_PREVIEW_CHARS: usize = 160;
    const MAX_PREVIEW_LINES: usize = 3;

    let mut preview = String::new();
    let mut preview_chars = 0usize;
    let mut shortened = false;
    for (included_lines, line) in raw_output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .enumerate()
    {
        if included_lines == MAX_PREVIEW_LINES {
            shortened = true;
            break;
        }
        let separator = if included_lines > 0 { " / " } else { "" };
        for ch in separator.chars().chain(line.chars()) {
            if preview_chars == MAX_PREVIEW_CHARS {
                shortened = true;
                break;
            }
            preview.push(ch);
            preview_chars += 1;
        }
        if shortened {
            break;
        }
    }
    if shortened {
        preview.push_str("...");
    }
    preview
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherToolArtifactRecord {
    pub id: String,
    pub workspace_id: String,
    pub message_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_run_id: Option<String>,
    pub tool_name: Option<String>,
    pub title: String,
    pub kind: String,
    pub preview: String,
    pub content: String,
    pub char_count: usize,
    pub line_count: usize,
    pub created_at: String,
}

impl DispatcherDb {
    pub fn insert_tool_artifacts_for_run(
        &self,
        workspace_id: &str,
        tool_run_id: &str,
        tool_call_id: &str,
        tool_name: &str,
        drafts: &[ToolArtifactDraft],
    ) -> Result<Vec<DispatcherToolArtifactRef>> {
        if drafts.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.conn()?;
        let tx = conn.transaction().context("begin insert run artifacts")?;
        let run_workspace = tx
            .query_row(
                "SELECT workspace_id FROM dispatcher_tool_runs WHERE id = ?1",
                params![tool_run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("load dispatcher tool run before artifact insert")?
            .with_context(|| format!("dispatcher tool run not found: {tool_run_id}"))?;
        if run_workspace != workspace_id {
            anyhow::bail!(
                "dispatcher tool run {tool_run_id} belongs to workspace {run_workspace}, not {workspace_id}"
            );
        }

        let created_at = now();
        let mut refs = Vec::with_capacity(drafts.len());
        for draft in drafts {
            let id = Uuid::new_v4().to_string();
            let char_count = i64::try_from(draft.char_count)
                .context("tool artifact char_count exceeds sqlite INTEGER range")?;
            let line_count = i64::try_from(draft.line_count)
                .context("tool artifact line_count exceeds sqlite INTEGER range")?;
            tx.execute(
                "INSERT INTO dispatcher_tool_artifacts (
                    id, workspace_id, message_id, tool_call_id, tool_run_id, tool_name,
                    title, kind, preview, content, char_count, line_count, created_at
                 ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    &id,
                    workspace_id,
                    tool_call_id,
                    tool_run_id,
                    tool_name,
                    &draft.title,
                    &draft.kind,
                    &draft.preview,
                    &draft.content,
                    char_count,
                    line_count,
                    &created_at,
                ],
            )
            .context("insert dispatcher run tool artifact")?;
            refs.push(DispatcherToolArtifactRef {
                id,
                title: draft.title.clone(),
                kind: draft.kind.clone(),
                preview: draft.preview.clone(),
                char_count: draft.char_count,
                line_count: draft.line_count,
                created_at: created_at.clone(),
            });
        }
        tx.commit().context("commit insert run artifacts")?;
        Ok(refs)
    }

    pub async fn insert_tool_artifacts_for_run_async(
        &self,
        workspace_id: &str,
        tool_run_id: &str,
        tool_call_id: &str,
        tool_name: &str,
        drafts: &[ToolArtifactDraft],
    ) -> Result<Vec<DispatcherToolArtifactRef>> {
        let db = self.clone();
        let workspace_id = workspace_id.to_string();
        let tool_run_id = tool_run_id.to_string();
        let tool_call_id = tool_call_id.to_string();
        let tool_name = tool_name.to_string();
        let drafts = drafts.to_vec();
        tokio::task::spawn_blocking(move || {
            db.insert_tool_artifacts_for_run(
                &workspace_id,
                &tool_run_id,
                &tool_call_id,
                &tool_name,
                &drafts,
            )
        })
        .await
        .context("insert_tool_artifacts_for_run spawn_blocking")?
    }

    pub fn get_tool_artifact(
        &self,
        workspace_id: &str,
        artifact_id: &str,
    ) -> Result<DispatcherToolArtifactRecord> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, workspace_id, message_id, tool_call_id, tool_run_id, tool_name,
                    title, kind, preview, content, char_count, line_count, created_at
             FROM dispatcher_tool_artifacts
             WHERE id = ?1 AND workspace_id = ?2",
            params![artifact_id, workspace_id],
            map_tool_artifact_record,
        )
        .optional()
        .context("load dispatcher tool artifact")?
        .with_context(|| format!("dispatcher tool artifact not found: {artifact_id}"))
    }
}

fn map_tool_artifact_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<DispatcherToolArtifactRecord> {
    Ok(DispatcherToolArtifactRecord {
        id: row.get("id")?,
        workspace_id: row.get("workspace_id")?,
        message_id: row.get("message_id")?,
        tool_call_id: row.get("tool_call_id")?,
        tool_run_id: row.get("tool_run_id")?,
        tool_name: row.get("tool_name")?,
        title: row.get("title")?,
        kind: row.get("kind")?,
        preview: row.get("preview")?,
        content: row.get("content")?,
        char_count: usize_from_sql(
            row.get::<_, i64>("char_count")?,
            row.as_ref().column_index("char_count")?,
        )?,
        line_count: usize_from_sql(
            row.get::<_, i64>("line_count")?,
            row.as_ref().column_index("line_count")?,
        )?,
        created_at: row.get("created_at")?,
    })
}

fn usize_from_sql(value: i64, column: usize) -> rusqlite::Result<usize> {
    usize::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::db::NewToolRun;
    use uuid::Uuid;

    fn test_db() -> DispatcherDb {
        let path = std::env::temp_dir().join(format!(
            "jkcodingagent-tool-artifacts-{}.sqlite3",
            Uuid::new_v4()
        ));
        DispatcherDb::new(path).expect("create test dispatcher db")
    }

    #[test]
    fn raw_artifact_preview_does_not_copy_an_unbounded_first_line() {
        let raw_output = "界".repeat(10_000);
        let artifact = ToolArtifactDraft::raw_tool_output("echo", &raw_output);

        assert!(artifact.preview.chars().count() <= 163);
        assert_eq!(artifact.content, raw_output);
        assert_eq!(artifact.char_count, 10_000);
    }

    #[test]
    fn artifact_round_trips_tool_run_ownership() {
        let db = test_db();
        let run = db
            .create_tool_run(NewToolRun {
                workspace_id: "ws".to_string(),
                tool_call_id: "call-1".to_string(),
                tool_name: "grep".to_string(),
                provider: "builtin".to_string(),
                category: "search".to_string(),
                arguments_json: "{}".to_string(),
                effective_arguments_json: "{}".to_string(),
                metadata_json: "{}".to_string(),
            })
            .expect("create tool run");
        let artifact_id = Uuid::new_v4().to_string();
        let conn = db.conn().expect("db conn");
        conn.execute(
            "INSERT INTO dispatcher_tool_artifacts
                (id, workspace_id, tool_call_id, tool_run_id, tool_name, title, kind,
                 preview, content, char_count, line_count, created_at)
             VALUES (?1, 'ws', 'call-1', ?2, 'grep', 'raw output', 'tool_raw_output',
                     'preview', 'content', 7, 1, '2026-01-01T00:00:00Z')",
            params![&artifact_id, &run.id],
        )
        .expect("insert artifact");
        drop(conn);

        let artifact = db
            .get_tool_artifact("ws", &artifact_id)
            .expect("load artifact");
        assert_eq!(artifact.tool_run_id.as_deref(), Some(run.id.as_str()));
        assert_eq!(artifact.content, "content");

        assert!(db
            .get_tool_artifact("other-workspace", &artifact_id)
            .is_err());
    }
}
