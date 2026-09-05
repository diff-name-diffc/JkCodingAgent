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
    pub session_id: String,
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
    pub fn list_session_keywords(&self, session_id: &str) -> Result<Vec<SessionKeywordRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT session_id, keyword, weight, created_at
             FROM session_keywords
             WHERE session_id = ?1
             ORDER BY weight DESC, keyword ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok(SessionKeywordRecord {
                session_id: row.get(0)?,
                keyword: row.get(1)?,
                weight: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| anyhow!("list_session_keywords: {}", e))
    }

    pub fn apply_keyword_actions(&self, session_id: &str, actions: &[KeywordAction]) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let ts = now();

        for action in actions {
            match action.action.as_str() {
                "add" => {
                    let Some(keyword) = action.keyword.as_deref() else {
                        continue;
                    };
                    let keyword = keyword.trim();
                    if keyword.is_empty() {
                        continue;
                    }
                    let weight = action.weight.unwrap_or(1.0);
                    // UPSERT：重复关键词聚合权重，且保留首次写入的 created_at。
                    tx.execute(
                        "INSERT INTO session_keywords (session_id, keyword, weight, created_at)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(session_id, keyword) DO UPDATE SET
                             weight = session_keywords.weight + excluded.weight",
                        params![session_id, keyword, weight, ts],
                    )
                    .with_context(|| format!("upsert session keyword {keyword}"))?;
                }
                "remove" => {
                    let Some(keyword) = action.keyword.as_deref() else {
                        continue;
                    };
                    let keyword = keyword.trim();
                    if keyword.is_empty() {
                        continue;
                    }
                    tx.execute(
                        "DELETE FROM session_keywords WHERE session_id = ?1 AND keyword = ?2",
                        params![session_id, keyword],
                    )
                    .with_context(|| format!("remove session keyword {keyword}"))?;
                }
                "keep" => {
                    // Nothing to do, keyword already persisted
                }
                "merge" => {
                    let Some(to_keyword) = action.to.as_deref() else {
                        continue;
                    };
                    let to_keyword = to_keyword.trim();
                    if to_keyword.is_empty() {
                        continue;
                    }
                    let weight = action.weight.unwrap_or(1.0);
                    if let Some(from_keywords) = &action.from {
                        for from_keyword in from_keywords {
                            let from_keyword = from_keyword.trim();
                            if from_keyword.is_empty() {
                                continue;
                            }
                            tx.execute(
                                "DELETE FROM session_keywords WHERE session_id = ?1 AND keyword = ?2",
                                params![session_id, from_keyword],
                            )
                            .with_context(|| {
                                format!("remove merged session keyword {from_keyword}")
                            })?;
                        }
                    }
                    // UPSERT：合并到已存在的关键词时聚合权重，且保留原 created_at，
                    // 不再用 INSERT OR REPLACE 整行重写。
                    tx.execute(
                        "INSERT INTO session_keywords (session_id, keyword, weight, created_at)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(session_id, keyword) DO UPDATE SET
                             weight = session_keywords.weight + excluded.weight",
                        params![session_id, to_keyword, weight, ts],
                    )
                    .with_context(|| format!("merge session keyword into {to_keyword}"))?;
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
                        WHERE sk.session_id = ds.id
                          AND sk.keyword LIKE ?3 ESCAPE '\\' COLLATE NOCASE
                    ), 0.0) AS relevance_score
             FROM dispatcher_sessions ds
             WHERE ds.kind = ?4
               AND (?5 IS NULL OR ds.project_id = ?5)
               AND ds.category != ?7
               AND (
                   ds.title LIKE ?3 ESCAPE '\\' COLLATE NOCASE
                   OR EXISTS (
                       SELECT 1
                       FROM session_keywords sk
                       WHERE sk.session_id = ds.id
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
                limit,
                super::sessions::INTERNAL_CHAT_CATEGORY
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
        let session_ids = results
            .iter()
            .map(|result| result.session_id.clone())
            .collect::<Vec<_>>();
        let mut keywords_by_session = load_keywords_by_session_ids(&conn, &session_ids)?;

        // 命中关键词直接在上面已批量取回的关键词列表上做大小写不敏感的包含过滤，
        // 不再为每个会话单独发一条 SQL（消除 N+1 查询）。
        // 列表本身按 weight DESC, keyword ASC 排序，过滤后顺序与单独查询一致。
        let query_lower = query.to_lowercase();
        for result in &mut results {
            let keywords = keywords_by_session
                .remove(&result.session_id)
                .unwrap_or_default();
            result.matched_keywords = keywords
                .iter()
                .filter(|keyword| keyword.to_lowercase().contains(&query_lower))
                .cloned()
                .collect();
            result.keywords = keywords;
        }

        Ok(results)
    }

    #[allow(dead_code)]
    pub fn clear_keywords(&self, session_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM session_keywords WHERE session_id = ?1",
            params![session_id],
        )
        .context("clear session keywords")?;
        Ok(())
    }
}

pub(super) fn load_keywords_by_session_ids(
    conn: &Connection,
    session_ids: &[String],
) -> Result<HashMap<String, Vec<String>>> {
    if session_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders = std::iter::repeat_n("?", session_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT session_id, keyword
         FROM session_keywords
         WHERE session_id IN ({placeholders})
         ORDER BY session_id ASC, weight DESC, keyword ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(session_ids), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut keywords_by_session = HashMap::<String, Vec<String>>::new();
    for row in rows {
        let (session_id, keyword) = row?;
        keywords_by_session
            .entry(session_id)
            .or_default()
            .push(keyword);
    }
    Ok(keywords_by_session)
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

    fn add_keyword(db: &DispatcherDb, session_id: &str, keyword: &str, weight: f64) {
        db.apply_keyword_actions(
            session_id,
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

    #[test]
    fn adding_existing_keyword_aggregates_weight_and_keeps_created_at() {
        let db = test_db();
        let session = db
            .create_chat_session("权重聚合", Some("tech"))
            .expect("create chat session");

        add_keyword(&db, &session.id, "Rust", 2.0);
        let before = db
            .list_session_keywords(&session.id)
            .expect("list keywords after first add");
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].weight, 2.0);
        let created_at = before[0].created_at.clone();

        add_keyword(&db, &session.id, "Rust", 3.5);
        let after = db
            .list_session_keywords(&session.id)
            .expect("list keywords after duplicate add");

        assert_eq!(after.len(), 1, "重复添加不得产生第二行");
        assert_eq!(after[0].weight, 5.5, "重复添加应聚合权重");
        assert_eq!(after[0].created_at, created_at, "created_at 必须保留");
    }

    #[test]
    fn merge_into_existing_keyword_aggregates_weight_and_keeps_created_at() {
        let db = test_db();
        let session = db
            .create_chat_session("合并聚合", Some("tech"))
            .expect("create chat session");
        add_keyword(&db, &session.id, "Tauri", 4.0);
        add_keyword(&db, &session.id, "桌面壳", 1.0);
        let created_at = db
            .list_session_keywords(&session.id)
            .expect("list keywords")
            .into_iter()
            .find(|record| record.keyword == "Tauri")
            .expect("tauri keyword")
            .created_at;

        db.apply_keyword_actions(
            &session.id,
            &[KeywordAction {
                action: "merge".to_string(),
                keyword: None,
                from: Some(vec!["桌面壳".to_string()]),
                to: Some("Tauri".to_string()),
                weight: Some(2.0),
            }],
        )
        .expect("merge keywords");

        let after = db
            .list_session_keywords(&session.id)
            .expect("list keywords after merge");
        assert_eq!(after.len(), 1, "来源关键词应被移除");
        assert_eq!(after[0].keyword, "Tauri");
        assert_eq!(after[0].weight, 6.0, "合并应聚合权重");
        assert_eq!(after[0].created_at, created_at, "created_at 必须保留");
    }
}
