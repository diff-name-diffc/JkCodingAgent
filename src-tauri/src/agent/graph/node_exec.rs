//! 图节点执行器。
//!
//! - subAgent 节点：复用 `SubAgentRuntime`（合成 `graphnode:{plan_id}:{node_id}`
//!   作为 tool_call_id，轨迹同时落 `sub_agent_run_traces` 供节点详情回放）。
//! - claude/codex 节点：`tokio::process::Command` 无头一次性执行（不走旧交互式
//!   PTY 协议），stdout/stderr 分行流式推送 `nodeOutputDelta`，整体超时 30 分钟，
//!   工作目录 = 项目根，命中取消标志时 kill 子进程。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tauri::AppHandle;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

use super::runner::emit_run_event;
use super::types::{GraphNode, GraphNodeAgent, GraphRunEvent};
use crate::agent::llm::OpenAiCompatProvider;
use crate::agent::sub_agent::runtime::SubAgentRuntime;
use crate::agent::sub_agent::SubAgentManager;
use crate::agent::tools::{ToolContext, ToolRegistry};
use crate::platform::{get_agent_bin_checked, get_login_shell_env};

/// CLI 节点整体超时（30 分钟，v1 常量，后续可进设置）。
const CLI_NODE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// claude 无头执行的权限模式（v1 固定，后续可进设置）。
const CLAUDE_PERMISSION_MODE: &str = "acceptEdits";
/// stdout 累积上限（超出后保留头部，防止超长输出撑爆内存）。
const STDOUT_ACCUM_MAX_CHARS: usize = 1_000_000;
/// stderr 尾部保留长度（仅用于失败诊断）。
const STDERR_TAIL_MAX_CHARS: usize = 8_000;

#[derive(Debug)]
pub(crate) enum NodeExecOutcome {
    Succeeded(String),
    Failed(String),
    Cancelled,
}

pub(crate) struct NodeExecContext {
    pub app: AppHandle,
    pub plan_id: String,
    pub workspace_id: String,
    pub workspace_root: PathBuf,
    pub session_title: String,
    pub user_requirement: String,
    pub node: GraphNode,
    /// 装配后的完整节点输入（总体需求 + 角色 + 子任务 + 上游输出 + state 节选）。
    pub input: String,
    /// subAgent 节点的父级 provider（子智能体默认继承其模型配置）。
    pub parent_provider: OpenAiCompatProvider,
    /// subAgent 节点可用的完整工具注册表（default_tools，含 MCP 动态工具）。
    pub tool_registry: Arc<ToolRegistry>,
    pub sub_agent_manager: Option<Arc<SubAgentManager>>,
    pub cancel_rx: watch::Receiver<bool>,
}

impl NodeExecContext {
    fn emit_delta(&self, delta: &str) {
        emit_run_event(
            &self.app,
            &self.plan_id,
            &self.workspace_id,
            GraphRunEvent::NodeOutputDelta {
                node_id: self.node.id.clone(),
                delta: delta.to_string(),
            },
        );
    }

    fn cancellation_requested(&self) -> bool {
        *self.cancel_rx.borrow()
    }
}

pub(crate) async fn execute_node(ctx: &NodeExecContext) -> NodeExecOutcome {
    match &ctx.node.agent {
        GraphNodeAgent::SubAgent { agent_id } => {
            execute_sub_agent_node(ctx, agent_id.clone()).await
        }
        GraphNodeAgent::Claude => execute_cli_node(ctx, "claude").await,
        GraphNodeAgent::Codex => execute_cli_node(ctx, "codex").await,
    }
}

/// subAgent 节点的轨迹关联键：`sub-agent-event` 载荷的 toolCallId 即此值，
/// 前端节点详情直接复用 subAgentEventStore 渲染执行轨迹。
pub(crate) fn graph_node_tool_call_id(plan_id: &str, node_id: &str) -> String {
    format!("graphnode:{plan_id}:{node_id}")
}

// ─── subAgent 节点 ────────────────────────────────────────────────────────────

async fn execute_sub_agent_node(ctx: &NodeExecContext, agent_id: String) -> NodeExecOutcome {
    let Some(manager) = &ctx.sub_agent_manager else {
        return NodeExecOutcome::Failed("子智能体管理器未初始化".to_string());
    };
    let Some(config) = manager.get(&agent_id) else {
        return NodeExecOutcome::Failed(format!("未找到子智能体 '{agent_id}'"));
    };
    if !config.enabled {
        return NodeExecOutcome::Failed(format!("子智能体 '{agent_id}' 已被禁用"));
    }

    let tool_call_id = graph_node_tool_call_id(&ctx.plan_id, &ctx.node.id);
    let tool_context = ToolContext {
        workspace_id: ctx.workspace_id.clone(),
        workspace: ctx.workspace_root.clone(),
        session_title: ctx.session_title.clone(),
        user_task: Some(ctx.user_requirement.clone()),
        ssh_review: None,
        exec_timeout_secs: 60,
        restrict_to_workspace: true,
        extra_allowed_dirs: dirs::home_dir()
            .map(|home| vec![home.join(".jkcodingagent")])
            .unwrap_or_default(),
        app_handle: Some(ctx.app.clone()),
        llm_provider: Some(ctx.parent_provider.clone()),
        vision_model: String::new(),
        image_model_url: String::new(),
        image_model_api_key: String::new(),
        image_model: String::new(),
        image_edit_model: String::new(),
        sub_agent_tool_registry: Some(Arc::clone(&ctx.tool_registry)),
        current_sub_agent_id: None,
        current_sub_agent_name: None,
        current_tool_call_id: Some(tool_call_id.clone()),
        sub_agent_parent_tool_call_id: None,
        sub_agent_trace_events: None,
    };

    let runtime = match SubAgentRuntime::build(
        &config,
        &ctx.parent_provider,
        Arc::clone(&ctx.tool_registry),
        tool_context,
    ) {
        Ok(runtime) => runtime,
        Err(error) => return NodeExecOutcome::Failed(format!("子智能体初始化失败：{error}")),
    };

    let outcome = runtime
        .execute_with_cancellation(
            &ctx.input,
            Some(ctx.app.clone()),
            &ctx.workspace_id,
            Some(ctx.cancel_rx.clone()),
        )
        .await;

    // trace 落库（主键 (workspace_id, tool_call_id)），供节点详情历史回放。
    let trace_json = runtime
        .trace_events_json()
        .unwrap_or_else(|_| "[]".to_string());
    let trace_status = if outcome.is_ok() { "completed" } else { "failed" };
    let persist_manager = Arc::clone(manager);
    let persist_workspace_id = ctx.workspace_id.clone();
    let persist_agent_id = config.agent_id.clone();
    let persisted = tokio::task::spawn_blocking(move || {
        persist_manager.save_run_trace(
            &persist_workspace_id,
            &tool_call_id,
            &persist_agent_id,
            trace_status,
            &trace_json,
        )
    })
    .await;
    match persisted {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => eprintln!("[graph] 子智能体轨迹持久化失败：{error}"),
        Err(error) => eprintln!("[graph] 子智能体轨迹任务失败：{error}"),
    }

    match outcome {
        Ok(output) => NodeExecOutcome::Succeeded(output),
        Err(error) => {
            if ctx.cancellation_requested() {
                NodeExecOutcome::Cancelled
            } else {
                NodeExecOutcome::Failed(format!("子智能体执行失败：{error}"))
            }
        }
    }
}

// ─── claude / codex 节点（无头一次性执行） ────────────────────────────────────

async fn execute_cli_node(ctx: &NodeExecContext, agent: &str) -> NodeExecOutcome {
    let bin = match get_agent_bin_checked(agent) {
        Ok(bin) => bin,
        Err(error) => {
            return NodeExecOutcome::Failed(format!(
                "读取 {agent} 可执行文件路径失败：{error}"
            ))
        }
    };

    // 项目配置的 agent.prompt_prefix 前置到节点输入（与旧 dispatch 行为一致）。
    let prompt = match load_project_prompt_prefix(&ctx.workspace_root).await {
        Some(prefix) => format!("{}\n\n{}", prefix.trim(), ctx.input),
        None => ctx.input.clone(),
    };

    let mut command = Command::new(&bin);
    if agent == "claude" {
        command
            .arg("-p")
            .arg(&prompt)
            .arg("--output-format")
            .arg("text")
            .arg("--permission-mode")
            .arg(CLAUDE_PERMISSION_MODE);
    } else {
        command.arg("exec").arg("--full-auto").arg(&prompt);
    }
    command
        .current_dir(&ctx.workspace_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in get_login_shell_env() {
        command.env(key, value);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return NodeExecOutcome::Failed(format!("启动 {agent} 进程失败：{error}")),
    };

    // stdout/stderr 分行读取，统一经 channel 汇入主循环后流式推送。
    let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<(bool, String)>();
    if let Some(stdout) = child.stdout.take() {
        let tx = line_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx.send((false, line)).is_err() {
                    break;
                }
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = line_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx.send((true, line)).is_err() {
                    break;
                }
            }
        });
    }
    drop(line_tx);

    let mut stdout_text = String::new();
    let mut stderr_tail = String::new();
    let mut cancel_rx = ctx.cancel_rx.clone();
    let deadline = tokio::time::Instant::now() + CLI_NODE_TIMEOUT;
    let mut exit_status: Option<std::process::ExitStatus> = None;
    let mut channels_open = true;

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline), if exit_status.is_none() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return NodeExecOutcome::Failed(format!(
                    "{agent} 节点执行超过 30 分钟，已终止"
                ));
            }
            _ = cancel_rx.changed(), if exit_status.is_none() => {
                if *cancel_rx.borrow() {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    return NodeExecOutcome::Cancelled;
                }
            }
            result = child.wait(), if exit_status.is_none() => {
                match result {
                    Ok(status) => exit_status = Some(status),
                    Err(error) => {
                        return NodeExecOutcome::Failed(format!(
                            "读取 {agent} 进程退出状态失败：{error}"
                        ))
                    }
                }
                if !channels_open {
                    break;
                }
            }
            line = line_rx.recv(), if channels_open => {
                match line {
                    Some((is_stderr, text)) => {
                        ctx.emit_delta(&format!("{text}\n"));
                        if is_stderr {
                            push_tail(&mut stderr_tail, &text, STDERR_TAIL_MAX_CHARS);
                        } else {
                            push_capped(&mut stdout_text, &text, STDOUT_ACCUM_MAX_CHARS);
                        }
                    }
                    None => {
                        channels_open = false;
                        if exit_status.is_some() {
                            break;
                        }
                    }
                }
            }
        }
    }

    let Some(status) = exit_status else {
        return NodeExecOutcome::Failed(format!("{agent} 进程状态异常"));
    };
    if !status.success() {
        let code = status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "未知".to_string());
        let detail = if stderr_tail.trim().is_empty() {
            tail_chars(&stdout_text, 2_000)
        } else {
            stderr_tail.trim().to_string()
        };
        return NodeExecOutcome::Failed(format!(
            "{agent} 进程以非 0 状态退出（退出码 {code}）：{detail}"
        ));
    }

    // 节点输出以 stdout 为准；stdout 为空时回退 stderr 尾部（部分 CLI 把结论打到 stderr）。
    let output = if stdout_text.trim().is_empty() {
        stderr_tail.trim().to_string()
    } else {
        stdout_text
    };
    NodeExecOutcome::Succeeded(output)
}

/// 读取项目配置的 agent.prompt_prefix（阻塞 I/O 走 spawn_blocking）。
async fn load_project_prompt_prefix(workspace_root: &Path) -> Option<String> {
    let path = workspace_root.to_string_lossy().into_owned();
    let prefix = tokio::task::spawn_blocking(move || {
        crate::project::read_project_config(path)
            .map(|config| config.agent.prompt_prefix)
            .unwrap_or_default()
    })
    .await
    .ok()?;
    let trimmed = prefix.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn push_capped(accumulator: &mut String, line: &str, max_chars: usize) {
    accumulator.push_str(line);
    accumulator.push('\n');
    let char_count = accumulator.chars().count();
    if char_count > max_chars {
        let keep = max_chars / 2;
        *accumulator = accumulator.chars().take(keep).collect();
        accumulator.push_str("\n...[中间输出已截断]...\n");
    }
}

fn push_tail(accumulator: &mut String, line: &str, max_chars: usize) {
    accumulator.push_str(line);
    accumulator.push('\n');
    let char_count = accumulator.chars().count();
    if char_count > max_chars {
        *accumulator = accumulator.chars().skip(char_count - max_chars).collect();
    }
}

fn tail_chars(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        text.trim().to_string()
    } else {
        text.chars().skip(char_count - max_chars).collect()
    }
}
