use rig::tool::{PortableDynamicTool, ToolOutput};
use serde_json::json;

use crate::ssh_tool::SshSessionManager;

pub(super) fn ssh_list_servers_tool(manager: SshSessionManager) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ssh_list_servers",
        "列出已启用的 SSH 服务器（全局配置，所有项目共享）。返回每台的 server_id、名称、主机地址（IP/域名）、描述与标签；当用户用 IP 或域名指代某台服务器时，可据此定位其 server_id。不暴露端口、账号或密码。",
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
