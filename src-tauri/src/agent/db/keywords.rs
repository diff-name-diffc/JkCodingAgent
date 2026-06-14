//! 会话关键词标签（session_keywords 表）及基于关键词的会话搜索。

use anyhow::{anyhow, Context, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::util::now;
use super::DispatcherDb;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionKeywordRecord {
    pub workspace_id: String,
    pub keyword: String,
    pub weight: f64,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeywordAction {
    pub action: String,
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub from: Option<Vec<String>>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub weight: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchResult {
    pub session_id: String,
    pub session_title: String,
    pub session_kind: String,
    pub category: String,
    pub matched_keywords: Vec<String>,
    pub relevance_score: f64,
    pub updated_at: String,
}

impl DispatcherDb {
    pub fn list_session_keywords(&self, workspace_id: &str) -> Result<Vec<SessionKeywordRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT workspace_id, keyword, weight, created_at
             FROM session_keywords
             WHERE workspace_id = ?1
             ORDER BY weight DESC, keyword ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id], |row| {
            Ok(SessionKeywordRecord {
                workspace_id: row.get(0)?,
                keyword: row.get(1)?,
                weight: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| anyhow!("list_session_keywords: {}", e))
    }

    pub fn apply_keyword_actions(
        &self,
        workspace_id: &str,
        actions: &[KeywordAction],
    ) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let ts = now();

        for action in actions {
            match action.action.as_str() {
                "add" => {
                    let Some(keyword) = action.keyword.as_deref() else {
                        continue;
                    };
                    let weight = action.weight.unwrap_or(1.0);
                    let _ = tx.execute(
                        "INSERT INTO session_keywords (workspace_id, keyword, weight, created_at)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![workspace_id, keyword.trim(), weight, ts],
                    );
                }
                "remove" => {
                    let Some(keyword) = action.keyword.as_deref() else {
                        continue;
                    };
                    let _ = tx.execute(
                        "DELETE FROM session_keywords WHERE workspace_id = ?1 AND keyword = ?2",
                        params![workspace_id, keyword.trim()],
                    );
                }
                "keep" => {
                    // Nothing to do, keyword already persisted
                }
                "merge" => {
                    let Some(to_keyword) = action.to.as_deref() else {
                        continue;
                    };
                    let weight = action.weight.unwrap_or(1.0);
                    if let Some(from_keywords) = &action.from {
                        for from_keyword in from_keywords {
                            let _ = tx.execute(
                                "DELETE FROM session_keywords WHERE workspace_id = ?1 AND keyword = ?2",
                                params![workspace_id, from_keyword.trim()],
                            );
                        }
                    }
                    let _ = tx.execute(
                        "INSERT OR REPLACE INTO session_keywords (workspace_id, keyword, weight, created_at)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![workspace_id, to_keyword.trim(), weight, ts],
                    );
                }
                _ => {}
            }
        }

        tx.commit().context("commit keyword actions")
    }

    #[allow(dead_code)]
    pub fn clear_keywords(&self, workspace_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM session_keywords WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear session keywords")?;
        Ok(())
    }

    pub fn search_sessions_by_keywords(
        &self,
        query: &str,
        limit: i64,
        kind: Option<&str>,
        project_id: Option<&str>,
    ) -> Result<Vec<SessionSearchResult>> {
        let conn = self.conn()?;
        let pattern = format!("%{}%", query.trim());

        let sql = r#"
            SELECT
                ds.id,
                COALESCE(cs.title, ps.title, ds.title) AS title,
                ds.kind,
                COALESCE(cs.category, '') AS category,
                ds.updated_at,
                GROUP_CONCAT(DISTINCT sk.keyword) AS matched_keywords,
                MAX(sk.weight) AS relevance_score
            FROM session_keywords sk
            JOIN dispatcher_sessions ds ON ds.id = sk.workspace_id
            LEFT JOIN chat_sessions cs ON cs.id = ds.id AND ds.kind = 'chat'
            LEFT JOIN project_sessions ps ON ps.id = ds.id AND ds.kind = 'project'
            WHERE sk.keyword LIKE ?1
              AND (?2 IS NULL OR ds.kind = ?2)
              AND (?3 IS NULL OR ds.project_id = ?3)
              AND (?4 IS NULL OR ps.project_id = ?4)
            GROUP BY ds.id
            ORDER BY relevance_score DESC, ds.updated_at DESC
            LIMIT ?5
        "#;

        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(
            params![pattern, kind, project_id, project_id, limit],
            |row| {
                let matched_str: String = row.get(5)?;
                let matched_keywords: Vec<String> = matched_str
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                Ok(SessionSearchResult {
                    session_id: row.get(0)?,
                    session_title: row.get(1)?,
                    session_kind: row.get(2)?,
                    category: row.get(3)?,
                    updated_at: row.get(4)?,
                    matched_keywords,
                    relevance_score: row.get(6)?,
                })
            },
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| anyhow!("search_sessions_by_keywords: {}", e))
    }
}
