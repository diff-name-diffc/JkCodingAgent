//! 普通聊天 Python 执行继承当前分类目录；项目会话保留原有运行方式。

use super::*;
use crate::agent::db::DispatcherSessionKind;
use crate::agent::rig_ext::r#loop::resources::{
    AcquireError, Claim, Resource, ResourceArbiter, ResourceLease,
};

/// 同步数据库与目录访问；调用方必须放入 spawn_blocking。
pub(super) fn resolve_chat_workspace(
    db: &DispatcherDb,
    root_dir: &Path,
    session_id: &str,
) -> Result<Option<ChatWorkspace>> {
    let session = db
        .get_dispatcher_session(session_id)?
        .ok_or_else(|| anyhow!("Python 执行所属会话不存在：{session_id}"))?;
    if session.kind != DispatcherSessionKind::Chat {
        return Ok(None);
    }
    let directories = db
        .get_chat_session_category_agent_config(session_id)?
        .map(|config| config.project_directories)
        .unwrap_or_default();
    let session_root = ChatWorkspace::session_directory(root_dir, session_id);
    ChatWorkspace::resolve(&session_root, &directories)
        .map(Some)
        .context("解析聊天 Python 执行目录失败")
}

/// 获得租约后再确认目标，避免排队期间被删会话/清空消息后重建运行文件。
pub(super) fn prepare_current_run_paths(
    db: &DispatcherDb,
    root_dir: &Path,
    record: &PythonCodeRunRecord,
    workspace: Option<&ChatWorkspace>,
) -> Result<RunPaths> {
    if !db.python_code_run_is_current(record)? {
        anyhow::bail!("Python 执行目标已被清理或替换：会话、消息或运行记录已失效，未启动执行");
    }
    prepare_paths(root_dir, &record.run_id, workspace)
}

pub(super) async fn acquire_chat_workspace(
    workspace: Option<&ChatWorkspace>,
    stop_rx: watch::Receiver<bool>,
) -> Result<Option<ResourceLease>> {
    let Some(workspace) = workspace else {
        return Ok(None);
    };
    let claims = std::iter::once(&workspace.session_root)
        .chain(&workspace.project_roots)
        .map(|root| Claim {
            resource: Resource::LocalFilesystem(root.clone()),
            write: true,
        })
        .collect();
    ResourceArbiter::shared()
        .acquire(
            claims,
            stop_rx,
            tokio::time::Instant::now() + Duration::from_secs(60),
        )
        .await
        .map(Some)
        .map_err(|error| match error {
            AcquireError::Cancelled => anyhow!("Python 执行已取消，尚未取得目录写入权限"),
            AcquireError::QueueTimeout { .. } => anyhow!("{}", error.timeout_message()),
        })
}
