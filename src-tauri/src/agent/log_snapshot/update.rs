use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use rig::tool::ToolExecutionError;
use serde_json::json;
use tauri::AppHandle;
use tokio::sync::{watch, RwLock};

mod local;

pub(crate) use local::validate_text_file;

use super::index::{build_index, IndexError};
use super::model::{
    docker_window, ssh_session_id, GenerationManifest, LogSource, SnapshotFile, UpdateOptions,
    UpdateRecord, UpdateState, SNAPSHOT_SCHEMA_VERSION,
};
use super::store::SnapshotStore;
use crate::agent::command_history::{self, CommandHistoryStatus};
use crate::agent::rig_ext::review::RigReviewContext;
use crate::agent::ssh_review::command_gate::{authorize_ssh_command, SshCommandGateRequest};
use crate::ssh_tool::SshSessionManager;
use local::{copy_local_file, resolve_local_sources};

#[derive(Clone)]
pub(crate) struct UpdateContext {
    pub workspace_id: String,
    pub session_root: PathBuf,
    pub extra_allowed_dirs: Vec<PathBuf>,
    pub ssh_manager: SshSessionManager,
    pub review_context: RigReviewContext,
    pub app_handle: Option<AppHandle>,
}

pub(crate) struct UpdateJob {
    pub job_id: String,
    pub snapshot_id: String,
    pub generation: u64,
    pub options: UpdateOptions,
    pub current: Option<GenerationManifest>,
    pub lease: Arc<RwLock<()>>,
}

#[derive(Clone)]
pub(crate) struct UpdateReporter {
    record: UpdateRecord,
    sender: watch::Sender<UpdateRecord>,
    cancel: watch::Receiver<bool>,
    raw_counter: Arc<AtomicU64>,
}

impl UpdateReporter {
    pub(crate) fn new(
        record: UpdateRecord,
        sender: watch::Sender<UpdateRecord>,
        cancel: watch::Receiver<bool>,
    ) -> Self {
        Self {
            record,
            sender,
            cancel,
            raw_counter: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(crate) fn record(&self) -> UpdateRecord {
        self.record.clone()
    }

    pub(crate) fn cancel(&self) -> watch::Receiver<bool> {
        self.cancel.clone()
    }

    pub(crate) fn raw_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.raw_counter)
    }

    pub(crate) fn set_state(&mut self, state: UpdateState) {
        self.record.state = state;
        self.record.updated_at = Utc::now().to_rfc3339();
        self.sender.send_replace(self.record.clone());
    }

    pub(crate) fn set_counts(&mut self, raw_bytes: u64, file_count: usize) {
        self.record.raw_bytes = raw_bytes;
        self.record.file_count = file_count;
        self.record.updated_at = Utc::now().to_rfc3339();
        self.sender.send_replace(self.record.clone());
    }

    pub(crate) fn finish(&mut self, result: Result<(u64, usize), UpdateFailure>) {
        self.record = self.sender.borrow().clone();
        match result {
            Ok((raw_bytes, file_count)) => {
                self.record.raw_bytes = raw_bytes;
                self.record.file_count = file_count;
                self.record.error = None;
                self.record.state = UpdateState::Ready;
            }
            Err(error) => {
                self.record.error = Some(error.message);
                self.record.state = if error.cancelled {
                    UpdateState::Cancelled
                } else {
                    UpdateState::Failed
                };
            }
        }
        self.record.updated_at = Utc::now().to_rfc3339();
    }

    pub(crate) fn notify(&self) {
        self.sender.send_replace(self.record.clone());
    }
}

#[derive(Debug)]
pub(crate) struct UpdateFailure {
    pub cancelled: bool,
    pub message: String,
}

impl UpdateFailure {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            cancelled: false,
            message: message.into(),
        }
    }

    fn cancelled(message: impl Into<String>) -> Self {
        Self {
            cancelled: true,
            message: message.into(),
        }
    }
}

impl From<String> for UpdateFailure {
    fn from(message: String) -> Self {
        Self::failed(message)
    }
}

pub(crate) async fn execute_update(
    context: UpdateContext,
    store: SnapshotStore,
    job: UpdateJob,
    mut reporter: UpdateReporter,
) -> Result<GenerationManifest, UpdateFailure> {
    let deadline = Instant::now() + Duration::from_secs(job.options.timeout_secs);
    let sources = if job.options.sources.is_empty() {
        job.current
            .as_ref()
            .map(|current| current.sources.clone())
            .ok_or_else(|| UpdateFailure::failed("首次创建快照必须提供 sources"))?
    } else {
        job.options.sources.clone()
    };
    let sources = resolve_local_sources(&context, sources).await?;
    let window = docker_window(
        job.options.since.as_deref(),
        job.options.until.as_deref(),
        job.options.tail,
    )
    .map_err(UpdateFailure::failed)?;
    check_work(&reporter, deadline)?;

    set_stage(&store, &mut reporter, UpdateState::Reviewing).await?;
    let review_args = json!({
        "snapshot_id": &job.snapshot_id,
        "generation": job.generation,
        "sources": &sources,
        "since": job.options.since.as_deref(),
        "until": job.options.until.as_deref(),
        "tail": job.options.tail,
    });
    let session_id = ssh_session_id(&context.workspace_id, &job.snapshot_id);
    let mut approvals = Vec::with_capacity(sources.len());
    for source in &sources {
        let approval = if let Some(command) = source.remote_command(&window) {
            let server_id = source.server_id().unwrap_or_default();
            authorize_ssh_command(SshCommandGateRequest {
                manager: &context.ssh_manager,
                workspace: &context.session_root,
                workspace_id: &context.workspace_id,
                session_title: &context.review_context.session_title,
                server_id,
                session_id: &session_id,
                tool_name: "ssh_log_snapshot_update",
                command: &command,
                stdin: None,
                elevate: source.sudo(),
                args: &review_args,
                review_context: &context.review_context,
                app_handle: context.app_handle.clone(),
                cancel_rx: Some(reporter.cancel()),
            })
            .await
            .map_err(tool_failure)?
        } else {
            None
        };
        approvals.push(approval);
        check_work(&reporter, deadline)?;
    }

    set_stage(&store, &mut reporter, UpdateState::Syncing).await?;
    let staging = blocking({
        let store = store.clone();
        let snapshot_id = job.snapshot_id.clone();
        let job_id = job.job_id.clone();
        move || store.prepare_staging(&snapshot_id, &job_id)
    })
    .await??;
    let mut files = Vec::with_capacity(sources.len());
    let mut raw_bytes = 0u64;
    for (index, source) in sources.iter().enumerate() {
        check_work(&reporter, deadline)?;
        let file_id = index as u64 + 1;
        let raw_path = format!("raw/{file_id}.log");
        let destination = staging.join(&raw_path);
        let remaining = job.options.max_total_bytes.saturating_sub(raw_bytes).max(1);
        let bytes = match source {
            LogSource::LocalFile { path } => {
                let source_path = PathBuf::from(path);
                let copied = blocking({
                    let source_path = source_path.clone();
                    let destination = destination.clone();
                    let cancel = reporter.cancel();
                    let counter = reporter.raw_counter();
                    move || {
                        copy_local_file(
                            &source_path,
                            &destination,
                            remaining,
                            cancel,
                            deadline,
                            counter,
                        )
                    }
                })
                .await??;
                copied
            }
            LogSource::Docker { .. } | LogSource::SshFile { .. } => {
                let command = source.remote_command(&window).unwrap_or_default();
                let result = context
                    .ssh_manager
                    .execute_stream(
                        context.session_root.clone(),
                        context.workspace_id.clone(),
                        context.review_context.session_title.clone(),
                        source.server_id().unwrap_or_default().to_string(),
                        session_id.clone(),
                        command.clone(),
                        destination.clone(),
                        remaining,
                        remaining_secs(deadline),
                        Some(reporter.cancel()),
                        approvals[index].clone(),
                        source.sudo(),
                        reporter.raw_counter(),
                    )
                    .await
                    .map_err(|failure| UpdateFailure::failed(failure.message))?;
                command_history::record(
                    &context.workspace_id,
                    "ssh_log_snapshot_update",
                    source.server_id().unwrap_or_default(),
                    &command,
                    CommandHistoryStatus::Executed,
                    &format!(
                        "snapshot={} generation={} bytes={} exit={}",
                        job.snapshot_id,
                        job.generation,
                        result.bytes_written,
                        result.execution.exit_code
                    ),
                );
                // 取消优先于退出码判定：用户主动 cancel 应落 Cancelled 态而非
                // 失败（与 copy_local_file 的本地取消语义一致）。
                if result.execution.cancelled {
                    return Err(UpdateFailure::cancelled(format!(
                        "远程日志读取已取消（{}），当前暂存不会发布",
                        source.label()
                    )));
                }
                if result.execution.exit_code != 0 || result.execution.external_state_unknown {
                    return Err(UpdateFailure::failed(format!(
                        "远程日志读取失败（{}）：exit={} {}",
                        source.label(),
                        result.execution.exit_code,
                        result.execution.stderr
                    )));
                }
                result.bytes_written
            }
        };
        blocking({
            let destination = destination.clone();
            move || validate_text_file(&destination)
        })
        .await??;
        raw_bytes = raw_bytes
            .checked_add(bytes)
            .ok_or_else(|| UpdateFailure::failed("日志总字节数溢出"))?;
        if raw_bytes > job.options.max_total_bytes {
            return Err(UpdateFailure::failed(format!(
                "日志总大小 {raw_bytes} 超过上限 {}",
                job.options.max_total_bytes
            )));
        }
        reporter.set_counts(raw_bytes, files.len() + 1);
        files.push(SnapshotFile {
            file_id,
            source: source.clone(),
            source_label: source.label(),
            raw_path,
            size_bytes: bytes,
            line_count: 0,
            long_lines: 0,
        });
    }

    set_stage(&store, &mut reporter, UpdateState::Indexing).await?;
    let index = blocking({
        let staging = staging.clone();
        let files = files.clone();
        let cancel = reporter.cancel();
        let max_index_bytes = job.options.max_total_bytes.saturating_mul(2);
        move || build_index(&staging, &files, max_index_bytes, cancel, deadline)
    })
    .await?
    .map_err(|error| match error {
        IndexError::Cancelled(message) => UpdateFailure::cancelled(message),
        IndexError::Failed(message) => UpdateFailure::failed(message),
    })?;
    files = index.files;
    let now = Utc::now().to_rfc3339();
    let manifest = GenerationManifest {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        snapshot_id: job.snapshot_id.clone(),
        workspace_id: context.workspace_id.clone(),
        generation: job.generation,
        label: job.options.label.clone().or_else(|| {
            job.current
                .as_ref()
                .and_then(|current| current.label.clone())
        }),
        created_at: job
            .current
            .as_ref()
            .map(|current| current.created_at.clone())
            .unwrap_or_else(|| now.clone()),
        updated_at: now,
        sources,
        files,
        raw_bytes,
        index_bytes: index.index_bytes,
        index_backend: index.backend,
    };
    blocking({
        let store = store.clone();
        let staging = staging.clone();
        let manifest = manifest.clone();
        move || store.write_generation_manifest(&staging, &manifest)
    })
    .await??;

    set_stage(&store, &mut reporter, UpdateState::Publishing).await?;
    let _guard = job.lease.write().await;
    blocking({
        let store = store.clone();
        let snapshot_id = job.snapshot_id.clone();
        let job_id = job.job_id.clone();
        let old_generation = job.current.as_ref().map(|current| current.generation);
        move || store.publish(&snapshot_id, &job_id, job.generation, old_generation)
    })
    .await??;
    Ok(manifest)
}

async fn set_stage(
    store: &SnapshotStore,
    reporter: &mut UpdateReporter,
    state: UpdateState,
) -> Result<(), UpdateFailure> {
    reporter.set_state(state);
    let record = reporter.record();
    blocking({
        let store = store.clone();
        move || store.write_update(&record)
    })
    .await??;
    Ok(())
}

fn check_work(reporter: &UpdateReporter, deadline: Instant) -> Result<(), UpdateFailure> {
    if *reporter.cancel.borrow() {
        return Err(UpdateFailure::cancelled("日志快照更新已取消"));
    }
    if Instant::now() >= deadline {
        return Err(UpdateFailure::failed("日志快照更新超过时间预算"));
    }
    Ok(())
}

fn remaining_secs(deadline: Instant) -> u64 {
    deadline
        .saturating_duration_since(Instant::now())
        .as_secs()
        .max(1)
}

fn tool_failure(error: ToolExecutionError) -> UpdateFailure {
    UpdateFailure {
        cancelled: error.kind() == rig::tool::ToolErrorKind::Cancelled,
        message: error
            .model_feedback()
            .map(str::to_string)
            .unwrap_or_else(|| error.message().to_string()),
    }
}

async fn blocking<T: Send + 'static>(
    task: impl FnOnce() -> T + Send + 'static,
) -> Result<T, UpdateFailure> {
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|error| UpdateFailure::failed(format!("日志后台任务失败：{error}")))
}
