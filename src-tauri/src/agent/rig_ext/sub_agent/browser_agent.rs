//! 浏览器探索的执行适配器：独立 browser-use 运行，不进入逐动作命令审查。
//! 父调用只接收最终结果；过程事件与检查点始终归属同一 session/tool_call_id。

use std::sync::Arc;

use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::Manager;

use super::browser_events::{bounded, BrowserTrace, BrowserUsage};
use super::events::SubAgentEvent;
use super::runner::{resolve_sub_agent_spec, RigSubAgentRequest};
use crate::agent::rig_ext::chat_workspace::ChatWorkspace;
use crate::agent::rig_ext::model::PurposeModelSpec;
use crate::agent::sub_agent::{config::SubAgentConfig, manager::SubAgentManager};
use crate::browser::BrowserManager;

pub(super) struct BrowserAgentRuntime {
    config: SubAgentConfig,
    spec: PurposeModelSpec,
    workspace: String,
    chat_workspace: Option<ChatWorkspace>,
    cancel_rx: Option<tokio::sync::watch::Receiver<bool>>,
    trace: BrowserTrace,
    status: Mutex<&'static str>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserResult {
    success: bool,
    result: String,
    iterations: u32,
    token_usage: BrowserUsage,
    #[serde(default)]
    urls: Vec<String>,
}

impl BrowserAgentRuntime {
    pub fn build(request: &RigSubAgentRequest<'_>) -> Self {
        let mut config = request.config.clone();
        if let Some(workspace) = &request.deps.chat_workspace {
            config.system_prompt.push_str(&workspace.render_prompt());
        }
        Self {
            config,
            spec: resolve_sub_agent_spec(request.config, request.parent_spec),
            workspace: request.deps.workspace.to_string_lossy().into_owned(),
            chat_workspace: request.deps.chat_workspace.clone(),
            cancel_rx: request.cancel_rx.clone(),
            trace: BrowserTrace::new(request),
            status: Mutex::new("running"),
        }
    }

    pub fn model(&self) -> &str {
        &self.spec.model
    }
    pub fn status(&self) -> &'static str {
        *self.status.lock()
    }
    pub fn trace_events_json(&self) -> Result<String, String> {
        self.trace.json()
    }

    pub async fn execute(
        &self,
        task: &str,
        manager: Arc<SubAgentManager>,
    ) -> Result<String, String> {
        self.trace.emit(SubAgentEvent::Started {
            agent_id: self.config.agent_id.clone(),
            agent_name: self.config.agent_name.clone(),
            task: task.into(),
            model: self.spec.model.clone(),
        });
        // 先登记运行中轨迹，再启动外部执行；WebView 重载仍能恢复执行者身份。
        let outcome = self.run(task, manager).await;
        match outcome {
            Ok(value) => match serde_json::from_value::<BrowserResult>(value) {
                Ok(result) if !result.result.trim().is_empty() => self.finish(result),
                Ok(_) => self.fail("浏览器探索未返回结果文本".into()),
                Err(error) => self.fail(format!("浏览器探索结果契约错误：{error}")),
            },
            Err(error) => self.fail(error),
        }
    }

    async fn run(&self, task: &str, manager: Arc<SubAgentManager>) -> Result<Value, String> {
        self.config.validate().map_err(|error| error.to_string())?;
        let app = self
            .trace
            .app
            .clone()
            .ok_or("浏览器探索需要桌面应用运行环境")?;
        self.checkpoint(manager.clone()).await?;
        let browser = app.state::<BrowserManager>();
        let params = self.exploration_params(task);
        let trace = self.trace.clone();
        let run_app = app.clone();
        let session_id = self.trace.session_id.clone();
        let workspace = self.workspace.clone();
        let cancel_rx = self.cancel_rx.clone();
        // 独立推进超时/取消；SQLite 检查点等待不能暂停浏览器监督器。
        // JoinSet 的 Drop 会 abort 子任务，随后 Operation 的 Drop 终止进程组。
        let mut runs = tokio::task::JoinSet::new();
        runs.spawn(async move {
            let browser = run_app.state::<BrowserManager>();
            browser
                .explore(
                    run_app.clone(),
                    session_id,
                    workspace,
                    uuid::Uuid::new_v4().to_string(),
                    trace.tool_call_id.clone(),
                    params,
                    cancel_rx,
                    Arc::new(move |event| trace.receive(event)),
                )
                .await
        });
        let mut checkpoint = tokio::time::interval(std::time::Duration::from_secs(2));
        checkpoint.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        checkpoint.tick().await;
        loop {
            tokio::select! {
                result = runs.join_next() => return result
                    .ok_or("浏览器监督任务丢失")?
                    .map_err(|error| format!("浏览器监督任务异常：{error}"))?,
                _ = checkpoint.tick() => {
                    if let Err(error) = self.checkpoint(manager.clone()).await {
                        // 轨迹写入失败不能继续无记录地操作网页。
                        let cleanup = browser.stop(&self.trace.session_id).await;
                        let _ = runs.join_next().await;
                        return Err(match cleanup {
                            Ok(()) => error,
                            Err(cleanup) => format!("{error}；停止浏览器失败：{cleanup}"),
                        });
                    }
                }
            }
        }
    }

    fn exploration_params(&self, task: &str) -> Value {
        let mut params = json!({
            "task": self.config.user_prompt_template.replace("{{task}}", task),
            "systemPrompt": self.config.system_prompt,
            "maxSteps": self.config.max_iterations,
            "useVision": self.config.browser_use_vision,
            "timeoutSecs": self.config.timeout_secs,
            "allowedFileRoot": self.workspace,
            "model": { "apiBase":self.spec.api_base, "apiKey":self.spec.api_key,
                "model":self.spec.model, "apiType":"openai", "maxTokens":self.spec.max_tokens,
                "temperature":self.spec.temperature },
        });
        if let Some(workspace) = &self.chat_workspace {
            params["allowedFileRoots"] = json!(std::iter::once(&workspace.session_root)
                .chain(workspace.project_roots.iter())
                .collect::<Vec<_>>());
            params["temporaryDirectory"] = json!(workspace
                .session_root
                .join(".jkcodingagent/browser-runs")
                .join(uuid::Uuid::new_v4().to_string()));
        }
        params
    }

    async fn checkpoint(&self, manager: Arc<SubAgentManager>) -> Result<(), String> {
        let trace = self.trace.clone();
        let model = self.spec.model.clone();
        tokio::task::spawn_blocking(move || {
            let events = trace.json()?;
            manager
                .save_run_trace(
                    &trace.session_id,
                    &trace.tool_call_id,
                    &trace.agent_id,
                    "running",
                    &events,
                    Some(&model),
                )
                .map_err(|error| format!("浏览器运行轨迹保存失败：{error}"))?;
            Ok(())
        })
        .await
        .map_err(|error| format!("浏览器轨迹任务失败：{error}"))?
    }

    fn finish(&self, result: BrowserResult) -> Result<String, String> {
        let mut output = bounded(&result.result, 32_000);
        let unfinished = self.trace.settle_actions(if result.success {
            "浏览器已结束，未收到该动作的完成确认"
        } else {
            &output
        });
        if unfinished > 0 {
            let note = format!("另有 {unfinished} 个浏览器动作未收到完成确认，执行结果不完整");
            if result.success {
                return self.fail(format!("{note}。\n\n浏览器原始结果：\n{output}"));
            }
            // 失败结果是诊断权威；动作收尾异常只能补充说明，不能覆盖实际原因。
            output = bounded(&format!("{output}\n\n{note}。"), 32_000);
        }
        *self.status.lock() = if result.success {
            "completed"
        } else {
            "failed"
        };
        if result.success {
            self.trace.emit(SubAgentEvent::Finished {
                agent_id: self.config.agent_id.clone(),
                agent_name: self.config.agent_name.clone(),
                result: output.clone(),
                iterations: result.iterations,
                elapsed_ms: self.trace.elapsed_ms(),
                token_usage: result.token_usage.into(),
            });
        } else {
            self.trace.emit(SubAgentEvent::UsageUpdated {
                agent_id: self.config.agent_id.clone(),
                agent_name: self.config.agent_name.clone(),
                token_usage: result.token_usage.into(),
                elapsed_ms: self.trace.elapsed_ms(),
            });
            self.trace.emit(SubAgentEvent::Failed {
                agent_id: self.config.agent_id.clone(),
                agent_name: self.config.agent_name.clone(),
                error: output.clone(),
                termination_reason: None,
            });
        }
        // 未完成/步数耗尽是任务结果，让主 Agent 决策；不按 fatal 终止整个父会话。
        Ok(json!({"success":result.success,"result":output,"iterations":result.iterations,
            "urls":result.urls.into_iter().take(100).map(|url| bounded(&url, 2_000)).collect::<Vec<_>>()}).to_string())
    }

    fn fail(&self, error: String) -> Result<String, String> {
        let status = failure_status(&error);
        *self.status.lock() = status;
        self.trace.settle_actions(&error);
        self.trace.emit(SubAgentEvent::Failed {
            agent_id: self.config.agent_id.clone(),
            agent_name: self.config.agent_name.clone(),
            error: bounded(&error, 4_000),
            termination_reason: matches!(status, "cancelled" | "timed_out").then(|| status.into()),
        });
        Err(error)
    }
}

pub(super) fn failure_status(error: &str) -> &'static str {
    if error.starts_with("[cancelled]") {
        "cancelled"
    } else if error.starts_with("[timeout]") {
        "timed_out"
    } else {
        "failed"
    }
}

#[cfg(test)]
mod tests;
