use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(crate) const SNAPSHOT_SCHEMA_VERSION: u32 = 1;
pub(crate) const MAX_SOURCES: usize = 8;
pub(crate) const DEFAULT_MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub(crate) const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024 * 1024;
pub(crate) const DEFAULT_UPDATE_TIMEOUT_SECS: u64 = 1_800;
pub(crate) const MAX_UPDATE_TIMEOUT_SECS: u64 = 21_600;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum LogSource {
    Docker {
        server_id: String,
        container: String,
        #[serde(default)]
        sudo: bool,
    },
    SshFile {
        server_id: String,
        path: String,
        #[serde(default)]
        sudo: bool,
    },
    LocalFile {
        path: String,
    },
}

impl LogSource {
    pub(crate) fn validate(&self) -> Result<(), String> {
        match self {
            Self::Docker {
                server_id,
                container,
                ..
            } => {
                validate_server_id(server_id)?;
                let mut chars = container.chars();
                if !chars.next().is_some_and(|ch| ch.is_ascii_alphanumeric())
                    || container.len() > 128
                    || !container
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || "._-".contains(ch))
                {
                    return Err(format!("容器 ID/名称不合法：{container}"));
                }
                Ok(())
            }
            Self::SshFile {
                server_id, path, ..
            } => {
                validate_server_id(server_id)?;
                validate_remote_path(path)
            }
            Self::LocalFile { path } => {
                if path.trim().is_empty() || path.chars().any(char::is_control) {
                    return Err("本地日志路径不能为空且不能包含控制字符".into());
                }
                Ok(())
            }
        }
    }

    pub(crate) fn server_id(&self) -> Option<&str> {
        match self {
            Self::Docker { server_id, .. } | Self::SshFile { server_id, .. } => Some(server_id),
            Self::LocalFile { .. } => None,
        }
    }

    pub(crate) fn sudo(&self) -> bool {
        match self {
            Self::Docker { sudo, .. } | Self::SshFile { sudo, .. } => *sudo,
            Self::LocalFile { .. } => false,
        }
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Self::Docker {
                server_id,
                container,
                ..
            } => format!("docker:{server_id}/{container}"),
            Self::SshFile {
                server_id, path, ..
            } => format!("ssh:{server_id}:{path}"),
            Self::LocalFile { path } => format!("local:{path}"),
        }
    }

    pub(crate) fn remote_command(&self, window: &DockerWindow) -> Option<String> {
        match self {
            Self::Docker { container, .. } => {
                let mut command = "docker logs --timestamps".to_string();
                if let Some(since) = &window.since {
                    command.push_str(&format!(
                        " --since {}",
                        crate::ssh_tool::shell_single_quote(since)
                    ));
                }
                command.push_str(&format!(
                    " --until {}",
                    crate::ssh_tool::shell_single_quote(&window.until)
                ));
                if let Some(tail) = window.tail {
                    command.push_str(&format!(" --tail {tail}"));
                }
                command.push_str(&format!(" {container} 2>&1"));
                Some(command)
            }
            Self::SshFile { path, .. } => Some(format!(
                "cat -- {}",
                crate::ssh_tool::shell_single_quote(path)
            )),
            Self::LocalFile { .. } => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DockerWindow {
    pub since: Option<String>,
    pub until: String,
    pub tail: Option<u64>,
}

pub(crate) fn docker_window(
    since: Option<&str>,
    until: Option<&str>,
    tail: Option<u64>,
) -> Result<DockerWindow, String> {
    if tail.is_some_and(|value| value == 0 || value > 1_000_000) {
        return Err("tail 必须在 1 到 1000000 之间".into());
    }
    let now = Utc::now();
    let until = parse_time_bound(until, now)?.unwrap_or(now);
    let since = parse_time_bound(since, now)?;
    if since.is_some_and(|value| value >= until) {
        return Err("since 必须早于 until".into());
    }
    Ok(DockerWindow {
        since: since.map(format_docker_time),
        until: format_docker_time(until),
        tail,
    })
}

fn parse_time_bound(
    raw: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, String> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if let Ok(value) = DateTime::parse_from_rfc3339(raw) {
        return Ok(Some(value.with_timezone(&Utc)));
    }
    let unit = raw
        .chars()
        .last()
        .ok_or_else(|| format!("时间不能为空：{raw}"))?;
    let number = &raw[..raw.len() - unit.len_utf8()];
    let amount = number
        .parse::<i64>()
        .map_err(|_| format!("时间必须是 RFC3339 或 30m/2h/1d 形式：{raw}"))?;
    if amount <= 0 || amount > 3650 {
        return Err(format!("相对时间必须在 1 到 3650 之间：{raw}"));
    }
    let duration = match unit {
        's' => Duration::seconds(amount),
        'm' => Duration::minutes(amount),
        'h' => Duration::hours(amount),
        'd' => Duration::days(amount),
        _ => return Err(format!("无法识别时间单位：{raw}")),
    };
    Ok(Some(now - duration))
}

fn format_docker_time(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[derive(Clone, Debug)]
pub(crate) struct UpdateOptions {
    pub snapshot_id: Option<String>,
    pub label: Option<String>,
    pub sources: Vec<LogSource>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub tail: Option<u64>,
    pub max_total_bytes: u64,
    pub timeout_secs: u64,
}

impl UpdateOptions {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if let Some(id) = &self.snapshot_id {
            validate_snapshot_id(id)?;
        }
        if self.sources.len() > MAX_SOURCES {
            return Err(format!("单次最多同步 {MAX_SOURCES} 个日志来源"));
        }
        if self
            .label
            .as_ref()
            .is_some_and(|label| label.chars().count() > 128 || label.chars().any(char::is_control))
        {
            return Err("快照名称不能超过 128 字符且不能包含控制字符".into());
        }
        if !(1024 * 1024..=MAX_TOTAL_BYTES).contains(&self.max_total_bytes) {
            return Err(format!(
                "max_total_bytes 必须在 1 MiB 到 {} GiB 之间",
                MAX_TOTAL_BYTES / 1024 / 1024 / 1024
            ));
        }
        if !(1..=MAX_UPDATE_TIMEOUT_SECS).contains(&self.timeout_secs) {
            return Err(format!(
                "timeout_secs 必须在 1 到 {MAX_UPDATE_TIMEOUT_SECS} 之间"
            ));
        }
        // 时间窗口在入口就解析：非法格式或 since >= until 立即拒绝，而不是
        // 让任务带着 Queued 状态进后台再异步失败。
        docker_window(self.since.as_deref(), self.until.as_deref(), self.tail)?;
        for source in &self.sources {
            source.validate()?;
        }
        Ok(())
    }
}

pub(crate) fn validate_snapshot_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
    {
        return Err("snapshot_id 仅支持小写字母、数字、短横线和下划线，长度 1 到 64".into());
    }
    Ok(())
}

pub(crate) fn ssh_session_id(workspace_id: &str, snapshot_id: &str) -> String {
    let digest = Sha256::digest(format!("{workspace_id}\n{snapshot_id}").as_bytes());
    let suffix = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("log-{suffix}")
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UpdateState {
    Queued,
    Reviewing,
    Syncing,
    Indexing,
    Publishing,
    Ready,
    Failed,
    Cancelled,
}

impl UpdateState {
    pub(crate) fn terminal(self) -> bool {
        matches!(self, Self::Ready | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateRecord {
    pub job_id: String,
    pub snapshot_id: String,
    pub generation: u64,
    pub state: UpdateState,
    pub created_at: String,
    pub updated_at: String,
    pub raw_bytes: u64,
    pub file_count: usize,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnapshotFile {
    pub file_id: u64,
    pub source: LogSource,
    pub source_label: String,
    pub raw_path: String,
    pub size_bytes: u64,
    pub line_count: u64,
    pub long_lines: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenerationManifest {
    pub schema_version: u32,
    pub snapshot_id: String,
    pub workspace_id: String,
    pub generation: u64,
    pub label: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub sources: Vec<LogSource>,
    pub files: Vec<SnapshotFile>,
    pub raw_bytes: u64,
    pub index_bytes: u64,
    pub index_backend: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CurrentPointer {
    pub schema_version: u32,
    pub snapshot_id: String,
    pub workspace_id: String,
    pub generation: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnapshotStatus {
    pub snapshot_id: String,
    pub current: Option<GenerationManifest>,
    pub update: Option<UpdateRecord>,
    pub active: bool,
}

fn validate_server_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 || id.chars().any(char::is_control) {
        return Err("server_id 不能为空、不能超过 64 字符且不能包含控制字符".into());
    }
    Ok(())
}

fn validate_remote_path(path: &str) -> Result<(), String> {
    if !path.starts_with('/')
        || path == "/"
        || path.chars().any(char::is_control)
        || path.split('/').any(|part| part == "." || part == "..")
    {
        return Err(format!(
            "远端日志路径必须是无 . 或 .. 的绝对文件路径：{path}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_quote_remote_inputs_and_freeze_window() {
        let window = DockerWindow {
            since: Some("2026-01-01T00:00:00.000Z".into()),
            until: "2026-01-01T01:00:00.000Z".into(),
            tail: Some(100),
        };
        let source = LogSource::Docker {
            server_id: "server".into(),
            container: "api-1".into(),
            sudo: false,
        };
        let command = source.remote_command(&window).unwrap();
        assert!(command.contains("--until '2026-01-01T01:00:00.000Z'"));
        assert!(command.ends_with(" api-1 2>&1"));
        let file = LogSource::SshFile {
            server_id: "server".into(),
            path: "/var/log/a'b.log".into(),
            sudo: false,
        };
        assert_eq!(
            file.remote_command(&window).unwrap(),
            "cat -- '/var/log/a'\\''b.log'"
        );
    }

    #[test]
    fn rejects_ambiguous_remote_paths_and_container_shell_input() {
        assert!(LogSource::SshFile {
            server_id: "server".into(),
            path: "/var/log/../secret".into(),
            sudo: false,
        }
        .validate()
        .is_err());
        assert!(LogSource::Docker {
            server_id: "server".into(),
            container: "api;rm".into(),
            sudo: false,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn invalid_unicode_time_is_an_error_not_a_panic() {
        assert!(docker_window(Some("过去"), None, None).is_err());
        assert!(docker_window(Some("1h"), Some("30m"), None).is_ok());
    }
}
