//! 会话 token 用量（dispatcher_session_token_usage 表）的读写与异步包装。

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::agent::llm::LlmUsage;

use super::util::{default_context_window_capacity, now};
use super::DispatcherDb;

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

impl DispatcherDb {
    pub fn upsert_session_token_usage(
        &self,
        workspace_id: &str,
        model: &str,
        source_kind: DispatcherSessionTokenUsageSource,
        usage: &LlmUsage,
    ) -> Result<DispatcherSessionTokenUsageRecord> {
        let updated_at = now();
        // 口径统一（G7-08）：total 一律按 prompt + completion 重算，不再采信
        // provider 上报的 total（后者与分项不一致时会造成累计口径分裂）。
        // context_window_tokens 是“最近一次请求的 prompt 占用量”快照（覆盖语义），
        // 与上面的累计字段语义不同，仅用于上下文窗口占用展示。
        let prompt_tokens = usage.prompt_tokens;
        let completion_tokens = usage.completion_tokens;
        let total_tokens = prompt_tokens.saturating_add(completion_tokens);
        // cached_tokens 采用累加语义：统计会话生命周期内累计的缓存命中输入 token 总量，
        // 与 prompt/completion 的累加口径一致（对应缓存折扣部分的累计计费量）。
        let cached_tokens = usage.cached_tokens();
        let record = DispatcherSessionTokenUsageRecord {
            workspace_id: workspace_id.to_string(),
            model: model.to_string(),
            source_kind,
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cached_tokens,
            context_window_tokens: usage.prompt_tokens,
            context_window_capacity: default_context_window_capacity(model),
            updated_at,
        };
        // source_kind 由枚举构造，as_sql_value 只会产生 "primary"/"summary"，
        // 入口即白名单，无需额外校验。
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO dispatcher_session_token_usage (
                workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
                cached_tokens, context_window_tokens, context_window_capacity, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(workspace_id, model, source_kind) DO UPDATE SET
                prompt_tokens = dispatcher_session_token_usage.prompt_tokens + excluded.prompt_tokens,
                completion_tokens = dispatcher_session_token_usage.completion_tokens + excluded.completion_tokens,
                total_tokens = (dispatcher_session_token_usage.prompt_tokens + excluded.prompt_tokens)
                    + (dispatcher_session_token_usage.completion_tokens + excluded.completion_tokens),
                cached_tokens = dispatcher_session_token_usage.cached_tokens + excluded.cached_tokens,
                context_window_tokens = excluded.context_window_tokens,
                context_window_capacity = excluded.context_window_capacity,
                updated_at = excluded.updated_at",
            params![
                &record.workspace_id,
                &record.model,
                source_kind.as_sql_value(),
                i64_from_u64(record.prompt_tokens, "prompt_tokens")?,
                i64_from_u64(record.completion_tokens, "completion_tokens")?,
                i64_from_u64(record.total_tokens, "total_tokens")?,
                i64_from_u64(record.cached_tokens, "cached_tokens")?,
                i64_from_u64(record.context_window_tokens, "context_window_tokens")?,
                i64_from_u64(record.context_window_capacity, "context_window_capacity")?,
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
            // 写后回读出现未知 source_kind 说明自身写入被破坏，必须显式报错。
            map_token_usage_record_strict,
        )
        .context("load dispatcher session token usage after upsert")
    }

    pub fn list_session_token_usage(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<DispatcherSessionTokenUsageRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
                    cached_tokens, context_window_tokens, context_window_capacity, updated_at
             FROM dispatcher_session_token_usage
             WHERE workspace_id = ?1
             ORDER BY updated_at DESC, model ASC, source_kind ASC",
        )?;
        // 读取存量数据时对未知 source_kind 记录日志并按 primary 兜底，
        // 避免单行脏数据导致整个用量列表加载失败。
        let rows = stmt.query_map(params![workspace_id], map_token_usage_record_lenient)?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list dispatcher session token usage")
    }
    pub async fn upsert_session_token_usage_async(
        &self,
        workspace_id: &str,
        model: &str,
        source_kind: DispatcherSessionTokenUsageSource,
        usage: &LlmUsage,
    ) -> Result<DispatcherSessionTokenUsageRecord> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let model = model.to_string();
        let usage = usage.clone();
        tokio::task::spawn_blocking(move || {
            db.upsert_session_token_usage(&wid, &model, source_kind, &usage)
        })
        .await
        .context("upsert_session_token_usage spawn_blocking")?
    }
}

impl DispatcherSessionTokenUsageSource {
    fn as_sql_value(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Summary => "summary",
        }
    }

    /// 未知值不再静默归为 Primary，而是显式报错，由调用方决定严格失败还是降级。
    fn from_sql_value(value: &str) -> std::result::Result<Self, String> {
        match value {
            "primary" => Ok(Self::Primary),
            "summary" => Ok(Self::Summary),
            other => Err(format!("未知的 token 用量来源类型：{other}")),
        }
    }
}

fn i64_from_u64(value: u64, field: &'static str) -> Result<i64> {
    i64::try_from(value).with_context(|| format!("{field} 超出 SQLite INTEGER 范围：{value}"))
}

fn u64_from_sql(value: i64) -> u64 {
    // 存量负值视为脏数据，收敛到 0 而非让整行映射失败。
    u64::try_from(value).unwrap_or(0)
}

struct TokenUsageRow {
    workspace_id: String,
    model: String,
    source_kind_raw: String,
    prompt_tokens: i64,
    completion_tokens: i64,
    total_tokens: i64,
    cached_tokens: i64,
    context_window_tokens: i64,
    context_window_capacity: i64,
    updated_at: String,
}

fn read_token_usage_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TokenUsageRow> {
    Ok(TokenUsageRow {
        workspace_id: row.get(0)?,
        model: row.get(1)?,
        source_kind_raw: row.get(2)?,
        prompt_tokens: row.get(3)?,
        completion_tokens: row.get(4)?,
        total_tokens: row.get(5)?,
        cached_tokens: row.get(6)?,
        context_window_tokens: row.get(7)?,
        context_window_capacity: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn build_token_usage_record(
    raw: TokenUsageRow,
    source_kind: DispatcherSessionTokenUsageSource,
) -> DispatcherSessionTokenUsageRecord {
    DispatcherSessionTokenUsageRecord {
        workspace_id: raw.workspace_id,
        model: raw.model,
        source_kind,
        prompt_tokens: u64_from_sql(raw.prompt_tokens),
        completion_tokens: u64_from_sql(raw.completion_tokens),
        total_tokens: u64_from_sql(raw.total_tokens),
        cached_tokens: u64_from_sql(raw.cached_tokens),
        context_window_tokens: u64_from_sql(raw.context_window_tokens),
        context_window_capacity: u64_from_sql(raw.context_window_capacity),
        updated_at: raw.updated_at,
    }
}

fn map_token_usage_record_strict(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<DispatcherSessionTokenUsageRecord> {
    let raw = read_token_usage_row(row)?;
    let source_kind = DispatcherSessionTokenUsageSource::from_sql_value(&raw.source_kind_raw)
        .map_err(|message| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                message.into(),
            )
        })?;
    Ok(build_token_usage_record(raw, source_kind))
}

fn map_token_usage_record_lenient(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<DispatcherSessionTokenUsageRecord> {
    let raw = read_token_usage_row(row)?;
    let source_kind = match DispatcherSessionTokenUsageSource::from_sql_value(&raw.source_kind_raw)
    {
        Ok(kind) => kind,
        Err(message) => {
            eprintln!(
                "警告：dispatcher_session_token_usage 存在脏 source_kind（{message}），已按 primary 统计"
            );
            DispatcherSessionTokenUsageSource::Primary
        }
    };
    Ok(build_token_usage_record(raw, source_kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_sql_value_accepts_known_kinds() {
        assert_eq!(
            DispatcherSessionTokenUsageSource::from_sql_value("primary"),
            Ok(DispatcherSessionTokenUsageSource::Primary)
        );
        assert_eq!(
            DispatcherSessionTokenUsageSource::from_sql_value("summary"),
            Ok(DispatcherSessionTokenUsageSource::Summary)
        );
    }

    #[test]
    fn from_sql_value_rejects_unknown_values() {
        assert!(DispatcherSessionTokenUsageSource::from_sql_value("primry").is_err());
        assert!(DispatcherSessionTokenUsageSource::from_sql_value("").is_err());
        assert!(DispatcherSessionTokenUsageSource::from_sql_value("legacy").is_err());
    }

    #[test]
    fn negative_counts_clamp_to_zero() {
        assert_eq!(u64_from_sql(-5), 0);
        assert_eq!(u64_from_sql(0), 0);
        assert_eq!(u64_from_sql(42), 42);
    }

    #[test]
    fn upsert_recomputes_total_from_prompt_and_completion() {
        let path = std::env::temp_dir().join(format!(
            "jkcodingagent-token-usage-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        let db = DispatcherDb::new(path).expect("create test dispatcher db");
        let usage = LlmUsage {
            prompt_tokens: 100,
            completion_tokens: 40,
            // provider 上报的 total 与分项不一致，落库口径必须以 prompt+completion 为准。
            total_tokens: 999,
            prompt_tokens_details: None,
        };

        let record = db
            .upsert_session_token_usage(
                "session-a",
                "model-x",
                DispatcherSessionTokenUsageSource::Primary,
                &usage,
            )
            .expect("first upsert");
        assert_eq!(record.prompt_tokens, 100);
        assert_eq!(record.completion_tokens, 40);
        assert_eq!(record.total_tokens, 140, "total 必须等于 prompt+completion");

        let record = db
            .upsert_session_token_usage(
                "session-a",
                "model-x",
                DispatcherSessionTokenUsageSource::Primary,
                &usage,
            )
            .expect("second upsert");
        assert_eq!(record.prompt_tokens, 200);
        assert_eq!(record.completion_tokens, 80);
        assert_eq!(
            record.total_tokens, 280,
            "累计后的 total 必须等于累计 prompt+completion 之和"
        );
    }
}
