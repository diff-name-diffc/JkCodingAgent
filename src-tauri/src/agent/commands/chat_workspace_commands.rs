//! 普通聊天「分类文件」面板的目录总览：暴露会话临时目录的真实路径与分类
//! 共享工程目录的逐条校验结果。前端据此把两侧目录作为授权根复用项目页的
//! 文件命令（read_dir_entries / read_file_content / write_file_content 等）
//! 完成浏览与编辑，本模块不提供任何读写命令。

use super::*;
use crate::agent::chat_directories::normalize_with_app_root;
use crate::agent::rig_ext::chat_workspace::ChatWorkspace;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatWorkspaceProjectDir {
    /// 设置中心写入的原始路径串。
    pub raw: String,
    /// 校验通过后的 canonical 绝对路径；失效为 None。
    pub canonical: Option<String>,
    /// 失效原因（canonical 为 None 时非空）。
    pub error: Option<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatWorkspaceOverview {
    /// 会话临时目录（存在时为 canonical；目录可能尚未创建，保留构造路径）。
    pub session_dir: Option<String>,
    pub session_dir_exists: bool,
    pub project_dirs: Vec<ChatWorkspaceProjectDir>,
}

/// 面板展示用工程目录校验：逐条回报（失效目录不静默剔除，面板显示原因）；
/// canonical 重复条目按运行时 normalize 的去重口径保留首个。
pub(crate) fn validate_project_directories_for_display(
    raw: &[String],
    app_root: &Path,
) -> Vec<ChatWorkspaceProjectDir> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for value in raw {
        let entry = match normalize_with_app_root(std::slice::from_ref(value), app_root) {
            Ok(paths) => match paths.into_iter().next() {
                Some(canonical) if seen.insert(canonical.clone()) => ChatWorkspaceProjectDir {
                    raw: value.clone(),
                    canonical: Some(canonical.to_string_lossy().into_owned()),
                    error: None,
                },
                Some(_) => continue,
                None => ChatWorkspaceProjectDir {
                    raw: value.clone(),
                    canonical: None,
                    error: Some("共享工程目录校验未返回路径".to_string()),
                },
            },
            Err(error) => ChatWorkspaceProjectDir {
                raw: value.clone(),
                canonical: None,
                error: Some(format_anyhow_error(&error)),
            },
        };
        entries.push(entry);
    }
    entries
}

/// 分类文件面板总览：会话临时目录仅在 session_id 属实且为普通聊天会话时返回
/// （与 chat_upload_file 同口径 fail-closed，防项目会话 id 探测路径）；
/// 工程目录按分类配置逐条校验。
#[tauri::command]
pub async fn chat_workspace_overview(
    state: tauri::State<'_, DispatcherState>,
    category_id: String,
    session_id: Option<String>,
) -> Result<ChatWorkspaceOverview, String> {
    let db = state.db().clone();
    let app_root = state.agent_config().root_dir;
    chat_workspace_overview_impl(db, app_root, category_id, session_id).await
}

async fn chat_workspace_overview_impl(
    db: DispatcherDb,
    app_root: PathBuf,
    category_id: String,
    session_id: Option<String>,
) -> Result<ChatWorkspaceOverview, String> {
    // 配置读取（rusqlite）与路径校验（canonicalize）都是同步阻塞，统一走阻塞线程。
    tokio::task::spawn_blocking(move || {
        let configs = db
            .list_chat_category_agent_configs()
            .map_err(|error| format_anyhow_error(&error))?;
        let raw_dirs = configs
            .iter()
            .find(|config| config.category_id == category_id)
            .map(|config| config.project_directories.clone())
            .unwrap_or_default();

        let session_dir = match session_id {
            Some(id) => {
                let session = db
                    .get_dispatcher_session(&id)
                    .map_err(|error| format_anyhow_error(&error))?
                    .ok_or_else(|| "会话不存在或已删除".to_string())?;
                if session.kind != DispatcherSessionKind::Chat {
                    return Err("仅普通聊天会话拥有会话临时目录".to_string());
                }
                let dir = ChatWorkspace::session_directory(&app_root, &id);
                // 存在时回 canonical（与 chat_upload_file 返回口径一致）；
                // 尚未创建保留构造路径，由前端显示「未创建」空态。
                Some(
                    dir.canonicalize()
                        .unwrap_or(dir)
                        .to_string_lossy()
                        .into_owned(),
                )
            }
            None => None,
        };
        let session_dir_exists = session_dir
            .as_deref()
            .map(|dir| Path::new(dir).is_dir())
            .unwrap_or(false);
        Ok(ChatWorkspaceOverview {
            session_dir,
            session_dir_exists,
            project_dirs: validate_project_directories_for_display(&raw_dirs, &app_root),
        })
    })
    .await
    .map_err(|error| format!("读取聊天工作区目录失败：{error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDirGuard;

    fn config_for(category_id: &str, dirs: Vec<String>) -> ChatCategoryAgentConfig {
        ChatCategoryAgentConfig {
            category_id: category_id.to_string(),
            category_name: "测试分类".to_string(),
            allowed_tools: vec![],
            system_prompt: String::new(),
            sub_agent_ids: vec![],
            project_directories: dirs,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn display_validation_reports_each_entry_independently() {
        let guard = TempDirGuard::new("chat-workspace-display");
        let app_root = guard.path().join(".jkcodingagent");
        std::fs::create_dir(&app_root).unwrap();
        let valid = guard.path().join("project");
        std::fs::create_dir(&valid).unwrap();
        let file = guard.path().join("notes.txt");
        std::fs::write(&file, "x").unwrap();

        let raw = vec![
            valid.display().to_string(),
            valid.join(".").display().to_string(),
            guard.path().join("missing").display().to_string(),
            file.display().to_string(),
        ];
        let entries = validate_project_directories_for_display(&raw, &app_root);
        // 同一目录的两种写法去重为一条，失效路径各自保留并带原因。
        assert_eq!(entries.len(), 3, "{entries:?}");
        assert_eq!(
            entries[0].canonical.as_deref(),
            Some(valid.canonicalize().unwrap().to_str().unwrap())
        );
        assert!(entries[0].error.is_none());
        assert!(entries[1].canonical.is_none());
        assert!(entries[1].error.as_deref().unwrap().contains("不存在"));
        assert!(entries[2].canonical.is_none());
        assert!(entries[2].error.as_deref().unwrap().contains("必须是目录"));
    }

    #[tokio::test]
    async fn overview_returns_session_dir_and_validated_project_dirs() {
        let guard = TempDirGuard::new("chat-workspace-overview");
        let app_root = guard.path().join("app");
        let db = DispatcherDb::new(guard.path().join("test.sqlite3")).unwrap();
        let project = guard.path().join("shared");
        std::fs::create_dir(&project).unwrap();
        db.save_chat_category_agent_configs(&[config_for(
            "tech",
            vec![project.display().to_string()],
        )])
        .unwrap();

        let session = db.create_chat_session("first", Some("tech")).unwrap();
        let overview = chat_workspace_overview_impl(
            db.clone(),
            app_root.clone(),
            "tech".to_string(),
            Some(session.id.clone()),
        )
        .await
        .unwrap();

        // 尚未运行过 run：目录未创建，路径确定性可得。
        assert!(!overview.session_dir_exists);
        let expected = ChatWorkspace::session_directory(&app_root, &session.id);
        assert_eq!(
            overview.session_dir.as_deref(),
            Some(expected.to_str().unwrap())
        );
        assert_eq!(overview.project_dirs.len(), 1);
        assert!(overview.project_dirs[0].error.is_none());

        std::fs::create_dir_all(&expected).unwrap();
        let overview = chat_workspace_overview_impl(
            db.clone(),
            app_root.clone(),
            "tech".to_string(),
            Some(session.id.clone()),
        )
        .await
        .unwrap();
        assert!(overview.session_dir_exists);
        assert_eq!(
            overview.session_dir.as_deref(),
            Some(expected.canonicalize().unwrap().to_str().unwrap())
        );

        // 非普通聊天会话 fail-closed；未知分类 → 空工程目录列表。
        let project_session = db.create_project_session("proj-1", "项目会话").unwrap();
        let error = chat_workspace_overview_impl(
            db.clone(),
            app_root.clone(),
            "tech".to_string(),
            Some(project_session.id),
        )
        .await
        .unwrap_err();
        assert!(error.contains("仅普通聊天"), "{error}");
        let unknown =
            chat_workspace_overview_impl(db, app_root, "no-such-category".to_string(), None)
                .await
                .unwrap();
        assert!(unknown.project_dirs.is_empty());
        assert!(unknown.session_dir.is_none());
    }

    #[tokio::test]
    async fn overview_marks_invalid_project_dirs_without_failing() {
        let guard = TempDirGuard::new("chat-workspace-invalid");
        let app_root = guard.path().join("app");
        let db = DispatcherDb::new(guard.path().join("test.sqlite3")).unwrap();
        let project = guard.path().join("shared");
        std::fs::create_dir(&project).unwrap();
        db.save_chat_category_agent_configs(&[config_for(
            "tech",
            vec![project.display().to_string()],
        )])
        .unwrap();
        // 保存后目录被删除：总览逐条回报失效，不整体失败。
        std::fs::remove_dir(&project).unwrap();
        let overview = chat_workspace_overview_impl(db, app_root, "tech".to_string(), None)
            .await
            .unwrap();
        assert_eq!(overview.project_dirs.len(), 1);
        assert!(overview.project_dirs[0].canonical.is_none());
        assert!(overview.project_dirs[0].error.is_some());
    }
}
