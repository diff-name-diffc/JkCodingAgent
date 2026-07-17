//! 会话关键词标签（session_keywords 表）及基于关键词的会话搜索。

use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, params_from_iter, Connection};
use serde::{Deserialize, Serialize};

use super::sessions::DispatcherSessionKind;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchResult {
    pub session_id: String,
    pub session_title: String,
    pub session_kind: DispatcherSessionKind,
    pub category: String,
    pub keywords: Vec<String>,
    pub matched_keywords: Vec<String>,
    pub relevance_score: f64,
    pub updated_at: String,
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

    pub fn search_sessions(
        &self,
        query: &str,
        kind: DispatcherSessionKind,
        project_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<SessionSearchResult>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        if kind == DispatcherSessionKind::Project && project_id.is_none() {
            return Err(anyhow!("project session search requires project_id"));
        }

        let conn = self.conn()?;
        let escaped_query = escape_like(query);
        let prefix_pattern = format!("{escaped_query}%");
        let contains_pattern = format!("%{escaped_query}%");
        let project_filter = match kind {
            DispatcherSessionKind::Project => project_id,
            DispatcherSessionKind::Chat => None,
        };
        let kind_value = kind.as_sql_value();
        let limit = limit.clamp(1, 100);

        let mut stmt = conn.prepare(
            "SELECT ds.id, ds.title, ds.category, ds.updated_at,
                    CASE
                        WHEN ds.title = ?1 COLLATE NOCASE THEN 1000.0
                        WHEN ds.title LIKE ?2 ESCAPE '\\' COLLATE NOCASE THEN 500.0
                        WHEN ds.title LIKE ?3 ESCAPE '\\' COLLATE NOCASE THEN 200.0
                        ELSE 0.0
                    END
                    + COALESCE((
                        SELECT SUM(sk.weight * CASE
                            WHEN sk.keyword = ?1 COLLATE NOCASE THEN 100.0
                            WHEN sk.keyword LIKE ?2 ESCAPE '\\' COLLATE NOCASE THEN 50.0
                            ELSE 25.0
                        END)
                        FROM session_keywords sk
                        WHERE sk.workspace_id = ds.id
                          AND sk.keyword LIKE ?3 ESCAPE '\\' COLLATE NOCASE
                    ), 0.0) AS relevance_score
             FROM dispatcher_sessions ds
             WHERE ds.kind = ?4
               AND (?5 IS NULL OR ds.project_id = ?5)
               AND (
                   ds.title LIKE ?3 ESCAPE '\\' COLLATE NOCASE
                   OR EXISTS (
                       SELECT 1
                       FROM session_keywords sk
                       WHERE sk.workspace_id = ds.id
                         AND sk.keyword LIKE ?3 ESCAPE '\\' COLLATE NOCASE
                   )
               )
             ORDER BY relevance_score DESC, ds.updated_at DESC, ds.id DESC
             LIMIT ?6",
        )?;
        let rows = stmt.query_map(
            params![
                query,
                prefix_pattern,
                contains_pattern,
                kind_value,
                project_filter,
                limit
            ],
            |row| {
                Ok(SessionSearchResult {
                    session_id: row.get(0)?,
                    session_title: row.get(1)?,
                    session_kind: kind,
                    category: row.get(2)?,
                    keywords: Vec::new(),
                    matched_keywords: Vec::new(),
                    updated_at: row.get(3)?,
                    relevance_score: row.get(4)?,
                })
            },
        )?;
        let mut results = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("search sessions")?;
        let workspace_ids = results
            .iter()
            .map(|result| result.session_id.clone())
            .collect::<Vec<_>>();
        let mut keywords_by_workspace = load_keywords_by_workspace_ids(&conn, &workspace_ids)?;

        let mut keyword_stmt = conn.prepare(
            "SELECT keyword
             FROM session_keywords
             WHERE workspace_id = ?1
               AND keyword LIKE ?2 ESCAPE '\\' COLLATE NOCASE
             ORDER BY weight DESC, keyword ASC",
        )?;
        for result in &mut results {
            result.keywords = keywords_by_workspace
                .remove(&result.session_id)
                .unwrap_or_default();
            result.matched_keywords = keyword_stmt
                .query_map(params![result.session_id, contains_pattern], |row| {
                    row.get(0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("load matched session keywords")?;
        }

        Ok(results)
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
}

pub(super) fn load_keywords_by_workspace_ids(
    conn: &Connection,
    workspace_ids: &[String],
) -> Result<HashMap<String, Vec<String>>> {
    if workspace_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders = std::iter::repeat_n("?", workspace_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT workspace_id, keyword
         FROM session_keywords
         WHERE workspace_id IN ({placeholders})
         ORDER BY workspace_id ASC, weight DESC, keyword ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(workspace_ids), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut keywords_by_workspace = HashMap::<String, Vec<String>>::new();
    for row in rows {
        let (workspace_id, keyword) = row?;
        keywords_by_workspace
            .entry(workspace_id)
            .or_default()
            .push(keyword);
    }
    Ok(keywords_by_workspace)
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn test_db() -> DispatcherDb {
        let path = std::env::temp_dir().join(format!(
            "jkcodingagent-session-keywords-{}.sqlite3",
            Uuid::new_v4()
        ));
        DispatcherDb::new(path).expect("create test dispatcher db")
    }

    fn add_keyword(db: &DispatcherDb, workspace_id: &str, keyword: &str, weight: f64) {
        db.apply_keyword_actions(
            workspace_id,
            &[KeywordAction {
                action: "add".to_string(),
                keyword: Some(keyword.to_string()),
                from: None,
                to: None,
                weight: Some(weight),
            }],
        )
        .expect("add keyword");
    }

    #[test]
    fn project_search_matches_titles_and_keywords_without_crossing_projects() {
        let db = test_db();
        let rust_session = db
            .create_project_session("project-a", "实现桌面应用")
            .expect("create rust session");
        let title_session = db
            .create_project_session("project-a", "修复登录搜索")
            .expect("create title session");
        let other_project_session = db
            .create_project_session("project-b", "另一个项目")
            .expect("create other project session");
        add_keyword(&db, &rust_session.id, "Rust", 8.0);
        add_keyword(&db, &rust_session.id, "Tauri", 6.0);
        add_keyword(&db, &other_project_session.id, "Rust", 10.0);

        let keyword_results = db
            .search_sessions(
                "rust",
                DispatcherSessionKind::Project,
                Some("project-a"),
                20,
            )
            .expect("search project keywords");
        assert_eq!(keyword_results.len(), 1);
        assert_eq!(keyword_results[0].session_id, rust_session.id);
        assert_eq!(keyword_results[0].keywords, vec!["Rust", "Tauri"]);
        assert_eq!(keyword_results[0].matched_keywords, vec!["Rust"]);

        let title_results = db
            .search_sessions(
                "登录",
                DispatcherSessionKind::Project,
                Some("project-a"),
                20,
            )
            .expect("search project titles");
        assert_eq!(title_results.len(), 1);
        assert_eq!(title_results[0].session_id, title_session.id);
        assert!(title_results[0].matched_keywords.is_empty());
    }

    #[test]
    fn session_pages_include_weight_sorted_keywords_and_search_escapes_wildcards() {
        let db = test_db();
        let tagged_session = db
            .create_chat_session("100% coverage", Some("tech"))
            .expect("create tagged chat session");
        db.create_chat_session("ordinary session", Some("tech"))
            .expect("create ordinary chat session");
        add_keyword(&db, &tagged_session.id, "测试", 3.0);
        add_keyword(&db, &tagged_session.id, "覆盖率", 9.0);

        let page = db
            .list_chat_sessions_paginated(None, None, 20)
            .expect("list chat sessions");
        let tagged = page
            .items
            .iter()
            .find(|session| session.id == tagged_session.id)
            .expect("tagged session in page");
        assert_eq!(tagged.keywords, vec!["覆盖率", "测试"]);

        let results = db
            .search_sessions("%", DispatcherSessionKind::Chat, None, 20)
            .expect("search literal wildcard");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, tagged_session.id);
    }

    #[test]
    fn project_search_requires_project_scope() {
        let db = test_db();
        let error = db
            .search_sessions("rust", DispatcherSessionKind::Project, None, 20)
            .expect_err("missing project scope must fail");
        assert!(error
            .to_string()
            .contains("project session search requires project_id"));
    }
}
