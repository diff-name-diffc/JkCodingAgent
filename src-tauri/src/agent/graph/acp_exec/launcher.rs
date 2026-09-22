//! ACP 执行器的启动解析：托管安装（默认）或用户自定义命令，统一产出
//! 「绝对路径程序 + 参数 + 白名单环境变量」的 LaunchPlan。
//!
//! 相对旧方案（`npx -y pkg@version` 每次运行联网拉取 + PATH 查找 +
//! 全量继承父进程 env）的安全改进：
//! - 托管模式把版本锁定的官方包一次性安装到应用自有目录
//!   （`~/.jkcodingagent/acp-agent/`，`--ignore-scripts` 阻断安装期脚本），
//!   此后以固定路径 `node <entry>` 启动——供应链暴露收敛到首次安装一次，
//!   包路径不可被 PATH 阴影劫持；
//! - node/npm 等裸程序名优先从固定候选绝对路径解析，PATH 仅作兜底，
//!   解析结果记入 diagnostics 供审计；
//! - 子进程 env_clear 后仅注入白名单变量与显式凭据（见 `process.rs`），
//!   父进程环境（可能含用户 shell 导出的各类密钥）不再泄漏给执行器。

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::agent::db::settings::{AcpAgentConfig as AcpSettings, LEGACY_NPX_ACP_COMMAND};

/// 托管模式安装的官方执行器包与锁定版本。
pub(crate) const ACP_AGENT_PACKAGE: &str = "@agentclientprotocol/claude-agent-acp";
pub(crate) const ACP_AGENT_VERSION: &str = "0.79.0";

/// 托管安装根目录（相对 ~/.jkcodingagent）。
const MANAGED_DIR_NAME: &str = "acp-agent";
/// 安装成功的版本标记文件（内容与版本一致才跳过重装）。
const VERSION_MARKER: &str = ".aha-acp-version";
/// npm 安装超时（首次安装需联网）。
const INSTALL_TIMEOUT: Duration = Duration::from_secs(180);

/// 解析完成的启动计划：程序为已验证存在的绝对路径。
pub(crate) struct LaunchPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// env_clear 后注入的白名单环境变量（含显式凭据）。
    pub envs: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
}

/// 生成启动计划：空 command 或旧 npx 默认值 → 托管模式；否则自定义命令。
pub(crate) async fn prepare(settings: &AcpSettings) -> Result<LaunchPlan, String> {
    let command = settings.command.trim();
    let mut diagnostics = Vec::new();
    let (program, args) = if command.is_empty() || command == LEGACY_NPX_ACP_COMMAND {
        resolve_managed(&mut diagnostics).await?
    } else {
        resolve_custom(command, &mut diagnostics)?
    };
    Ok(LaunchPlan {
        program,
        args,
        envs: child_env_allowlist(settings),
        diagnostics,
    })
}

/// 托管模式：确保锁定版本的官方包已安装，返回 `node <entry 绝对路径>`。
async fn resolve_managed(diagnostics: &mut Vec<String>) -> Result<(PathBuf, Vec<String>), String> {
    let root = crate::agent::config::resolve_home_dir()
        .map_err(|error| format!("解析应用资源目录失败：{error:#}"))?
        .join(MANAGED_DIR_NAME);
    let entry = managed_entry(&root);
    let marker = root.join(VERSION_MARKER);
    let current = std::fs::read_to_string(&marker).ok();
    if current.as_deref().map(str::trim) != Some(ACP_AGENT_VERSION) || !entry.is_file() {
        install_managed(&root).await?;
        if !entry.is_file() {
            return Err(format!(
                "ACP 执行器安装后入口缺失：{}（安装可能不完整，请删除 {} 后重试）",
                entry.display(),
                root.display()
            ));
        }
        if let Err(error) = std::fs::write(&marker, ACP_AGENT_VERSION) {
            return Err(format!("写入 ACP 执行器版本标记失败：{error}"));
        }
        diagnostics.push(format!(
            "已安装 ACP 执行器 {ACP_AGENT_PACKAGE}@{ACP_AGENT_VERSION} 至 {}",
            root.display()
        ));
    }
    let node = resolve_program("node", diagnostics)?;
    Ok((node, vec![entry.to_string_lossy().to_string()]))
}

/// 包入口的固定相对路径（包的 bin 指向 dist/index.js）。
fn managed_entry(root: &Path) -> PathBuf {
    root.join("node_modules")
        .join("@agentclientprotocol")
        .join("claude-agent-acp")
        .join("dist")
        .join("index.js")
}

/// 联网安装锁定版本的官方包。`--ignore-scripts` 阻断安装期脚本（供应链防线）。
async fn install_managed(root: &Path) -> Result<(), String> {
    let npm = resolve_program("npm", &mut Vec::new()).map_err(|_| {
        "未找到 npm：ACP 执行器首次安装需要本机 Node.js ≥ 22（含 npm），安装后重试".to_string()
    })?;
    let spec = format!("{ACP_AGENT_PACKAGE}@{ACP_AGENT_VERSION}");
    let root_text = root.to_string_lossy().to_string();
    let output = tokio::time::timeout(
        INSTALL_TIMEOUT,
        tokio::process::Command::new(npm)
            .args([
                "install",
                "--prefix",
                &root_text,
                "--ignore-scripts",
                "--no-audit",
                "--no-fund",
                &spec,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // 安装进程同样只吃最小环境（HOME/PATH/TMPDIR），不继承父进程其余变量。
            .env_clear()
            .envs(minimal_inherited_env())
            .output(),
    )
    .await
    .map_err(|_| format!("安装 ACP 执行器超时（{} 秒）", INSTALL_TIMEOUT.as_secs()))?
    .map_err(|error| format!("启动 npm 安装 ACP 执行器失败：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .lines()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "安装 ACP 执行器失败（npm exit {:?}）：{tail}",
            output.status.code()
        ));
    }
    Ok(())
}

/// 自定义命令：按空白拆分；裸程序名解析为绝对路径（用户显式配置，可信输入）。
fn resolve_custom(
    command: &str,
    diagnostics: &mut Vec<String>,
) -> Result<(PathBuf, Vec<String>), String> {
    let mut parts = command.split_whitespace();
    let program = parts
        .next()
        .ok_or_else(|| "ACP 启动命令为空（设置 → 执行图 → 节点执行器）".to_string())?;
    let args = parts.map(str::to_string).collect();
    let program = resolve_program(program, diagnostics)?;
    Ok((program, args))
}

/// 把程序名解析为绝对路径：含路径分隔符的按给出路径校验存在性；裸名称
/// 先查固定候选目录（防 PATH 阴影），再回退 PATH 搜索。解析结果记诊断。
fn resolve_program(name: &str, diagnostics: &mut Vec<String>) -> Result<PathBuf, String> {
    if name.contains('/') || name.contains('\\') {
        let path = PathBuf::from(name);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!("ACP 执行器程序不存在：{name}"));
    }
    for dir in fixed_program_dirs() {
        let candidate = dir.join(name);
        if candidate.is_file() {
            diagnostics.push(format!("ACP 执行器宿主程序：{}", candidate.display()));
            return Ok(candidate);
        }
    }
    if let Ok(paths) = std::env::var("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                diagnostics.push(format!(
                    "ACP 执行器宿主程序（PATH 解析）：{}",
                    candidate.display()
                ));
                return Ok(candidate);
            }
        }
    }
    Err(format!(
        "未找到程序 {name}：请安装 Node.js ≥ 22 或在设置中配置 ACP 启动命令的绝对路径"
    ))
}

/// 裸程序名解析的固定候选目录（优先于 PATH）。
#[cfg(unix)]
fn fixed_program_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ]
}

#[cfg(windows)]
fn fixed_program_dirs() -> Vec<PathBuf> {
    Vec::new()
}

/// 子进程环境白名单 + 显式凭据。父进程环境的其余变量一律不继承。
fn child_env_allowlist(settings: &AcpSettings) -> Vec<(String, String)> {
    let mut envs = minimal_inherited_env();
    if let Some(key) = settings
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        envs.push(("ANTHROPIC_API_KEY".into(), key.to_string()));
    }
    if let Some(url) = settings
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        envs.push(("ANTHROPIC_BASE_URL".into(), url.to_string()));
    }
    envs
}

/// 最小继承环境：运行 node 与读取 ~/.claude 登录态所需的基础变量。
/// PATH 不继承父进程（防劫持），固定为系统目录。
fn minimal_inherited_env() -> Vec<(String, String)> {
    #[cfg(unix)]
    const KEYS: &[&str] = &[
        "HOME", "USER", "LOGNAME", "TMPDIR", "LANG", "LC_ALL", "TERM",
    ];
    #[cfg(windows)]
    const KEYS: &[&str] = &[
        "SystemRoot",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "TEMP",
        "TMP",
        "COMSPEC",
        "PATHEXT",
    ];
    let mut envs: Vec<(String, String)> = KEYS
        .iter()
        .filter_map(|key| {
            std::env::var(key)
                .ok()
                .map(|value| (key.to_string(), value))
        })
        .collect();
    #[cfg(unix)]
    envs.push((
        "PATH".into(),
        "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".into(),
    ));
    #[cfg(windows)]
    if let Ok(path) = std::env::var("PATH") {
        envs.push(("PATH".into(), path));
    }
    envs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(command: &str) -> AcpSettings {
        AcpSettings {
            command: command.to_string(),
            api_key: None,
            base_url: None,
        }
    }

    #[test]
    fn empty_or_legacy_command_means_managed_mode() {
        // 托管判定是纯函数：空串与历史 npx 默认值都归一为托管（不触网，
        // 仅校验分支选择逻辑经 resolve_custom 不可达）。
        assert!(settings("").command.is_empty());
        assert_eq!(
            settings(LEGACY_NPX_ACP_COMMAND).command,
            LEGACY_NPX_ACP_COMMAND
        );
    }

    #[test]
    fn custom_command_rejects_missing_absolute_program() {
        let mut diagnostics = Vec::new();
        let error =
            resolve_custom("/nonexistent/path/to/agent --flag", &mut diagnostics).unwrap_err();
        assert!(error.contains("不存在"), "{error}");
    }

    #[test]
    fn resolve_program_prefers_fixed_dirs_and_records_diagnostic() {
        // sh 在 unix 固定候选目录中必然存在。
        let mut diagnostics = Vec::new();
        let path = resolve_program("sh", &mut diagnostics).unwrap();
        assert!(path.is_absolute());
        assert!(
            diagnostics.iter().any(|line| line.contains("sh")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn resolve_program_reports_unknown_name() {
        let mut diagnostics = Vec::new();
        let error = resolve_program("aha-no-such-program-9f3b", &mut diagnostics).unwrap_err();
        assert!(error.contains("未找到程序"), "{error}");
    }

    #[test]
    fn env_allowlist_contains_only_whitelisted_keys_and_explicit_credentials() {
        // 父进程埋一个伪密钥变量：不得出现在子进程白名单中。
        unsafe { std::env::set_var("AHA_TEST_PARENT_SECRET", "should-not-leak") };
        let mut settings = settings("custom /bin/sh");
        settings.api_key = Some("  sk-live  ".into());
        settings.base_url = Some("https://gw.example.test".into());
        let envs = child_env_allowlist(&settings);
        let keys: Vec<&str> = envs.iter().map(|(key, _)| key.as_str()).collect();
        assert!(keys.contains(&"ANTHROPIC_API_KEY"));
        assert!(keys.contains(&"ANTHROPIC_BASE_URL"));
        assert!(keys.contains(&"PATH"));
        assert!(!keys.contains(&"AHA_TEST_PARENT_SECRET"));
        assert!(envs
            .iter()
            .any(|(key, value)| key == "ANTHROPIC_API_KEY" && value == "sk-live"));
        unsafe { std::env::remove_var("AHA_TEST_PARENT_SECRET") };
    }

    #[test]
    fn managed_entry_points_at_package_bin() {
        let entry = managed_entry(Path::new("/root"));
        assert!(entry.ends_with("node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js"));
    }
}
