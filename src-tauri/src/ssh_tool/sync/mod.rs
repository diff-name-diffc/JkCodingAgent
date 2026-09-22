//! rsync 编排；数据协议交给系统 rsync/OpenSSH，信任与认证来自现有 SSH 配置。
mod binaries;
mod process;
#[cfg(test)]
mod tests;
mod transport;

use super::{SshAuditRecord, SshServerConfig, SshSessionManager};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::watch;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SyncDirectory {
    pub ssh_profile: String,
    pub source: String,
    pub destination: String,
    #[serde(default)]
    pub delete: bool,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncProgress {
    pub transferred_bytes: u64,
    pub percent: u8,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncResult {
    pub ssh_profile: String,
    pub source: String,
    pub destination: String,
    pub delete: bool,
    pub dry_run: bool,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub timed_out: bool,
    pub duration_ms: u64,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
    pub progress: Option<SyncProgress>,
    pub audit_error: Option<String>,
}

impl SyncDirectory {
    pub fn validate(&self) -> Result<(), String> {
        if self.ssh_profile.is_empty() || self.source.is_empty() {
            return Err("ssh_profile 和 source 不能为空".into());
        }
        // 远端路径按 POSIX 解释；拒绝根目录与歧义路径。
        if !self.destination.starts_with('/')
            || self.destination.split('/').all(|s| s.is_empty())
            || self.destination.split('/').any(|s| s == "." || s == "..")
            || self.destination.chars().any(char::is_control)
            || self.source.chars().any(char::is_control)
        {
            return Err("source 不得含控制字符；destination 必须为非根目录的绝对 POSIX 路径，且不能含 .、.. 或控制字符".into());
        }
        Ok(())
    }

    pub fn command_description(&self) -> String {
        format!("sync_directory: 同步 source 目录内容到远端 destination；覆盖同名文件；delete=true 删除远端多余文件；dry_run=true 仅预演。{}", serde_json::to_string(self).expect("SyncDirectory is serializable"))
    }
}

struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

impl SshSessionManager {
    pub(crate) async fn append_sync_audit(&self, record: SshAuditRecord) -> Result<(), String> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || db.append_audit_record(&record))
            .await
            .map_err(|e| e.to_string())?
    }

    pub(crate) async fn sync_directory(
        &self,
        server: SshServerConfig,
        request: SyncDirectory,
        source: PathBuf,
        cancel: Option<watch::Receiver<bool>>,
        progress: Arc<dyn Fn(SyncProgress) + Send + Sync>,
    ) -> Result<SyncResult, String> {
        request.validate()?;
        transport::validate_endpoint(&server)?;
        if !cfg!(unix) {
            return Err("目录同步当前要求 macOS/Linux 的 rsync 与 OpenSSH".into());
        }
        if cancel.as_ref().is_some_and(|rx| *rx.borrow()) {
            return Err("目录同步已取消，未执行".into());
        }
        let binaries = tokio::task::spawn_blocking(transport::find_binaries)
            .await
            .map_err(|e| e.to_string())??;
        let connection = tokio::time::timeout(
            Duration::from_secs(server.default_timeout_secs.clamp(1, 300)),
            super::connection::connect_verified(&server, &self.db),
        );
        let (handle, key) = tokio::select! {
            result = connection => result.map_err(|_| "SSH 同步认证超时".to_string())??,
            _ = wait_for_cancel(cancel.clone()) => return Err("SSH 同步认证已取消".into()),
        };
        handle
            .disconnect(russh::Disconnect::ByApplication, "rsync transport", "")
            .await
            .map_err(|e| format!("关闭 SSH 校验连接失败：{e}"))?;
        let public_key = key.to_openssh().map_err(|e| e.to_string())?;
        let abort = Arc::new(AtomicBool::new(false));
        let _guard = CancelOnDrop(abort.clone());
        tokio::task::spawn_blocking(move || {
            let transport = transport::Transport::new(&server, &public_key, binaries)?;
            process::run(&server, request, source, transport, cancel, abort, progress)
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

async fn wait_for_cancel(mut cancel: Option<watch::Receiver<bool>>) {
    if let Some(rx) = &mut cancel {
        loop {
            if *rx.borrow() {
                return;
            }
            if rx.changed().await.is_err() {
                break;
            }
        }
    }
    std::future::pending::<()>().await;
}
