//! 显式 tmux 安装入口：固定命令委托 ssh_exec，共用审查、sudo、取消和审计。

use rig::tool::{PortableDynamicTool, ToolExecutionError};
use serde::Deserialize;
use serde_json::{json, Value};

const INSTALL_TMUX_COMMAND: &str = r#"sh -c '
set -eu
exec </dev/null
if command -v tmux >/dev/null 2>&1; then
    tmux -V
    exit 0
fi
if command -v apt-get >/dev/null 2>&1; then
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends tmux
elif command -v dnf >/dev/null 2>&1; then
    dnf -y install tmux
else
    printf "%s\n" "错误：未找到支持的包管理器 apt-get 或 dnf，未执行安装；请自行安装 tmux 后使用 ssh_term_open(tmux=required)。" >&2
    exit 1
fi
tmux -V
'"#;

pub(super) fn ssh_tmux_install_tool(ssh_exec: PortableDynamicTool) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ssh_tmux_install",
        "显式安装远端 tmux，为断连保活、现场恢复和人类共屏准备环境。只有调用本工具才安装，ssh_term_open 不会隐式安装。已安装时仅返回 tmux 版本；否则只支持 apt-get/dnf 非交互安装 tmux，不执行 update/upgrade，不卸载软件。安装命令复用 ssh_exec 的安全审查、取消与审计；需要提权时设置 sudo=true（默认 false），使用已配置凭据，不接受密码。失败返回原始错误，不降级到裸 PTY。成功后再调用 ssh_term_open，设置 tmux=required。timeout_secs 默认 300，范围 1..300。",
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "server_id": { "type": "string", "minLength": 1, "description": "ssh_list_servers 返回的服务器 id" },
                "session_id": { "type": "string", "minLength": 1, "description": "当前任务的稳定会话 id；与 ssh_exec 共用 SSH 连接" },
                "sudo": { "type": "boolean", "default": false, "description": "需 root 权限时显式提权；沿用 ssh_exec 的凭据与审查路径" },
                "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 300, "default": 300, "description": "安装命令超时秒数" }
            },
            "required": ["server_id", "session_id"]
        }),
        move |args| {
            let ssh_exec = ssh_exec.clone();
            Box::pin(async move {
                let args = execution_args(args)?;
                // 不另起 task：保留当前 ToolInvocationContext，ssh_exec 读取同一取消源。
                ssh_exec.execute(args).await
            })
        },
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallArgs {
    server_id: String,
    session_id: String,
    #[serde(default)]
    sudo: bool,
    #[serde(default = "default_timeout_secs")]
    timeout_secs: u64,
}

fn default_timeout_secs() -> u64 {
    300
}

fn execution_args(args: Value) -> Result<Value, ToolExecutionError> {
    let args: InstallArgs = serde_json::from_value(args).map_err(|error| {
        ToolExecutionError::invalid_args(format!("错误：ssh_tmux_install 参数无效：{error}"))
    })?;
    if args.server_id.trim().is_empty() || args.session_id.trim().is_empty() {
        return Err(ToolExecutionError::invalid_args(
            "错误：server_id 和 session_id 不能为空",
        ));
    }
    if !(1..=300).contains(&args.timeout_secs) {
        return Err(ToolExecutionError::invalid_args(
            "错误：timeout_secs 必须为 1..300 的整数",
        ));
    }
    Ok(json!({
        "server_id": args.server_id.trim(),
        "session_id": args.session_id.trim(),
        "command": INSTALL_TMUX_COMMAND,
        "sudo": args.sudo,
        "timeout_secs": args.timeout_secs,
    }))
}

#[cfg(test)]
#[path = "install_tmux_tests.rs"]
mod tests;
