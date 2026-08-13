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

/// SSH 命令执行工具。
///
/// 并发语义：同一 `server_id + session_id` 的并发调用复用同一条 SSH 连接，
/// 连接级锁保证命令整段串行执行（输出不交错、状态不串扰）；后到的调用会排队
/// 等待，最长占用一个命令超时周期（≤300s）。跨 session_id 的调用互不阻塞。
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
                        "description": "可选。命令启动后写入其标准输入的字节，随后关闭输入端。用于 here-doc / 管道喂入场景（如向读取 stdin 的程序提供内容）。stdin 内容会随命令一起提交安全审查，禁止用 stdin 携带未审查的破坏性内容。不可用于回应交互式密码/确认提示——这类场景请改写为非交互命令。"
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

        // 安全审查门禁：fail-closed。
        // - 未配置审查模型：默认拦截可执行命令，不得跳过审查放行。
        // - 已配置且服务器开启审查：执行前评估命令（含 stdin）安全性。
        // - 审查异常或判定不通过：拦截并写入审计，不执行命令。
        // - 服务器显式关闭「执行前审查」开关：按配置放行（设计内的豁免通道）。
        let review_outcome: Option<crate::ssh_tool::SshAuditReview> =
            match context.ssh_review.as_ref() {
                None => {
                    let blocked = crate::ssh_tool::SshAuditReview {
                        allowed: false,
                        reason: "未配置安全审查，无法评估命令安全性".to_string(),
                    };
                    if let Ok(record) = self
                        .manager
                        .record_review_blocked(
                            context.workspace.clone(),
                            context.workspace_id.clone(),
                            context.session_title.clone(),
                            server_id.clone(),
                            session_id.clone(),
                            command.clone(),
                            blocked,
                        )
                        .await
                    {
                        return format!(
                            "错误：未配置安全审查，已拒绝执行命令。请先在应用设置中配置安全审查模型。\n\n{}",
                            crate::ssh_tool::render_ssh_audit_record_markdown(&record)
                        );
                    }
                    return "错误：未配置安全审查，已拒绝执行命令。请先在应用设置中配置安全审查模型。"
                        .to_string();
                }
                Some(review_config) => {
                    match self
                        .manager
                        .server_config_async(context.workspace.clone(), server_id.clone())
                        .await
                    {
                        Ok(server) if server.review_enabled => {
                            let intent = string_arg(args, "compress_intent")
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(|| context.session_title.clone());
                            let payload = crate::agent::ssh_review::SshReviewPayload {
                                intent,
                                task: context.user_task.clone().unwrap_or_default(),
                                server_info: crate::agent::ssh_review::SshReviewServerInfo {
                                    id: server.id.clone(),
                                    description: server.description.clone(),
                                    host: server.host.clone(),
                                    port: server.port,
                                    username: server.username.clone(),
                                    tags: server.tags.clone(),
                                },
                                command: command.clone(),
                                stdin: stdin.clone(),
                            };
                            match crate::agent::ssh_review::review_command(
                                review_config, &payload,
                            )
                            .await
                            {
                                Ok(verdict) => Some(crate::ssh_tool::SshAuditReview {
                                    allowed: verdict.allowed,
                                    reason: verdict.reason,
                                }),
                                Err(error) => {
                                    let blocked = crate::ssh_tool::SshAuditReview {
                                        allowed: false,
                                        reason: format!("审查服务异常：{error}"),
                                    };
                                    let record_result = self
                                        .manager
                                        .record_review_blocked(
                                            context.workspace.clone(),
                                            context.workspace_id.clone(),
                                            context.session_title.clone(),
                                            server_id.clone(),
                                            session_id.clone(),
                                            command.clone(),
                                            blocked,
                                        )
                                        .await;
                                    if let Ok(record) = record_result {
                                        return format!(
                                            "错误：命令已被安全审查拦截（审查服务异常：{error}）。\n\n{}",
                                            crate::ssh_tool::render_ssh_audit_record_markdown(
                                                &record
                                            )
                                        );
                                    }
                                    return format!(
                                        "错误：命令已被安全审查拦截（审查服务异常：{error}）。如需放行，可在 SSH 工具配置中关闭该服务器的「执行前审查」开关。"
                                    );
                                }
                            }
                        }
                        Ok(_) => None,
                        Err(error) => return format!("错误：{error}"),
                    }
                }
            };

        // 判定为不通过：写入「被拦截」审计记录并阻断。
        if let Some(ref review) = review_outcome {
            if !review.allowed {
                let reason = review.reason.clone();
                let record_result = self
                    .manager
                    .record_review_blocked(
                        context.workspace.clone(),
                        context.workspace_id.clone(),
                        context.session_title.clone(),
                        server_id.clone(),
                        session_id.clone(),
                        command.clone(),
                        review.clone(),
                    )
                    .await;
                if let Ok(record) = record_result {
                    return crate::ssh_tool::render_ssh_audit_record_markdown(&record);
                }
                return format!(
                    "错误：命令已被安全审查拦截：{reason}。如需放行，可在 SSH 工具配置中关闭该服务器的「执行前审查」开关。"
                );
            }
        }

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
                review_outcome,
            )
            .await
        {
            Ok(result) => serde_json::to_string_pretty(&result)
                .unwrap_or_else(|error| format!("错误：序列化 SSH 执行结果失败：{error}")),
            Err(error) => format!("错误：SSH 命令执行失败：{error}"),
        }
    }
}
