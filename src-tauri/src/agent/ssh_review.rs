//! 命令安全审查：在 ssh_exec / local_zsh 执行命令前，把意图、用户任务、目标环境信息和
//! 待执行命令交给 OpenAI 兼容审查模型，判断是否可安全执行。
//!
//! 失败即阻断（fail-closed）：模型调用失败、超时或返回内容无法解析时一律返回 `Err`，
//! 由调用方阻断命令执行。

use std::time::Duration;

use tokio::time::timeout;

use crate::agent::db::settings::{SshReviewConfig, DEFAULT_REVIEW_SYSTEM_PROMPT};
use crate::agent::llm::{ChatMessage, OpenAiCompatProvider};

const REVIEW_TIMEOUT_SECS: u64 = 30;
const REVIEW_MAX_TOKENS: u32 = 256;

/// 待审查命令的目标服务器信息（剔除密码 / 私钥 / 口令等敏感字段）。
#[derive(Debug, Clone)]
pub struct SshReviewServerInfo {
    pub id: String,
    pub description: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub tags: Vec<String>,
}

/// 交给审查模型的载荷。
#[derive(Debug, Clone)]
pub struct SshReviewPayload {
    pub intent: String,
    pub task: String,
    pub server_info: SshReviewServerInfo,
    pub command: String,
}

/// 待审查命令的执行目标环境（剔除密码 / 私钥 / 口令等敏感字段）。
#[derive(Debug, Clone)]
pub enum CommandReviewTarget {
    Ssh(SshReviewServerInfo),
    LocalZsh {
        workspace_path: String,
        run_dir: String,
    },
}

/// 交给审查模型的通用命令载荷。
#[derive(Debug, Clone)]
pub struct CommandReviewPayload {
    pub intent: String,
    pub task: String,
    pub target: CommandReviewTarget,
    pub command: String,
}

/// 审查结论。
#[derive(Debug, Clone)]
pub struct SshReviewVerdict {
    pub allowed: bool,
    pub reason: String,
}

/// 调用审查模型评估命令安全性。
///
/// - 返回 `Ok(Verdict)` 表示模型已给出判定（`allowed` 可能为 false）。
/// - 返回 `Err` 表示审查链路本身出错（网络/超时/解析失败），调用方应按 fail-closed 阻断。
pub async fn review_command(
    config: &SshReviewConfig,
    payload: &SshReviewPayload,
) -> Result<SshReviewVerdict, String> {
    review_shell_command(
        config,
        &CommandReviewPayload {
            intent: payload.intent.clone(),
            task: payload.task.clone(),
            target: CommandReviewTarget::Ssh(payload.server_info.clone()),
            command: payload.command.clone(),
        },
    )
    .await
}

pub async fn review_shell_command(
    config: &SshReviewConfig,
    payload: &CommandReviewPayload,
) -> Result<SshReviewVerdict, String> {
    let model_name = config.model_config.model.trim();
    if model_name.is_empty() {
        return Err("审查模型未配置 model".to_string());
    }

    let provider = OpenAiCompatProvider::new(
        config.model_config.api_key.clone(),
        config.model_config.url.clone(),
        config.model_config.model.clone(),
        REVIEW_MAX_TOKENS,
        0.0,
    );

    let system_prompt = {
        let trimmed = config.system_prompt.trim();
        if trimmed.is_empty() {
            DEFAULT_REVIEW_SYSTEM_PROMPT.to_string()
        } else {
            trimmed.to_string()
        }
    };
    let user_prompt = build_command_user_prompt(payload);
    let messages = vec![
        ChatMessage::system(system_prompt),
        ChatMessage {
            role: "user".to_string(),
            content: user_prompt,
            content_parts: Vec::new(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        },
    ];

    let response = timeout(
        Duration::from_secs(REVIEW_TIMEOUT_SECS),
        provider.chat_stream(&messages, &[], false, |_| {}),
    )
    .await
    .map_err(|_| format!("审查模型 `{model_name}` 调用超时（>{REVIEW_TIMEOUT_SECS}s）"))
    .map_err(|error| format!("审查模型 `{model_name}` 调用失败：{error}"))?
    .map_err(|error| format!("审查模型 `{model_name}` 调用失败：{error}"))?;

    let content = response.content.trim();
    if content.is_empty() {
        return Err(format!("审查模型 `{model_name}` 返回空内容"));
    }

    parse_verdict(content)
}

fn build_command_user_prompt(payload: &CommandReviewPayload) -> String {
    let target_info = match &payload.target {
        CommandReviewTarget::Ssh(server) => {
            let tags = if server.tags.is_empty() {
                String::from("（无）")
            } else {
                server.tags.join(", ")
            };
            format!(
                "【目标环境】\n- 类型：SSH 远程服务器\n- id：{}\n- 描述：{}\n- host:port：{}:{}\n- 登录用户：{}\n- 标签：{}",
                server.id, server.description, server.host, server.port, server.username, tags
            )
        }
        CommandReviewTarget::LocalZsh {
            workspace_path,
            run_dir,
        } => format!(
            "【目标环境】\n- 类型：本地 macOS zsh\n- 工作区：{}\n- 执行目录：{}\n- 约束：命令固定通过 /bin/zsh -lc 执行，产物应留在执行目录内",
            workspace_path, run_dir
        ),
    };
    format!(
        "【当前意图】\n{}\n\n\
         【用户任务】\n{}\n\n\
         {}\n\n\
         【待执行命令】\n{}",
        if payload.intent.trim().is_empty() {
            "（未提供）"
        } else {
            payload.intent.trim()
        },
        if payload.task.trim().is_empty() {
            "（未提供）"
        } else {
            payload.task.trim()
        },
        target_info,
        payload.command,
    )
}

/// 解析模型输出为判定。无法解析时返回 `Err`（fail-closed）。
fn parse_verdict(content: &str) -> Result<SshReviewVerdict, String> {
    let first_line = content.lines().next().unwrap_or("").trim();
    let lower = first_line.to_lowercase();

    // 允许：ALLOW / 允许 / 通过
    if lower == "allow"
        || lower.starts_with("allow")
        || lower.contains("允许")
        || lower.contains("通过")
    {
        // 避免把 "不允许" / "不通过" 误判为允许
        if !(lower.contains("不允许") || lower.contains("不通过") || lower.contains("拒绝"))
        {
            return Ok(SshReviewVerdict {
                allowed: true,
                reason: String::new(),
            });
        }
    }

    // 拒绝：DENY: <reason> / 拒绝 / 不允许 / 不通过
    let deny_reason = if let Some(rest) = first_line.strip_prefix("DENY:") {
        rest.trim().to_string()
    } else if let Some(rest) = first_line.strip_prefix("deny:") {
        rest.trim().to_string()
    } else if lower.contains("拒绝")
        || lower.contains("不允许")
        || lower.contains("不通过")
        || lower.starts_with("deny")
    {
        // 取冒号/顿号后的说明，无则用整行
        first_line
            .split_once(['：', ':'])
            .map(|(_, r)| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| "审查模型判定为不安全".to_string())
    } else {
        return Err(format!("无法解析审查模型输出：{content}"));
    };

    let reason = if deny_reason.is_empty() {
        "审查模型判定为不安全".to_string()
    } else {
        deny_reason
    };
    Ok(SshReviewVerdict {
        allowed: false,
        reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_allow_variants() {
        assert!(parse_verdict("ALLOW").unwrap().allowed);
        assert!(parse_verdict("允许执行").unwrap().allowed);
        assert!(parse_verdict("通过").unwrap().allowed);
    }

    #[test]
    fn parses_deny_with_reason() {
        let v = parse_verdict("DENY: rm 指向根目录，可能删除系统文件").unwrap();
        assert!(!v.allowed);
        assert_eq!(v.reason, "rm 指向根目录，可能删除系统文件");
    }

    #[test]
    fn parses_deny_chinese() {
        let v = parse_verdict("拒绝：关机命令不可执行").unwrap();
        assert!(!v.allowed);
        assert_eq!(v.reason, "关机命令不可执行");
    }

    #[test]
    fn rejects_unparseable() {
        assert!(parse_verdict("我觉得这个命令还行").is_err());
    }

    #[test]
    fn does_not_misread_disallow_as_allow() {
        assert!(!parse_verdict("不允许").unwrap().allowed);
        assert!(!parse_verdict("不通过").unwrap().allowed);
    }
}
