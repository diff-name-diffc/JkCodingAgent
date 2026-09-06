//! SSH 服务器运维备忘录工具：`ssh_memo_read` / `ssh_memo_upsert` / `ssh_memo_delete`。
//!
//! 每台服务器一份备忘录文件（`ssh_tool::memo` 管理），记录对该服务器运维
//! 长期有价值的必要信息。写入语义是**段落级整体重写**（upsert 原位替换整段、
//! delete 删整段），不提供简单追加——配合提示词纪律（先读后写、合并去重、
//! 克制记录）与硬性字符上限（超限拒绝写盘），防止备忘录无限增长。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::common::{string_arg, with_compression_parameters};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;
use crate::agent::tools::ToolResult;
use crate::ssh_tool::memo;
use crate::ssh_tool::SshSessionManager;

pub(super) fn ssh_memo_tools(manager: SshSessionManager) -> Vec<Box<dyn AgentTool>> {
    vec![
        Box::new(SshMemoReadTool {
            manager: manager.clone(),
        }),
        Box::new(SshMemoUpsertTool {
            manager: manager.clone(),
        }),
        Box::new(SshMemoDeleteTool { manager }),
    ]
}

/// 校验 `server_id` 参数与服务器存在性，返回 (server_id, 展示名)。
/// 备忘录不要求服务器处于启用状态：禁用中的服务器积累的环境知识仍可读写。
async fn resolve_server(manager: &SshSessionManager, args: &Value) -> Result<(String, String), String> {
    let Some(server_id) = string_arg(args, "server_id") else {
        return Err("错误：缺少必填参数 server_id；请先调用 ssh_list_servers。".to_string());
    };
    match manager.find_server_any_async(&server_id).await {
        Ok(Some(server)) => {
            let display_name = if server.name.trim().is_empty() {
                server.id.clone()
            } else {
                server.name.clone()
            };
            Ok((server_id, display_name))
        }
        Ok(None) => Err(format!(
            "错误：未找到 SSH server：{server_id}；请先调用 ssh_list_servers 确认。"
        )),
        Err(error) => Err(format!("错误：读取 SSH 服务器信息失败：{error}")),
    }
}

struct SshMemoReadTool {
    manager: SshSessionManager,
}

struct SshMemoUpsertTool {
    manager: SshSessionManager,
}

struct SshMemoDeleteTool {
    manager: SshSessionManager,
}

#[async_trait]
impl AgentTool for SshMemoReadTool {
    fn name(&self) -> &'static str {
        "ssh_memo_read"
    }

    fn description(&self) -> &'static str {
        "读取指定 SSH 服务器的运维备忘录：部署/服务路径、特殊命令与操作方式、端口与依赖、已知问题与解法等长期有效信息。对不熟悉的服务器执行运维操作前建议先调用，避免重复试错；返回全部段落与字符用量（上限 8000 字符）。尚无备忘录时返回使用指引。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "server_id": {
                        "type": "string",
                        "description": "ssh_list_servers 返回的服务器 id"
                    }
                },
                "required": ["server_id"]
            }),
            false,
            "备忘录有 8000 字符硬上限，输出通常不长，默认关闭压缩。",
        )
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> ToolResult {
        ToolResult::from_text(self.execute_text(args).await)
    }
}

impl SshMemoReadTool {
    async fn execute_text(&self, args: &Value) -> String {
        let (server_id, display_name) = match resolve_server(&self.manager, args).await {
            Ok(value) => value,
            Err(error) => return error,
        };
        // 下方 spawn_blocking 会按值消费 server_id，成功消息还需要它，先克隆留存。
        let message_server_id = server_id.clone();
        let payload = match tokio::task::spawn_blocking(move || memo::read_memo(&server_id))
            .await
            .map_err(|error| error.to_string())
        {
            Ok(Ok(payload)) => payload,
            Ok(Err(error)) => return format!("错误：{error}"),
            Err(error) => return format!("错误：读取备忘录任务失败：{error}"),
        };
        if payload.content.trim().is_empty() {
            return format!(
                "# 运维备忘录 · {display_name}（{message_server_id}）\n用量：0/{} 字符\n\n\
尚无运维备忘录。运维中发现长期有价值的信息时，用 ssh_memo_upsert 按段落写入，建议的段落：\n\n\
- **部署路径**：应用/服务的代码、配置、日志所在目录。\n\
- **特殊命令与操作方式**：非通用的启动/停止/发布/备份命令与操作方式。\n\
- **已知问题与解法**：遇到过的故障、原因与处置方法。\n\n\
克制记录：只写必要信息，禁止写入密码/密钥等凭据与临时调试输出。",
                memo::MEMO_MAX_CHARS
            );
        }
        let total_chars = payload.content.chars().count();
        let parsed = memo::parse_sections(&payload.content);
        let mut out = format!(
            "# 运维备忘录 · {display_name}（{message_server_id}）\n用量：{total_chars}/{} 字符\n",
            memo::MEMO_MAX_CHARS
        );
        if !parsed.preamble.is_empty() {
            out.push('\n');
            out.push_str(&parsed.preamble);
            out.push('\n');
        }
        if parsed.sections.is_empty() {
            out.push_str("\n（备忘录暂无段落。）\n");
        }
        for section in &parsed.sections {
            out.push_str(&format!("\n## {}\n\n{}\n", section.title, section.body));
        }
        out.push_str(
            "\n[更新方式：先读后写——把合并去重后的内容用 ssh_memo_upsert 整段重写；整段过时时用 ssh_memo_delete。禁止写入凭据。]",
        );
        out
    }
}

#[async_trait]
impl AgentTool for SshMemoUpsertTool {
    fn name(&self) -> &'static str {
        "ssh_memo_upsert"
    }

    fn description(&self) -> &'static str {
        "创建或重写指定 SSH 服务器运维备忘录的一个段落：按段落标题（title）原位替换整段内容，段落不存在时追加到末尾。这是备忘录唯一的写入方式——务必先调用 ssh_memo_read，把新信息与现有内容合并去重后整段重写，不要盲目追加重复条目。克制记录，只写对后续运维长期有价值的必要信息：部署/服务路径、非通用命令与操作方式、端口与依赖、已知问题与解法；禁止记录密码/密钥/令牌等凭据、临时调试输出和过程性日志。全文上限 8000 字符、单段上限 4000 字符；超限写入会被拒绝，需先合并精简、删除过时条目再重试。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server_id": {
                    "type": "string",
                    "description": "ssh_list_servers 返回的服务器 id"
                },
                "title": {
                    "type": "string",
                    "description": "段落标题（无需 ## 前缀，如「部署路径」），不超过 40 个字符。同名标题会整体替换该段落。"
                },
                "content": {
                    "type": "string",
                    "description": "该段落的完整新内容（Markdown 正文，不需要再写标题行）。将完全替换段落旧内容；单段上限 4000 字符。"
                }
            },
            "required": ["server_id", "title", "content"]
        })
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> ToolResult {
        ToolResult::from_text(self.execute_text(args).await)
    }
}

impl SshMemoUpsertTool {
    async fn execute_text(&self, args: &Value) -> String {
        let (server_id, display_name) = match resolve_server(&self.manager, args).await {
            Ok(value) => value,
            Err(error) => return error,
        };
        let Some(title) = string_arg(args, "title") else {
            return "错误：缺少必填参数 title（段落标题）。".to_string();
        };
        let Some(content) = string_arg(args, "content") else {
            return "错误：缺少必填参数 content（段落完整内容）。".to_string();
        };
        // 下方 spawn_blocking 会按值消费这三个标识，成功消息还需要，先克隆留存。
        let message_server_id = server_id.clone();
        let message_title = title.clone();
        match tokio::task::spawn_blocking(move || {
            memo::upsert_section(&server_id, &title, &content)
        })
        .await
        .map_err(|error| error.to_string())
        {
            Ok(Ok(outcome)) => format!(
                "已更新运维备忘录 · {display_name}（{message_server_id}）：段落「{message_title}」{}；全文 {}/{} 字符。",
                if outcome.replaced { "已替换" } else { "已新增" },
                outcome.total_chars,
                memo::MEMO_MAX_CHARS
            ),
            Ok(Err(error)) => format!("错误：{error}"),
            Err(error) => format!("错误：更新备忘录任务失败：{error}"),
        }
    }
}

#[async_trait]
impl AgentTool for SshMemoDeleteTool {
    fn name(&self) -> &'static str {
        "ssh_memo_delete"
    }

    fn description(&self) -> &'static str {
        "按段落标题删除指定 SSH 服务器运维备忘录中的整个段落。仅在整个段落过时、作废或需要整体重写前清理时使用；部分内容更新请改用 ssh_memo_read + ssh_memo_upsert 整段重写。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server_id": {
                    "type": "string",
                    "description": "ssh_list_servers 返回的服务器 id"
                },
                "title": {
                    "type": "string",
                    "description": "要删除的段落标题（无需 ## 前缀，需与备忘录中的标题完全一致）"
                }
            },
            "required": ["server_id", "title"]
        })
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> ToolResult {
        ToolResult::from_text(self.execute_text(args).await)
    }
}

impl SshMemoDeleteTool {
    async fn execute_text(&self, args: &Value) -> String {
        let (server_id, display_name) = match resolve_server(&self.manager, args).await {
            Ok(value) => value,
            Err(error) => return error,
        };
        let Some(title) = string_arg(args, "title") else {
            return "错误：缺少必填参数 title（段落标题）。".to_string();
        };
        // 下方 spawn_blocking 会按值消费这两个标识，成功消息还需要，先克隆留存。
        let message_server_id = server_id.clone();
        let message_title = title.clone();
        match tokio::task::spawn_blocking(move || memo::remove_section(&server_id, &title))
            .await
            .map_err(|error| error.to_string())
        {
            Ok(Ok(true)) => format!(
                "已删除运维备忘录 · {display_name}（{message_server_id}）中的段落「{message_title}」。"
            ),
            Ok(Ok(false)) => format!(
                "运维备忘录 · {display_name}（{message_server_id}）中没有段落「{message_title}」，未做改动。可先调用 ssh_memo_read 查看现有段落。"
            ),
            Ok(Err(error)) => format!("错误：{error}"),
            Err(error) => format!("错误：删除备忘录段落任务失败：{error}"),
        }
    }
}
