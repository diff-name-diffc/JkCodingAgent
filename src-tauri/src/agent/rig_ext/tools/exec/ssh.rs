//! SSH 工具组：`ssh_list_servers` / `ssh_exec`。
//! 移植自旧 `tools/builtin/ssh.rs`；连接池复用 `crate::ssh_tool::SshSessionManager`。

use serde_json::{json, Value};

use super::super::common::{
    string_arg, u64_arg, with_compression_parameters, COMMAND_FORCE_COMPRESS_AFTER_CHARS,
};
use crate::agent::command_history::{self, CommandHistoryStatus};
use crate::agent::db::DispatcherDb;
use crate::ssh_tool::SshSessionManager;
use rig::tool::{PortableDynamicTool, ToolOutput};
use std::path::PathBuf;

pub(super) fn ssh_tools(
    manager: SshSessionManager,
    workspace: PathBuf,
    workspace_id: String,
    db: DispatcherDb,
) -> Vec<PortableDynamicTool> {
    vec![
        ssh_list_servers_tool(manager.clone()),
        ssh_exec_tool(manager, workspace, workspace_id, db),
    ]
}

fn ssh_list_servers_tool(manager: SshSessionManager) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ssh_list_servers",
        "列出已启用的 SSH 服务器（全局配置，所有项目共享）。只返回 server_id、名称、描述和标签，不暴露 IP、端口、账号或密码。",
        json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        move |_args| {
            let manager = manager.clone();
            Box::pin(async move {
                let text = match manager.list_servers_async().await {
                    Ok(servers) => {
                        if servers.is_empty() {
                            "没有已启用的 SSH server。请先在 Aha 智能体设置中配置 SSH 工具。"
                                .to_string()
                        } else {
                            match serde_json::to_string_pretty(&json!({ "servers": servers })) {
                                Ok(text) => text,
                                Err(error) => {
                                    format!("错误：序列化 SSH server 列表失败：{error}")
                                }
                            }
                        }
                    }
                    Err(error) => format!("错误：读取 SSH server 列表失败：{error}"),
                };
                Ok(ToolOutput::text(text))
            })
        },
    )
}

/// SSH 命令执行工具。
///
/// 并发语义：同一 `server_id + session_id` 的并发调用复用同一条 SSH 连接，
/// 并发命令各走独立 channel（协议级隔离，输出不交错）；跨 session_id 的调用互不阻塞。
fn ssh_exec_tool(
    manager: SshSessionManager,
    workspace: PathBuf,
    workspace_id: String,
    db: DispatcherDb,
) -> PortableDynamicTool {
    let parameters = with_compression_parameters(
        json!({
            "type": "object",
            "properties": {
                "server_id": {
                    "type": "string",
                    "description": "ssh_list_servers 返回的服务器 id"
                },
                "session_id": {
                    "type": "string",
                    "description": "会话 id。相同 server_id + session_id 会复用 SSH 连接；建议使用当前任务或排障主题的稳定短 id"
                },
                "command": {
                    "type": "string",
                    "description": "要在远程服务器执行的非交互式 shell 命令。禁止依赖交互输入（密码提示、y/n 确认、分页器、REPL）；这类命令会被检测到并中止。"
                },
                "stdin": {
                    "type": "string",
                    "description": "可选。命令启动后写入其标准输入的内容，随后关闭输入端，上限 32000 字符（与安全审查送审上限一致，超出会被拒绝执行）。用于 here-doc / 管道喂入场景（如向读取 stdin 的程序提供内容）。stdin 内容会随命令一起提交安全审查，禁止用 stdin 携带未审查的破坏性内容。不可用于回应交互式密码/确认提示——这类场景请改写为非交互命令。"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "本次命令超时时间，单位秒，默认使用服务器配置",
                    "minimum": 1,
                    "maximum": 300
                }
            },
            "required": ["server_id", "session_id", "command"]
        }),
        false,
        COMMAND_FORCE_COMPRESS_AFTER_CHARS,
        "默认直接返回原始输出：8000 字符内全量内联，超出截断且完整原文保留在工具产物中。报错原文、状态值、配置片段等需要逐字核对的内容应保持 compress=false。仅当预期输出很大（全量日志、大目录清单等）且只需确认特定信息时才设 compress=true，并在 compress_intent 中写明要确认什么。",
    );
    PortableDynamicTool::new(
        "ssh_exec",
        "在指定 SSH server 上执行单个非交互式命令，返回 stdout/stderr/退出码。复用 session_id 对应的 SSH 连接，但每次调用是一次独立命令（不保留 cd、环境变量等 shell 状态）。\n\
重要约束：命令必须是非交互式的——不得弹出密码、y/n 确认、分页器或进入 REPL。若命令疑似在等待输入，工具会主动中止并报错：输出末尾命中密码/确认等提示符时最快 8 秒中止；完全静默的命令（如 sleep、慢查询、写文件的备份）按 timeout_secs 放宽静默容忍（最长 60 秒）。改用非交互等价形式：sudo 用免密账号或 NOPASSWD；包管理/删除加 -y/--yes；分页器设 PAGER=cat、GIT_PAGER=cat；mysql/psql 用 -e/-c；需要向命令喂内容时用 stdin 参数（here-doc），不要指望终端回应交互提示。",
        parameters,
        move |args| {
            let manager = manager.clone();
            let workspace = workspace.clone();
            let workspace_id = workspace_id.clone();
            let db = db.clone();
            Box::pin(async move {
                let text = ssh_exec_text(&args, manager, workspace, workspace_id, db).await;
                Ok(ToolOutput::text(text))
            })
        },
    )
}

async fn ssh_exec_text(
    args: &Value,
    manager: SshSessionManager,
    workspace: PathBuf,
    workspace_id: String,
    db: DispatcherDb,
) -> String {
    let Some(server_id) = string_arg(args, "server_id") else {
        return "错误：缺少必填参数 server_id；请先调用 ssh_list_servers。".to_string();
    };
    let Some(session_id) = string_arg(args, "session_id") else {
        return "错误：缺少必填参数 session_id。".to_string();
    };
    let Some(command) = string_arg(args, "command") else {
        return "错误：缺少必填参数 command。".to_string();
    };
    let stdin = string_arg(args, "stdin");

    // TODO(T3)：审查门禁移至 runtime ToolExecutionPolicy——旧实现在此对
    // 命令+stdin 做 fail-closed 安全审查（未配置审查即拦截、服务器可显式豁免、
    // 拦截写审计与命令台账）；迁移后工具层不再审查，审计记录的 review 字段暂为 None。

    // 审计元数据需要会话标题：旧实现由 ToolContext 注入，此处从会话库按
    // workspace_id 现查（查不到回退空串，不阻断命令执行）。
    let session_title = db
        .get_session_title_async(&workspace_id)
        .await
        .unwrap_or_default();

    // 台账登记需要命令与目标标识，而下方 execute 会按值消费它们，先克隆留存。
    let history_server_id = server_id.clone();
    let history_command = command.clone();
    match manager
        .execute(
            workspace,
            workspace_id.clone(),
            session_title,
            server_id,
            session_id,
            command,
            stdin,
            u64_arg(args, "timeout_secs"),
            None,
        )
        .await
    {
        Ok(result) => {
            let rendered = serde_json::to_string_pretty(&result)
                .unwrap_or_else(|error| format!("错误：序列化 SSH 执行结果失败：{error}"));
            command_history::record(
                &workspace_id,
                "ssh_exec",
                &history_server_id,
                &history_command,
                CommandHistoryStatus::Executed,
                &rendered,
            );
            rendered
        }
        Err(error) => format!("错误：SSH 命令执行失败：{error}"),
    }
}
