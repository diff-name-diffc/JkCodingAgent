use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::project::atomic_write;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

// ── Version 缓存 ─────────────────────────────────────────────────────────────

static CACHED_CLAUDE_VERSION: OnceLock<Mutex<Option<Option<String>>>> = OnceLock::new();
static CACHED_CODEX_VERSION: OnceLock<Mutex<Option<Option<String>>>> = OnceLock::new();

// ── Login shell 环境解析 ─────────────────────────────────────────────────────

static LOGIN_SHELL_ENV: OnceLock<Vec<(String, String)>> = OnceLock::new();
static LOGIN_SHELL_PATH: OnceLock<String> = OnceLock::new();
const ENV_SENTINEL: &[u8] = b"__JKCODINGAGENT_ENV_START__\0";

/// 返回用户 login shell 导出的完整环境变量。
/// 首次调用时执行 `$SHELL -l -i -c 'env -0'`，之后从缓存返回。
pub fn get_login_shell_env() -> &'static [(String, String)] {
    LOGIN_SHELL_ENV
        .get_or_init(resolve_login_shell_env)
        .as_slice()
}

/// 返回用户 login shell 解析后的完整 PATH。
/// 基于缓存的 login shell 环境提取，避免重复启动 shell。
pub fn get_login_shell_path() -> &'static str {
    LOGIN_SHELL_PATH.get_or_init(|| {
        get_login_shell_env()
            .iter()
            .find(|(key, _)| key == "PATH")
            .map(|(_, value)| value.clone())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(build_fallback_path)
    })
}

fn resolve_login_shell_env() -> Vec<(String, String)> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());

    // -l: login shell，source .zprofile / .bash_profile
    // -i: interactive，source .zshrc / .bashrc（nvm 等通常在此初始化）
    if let Some(env) = read_shell_env(&shell, true) {
        return env;
    }

    // 降级：尝试不带 -i（兼容某些 rc 文件有交互式命令的情况）
    if let Some(env) = read_shell_env(&shell, false) {
        return env;
    }

    build_fallback_env()
}

fn read_shell_env(shell: &str, interactive: bool) -> Option<Vec<(String, String)>> {
    let args: &[&str] = if interactive {
        &[
            "-l",
            "-i",
            "-c",
            "printf '__JKCODINGAGENT_ENV_START__\\0'; env -0",
        ]
    } else {
        &[
            "-l",
            "-c",
            "printf '__JKCODINGAGENT_ENV_START__\\0'; env -0",
        ]
    };

    let output = Command::new(shell)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    parse_shell_env_output(&output.stdout)
}

fn parse_shell_env_output(stdout: &[u8]) -> Option<Vec<(String, String)>> {
    let start = stdout
        .windows(ENV_SENTINEL.len())
        .position(|window| window == ENV_SENTINEL)?
        + ENV_SENTINEL.len();

    let mut env = Vec::new();
    for entry in stdout[start..].split(|byte| *byte == 0) {
        if entry.is_empty() {
            continue;
        }

        let Some(eq) = entry.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let key = String::from_utf8_lossy(&entry[..eq]).into_owned();
        if key.is_empty() || matches!(key.as_str(), "PWD" | "OLDPWD" | "SHLVL" | "_") {
            continue;
        }
        let value = String::from_utf8_lossy(&entry[eq + 1..]).into_owned();
        env.push((key, value));
    }

    if env.is_empty() {
        None
    } else {
        Some(env)
    }
}

fn build_fallback_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let current = std::env::var("PATH").unwrap_or_default();
    let extras = [
        format!("{home}/.local/bin"),
        format!("{home}/.npm-global/bin"),
        "/opt/homebrew/bin".to_string(),
        "/opt/homebrew/sbin".to_string(),
        "/usr/local/bin".to_string(),
        "/usr/bin".to_string(),
        "/bin".to_string(),
        "/usr/sbin".to_string(),
        "/sbin".to_string(),
    ];
    let mut parts: Vec<String> = extras.to_vec();
    for p in current.split(':') {
        if !p.is_empty() && !parts.contains(&p.to_string()) {
            parts.push(p.to_string());
        }
    }
    parts.join(":")
}

fn build_fallback_env() -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = std::env::vars()
        .filter(|(key, _)| !matches!(key.as_str(), "PWD" | "OLDPWD" | "SHLVL" | "_"))
        .collect();

    if let Some((_, path)) = env.iter_mut().find(|(key, _)| key == "PATH") {
        *path = build_fallback_path();
    } else {
        env.push(("PATH".to_string(), build_fallback_path()));
    }

    if !env.iter().any(|(key, _)| key == "HOME") {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            env.push(("HOME".to_string(), home));
        }
    }

    env
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct AppSettings {
    #[serde(default)]
    pub claude_path: String,
    #[serde(default)]
    pub codex_path: String,
}

fn get_agent_bin_from_settings(settings: &AppSettings, agent: &str) -> String {
    match agent {
        "codex" => {
            if settings.codex_path.is_empty() {
                "codex".to_string()
            } else {
                settings.codex_path.clone()
            }
        }
        _ => {
            if settings.claude_path.is_empty() {
                "claude".to_string()
            } else {
                settings.claude_path.clone()
            }
        }
    }
}

fn clear_cached_versions() {
    *CACHED_CLAUDE_VERSION
        .get_or_init(|| Mutex::new(None))
        .lock() = None;
    *CACHED_CODEX_VERSION.get_or_init(|| Mutex::new(None)).lock() = None;
}

fn app_data_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "找不到用户主目录".to_string())?;
    Ok(home.join(".jkcodingagent"))
}

fn settings_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("settings.json"))
}

/// 执行 `which <binary>` 返回完整路径，找不到则返回空字符串。
/// 使用 login shell 解析后的完整 PATH，确保 nvm 等版本管理器的路径也能被找到。
fn detect_path(binary: &str) -> String {
    let shell_path = get_login_shell_path();

    let output = Command::new("which")
        .arg(binary)
        .env("PATH", shell_path)
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() {
                return p;
            }
        }
    }
    String::new()
}

/// 内部工具函数：从文件读取设置。文件不存在时自动检测并保存。
pub fn load_settings_internal() -> AppSettings {
    let path = match settings_path() {
        Ok(p) => p,
        Err(_) => return AppSettings::default(),
    };

    if !path.exists() {
        // 首次启动：用 which 自动检测并保存
        let settings = AppSettings {
            claude_path: detect_path("claude"),
            codex_path: detect_path("codex"),
        };
        if let Ok(dir) = app_data_dir() {
            let _ = fs::create_dir_all(&dir);
        }
        if let Ok(raw) = serde_json::to_string_pretty(&settings) {
            let _ = atomic_write(&path, &raw);
        }
        return settings;
    }

    let raw = match fs::read_to_string(&path) {
        Ok(r) => r,
        Err(_) => return AppSettings::default(),
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// 根据 agent 名称（"claude" 或 "codex"）返回对应的可执行文件路径。
/// 若配置为空，则回退到直接使用二进制名称。
pub fn get_agent_bin(agent: &str) -> String {
    get_agent_bin_from_settings(&load_settings_internal(), agent)
}

fn load_settings_for_agent_execution() -> Result<AppSettings, String> {
    let path = settings_path()?;
    if !path.exists() {
        return Ok(load_settings_internal());
    }

    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("读取智能体设置失败（{}）：{error}", path.display()))?;
    serde_json::from_str(&raw)
        .map_err(|error| format!("解析智能体设置失败（{}）：{error}", path.display()))
}

pub fn get_agent_bin_checked(agent: &str) -> Result<String, String> {
    Ok(get_agent_bin_from_settings(
        &load_settings_for_agent_execution()?,
        agent,
    ))
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub fn load_app_settings() -> Result<AppSettings, String> {
    Ok(load_settings_internal())
}

#[tauri::command]
pub fn save_app_settings(settings: AppSettings) -> Result<(), String> {
    let dir = app_data_dir()?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = settings_path()?;
    let raw = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    atomic_write(&path, &raw)?;
    clear_cached_versions();
    Ok(())
}

#[tauri::command]
pub fn detect_agent_paths() -> Result<AppSettings, String> {
    Ok(AppSettings {
        claude_path: detect_path("claude"),
        codex_path: detect_path("codex"),
    })
}

// ── Version detection ──────────────────────────────────────────────────────────

/// 运行 `<binary> --version` 解析版本号。
/// 支持的输出格式：
///   "2.1.87 (Claude Code)"   →  "2.1.87"
///   "Codex v0.1.2025"        →  "0.1.2025"
fn detect_version(binary: &str) -> Option<String> {
    let shell_path = get_login_shell_path();
    let output = Command::new(binary)
        .arg("--version")
        .env("PATH", shell_path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    // 找第一个以数字开头的 token（形如 "1.2.3"）
    text.split_whitespace()
        .find(|s| s.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(|s| s.to_string())
}

fn detect_versions_for_settings(settings: &AppSettings) -> AgentVersions {
    AgentVersions {
        claude_version: detect_version(&get_agent_bin_from_settings(settings, "claude"))
            .unwrap_or_default(),
        codex_version: detect_version(&get_agent_bin_from_settings(settings, "codex"))
            .unwrap_or_default(),
    }
}

/// 将版本字符串解析为 (major, minor, patch) 三元组。
fn parse_semver(v: &str) -> (u32, u32, u32) {
    let parts: Vec<&str> = v.split('.').collect();
    (
        parts.first().and_then(|s| s.parse().ok()).unwrap_or(0),
        parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0),
        parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0),
    )
}

/// 检测 Claude Code 版本（进程级缓存）。
pub fn detect_claude_version() -> Option<String> {
    let cache = CACHED_CLAUDE_VERSION.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock();
    if let Some(version) = guard.clone() {
        return version;
    }

    let detected = detect_version(&get_agent_bin("claude"));
    *guard = Some(detected.clone());
    detected
}

/// 检测 Codex 版本（进程级缓存）。
pub fn detect_codex_version() -> Option<String> {
    let cache = CACHED_CODEX_VERSION.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock();
    if let Some(version) = guard.clone() {
        return version;
    }

    let detected = detect_version(&get_agent_bin("codex"));
    *guard = Some(detected.clone());
    detected
}

/// 判断 Claude Code 版本是否 >= 指定最低版本。
/// 优先使用已传入的 `saved_version`（来自项目配置），为空时再执行自动检测。
pub fn claude_version_gte(saved_version: &str, min_version: &str) -> bool {
    let version = if saved_version.is_empty() {
        match detect_claude_version() {
            Some(v) => v,
            None => return false,
        }
    } else {
        saved_version.to_string()
    };
    parse_semver(&version) >= parse_semver(min_version)
}

/// Tauri 命令：检测 Claude 和 Codex 的版本并返回。
#[tauri::command]
pub fn detect_agent_versions() -> Result<AgentVersions, String> {
    Ok(AgentVersions {
        claude_version: detect_claude_version().unwrap_or_default(),
        codex_version: detect_codex_version().unwrap_or_default(),
    })
}

#[tauri::command]
pub fn detect_agent_versions_for_settings(settings: AppSettings) -> Result<AgentVersions, String> {
    Ok(detect_versions_for_settings(&settings))
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct AgentVersions {
    pub claude_version: String,
    pub codex_version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── AppSettings defaults ─────────────────────────────────────────────────

    #[test]
    fn app_settings_default_empty_paths() {
        let settings = AppSettings::default();
        assert!(settings.claude_path.is_empty());
        assert!(settings.codex_path.is_empty());
    }

    #[test]
    fn app_settings_deserializes_empty_object() {
        let settings: AppSettings = serde_json::from_str("{}").unwrap();
        assert!(settings.claude_path.is_empty());
        assert!(settings.codex_path.is_empty());
    }

    #[test]
    fn app_settings_serializes_and_deserializes() {
        let settings = AppSettings {
            claude_path: "/usr/local/bin/claude".to_string(),
            codex_path: "/usr/local/bin/codex".to_string(),
        };
        let json = serde_json::to_string(&settings).unwrap();
        let parsed: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.claude_path, "/usr/local/bin/claude");
        assert_eq!(parsed.codex_path, "/usr/local/bin/codex");
    }

    #[test]
    fn app_settings_deserializes_partial() {
        // Missing fields should use serde(default)
        let json = r#"{"claude_path":"/opt/claude"}"#;
        let settings: AppSettings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.claude_path, "/opt/claude");
        assert!(settings.codex_path.is_empty());
    }

    // ── get_agent_bin_from_settings ──────────────────────────────────────────

    #[test]
    fn agent_bin_returns_configured_claude_path() {
        let settings = AppSettings {
            claude_path: "/custom/claude".to_string(),
            codex_path: "".to_string(),
        };
        assert_eq!(get_agent_bin_from_settings(&settings, "claude"), "/custom/claude");
    }

    #[test]
    fn agent_bin_returns_default_claude_when_empty() {
        let settings = AppSettings {
            claude_path: "".to_string(),
            codex_path: "".to_string(),
        };
        assert_eq!(get_agent_bin_from_settings(&settings, "claude"), "claude");
    }

    #[test]
    fn agent_bin_returns_configured_codex_path() {
        let settings = AppSettings {
            claude_path: "".to_string(),
            codex_path: "/custom/codex".to_string(),
        };
        assert_eq!(get_agent_bin_from_settings(&settings, "codex"), "/custom/codex");
    }

    #[test]
    fn agent_bin_returns_default_codex_when_empty() {
        let settings = AppSettings {
            claude_path: "".to_string(),
            codex_path: "".to_string(),
        };
        assert_eq!(get_agent_bin_from_settings(&settings, "codex"), "codex");
    }

    #[test]
    fn agent_bin_returns_claude_for_unknown_agent() {
        let settings = AppSettings {
            claude_path: "/my/claude".to_string(),
            codex_path: "/my/codex".to_string(),
        };
        // Any agent name that isn't "codex" should return claude path
        assert_eq!(get_agent_bin_from_settings(&settings, "unknown"), "/my/claude");
        assert_eq!(get_agent_bin_from_settings(&settings, "other"), "/my/claude");
    }

    // ── parse_semver ─────────────────────────────────────────────────────────

    #[test]
    fn parse_semver_standard() {
        assert_eq!(parse_semver("1.2.3"), (1, 2, 3));
    }

    #[test]
    fn parse_semver_zero() {
        assert_eq!(parse_semver("0.0.0"), (0, 0, 0));
    }

    #[test]
    fn parse_semver_large_numbers() {
        assert_eq!(parse_semver("100.200.300"), (100, 200, 300));
    }

    #[test]
    fn parse_semver_two_parts() {
        // Only two parts: minor and patch default to 0
        assert_eq!(parse_semver("5.3"), (5, 3, 0));
    }

    #[test]
    fn parse_semver_one_part() {
        assert_eq!(parse_semver("7"), (7, 0, 0));
    }

    #[test]
    fn parse_semver_empty() {
        assert_eq!(parse_semver(""), (0, 0, 0));
    }

    #[test]
    fn parse_semver_non_numeric_parts() {
        // Non-numeric parts parse to 0
        assert_eq!(parse_semver("a.b.c"), (0, 0, 0));
    }

    #[test]
    fn parse_semver_mixed_numeric_nonnumeric() {
        assert_eq!(parse_semver("1.a.3"), (1, 0, 3));
    }

    #[test]
    fn parse_semver_four_parts_takes_first_three() {
        // Fourth part is ignored
        assert_eq!(parse_semver("1.2.3.4"), (1, 2, 3));
    }

    // ── parse_shell_env_output ───────────────────────────────────────────────

    #[test]
    fn shell_env_parses_simple_output() {
        let sentinel = b"__JKCODINGAGENT_ENV_START__\0";
        let mut input = Vec::new();
        input.extend_from_slice(sentinel);
        input.extend_from_slice(b"HOME=/Users/test\0PATH=/usr/bin\0");

        let result = parse_shell_env_output(&input).unwrap();
        assert!(result.iter().any(|(k, v)| k == "HOME" && v == "/Users/test"));
        assert!(result.iter().any(|(k, v)| k == "PATH" && v == "/usr/bin"));
    }

    #[test]
    fn shell_env_skips_filtered_keys() {
        let sentinel = b"__JKCODINGAGENT_ENV_START__\0";
        let mut input = Vec::new();
        input.extend_from_slice(sentinel);
        input.extend_from_slice(b"PWD=/current\0OLDPWD=/old\0SHLVL=1\0_=cmd\0HOME=/Users/test\0");

        let result = parse_shell_env_output(&input).unwrap();
        assert!(!result.iter().any(|(k, _)| k == "PWD"));
        assert!(!result.iter().any(|(k, _)| k == "OLDPWD"));
        assert!(!result.iter().any(|(k, _)| k == "SHLVL"));
        assert!(!result.iter().any(|(k, _)| k == "_"));
        assert!(result.iter().any(|(k, _)| k == "HOME"));
    }

    #[test]
    fn shell_env_skips_entries_without_equals() {
        let sentinel = b"__JKCODINGAGENT_ENV_START__\0";
        let mut input = Vec::new();
        input.extend_from_slice(sentinel);
        input.extend_from_slice(b"HOME=/Users/test\0NOEQUALSSIGN\0PATH=/usr/bin\0");

        let result = parse_shell_env_output(&input).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn shell_env_skips_empty_key() {
        let sentinel = b"__JKCODINGAGENT_ENV_START__\0";
        let mut input = Vec::new();
        input.extend_from_slice(sentinel);
        input.extend_from_slice(b"=value\0HOME=/test\0");

        let result = parse_shell_env_output(&input).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.iter().any(|(k, _)| k == "HOME"));
    }

    #[test]
    fn shell_env_returns_none_without_sentinel() {
        let input = b"HOME=/Users/test\0PATH=/usr/bin\0";
        assert!(parse_shell_env_output(input).is_none());
    }

    #[test]
    fn shell_env_returns_none_for_empty_after_sentinel() {
        let sentinel = b"__JKCODINGAGENT_ENV_START__\0";
        let input = sentinel.to_vec();
        assert!(parse_shell_env_output(&input).is_none());
    }

    #[test]
    fn shell_env_handles_empty_value() {
        let sentinel = b"__JKCODINGAGENT_ENV_START__\0";
        let mut input = Vec::new();
        input.extend_from_slice(sentinel);
        input.extend_from_slice(b"EMPTY=\0HOME=/test\0");

        let result = parse_shell_env_output(&input).unwrap();
        assert!(result.iter().any(|(k, v)| k == "EMPTY" && v.is_empty()));
    }

    // ── build_fallback_path ──────────────────────────────────────────────────

    #[test]
    fn fallback_path_includes_common_directories() {
        let path = build_fallback_path();
        assert!(path.contains("/opt/homebrew/bin"));
        assert!(path.contains("/usr/local/bin"));
        assert!(path.contains("/usr/bin"));
        assert!(path.contains("/bin"));
    }

    #[test]
    fn fallback_path_is_colon_separated() {
        let path = build_fallback_path();
        let parts: Vec<&str> = path.split(':').collect();
        assert!(parts.len() >= 5);
    }

    // ── build_fallback_env ───────────────────────────────────────────────────

    #[test]
    fn fallback_env_has_path() {
        let env = build_fallback_env();
        assert!(env.iter().any(|(k, _)| k == "PATH"));
    }

    #[test]
    fn fallback_env_has_home_if_available() {
        let env = build_fallback_env();
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            assert!(env.iter().any(|(k, v)| k == "HOME" && v == &home));
        }
    }

    #[test]
    fn fallback_env_skips_filtered_keys() {
        let env = build_fallback_env();
        assert!(!env.iter().any(|(k, _)| k == "PWD" || k == "OLDPWD" || k == "SHLVL" || k == "_"));
    }

    // ── AgentVersions ────────────────────────────────────────────────────────

    #[test]
    fn agent_versions_default_empty() {
        let versions = AgentVersions::default();
        assert!(versions.claude_version.is_empty());
        assert!(versions.codex_version.is_empty());
    }

    #[test]
    fn agent_versions_serializes() {
        let versions = AgentVersions {
            claude_version: "2.1.87".to_string(),
            codex_version: "0.1.2025".to_string(),
        };
        let json = serde_json::to_string(&versions).unwrap();
        assert!(json.contains("2.1.87"));
        assert!(json.contains("0.1.2025"));
    }

    // ── app_data_dir ─────────────────────────────────────────────────────────

    #[test]
    fn app_data_dir_uses_home() {
        let dir = app_data_dir().unwrap();
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
        assert_eq!(dir, home.join(".jkcodingagent"));
    }

    // ── settings_path ────────────────────────────────────────────────────────

    #[test]
    fn settings_path_under_app_data_dir() {
        let path = settings_path().unwrap();
        assert!(path.to_string_lossy().ends_with("settings.json"));
        assert!(path.to_string_lossy().contains(".jkcodingagent"));
    }
}
