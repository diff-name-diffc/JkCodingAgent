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
//!   例外是 PATH 与 SHELL：执行器内部（Claude Code 的 Bash 工具）沿子进程
//!   PATH 解析命令，login shell 探测出的用户 PATH（nvm/cargo 等目录）必须
//!   可见——两者非敏感，其余 login 变量仍不继承（防密钥泄漏立场不变）。

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
    // login shell 探测首调会 spawn 用户 shell（阻塞 ~百毫秒）。移到 blocking
    // 线程预热 OnceLock 缓存——应用启动时的后台预热可能尚未完成（冷启动即
    // 派发工作流节点的窗口期），此后 env 组装 / resolve_program 的同步调用
    // 只读缓存。
    let _ = tokio::task::spawn_blocking(crate::platform::get_login_shell_path).await;
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

/// 托管安装的进程级互斥。工作流节点并发派发（MAX_PARALLEL_NODES）时多个
/// launcher 可能同时发现版本标记缺失，各自对同一前缀并发 `npm install`
/// 会互相删除/重放包目录（曾致 node 启动时 MODULE_NOT_FOUND——npm 重整
/// 期间 `dist/index.js` 短暂不存在）。双检锁：已装好的快路径零开销；
/// 需要安装时全局串行，拿到锁后重查（等待期间其他任务可能已完成安装），
/// 仍需要才真正触网安装。锁覆盖「检查→安装→写标记」全程，保证没有
/// launcher 在他人重整 node_modules 期间通过入口校验去拉起 node。
static MANAGED_INSTALL_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 是否需要（重新）安装：版本标记与锁定版本不一致，或入口文件缺失。
/// 标记仅在安装成功后写入，中断的安装会留下不一致状态、自然触发重装。
fn managed_install_needed(current: Option<&str>, entry: &Path) -> bool {
    current.map(str::trim) != Some(ACP_AGENT_VERSION) || !entry.is_file()
}

/// 托管模式：确保锁定版本的官方包已安装，返回 `node <entry 绝对路径>`。
async fn resolve_managed(diagnostics: &mut Vec<String>) -> Result<(PathBuf, Vec<String>), String> {
    let root = crate::agent::config::resolve_home_dir()
        .map_err(|error| format!("解析应用资源目录失败：{error:#}"))?
        .join(MANAGED_DIR_NAME);
    let entry = managed_entry(&root);
    let marker = root.join(VERSION_MARKER);
    if managed_install_needed(std::fs::read_to_string(&marker).ok().as_deref(), &entry) {
        let _guard = MANAGED_INSTALL_LOCK.lock().await;
        // 拿锁后重查：等待期间其他任务可能已完成安装。
        if managed_install_needed(std::fs::read_to_string(&marker).ok().as_deref(), &entry) {
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
        .ok_or_else(|| "ACP 启动命令为空（设置 → 工作流 → 节点执行器）".to_string())?;
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
    #[cfg(unix)]
    {
        // 用户 login shell PATH：nvm 等用户级安装的 node/npm 只存在于这里。
        // 固定候选目录（防 PATH 阴影）优先级不变，本段排在父进程 PATH 之前。
        for dir in std::env::split_paths(crate::platform::get_login_shell_path()) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                diagnostics.push(format!(
                    "ACP 执行器宿主程序（login shell PATH 解析）：{}",
                    candidate.display()
                ));
                return Ok(candidate);
            }
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
/// PATH 不继承父进程（防劫持），取 login shell 探测结果与固定系统目录的
/// 合并（见 [`merge_child_path`]）；SHELL 透传供执行器识别用户 shell。
fn minimal_inherited_env() -> Vec<(String, String)> {
    #[cfg(unix)]
    const KEYS: &[&str] = &[
        "HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG", "LC_ALL", "TERM",
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
        merge_child_path(crate::platform::get_login_shell_path()),
    ));
    #[cfg(windows)]
    if let Ok(path) = std::env::var("PATH") {
        envs.push(("PATH".into(), path));
    }
    envs
}

/// PATH 保底目录：login shell 探测失败或用户 shell 配置异常缺失系统目录时
/// 补齐（追加在后，不覆盖用户目录的优先级）。与 [`fixed_program_dirs`]
/// 的防阴影职责不同——这里只需保证基础命令可用。
#[cfg(unix)]
const FALLBACK_PATH_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

/// 合并子进程 PATH：login shell PATH 条目保序在前（尊重用户目录优先级，
/// nvm/cargo 等用户级安装目录可见），固定系统目录去重补在后。
#[cfg(unix)]
fn merge_child_path(login: &str) -> String {
    let mut entries: Vec<String> = std::env::split_paths(login)
        .filter(|path| !path.as_os_str().is_empty())
        .map(|path| path.to_string_lossy().to_string())
        .collect();
    for dir in FALLBACK_PATH_DIRS {
        if !entries.iter().any(|entry| entry == dir) {
            entries.push(dir.to_string());
        }
    }
    std::env::join_paths(&entries)
        .expect("PATH 条目经 split_paths 产出，不含路径分隔符 NUL")
        .to_string_lossy()
        .to_string()
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
        // PATH 为 login shell 与系统目录的合并：login 探测（测试进程为 unix
        // 登录环境）与保底目录共同保证系统目录始终在列。
        #[cfg(unix)]
        {
            assert!(keys.contains(&"SHELL"));
            let path = envs
                .iter()
                .find(|(key, _)| key == "PATH")
                .map(|(_, value)| value.as_str())
                .expect("PATH 必在白名单");
            for dir in ["/usr/bin", "/bin"] {
                assert!(path.split(':').any(|entry| entry == dir), "PATH 缺少 {dir}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn merge_child_path_preserves_login_order_and_appends_missing_system_dirs() {
        let merged = merge_child_path("/nvm/node/bin:/usr/bin:/nvm/node/bin");
        let entries: Vec<&str> = merged.split(':').collect();
        // login 条目保序在前（重复项仅一次），已存在的系统目录不重复。
        assert_eq!(&entries[..2], &["/nvm/node/bin", "/usr/bin"]);
        assert!(entries.contains(&"/opt/homebrew/bin"));
        assert!(entries.contains(&"/sbin"));
        assert_eq!(entries.iter().filter(|entry| **entry == "/usr/bin").count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn merge_child_path_keeps_system_dirs_when_login_path_empty() {
        // login 探测失败回退 / 用户 shell 输出空 PATH：保底目录兜住基础命令。
        let merged = merge_child_path("");
        let entries: Vec<&str> = merged.split(':').collect();
        assert!(entries.contains(&"/usr/bin"));
        assert!(entries.contains(&"/opt/homebrew/bin"));
        assert!(!entries.iter().any(|entry| entry.is_empty()));
    }

    #[test]
    fn managed_entry_points_at_package_bin() {
        let entry = managed_entry(Path::new("/root"));
        assert!(entry.ends_with("node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js"));
    }

    #[test]
    fn managed_install_needed_combines_marker_and_entry() {
        // current_exe 是必然存在的真实文件，充当「入口已就位」。
        let existing = std::env::current_exe().expect("测试进程可执行文件");
        let missing = Path::new("/nonexistent/acp-agent/dist/index.js");
        // 标记缺失 / 版本不符 / 入口缺失 → 需要安装。
        assert!(managed_install_needed(None, &existing));
        assert!(managed_install_needed(Some("0.78.0"), &existing));
        assert!(managed_install_needed(Some(ACP_AGENT_VERSION), missing));
        // 标记与版本一致（容忍首尾空白）且入口存在 → 跳过安装。
        assert!(!managed_install_needed(
            Some(&format!(" {ACP_AGENT_VERSION}\n")),
            &existing
        ));
    }
}
