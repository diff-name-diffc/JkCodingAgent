use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::watch;
use tokio::time::{Duration, Instant};

use super::paths::{resolve_driver_path, sidecar_command};
use super::process::{self, BrowserProcess};
use super::session::{Operation, Session};
use super::{
    import_chrome_profile_blocking, load_launch_options, BrowserAgentEventCallback, BrowserManager,
    BrowserProfileImportResult, BrowserStatus,
};

impl BrowserManager {
    pub(super) async fn session(&self, session_id: &str) -> Arc<Session> {
        self.sessions
            .lock()
            .entry(session_id.into())
            .or_default()
            .clone()
    }

    pub async fn start(
        &self,
        app: AppHandle,
        session_id: String,
        project_path: String,
    ) -> Result<BrowserStatus, String> {
        let deadline = Instant::now() + Duration::from_secs(180);
        let session = self.session(&session_id).await;
        let mut operation = session.enter(false, deadline, None).await?;
        let process = operation
            .run(
                deadline,
                None,
                None,
                ensure_started(&session, app, session_id, project_path),
            )
            .await;
        complete_if_healthy(&session, &mut operation, &process);
        process.map(|process| process.status())
    }

    pub async fn stop(&self, session_id: &str) -> Result<(), String> {
        let session = self.sessions.lock().get(session_id).cloned();
        if let Some(session) = session {
            session.stop().await?;
        }
        Ok(())
    }

    pub async fn status(&self, session_id: &str) -> BrowserStatus {
        let session = self.sessions.lock().get(session_id).cloned();
        session
            .and_then(|session| session.process())
            .map(|process| process.status())
            .unwrap_or_else(|| BrowserStatus {
                session_id: session_id.into(),
                state: "closed".into(),
                url: None,
                message: None,
            })
    }

    pub async fn command(
        &self,
        app: AppHandle,
        session_id: String,
        project_path: String,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        self.command_with_cancellation(
            app,
            session_id,
            project_path,
            method,
            params,
            None,
            Duration::from_secs(180),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn command_with_cancellation(
        &self,
        app: AppHandle,
        session_id: String,
        project_path: String,
        method: &str,
        params: Value,
        cancel_rx: Option<watch::Receiver<bool>>,
        timeout: Duration,
    ) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        let session = self.session(&session_id).await;
        let mut operation = session.enter(false, deadline, cancel_rx.clone()).await?;
        let result = operation
            .run(deadline, cancel_rx, None, async {
                let process = ensure_started(&session, app, session_id, project_path).await?;
                process.request(method, params).await
            })
            .await;
        complete_if_healthy(&session, &mut operation, &result);
        result
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn explore(
        &self,
        app: AppHandle,
        session_id: String,
        project_path: String,
        run_id: String,
        tool_call_id: String,
        mut params: Value,
        cancel_rx: Option<watch::Receiver<bool>>,
        on_event: BrowserAgentEventCallback,
    ) -> Result<Value, String> {
        let timeout_secs = params
            .get("timeoutSecs")
            .and_then(Value::as_u64)
            .filter(|seconds| {
                *seconds > 0 && *seconds <= crate::agent::sub_agent::config::MAX_TIMEOUT_SECS
            })
            .ok_or_else(|| {
                format!(
                    "浏览器探索 timeoutSecs 必须为 1–{} 秒",
                    crate::agent::sub_agent::config::MAX_TIMEOUT_SECS
                )
            })?;
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        let session = self.session(&session_id).await;
        if tool_call_id.trim().is_empty() {
            return Err("浏览器探索缺少工具调用身份".into());
        }
        let mut operation = session
            .enter_exploration(tool_call_id.clone(), deadline, cancel_rx.clone())
            .await?;
        let parameters = params
            .as_object_mut()
            .ok_or_else(|| "浏览器探索参数必须为对象".to_string())?;
        parameters.insert("runId".into(), Value::String(run_id.clone()));
        parameters.insert("toolCallId".into(), Value::String(tool_call_id.clone()));
        let result = operation
            .run(deadline, cancel_rx, Some(&run_id), async {
                let process = ensure_started(&session, app, session_id, project_path).await?;
                process.set_agent(run_id.clone(), tool_call_id.clone(), on_event);
                let result = process.request("explore", params).await;
                // 失败也必须解除 automating 绑定，否则保留进程时状态卡在探索中。
                process.finish_agent();
                result
            })
            .await;
        complete_if_healthy(&session, &mut operation, &result);
        result
    }

    pub(super) async fn import_profile(
        &self,
        app: AppHandle,
        session_id: String,
        _project_path: PathBuf,
        chrome_profile_path: PathBuf,
    ) -> Result<BrowserProfileImportResult, String> {
        let deadline = Instant::now() + Duration::from_secs(180);
        let session = self.session(&session_id).await;
        let mut operation = session.enter(false, deadline, None).await?;
        if let Some(process) = session.process() {
            process.terminate("正在导入 Chrome 登录态").await?;
        }
        // 文件复制不触碰浏览器，保持会话租约直到复制结算，禁止中途启动同一 profile。
        tokio::task::spawn_blocking(move || {
            let project_path = super::profile::workspace_for_session(
                app.state::<crate::agent::DispatcherState>().db(),
                &session_id,
            )?;
            let result =
                import_chrome_profile_blocking(project_path, chrome_profile_path, &session_id);
            operation.complete();
            result
        })
        .await
        .map_err(|error| format!("导入 Chrome 登录态任务失败：{error}"))?
    }
}

/// sidecar 明确返回业务错误（导航失败、参数被拒等，进程仍健康）时不终止
/// 进程树——手动浏览打错一个域名不应杀掉整个浏览器。取消/超时在
/// `Operation::run` 内已 terminate（进程不 alive），启动失败由
/// `ensure_started` 先行清理，二者都会走 Drop 的 abort 兜底。
fn complete_if_healthy<T>(
    session: &Arc<Session>,
    operation: &mut Operation,
    result: &Result<T, String>,
) {
    if result.is_ok() || session.process().is_some_and(|process| process.is_alive()) {
        operation.complete();
    }
}

async fn ensure_started(
    session: &Arc<Session>,
    app: AppHandle,
    session_id: String,
    project_path: String,
) -> Result<Arc<BrowserProcess>, String> {
    if let Some(process) = session.process() {
        if process.is_alive() {
            return Ok(process);
        }
        if !process.has_exited() {
            process.terminate("重启前确认旧浏览器已停止").await?;
        }
    }
    let options_app = app.clone();
    let options_session = session_id.clone();
    let (options, command) = tokio::task::spawn_blocking(move || {
        Ok::<_, String>((
            load_launch_options(&options_app, &PathBuf::from(project_path), &options_session)?,
            sidecar_command(&resolve_driver_path(&options_app)?),
        ))
    })
    .await
    .map_err(|error| format!("读取浏览器启动配置失败：{error}"))??;
    if !options.config.enabled {
        return Err("浏览器已在全局浏览器设置中禁用".into());
    }
    let events = session.event_sink(Arc::new(move |name, value| {
        let _ = app.emit(name, value);
    }));
    let process = process::spawn(command, session_id.clone(), events)?;
    *session.process.lock() = Some(process.clone());
    if let Err(error) = process
        .request(
            "start",
            json!({
                "sessionId": session_id, "userDataDir": options.user_data_dir, "headless": true,
                "proxy": empty_to_null(&options.config.proxy), "locale": empty_to_null(&options.config.locale),
                "timezone": empty_to_null(&options.config.timezone), "profileDirectory": options.profile_directory,
                "viewport": { "width": options.config.viewport_width, "height": options.config.viewport_height }
            }),
        )
        .await
    {
        // 半启动的服务不能留作复用：start 未完成时后续请求只会得到「浏览器
        // 尚未启动」。先终止再上抛，Drop 的 abort 幂等兜底残余进程组。
        let _ = process.terminate("浏览器启动未完成").await;
        return Err(error);
    }
    if !process.is_alive() {
        return Err("浏览器服务在启动完成前退出".into());
    }
    session.preview.attach(&process);
    Ok(process)
}

fn empty_to_null(value: &str) -> Value {
    if value.trim().is_empty() {
        Value::Null
    } else {
        Value::String(value.trim().into())
    }
}
