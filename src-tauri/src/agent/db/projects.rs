//! 受管项目注册表：projects 表的读写与项目级联删除。
//!
//! 项目注册表是应用生命周期配置的一部分（全局权威源），历史上存放在
//! `~/.jkcodingagent/projects.json`，v31 迁移将其导入 SQLite；会话等运行数据
//! 以 `project_id` 关联本表，项目删除时在同一事务内级联清理。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension, Transaction};

use super::content::{delete_chat_image_resources, remove_chat_image_files};
use super::DispatcherDb;
use crate::project::storage::Project;

const PROJECTS_DDL: &str = "CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    path TEXT NOT NULL UNIQUE,
    branch TEXT,
    last_opened_at INTEGER NOT NULL DEFAULT 0,
    sort_order INTEGER NOT NULL DEFAULT 0
)";

/// 建表（幂等）。基线 DDL 与 v31 迁移共用，保持同文。
pub(crate) fn ensure_projects_table_tx(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(PROJECTS_DDL)
        .context("create projects table")
}

fn row_to_project(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        path: row.get(2)?,
        branch: row.get(3)?,
        last_opened_at: row.get(4)?,
    })
}

/// v31 迁移：把 `root_dir/projects.json` 一次性导入 projects 表。
/// 表非空时短路（幂等），文件不存在/解析失败按空列表处理，不阻断升级。
/// 逐字段宽容解析（缺 id/path 的条目跳过，其余字段取默认值），
/// 兼容测试夹具与历史极简格式。返回导入的条数；调用方在事务提交
/// 成功后再删除旧文件。
pub(crate) fn import_legacy_projects_json(tx: &Transaction<'_>, root_dir: &Path) -> Result<usize> {
    let existing: i64 = tx
        .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
        .context("count projects")?;
    if existing > 0 {
        return Ok(0);
    }
    let Ok(raw) = std::fs::read_to_string(root_dir.join("projects.json")) else {
        return Ok(0);
    };
    let Ok(values) = serde_json::from_str::<Vec<serde_json::Value>>(&raw) else {
        return Ok(0);
    };
    let mut imported = 0usize;
    for (sort_order, value) in values.iter().enumerate() {
        let str_field = |key: &str| {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let id = str_field("id");
        let path = str_field("path");
        if id.trim().is_empty() || path.trim().is_empty() {
            continue;
        }
        let name = {
            let name = str_field("name");
            if name.trim().is_empty() { path.clone() } else { name }
        };
        let branch = value.get("branch").and_then(serde_json::Value::as_str);
        let last_opened_at = value
            .get("lastOpenedAt")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let changed = tx
            .execute(
                "INSERT OR IGNORE INTO projects (id, name, path, branch, last_opened_at, sort_order)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, name, path, branch, last_opened_at, sort_order as i64],
            )
            .with_context(|| format!("import project {id}"))?;
        imported += changed;
    }
    Ok(imported)
}

/// 项目删除后需要清理的文件资源（提交成功后 best-effort 执行）。
pub(crate) struct ProjectCleanupPlan {
    /// 聊天图片实际文件路径（DB 行已在事务内删除）。
    pub image_paths: Vec<PathBuf>,
    /// 项目仓库内应用自有的运行期数据目录（browser-profile / local_env）。
    pub workspace_dirs: Vec<PathBuf>,
}

impl DispatcherDb {
    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, path, branch, last_opened_at FROM projects ORDER BY sort_order, rowid",
            )
            .context("prepare list projects")?;
        let projects = stmt
            .query_map([], row_to_project)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("load projects")?;
        Ok(projects)
    }

    pub fn find_project(&self, project_id: &str) -> Result<Option<Project>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, name, path, branch, last_opened_at FROM projects WHERE id = ?1",
            params![project_id],
            row_to_project,
        )
        .optional()
        .context("load project")
    }

    /// 整列表同步保存：前端仍按「重写整个列表」的语义调用，事务内
    /// 先清空再按顺序插入，保持列表顺序（sort_order）稳定。
    pub fn save_projects_all(&self, projects: &[Project]) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM projects", [])
            .context("clear projects")?;
        for (sort_order, project) in projects.iter().enumerate() {
            tx.execute(
                "INSERT INTO projects (id, name, path, branch, last_opened_at, sort_order)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    project.id,
                    project.name,
                    project.path,
                    project.branch,
                    project.last_opened_at,
                    sort_order as i64
                ],
            )
            .with_context(|| format!("insert project {}", project.id))?;
        }
        tx.commit().context("commit save projects")
    }

    /// 删除项目及其全部关联数据：遍历该项目所有会话，在一个事务内执行与会话
    /// 删除相同的级联清理（`delete_project_session` 的表集合），并删除项目行。
    /// 返回提交后需要 best-effort 清理的文件资源清单。
    pub fn delete_project(&self, project_id: &str) -> Result<ProjectCleanupPlan> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;

        let project = tx
            .query_row(
                "SELECT id, name, path, branch, last_opened_at FROM projects WHERE id = ?1",
                params![project_id],
                row_to_project,
            )
            .optional()
            .context("load project for delete")?
            .ok_or_else(|| anyhow::anyhow!("project not found: {project_id}"))?;

        let workspace_ids: Vec<String> = {
            let mut stmt = tx
                .prepare("SELECT id FROM dispatcher_sessions WHERE project_id = ?1")
                .context("load project sessions")?;
            let ids = stmt
                .query_map(params![project_id], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .context("collect project session ids")?;
            ids
        };

        let mut image_paths: Vec<PathBuf> = Vec::new();
        for workspace_id in &workspace_ids {
            image_paths.extend(delete_chat_image_resources(&tx, workspace_id)?);
            tx.execute(
                "DELETE FROM dispatcher_tool_artifacts WHERE workspace_id = ?1",
                params![workspace_id],
            )?;
            tx.execute(
                "DELETE FROM dispatcher_tool_runs WHERE workspace_id = ?1",
                params![workspace_id],
            )?;
            tx.execute(
                "DELETE FROM sub_agent_run_traces WHERE workspace_id = ?1",
                params![workspace_id],
            )?;
            tx.execute(
                "DELETE FROM graph_node_runs
                 WHERE plan_id IN (SELECT id FROM graph_plans WHERE workspace_id = ?1)",
                params![workspace_id],
            )?;
            tx.execute(
                "DELETE FROM graph_plans WHERE workspace_id = ?1",
                params![workspace_id],
            )?;
            tx.execute(
                "DELETE FROM dispatcher_session_token_usage WHERE workspace_id = ?1",
                params![workspace_id],
            )?;
            tx.execute(
                "DELETE FROM session_keywords WHERE session_id = ?1",
                params![workspace_id],
            )?;
            tx.execute(
                "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
                params![workspace_id],
            )?;
            tx.execute(
                "DELETE FROM project_sessions WHERE id = ?1",
                params![workspace_id],
            )?;
            tx.execute(
                "DELETE FROM dispatcher_sessions WHERE id = ?1",
                params![workspace_id],
            )?;
        }
        tx.execute("DELETE FROM projects WHERE id = ?1", params![project_id])?;
        tx.commit().context("commit delete project")?;

        let mut workspace_dirs = Vec::new();
        let repo_root = PathBuf::from(&project.path);
        let jk_dir = repo_root.join(".jkcodingagent");
        // 只清理应用自有的运行期数据；config.toml / mcp.json 可能随仓库共享给
        // 团队（git 提交），删除项目时保留。
        workspace_dirs.push(jk_dir.join("browser-profile"));
        workspace_dirs.push(jk_dir.join("local_env"));

        Ok(ProjectCleanupPlan {
            image_paths,
            workspace_dirs,
        })
    }
}

/// 项目删除提交后的文件清理：图片文件与仓库内运行期目录，失败仅告警。
pub(crate) fn cleanup_project_files(plan: &ProjectCleanupPlan) {
    if let Err(error) = remove_chat_image_files(&plan.image_paths) {
        eprintln!("remove chat image files failed (project delete): {error:#}");
    }
    for dir in &plan.workspace_dirs {
        if !dir.exists() {
            continue;
        }
        if let Err(error) = std::fs::remove_dir_all(dir) {
            eprintln!(
                "remove project workspace dir failed ({}): {error}",
                dir.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> (DispatcherDb, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "aha-projects-db-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        (DispatcherDb::new(root.join("jkbot.sqlite3")).unwrap(), root)
    }

    fn sample_project(id: &str, path: &str) -> Project {
        Project {
            id: id.to_string(),
            name: format!("项目 {id}"),
            path: path.to_string(),
            branch: Some("main".to_string()),
            last_opened_at: 1_700_000_000,
        }
    }

    #[test]
    fn migrates_legacy_projects_json_on_first_init() {
        let (db, root) = test_db();
        // DispatcherDb::new 已完成 v31 迁移，但初始没有 projects.json → 空表。
        assert!(db.list_projects().unwrap().is_empty());

        // 模拟旧用户：写入 projects.json 后用新库文件重跑迁移。
        drop(db);
        std::fs::remove_file(root.join("jkbot.sqlite3")).unwrap();
        let legacy = r#"[
            {"id":"p1","name":"A","path":"/tmp/a","branch":"main","lastOpenedAt":111},
            {"id":"p2","name":"B","path":"/tmp/b","branch":null,"lastOpenedAt":222}
        ]"#;
        std::fs::write(root.join("projects.json"), legacy).unwrap();
        let db = DispatcherDb::new(root.join("jkbot.sqlite3")).unwrap();
        let projects = db.list_projects().unwrap();
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].id, "p1");
        assert_eq!(projects[1].branch, None);
        assert_eq!(projects[1].last_opened_at, 222);
        // 提交成功后旧文件被清理。
        assert!(!root.join("projects.json").exists());
    }

    #[test]
    fn save_projects_all_replaces_whole_list_and_keeps_order() {
        let (db, _root) = test_db();
        db.save_projects_all(&[
            sample_project("a", "/tmp/a"),
            sample_project("b", "/tmp/b"),
        ])
        .unwrap();
        db.save_projects_all(&[sample_project("b", "/tmp/b"), sample_project("c", "/tmp/c")])
            .unwrap();
        let projects = db.list_projects().unwrap();
        let ids: Vec<&str> = projects.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["b", "c"]);
        assert!(db.find_project("a").unwrap().is_none());
    }

    #[test]
    fn delete_project_cascades_sessions_and_keeps_other_projects() {
        let (db, _root) = test_db();
        db.save_projects_all(&[sample_project("p1", "/tmp/p1"), sample_project("p2", "/tmp/p2")])
            .unwrap();

        let conn = db.conn().unwrap();
        for (session, project) in [("s1", "p1"), ("s2", "p1"), ("s3", "p2")] {
            conn.execute(
                "INSERT INTO dispatcher_sessions (id, project_id, kind, title, category, created_at, updated_at)
                 VALUES (?1, ?2, 'project', 't', '', '2026-01-01', '2026-01-01')",
                params![session, project],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO project_sessions (id, project_id, title, created_at, updated_at)
                 VALUES (?1, ?2, 't', '2026-01-01', '2026-01-01')",
                params![session, project],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO dispatcher_messages (id, workspace_id, role, created_at, context_payload)
                 VALUES (?1, ?2, 'user', '2026-01-01', '{}')",
                params![format!("msg-{session}"), session],
            )
            .unwrap();
        }
        drop(conn);

        let plan = db.delete_project("p1").unwrap();
        cleanup_project_files(&plan);

        let conn = db.conn().unwrap();
        let sessions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatcher_sessions WHERE project_id = 'p1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sessions, 0);
        let messages: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatcher_messages WHERE workspace_id IN ('s1','s2')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(messages, 0);
        let kept: i64 = conn
            .query_row("SELECT COUNT(*) FROM dispatcher_messages", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(kept, 1, "其他项目的会话消息不受影响");
        drop(conn);
        assert!(db.find_project("p1").unwrap().is_none());
        assert!(db.find_project("p2").unwrap().is_some());

        // 删除不存在的项目显式报错。
        assert!(db.delete_project("missing").is_err());
    }
}
