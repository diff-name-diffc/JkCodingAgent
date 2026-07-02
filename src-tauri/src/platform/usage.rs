use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Context;
use chrono::DateTime;
use once_cell::sync::Lazy;
use serde::Serialize;
use serde_json::{json, Value};

use super::app_settings::{detect_claude_version, get_agent_bin, get_login_shell_path};
use crate::shared::error::{CommandResult, IntoCommandResult};

const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_BETA_HEADER: &str = "oauth-2025-04-20";
const CLAUDE_TIMEOUT_SECS: u64 = 12;
// 每次 Codex 尝试的超时上限；最多重试两次，所以总计最长 20 秒。
const CODEX_ATTEMPT_TIMEOUT_SECS: u64 = 10;

type UsageResult<T> = std::result::Result<T, UsageError>;

#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    #[error("{action} 失败：{source}")]
    Io {
        action: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("{action} 不是有效 UTF-8：{source}")]
    Utf8 {
        action: &'static str,
        #[source]
        source: std::string::FromUtf8Error,
    },
    #[error("{action} 不是有效 JSON：{source}")]
    Json {
        action: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("Claude credentials are unavailable.")]
    ClaudeCredentialsUnavailable,
    #[error("Claude credentials are unavailable: {0}")]
    ClaudeCredentialsCommand(String),
    #[error("Claude access token was missing from Keychain data.")]
    ClaudeTokenMissing,
    #[error("Claude 用量请求失败：{0}")]
    ClaudeRequest(#[from] reqwest::Error),
    #[error("Claude 用量请求返回 HTTP {0}")]
    ClaudeHttp(reqwest::StatusCode),
    #[error("Claude 用量响应中未包含可识别的窗口数据。")]
    ClaudeUsageWindowMissing,
    #[error("Codex app-server {0} was unavailable.")]
    CodexPipeUnavailable(&'static str),
    #[error("Codex app-server returned error: {0}")]
    CodexAppServer(String),
    #[error("Timed out waiting for Codex response {expected_id}.")]
    CodexTimeout { expected_id: i64 },
    #[error("Codex app-server closed before response {expected_id}.")]
    CodexChannelClosed { expected_id: i64 },
    #[error("Codex response {expected_id} did not include result or error.")]
    CodexInvalidResponse { expected_id: i64 },
    #[error("后台用量任务失败：{0}")]
    Join(#[from] tokio::task::JoinError),
}

fn io_error(action: &'static str) -> impl FnOnce(std::io::Error) -> UsageError {
    move |source| UsageError::Io { action, source }
}

fn json_error(action: &'static str) -> impl FnOnce(serde_json::Error) -> UsageError {
    move |source| UsageError::Json { action, source }
}

static HTTP_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(CLAUDE_TIMEOUT_SECS))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum UsageSource<T> {
    Available { data: T },
    Unavailable { reason: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageSnapshot {
    pub claude: UsageSource<ClaudeUsageData>,
    pub codex: UsageSource<CodexUsageData>,
    #[serde(rename = "fetchedAt")]
    pub fetched_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageWindow {
    #[serde(rename = "usedPercent")]
    pub used_percent: u8,
    #[serde(rename = "remainingPercent")]
    pub remaining_percent: u8,
    #[serde(rename = "resetAt")]
    pub reset_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClaudeUsageData {
    #[serde(rename = "fiveHour")]
    pub five_hour: Option<UsageWindow>,
    #[serde(rename = "sevenDay")]
    pub seven_day: Option<UsageWindow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodexUsageData {
    pub email: Option<String>,
    #[serde(rename = "planType")]
    pub plan_type: Option<String>,
    pub primary: Option<UsageWindow>,
    pub secondary: Option<UsageWindow>,
}

#[tauri::command]
pub async fn read_usage_snapshot() -> CommandResult<UsageSnapshot> {
    read_usage_snapshot_impl()
        .await
        .context("读取用量快照失败")
        .into_command_result()
}

async fn read_usage_snapshot_impl() -> anyhow::Result<UsageSnapshot> {
    let (claude, codex) = tokio::join!(read_claude_usage(), read_codex_usage());

    Ok(UsageSnapshot {
        claude,
        codex,
        fetched_at: chrono::Utc::now().timestamp(),
    })
}

async fn read_claude_usage() -> UsageSource<ClaudeUsageData> {
    if !cfg!(target_os = "macos") {
        return unavailable("Claude 用量读取当前依赖 macOS 钥匙串。");
    }

    let token_result = tokio::task::spawn_blocking(|| -> UsageResult<(String, Option<String>)> {
        let shell_path = get_login_shell_path();
        let output = Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                "Claude Code-credentials",
                "-w",
            ])
            .env("PATH", shell_path)
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(io_error("读取 Claude credentials"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if stderr.is_empty() {
                UsageError::ClaudeCredentialsUnavailable
            } else {
                UsageError::ClaudeCredentialsCommand(stderr)
            });
        }

        let raw = String::from_utf8(output.stdout).map_err(|source| UsageError::Utf8 {
            action: "Claude credential output",
            source,
        })?;
        let parsed: Value =
            serde_json::from_str(raw.trim()).map_err(json_error("Claude credentials JSON"))?;

        let token = parsed
            .pointer("/claudeAiOauth/accessToken")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or(UsageError::ClaudeTokenMissing)?;

        Ok((token.to_string(), detect_claude_version()))
    })
    .await;

    let (token, version) = match token_result {
        Ok(Ok(value)) => value,
        Ok(Err(reason)) => return unavailable(reason.to_string()),
        Err(err) => return unavailable(format!("加载 Claude 凭据失败：{err}")),
    };

    let user_agent = format!(
        "claude-code/{}",
        version.unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
    );

    let response = match HTTP_CLIENT
        .get(CLAUDE_USAGE_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("anthropic-beta", CLAUDE_BETA_HEADER)
        .header("User-Agent", user_agent)
        .header("Accept", "application/json")
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => return unavailable(UsageError::ClaudeRequest(err).to_string()),
    };

    if !response.status().is_success() {
        return unavailable(UsageError::ClaudeHttp(response.status()).to_string());
    }

    let payload = match response.json::<Value>().await {
        Ok(value) => value,
        Err(err) => return unavailable(UsageError::ClaudeRequest(err).to_string()),
    };

    let data = ClaudeUsageData {
        five_hour: payload.get("five_hour").and_then(parse_claude_window),
        seven_day: payload.get("seven_day").and_then(parse_claude_window),
    };

    if data.five_hour.is_none() && data.seven_day.is_none() {
        unavailable(UsageError::ClaudeUsageWindowMissing.to_string())
    } else {
        UsageSource::Available { data }
    }
}

async fn read_codex_usage() -> UsageSource<CodexUsageData> {
    match tokio::task::spawn_blocking(read_codex_usage_blocking).await {
        Ok(Ok(data)) => UsageSource::Available { data },
        Ok(Err(reason)) => unavailable(reason.to_string()),
        Err(err) => unavailable(format!("读取 Codex 用量失败：{err}")),
    }
}

fn read_codex_usage_blocking() -> UsageResult<CodexUsageData> {
    let mut reasons = Vec::new();
    for params in [Value::Null, json!({})] {
        let deadline = Instant::now() + Duration::from_secs(CODEX_ATTEMPT_TIMEOUT_SECS);
        match read_codex_usage_blocking_once(params, deadline) {
            Ok(data) => return Ok(data),
            Err(reason) => reasons.push(reason.to_string()),
        }
    }

    Err(UsageError::CodexAppServer(reasons.join(" | ")))
}

fn read_codex_usage_blocking_once(
    rate_limit_params: Value,
    deadline: Instant,
) -> UsageResult<CodexUsageData> {
    let shell_path = get_login_shell_path();
    let binary = get_agent_bin("codex");
    let mut child = Command::new(&binary)
        .arg("app-server")
        .env("PATH", shell_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(io_error("启动 Codex app-server"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or(UsageError::CodexPipeUnavailable("stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or(UsageError::CodexPipeUnavailable("stderr"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or(UsageError::CodexPipeUnavailable("stdin"))?;

    let (message_tx, message_rx) = mpsc::channel::<UsageResult<Value>>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let line = match line {
                Ok(line) => line,
                Err(err) => {
                    let _ = message_tx.send(Err(UsageError::Io {
                        action: "读取 Codex app-server output",
                        source: err,
                    }));
                    break;
                }
            };

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let parsed =
                serde_json::from_str::<Value>(trimmed).map_err(json_error("Codex app-server JSON"));
            if message_tx.send(parsed).is_err() {
                break;
            }
        }
    });

    // stderr 线程仅用于耗尽管道，防止子进程因 stderr 满而阻塞。
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut buf = String::new();
        let _ = reader.read_to_string(&mut buf);
    });

    let result = (|| -> UsageResult<CodexUsageData> {
        write_json_line(
            &mut stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "clientInfo": {
                        "name": "jkcodingagent",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "capabilities": {},
                }
            }),
        )?;
        wait_for_result(&message_rx, 1, deadline)?;

        write_json_line(
            &mut stdin,
            &json!({ "jsonrpc": "2.0", "method": "initialized" }),
        )?;

        write_json_line(
            &mut stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "account/read",
                "params": {}
            }),
        )?;
        let account = wait_for_result(&message_rx, 2, deadline)?;

        write_json_line(
            &mut stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "account/rateLimits/read",
                "params": rate_limit_params
            }),
        )?;
        let rate_limits = wait_for_result(&message_rx, 3, deadline)?;

        Ok(parse_codex_usage(account, rate_limits))
    })();

    let _ = child.kill();
    let _ = child.wait();

    result
}

fn write_json_line(stdin: &mut dyn Write, value: &Value) -> UsageResult<()> {
    let payload = serde_json::to_string(value).map_err(json_error("序列化 Codex request"))?;
    stdin
        .write_all(payload.as_bytes())
        .map_err(io_error("写入 Codex request"))?;
    stdin
        .write_all(b"\n")
        .map_err(io_error("写入 Codex request terminator"))?;
    stdin.flush().map_err(io_error("刷新 Codex request"))?;
    Ok(())
}

fn wait_for_result(
    rx: &mpsc::Receiver<UsageResult<Value>>,
    expected_id: i64,
    deadline: Instant,
) -> UsageResult<Value> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(UsageError::CodexTimeout { expected_id });
        }

        let remaining = deadline.saturating_duration_since(now);
        let message = rx
            .recv_timeout(remaining)
            .map_err(|_| UsageError::CodexChannelClosed { expected_id })??;

        let matches_id = message.get("id").and_then(Value::as_i64) == Some(expected_id);
        if !matches_id {
            continue;
        }

        if let Some(result) = message.get("result") {
            return Ok(result.clone());
        }

        if let Some(error) = message.get("error") {
            let msg = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Unknown Codex app-server error");
            return Err(UsageError::CodexAppServer(msg.to_string()));
        }

        return Err(UsageError::CodexInvalidResponse { expected_id });
    }
}

fn parse_codex_usage(account: Value, rate_limits: Value) -> CodexUsageData {
    let account_node = account.get("account").unwrap_or(&Value::Null);
    let rate_limit_source = rate_limits
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
        .and_then(|limits| {
            limits
                .get("codex")
                .cloned()
                .or_else(|| limits.values().next().cloned())
        })
        .or_else(|| rate_limits.get("rateLimits").cloned())
        .unwrap_or(Value::Null);

    CodexUsageData {
        email: account_node
            .get("email")
            .and_then(Value::as_str)
            .map(str::to_string),
        plan_type: account_node
            .get("planType")
            .and_then(Value::as_str)
            .map(str::to_string),
        primary: rate_limit_source
            .get("primary")
            .and_then(parse_codex_window),
        secondary: rate_limit_source
            .get("secondary")
            .and_then(parse_codex_window),
    }
}

fn parse_codex_window(value: &Value) -> Option<UsageWindow> {
    // Codex returns usedPercent as an integer 0–100, not a 0.0–1.0 fraction.
    let used_percent = value.get("usedPercent").and_then(|v| {
        let raw = match v {
            Value::Number(n) => n.as_f64()?,
            Value::String(s) => s.parse::<f64>().ok()?,
            _ => return None,
        };
        Some(raw.clamp(0.0, 100.0).round() as u8)
    })?;
    Some(UsageWindow {
        used_percent,
        remaining_percent: 100_u8.saturating_sub(used_percent),
        reset_at: value.get("resetsAt").and_then(parse_reset_value),
    })
}

fn parse_claude_window(value: &Value) -> Option<UsageWindow> {
    let used_percent = value.get("utilization").and_then(parse_percent_value)?;
    Some(UsageWindow {
        used_percent,
        remaining_percent: 100_u8.saturating_sub(used_percent),
        reset_at: value.get("resets_at").and_then(parse_reset_value),
    })
}

fn parse_percent_value(value: &Value) -> Option<u8> {
    let raw = match value {
        Value::Number(number) => number.as_f64()?,
        Value::String(string) => string.parse::<f64>().ok()?,
        _ => return None,
    };

    let normalized = if raw <= 1.0 { raw * 100.0 } else { raw };
    let clamped = normalized.clamp(0.0, 100.0).round();
    Some(clamped as u8)
}

fn parse_reset_value(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(string) => {
            if let Ok(timestamp) = string.parse::<i64>() {
                Some(timestamp)
            } else {
                DateTime::parse_from_rfc3339(string)
                    .ok()
                    .map(|dt| dt.timestamp())
            }
        }
        _ => None,
    }
}

fn unavailable<T>(reason: impl Into<String>) -> UsageSource<T> {
    UsageSource::Unavailable {
        reason: reason.into(),
    }
}
