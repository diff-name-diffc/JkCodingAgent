//! 受管项目注册表：projects 表的读写与项目级联删除。
//!
//! 项目注册表是应用生命周期配置的一部分（全局权威源，存 SQLite）；
//! 会话等运行数据以 `project_id` 关联本表，项目删除时在同一事务内级联清理。

use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};

use super::content::{delete_chat_image_resources, remove_chat_image_dir};
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

/// 建表（幂等）。schema.rs 基线建库时调用，DDL 单一出处。
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

/// 项目删除后需要清理的文件资源（提交成功后 best-effort 执行）。
pub(crate) struct ProjectCleanupPlan {
    /// 被级联删除的会话 id 列表（供前端清理内存 store 中的会话残留状态）。
    pub deleted_session_ids: Vec<String>,
    /// 会话图片目录（chat-images/{workspace_id}，DB 行已在事务内删除）。
    pub image_dirs: Vec<PathBuf>,
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
    ///
    /// 防误用保护：载荷不允许缺失任何现存项目 id——「移除项目」必须走带级联
    /// 清理的 `delete_project`，否则该项目下的会话/图片等关联数据会成为孤儿。
    /// 事务用 IMMEDIATE：一上来就持有写锁，「读现存 id 校验 → DELETE 重写」
    /// 全程不会有其它写者插入提交，消除 DEFERRED 快照下的并发窗口。
    pub fn save_projects_all(&self, projects: &[Project]) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing_ids = tx
            .prepare("SELECT id FROM projects")
            .context("load existing project ids")?
            .query_map([], |row| row.get::<_, String>(0))
            .context("load existing project ids")?
            .collect::<std::result::Result<std::collections::HashSet<String>, _>>()
            .context("load existing project ids")?;
        let payload_ids: std::collections::HashSet<&str> =
            projects.iter().map(|project| project.id.as_str()).collect();
        if let Some(missing) = existing_ids
            .iter()
            .find(|id| !payload_ids.contains(id.as_str()))
        {
            anyhow::bail!(
                "不允许通过 save_projects 移除项目（会绕过级联清理），请使用 project_delete：缺失项目 {missing}"
            );
        }
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
    /// 返回被删会话 id 列表与提交后需要 best-effort 清理的文件资源清单。
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

        let mut image_dirs: Vec<PathBuf> = Vec::new();
        for workspace_id in &workspace_ids {
            if let Some(dir) = delete_chat_image_resources(&tx, workspace_id)? {
                image_dirs.push(dir);
            }
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
            deleted_session_ids: workspace_ids,
            image_dirs,
            workspace_dirs,
        })
    }
}

/// 项目删除提交后的文件清理：会话图片目录与仓库内运行期目录，失败仅告警。
pub(crate) fn cleanup_project_files(plan: &ProjectCleanupPlan) {
    for dir in &plan.image_dirs {
        if let Err(error) = remove_chat_image_dir(dir) {
            eprintln!("remove chat image dir failed (project delete): {error:#}");
        }
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
    fn save_projects_all_replaces_whole_list_and_keeps_order() {
        let (db, _root) = test_db();
        db.save_projects_all(&[sample_project("a", "/tmp/a"), sample_project("b", "/tmp/b")])
            .unwrap();
        // 允许重排与新增，但不允许缺失现存 id（见下条测试）。
        db.save_projects_all(&[
            sample_project("b", "/tmp/b"),
            sample_project("a", "/tmp/a"),
            sample_project("c", "/tmp/c"),
        ])
        .unwrap();
        let projects = db.list_projects().unwrap();
        let ids: Vec<&str> = projects.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["b", "a", "c"]);
    }

    #[test]
    fn save_projects_all_rejects_removal_and_rolls_back() {
        let (db, _root) = test_db();
        db.save_projects_all(&[sample_project("a", "/tmp/a"), sample_project("b", "/tmp/b")])
            .unwrap();

        // 载荷缺失现存 id：视为误用「移除项目」，整事务回滚并报错引导 project_delete。
        let error = db
            .save_projects_all(&[sample_project("b", "/tmp/b")])
            .unwrap_err();
        assert!(error.to_string().contains("project_delete"));

        let projects = db.list_projects().unwrap();
        let ids: Vec<&str> = projects.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"], "事务回滚后原列表保持不变");
    }

    #[test]
    fn delete_project_cascades_sessions_and_keeps_other_projects() {
        let (db, _root) = test_db();
        db.save_projects_all(&[
            sample_project("p1", "/tmp/p1"),
            sample_project("p2", "/tmp/p2"),
        ])
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
        let mut deleted_ids = plan.deleted_session_ids.clone();
        deleted_ids.sort();
        assert_eq!(deleted_ids, vec!["s1", "s2"], "返回值带出被删会话 id");
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
