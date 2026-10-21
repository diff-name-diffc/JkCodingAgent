//! 普通聊天删除/清空的目录清理：只回收当前会话临时目录，不读取分类成果目录。

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::agent::db::{DispatcherDb, DispatcherSessionKind};
use crate::agent::rig_ext::chat_workspace::ChatWorkspace;
use crate::agent::rig_ext::r#loop::resources::{
    AcquireError, Claim, Resource, ResourceArbiter, ResourceLease,
};
use crate::shared::error::format_anyhow_error;

pub(super) struct ChatCleanupGuard {
    plan: CleanupPlan,
    _lease: ResourceLease,
}

struct CleanupPlan {
    session_id: String,
    session_root: PathBuf,
    resource_root: PathBuf,
}

/// 只读准备与短等待发生在任何删除之前；Python/Shell 占用目录时明确拒绝。
pub(super) async fn prepare(
    db: &DispatcherDb,
    app_root: PathBuf,
    session_id: &str,
) -> Result<Option<ChatCleanupGuard>, String> {
    let db = db.clone();
    let session_id = session_id.to_string();
    let plan = tokio::task::spawn_blocking(move || -> Result<Option<CleanupPlan>> {
        let session = db
            .get_dispatcher_session(&session_id)?
            .with_context(|| format!("会话不存在：{session_id}"))?;
        CleanupPlan::new(&app_root, &session_id, session.kind)
    })
    .await
    .map_err(|error| format!("准备会话目录清理任务失败：{error}"))?
    .map_err(|error| format_anyhow_error(&error))?;
    let Some(plan) = plan else {
        return Ok(None);
    };
    plan.acquire().await.map(Some)
}

impl CleanupPlan {
    fn new(app_root: &Path, session_id: &str, kind: DispatcherSessionKind) -> Result<Option<Self>> {
        if kind != DispatcherSessionKind::Chat {
            return Ok(None);
        }
        let app_root = app_root.canonicalize().context("无法解析应用数据目录")?;
        let session_root = ChatWorkspace::session_directory(&app_root, session_id);
        // 父目录异常（如被替换为符号链接）只跳过目录回收：DB 删除/清空是权威
        // 操作，目录清理与 after_commit 一样保持 best-effort，不能因此让会话
        // 永远无法删除。
        if let Err(error) = validate_parent(&session_root) {
            eprintln!(
                "[chat-cleanup] 会话目录父路径校验未通过，跳过目录回收（session={session_id}）：{error:#}"
            );
            return Ok(None);
        }
        // 运行方使用 canonical 根；会话目录即使是链接，仲裁也必须锁其真实目标。
        // 不存在的会话目录（包括断链）没有运行中的文件操作，保留确定性路径身份。
        let resource_root = match session_root.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => session_root.clone(),
            Err(error) => return Err(error).context("无法解析会话临时目录"),
        };
        Ok(Some(Self {
            session_id: session_id.into(),
            session_root,
            resource_root,
        }))
    }

    async fn acquire(self) -> Result<ChatCleanupGuard, String> {
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let lease = ResourceArbiter::shared()
            .acquire(
                vec![Claim {
                    resource: Resource::LocalFilesystem(self.resource_root.clone()),
                    write: true,
                }],
                cancel_rx,
                Instant::now() + Duration::from_millis(100),
            )
            .await
            .map_err(|error| match error {
                AcquireError::QueueTimeout { .. } => {
                    "会话临时目录正在被 Python 或其他工具使用，请先停止相关运行后再删除或清空"
                        .to_string()
                }
                AcquireError::Cancelled => "会话目录清理已取消，尚未删除任何数据".into(),
            })?;
        Ok(ChatCleanupGuard {
            plan: self,
            _lease: lease,
        })
    }

    fn remove_directory(&self) -> Result<()> {
        // DB 操作之后再次校验父目录，防止它被替换成指向工程的链接。
        validate_parent(&self.session_root)?;
        let metadata = match std::fs::symlink_metadata(&self.session_root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).context("读取会话临时目录状态失败"),
        };
        // remove_dir_all 不跟随根目录或目录内的符号链接，只移除链接本身。
        let result = if metadata.is_dir() || metadata.file_type().is_symlink() {
            std::fs::remove_dir_all(&self.session_root)
        } else {
            std::fs::remove_file(&self.session_root)
        };
        match result {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("删除会话临时目录失败"),
        }
    }
}

impl ChatCleanupGuard {
    /// 调用方将守卫 move 进执行 DB 修改的 spawn_blocking 闭包，并在 DB 提交后调用。
    /// 即使上层 future 被取消，租约仍覆盖完整 DB 操作及目录删除。
    pub(super) fn after_commit(self, operation: &str) {
        if let Err(error) = self.plan.remove_directory() {
            eprintln!(
                "[chat-cleanup] {operation}已完成，但会话临时目录清理失败（session={}，目录={}）：{error:#}",
                self.plan.session_id,
                self.plan.session_root.display()
            );
        }
    }
}

fn validate_parent(session_root: &Path) -> Result<()> {
    let parent = session_root.parent().context("会话临时目录缺少父目录")?;
    match std::fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => anyhow::bail!(
            "会话目录的父路径不是普通目录，拒绝清理：{}",
            parent.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("读取会话临时目录父路径失败"),
    }
}

#[cfg(test)]
mod tests;
