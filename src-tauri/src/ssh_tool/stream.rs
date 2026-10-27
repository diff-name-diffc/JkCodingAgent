use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;

use super::{
    CommandFailure, SshAuditRecord, SshAuditReview, SshExecResult, SshSessionKey,
    SshSessionManager, SshStreamResult,
};

impl SshSessionManager {
    /// 流式执行远程命令并落盘。**前置条件：`destination` 必须位于 `workspace_path`
    /// 下的快照暂存目录内**（调用方 `prepare_staging` 已按 snapshot_id/job_id 校验）；
    /// 本函数不再重复校验，新增调用方必须先保证该条件，否则会写出工作区之外。
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn execute_stream(
        &self,
        workspace_path: PathBuf,
        workspace_id: String,
        session_title: String,
        server_id: String,
        session_id: String,
        command: String,
        destination: PathBuf,
        max_bytes: u64,
        timeout_secs: u64,
        cancel_rx: Option<tokio::sync::watch::Receiver<bool>>,
        review: Option<SshAuditReview>,
        elevate: bool,
        written_counter: Arc<AtomicU64>,
    ) -> Result<SshStreamResult, CommandFailure> {
        let server = self
            .server_config_async(server_id.clone())
            .await
            .map_err(CommandFailure::other)?;
        super::validation::validate_session_id(&session_id).map_err(CommandFailure::other)?;
        super::validation::validate_command(&command).map_err(CommandFailure::other)?;
        if max_bytes == 0 {
            return Err(CommandFailure::other(
                "日志同步字节上限必须大于 0".to_string(),
            ));
        }
        if elevate && server.password.is_empty() {
            return Err(CommandFailure::other(
                "错误：服务器未配置登录口令，无法通过 sudo 读取日志；请使用有权限的账号或配置 NOPASSWD。"
                    .to_string(),
            ));
        }
        let key = SshSessionKey {
            server_id: server.id.clone(),
            session_id: session_id.clone(),
        };
        let started = Instant::now();
        let result = match self.connection_for(key.clone(), &server).await {
            Ok(connection) => {
                super::command_stream::run_command_to_file_on_connection(
                    &connection,
                    &server,
                    &server_id,
                    &session_id,
                    &command,
                    destination,
                    max_bytes,
                    timeout_secs.clamp(1, 21_600),
                    started,
                    cancel_rx,
                    elevate,
                    written_counter,
                )
                .await
            }
            Err(error) => Err(CommandFailure::other(error)),
        };
        if result.as_ref().is_err_and(|failure| failure.stale) {
            self.drop_session(&key);
        }
        let audit_result: Result<SshExecResult, CommandFailure> = match &result {
            Ok(stream) => {
                let mut execution = stream.execution.clone();
                execution.stdout = format!(
                    "[远程日志已流式写入会话快照暂存区：{} bytes]",
                    stream.bytes_written
                );
                Ok(execution)
            }
            Err(failure) => Err(CommandFailure {
                message: failure.message.clone(),
                stale: failure.stale,
                kind: failure.kind,
            }),
        };
        let record = SshAuditRecord::from_execution(
            &workspace_path,
            workspace_id,
            session_title,
            server_id,
            session_id,
            command,
            &audit_result,
            review.as_ref(),
            elevate,
        );
        let ssh_db = self.db.clone();
        let audit_error = tokio::task::spawn_blocking(move || ssh_db.append_audit_record(&record))
            .await
            .map_err(|error| CommandFailure::other(error.to_string()))?
            .err();
        if let Some(error) = audit_error {
            eprintln!("[ssh-log-stream] 写入 SSH 审计失败：{error}");
            if let Ok(stream) = &result {
                let mut execution = stream.execution.clone();
                execution
                    .stderr
                    .push_str(&format!("\n[警告：审计记录写入失败：{error}]"));
                return Ok(SshStreamResult {
                    execution,
                    bytes_written: stream.bytes_written,
                });
            }
        }
        result
    }
}
