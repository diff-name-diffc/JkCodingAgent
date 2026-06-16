use async_trait::async_trait;
use serde_json::{json, Value};

use super::common::{string_arg, u64_arg, with_compression_parameters};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;
use crate::ssh_tool::SshSessionManager;

pub(super) fn ssh_tools(manager: SshSessionManager) -> Vec<Box<dyn AgentTool>> {
    vec![
        Box::new(SshListServersTool {
            manager: manager.clone(),
        }),
        Box::new(SshExecTool { manager }),
    ]
}

struct SshListServersTool {
    manager: SshSessionManager,
}

struct SshExecTool {
    manager: SshSessionManager,
}

#[async_trait]
impl AgentTool for SshListServersTool {
    fn name(&self) -> &'static str {
        "ssh_list_servers"
    }

    fn description(&self) -> &'static str {
        "列出当前项目已启用的 SSH 服务器。只返回 server_id、描述和标签，不暴露 IP、端口、账号或密码。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _args: &Value, context: &ToolContext) -> String {
        match self
            .manager
            .list_servers_async(context.workspace.clone())
            .await
        {
            Ok(servers) => {
                if servers.is_empty() {
                    return "当前项目没有已启用的 SSH server。请先在 Aha 智能体设置中配置 SSH 工具。".to_string();
                }
                serde_json::to_string_pretty(&json!({ "servers": servers }))
                    .unwrap_or_else(|error| format!("错误：序列化 SSH server 列表失败：{error}"))
            }
            Err(error) => format!("错误：读取 SSH server 列表失败：{error}"),
        }
    }
}

#[async_trait]
impl AgentTool for SshExecTool {
    fn name(&self) -> &'static str {
        "ssh_exec"
    }

    fn description(&self) -> &'static str {
        "在指定 SSH server 上执行单个非交互式命令，返回 stdout/stderr/退出码。复用 session_id 对应的 SSH 连接，但每次调用是一次独立命令（不保留 cd、环境变量等 shell 状态）。\n\
重要约束：命令必须是非交互式的——不得弹出密码、y/n 确认、分页器或进入 REPL。若命令疑似在等待输入（连续静默且未退出），工具会主动中止并报错。改用非交互等价形式：sudo 用免密账号或 NOPASSWD；包管理/删除加 -y/--yes；分页器设 PAGER=cat、GIT_PAGER=cat；mysql/psql 用 -e/-c；需要向命令喂内容时用 stdin 参数（here-doc），不要指望终端回应交互提示。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
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
                        "description": "可选。命令启动后写入其标准输入的字节，随后关闭输入端。用于 here-doc / 管道喂入场景（如向读取 stdin 的程序提供内容）。不可用于回应交互式密码/确认提示——这类场景请改写为非交互命令。"
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
            true,
            "SSH 命令结果可能较长，默认开启压缩。compress_intent 应说明要从命令输出中确认什么。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
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

        match self
            .manager
            .execute(
                context.workspace.clone(),
                context.workspace_id.clone(),
                context.session_title.clone(),
                server_id,
                session_id,
                command,
                stdin,
                u64_arg(args, "timeout_secs"),
            )
            .await
        {
            Ok(result) => serde_json::to_string_pretty(&result)
                .unwrap_or_else(|error| format!("错误：序列化 SSH 执行结果失败：{error}")),
            Err(error) => format!("错误：SSH 命令执行失败：{error}"),
        }
    }
}
