use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

pub const DEFAULT_SUMMARY_MODEL: &str = "deepseek-v4-flash";

pub const DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT: &str = r#"# 普通聊天

你是桌面客户端中的普通聊天助手。
当前会话不是项目 Agent 会话，没有项目目录、项目文件系统或子进程能力。
你可以调用 local_zsh 在受限本地目录 .jkcodingagent/local_env/zsh 中执行 macOS zsh 命令；所有产物应留在该目录，工具会维护 audit.json 审计历史。
如果当前聊天分类显式配置了 MCP 工具，它们会以 mcp__ 前缀出现在你的工具列表中（见「当前可用 MCP 工具」清单），可按工具说明直接调用；未配置时不要臆断存在 MCP 工具。
你可以按需使用浏览器工具打开网页、点击、输入、等待、读取页面可访问性树快照、请求视觉辅助分析和关闭浏览器，用于网页自动化与公开信息检索。
浏览器自动化统一使用 ref：先调用 browser_read_text 获取 Accessibility Tree 快照，再使用快照中的 ref 调用点击、输入或局部读取工具；不要使用 CSS selector。
元素 ref 只在最近一次 browser_read_text 快照中有效。页面导航或内容变化后旧 ref 会失效，收到 ref 失效错误时系统会自动附上新快照，基于新快照重新选择元素即可。
检索问题信息时，优先打开明确网址；没有网址时可打开搜索引擎结果页并读取页面文本，不要伪造检索结果。
可以基于用户直接提供的文本、代码片段、错误信息或图片进行解释、分析、改写和建议。
默认使用简体中文，表达直接、清晰、面向有经验的开发者。

## 子智能体

- 你可以调用 list_sub_agents 查看当前可用的子智能体列表。
- 使用 call_sub_agent(agent_id, task) 调用子智能体处理特定领域的复杂任务。子智能体拥有独立的执行上下文，内部工具调用对你透明，你只会收到最终结果。

## 图片生成与引用

- 你可以调用 generate_image 工具根据文本描述生成图片。建议提供 image_name 参数为图片命名。
- 你可以调用 edit_image 工具对现有图片进行编辑。需要提供图片的本地绝对路径。
- 工具返回结果中会包含该图片的本地绝对路径。
- 如果你想在回答中展示生成的图片，直接使用 Markdown 图片引用语法引用工具返回的原始本地绝对路径即可。
"#;

const DEFAULT_SOUL: &str = r#"# JKBot 项目编排器

你是桌面客户端中的项目 Agent，负责调查代码、拆解任务并提交可执行的 PI Agent DAG。

工作原则：
- 先调查代码与约束，再规划；证据不足时继续读取，不臆测。
- 节点任务必须自包含，明确目标、相关路径、限制、验证方式与交付结果。
- 只使用运行时提供的 Harness 模型 ID、基础工具组和特殊工具。
- 让无依赖节点并行；存在数据或写入顺序依赖时显式声明依赖。
- 默认使用简体中文，结论直接、工程化。
"#;

const DEFAULT_USER: &str = r#"# 用户偏好

用户是程序员，偏好高信息密度、面向实现的协作方式。

- 少讲基础概念，优先给事实、路径、符号、原因、风险和可执行结论。
- 调查阶段重证据，执行阶段重交付和验证。
- 如果需要委派子任务，任务说明必须具体到可直接开工。
- 默认用中文；只有用户明确要求时再切换语言。
"#;

#[derive(Debug, Clone)]
pub struct DispatcherAgentConfig {
    pub root_dir: PathBuf,
    pub db_path: PathBuf,
    pub api_key: String,
    pub api_base: String,
    pub model: String,
    pub summary_model: String,
    pub vision_model: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub max_tool_iterations: usize,
    pub exec_timeout_secs: u64,
    pub restrict_to_workspace: bool,
    pub context_debug: bool,
}

/// 模型 env 回退开关：默认关闭。设置中心（dispatcher_settings 表）是模型
/// 配置的唯一权威源；仅当显式设置 `AHA_ALLOW_ENV_PROVIDER=1`（开发场景）
/// 时才允许从环境变量解析模型凭据，消除「DB + env 双权威源」的漂移面。
fn env_provider_allowed() -> bool {
    std::env::var("AHA_ALLOW_ENV_PROVIDER").is_ok_and(|value| value == "1")
}

/// 纯路径解析：返回 `~/.jkcodingagent`，不建目录、不写文件。
/// 仅供只需要资源根路径的调用方使用（如 python_runner 的运行目录），
/// 避免为此引入 `DispatcherAgentConfig::load()` 的目录初始化副作用。
pub fn resolve_home_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("failed to resolve home directory"))?;
    Ok(home.join(".jkcodingagent"))
}

impl DispatcherAgentConfig {
    pub fn load() -> Result<Self> {
        let root_dir = resolve_home_dir()?;

        fs::create_dir_all(root_dir.join("memory")).context("create ~/.jkcodingagent/memory")?;
        fs::create_dir_all(root_dir.join("skills")).context("create ~/.jkcodingagent/skills")?;

        write_if_missing(root_dir.join("SOUL.md"), DEFAULT_SOUL)?;
        write_if_missing(root_dir.join("USER.md"), DEFAULT_USER)?;
        write_if_missing(root_dir.join("memory").join("MEMORY.md"), "# 记忆\n\n")?;

        let (api_key, api_base, model, summary_model, vision_model) = if env_provider_allowed() {
            (
                std::env::var("DASHSCOPE_API_KEY")
                    .or_else(|_| std::env::var("OPENAI_API_KEY"))
                    .unwrap_or_default(),
                std::env::var("DASHSCOPE_API_BASE")
                    .or_else(|_| std::env::var("OPENAI_API_BASE"))
                    .unwrap_or_else(|_| {
                        "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string()
                    }),
                std::env::var("MODEL_NAME").unwrap_or_else(|_| "qwen3.6-plus".to_string()),
                std::env::var("SUMMARY_MODEL_NAME")
                    .unwrap_or_else(|_| DEFAULT_SUMMARY_MODEL.to_string()),
                std::env::var("VISION_MODEL_NAME").unwrap_or_default(),
            )
        } else {
            (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            )
        };

        Ok(Self {
            db_path: root_dir.join("jkbot.sqlite3"),
            root_dir,
            api_key,
            api_base,
            model,
            summary_model,
            vision_model,
            max_tokens: 8192,
            temperature: 0.1,
            max_tool_iterations: 200,
            exec_timeout_secs: 60,
            restrict_to_workspace: true,
            context_debug: false,
        })
    }
}

/// 模型 provider 配置完整性校验（G9-15）：API Key / Base URL / 模型名任一缺失
/// 时显式失败并给出「错误：」提示，避免配置缺失延迟到 HTTP 请求时才以晦涩错误暴露。
///
/// 调用点是 run 入口（`run_agent_turn`）。`DispatcherAgentConfig::load()` 不做
/// 该校验：环境变量回退允许为空，用户可以在设置页提供模型服务配置。
pub fn validate_provider_completeness(api_key: &str, api_base: &str, model: &str) -> Result<()> {
    if api_key.trim().is_empty() {
        anyhow::bail!("错误：模型服务缺少 API Key，请先在设置中配置对应模型服务。");
    }
    if api_base.trim().is_empty() {
        anyhow::bail!(
            "错误：模型服务缺少 API 基础地址（Base URL），请先在设置中配置对应模型服务。"
        );
    }
    if model.trim().is_empty() {
        anyhow::bail!("错误：模型服务缺少模型名称，请先在设置中配置对应模型服务。");
    }
    Ok(())
}

/// 原子写：先写同目录临时文件，再 rename 原子替换（G9-16）。
///
/// `fs::write` 直接截断覆盖是非原子的：进程在写入中途崩溃（或磁盘错误）会留下
/// 截断/损坏的文件。同一文件系统下 rename 是原子的：目标文件要么是旧内容、
/// 要么是完整新内容。
fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let tmp_path = PathBuf::from(format!("{}.tmp-{}", path.display(), std::process::id()));
    let result = fs::write(&tmp_path, content)
        .with_context(|| format!("write {}", tmp_path.display()))
        .and_then(|()| {
            fs::rename(&tmp_path, path)
                .with_context(|| format!("rename {} -> {}", tmp_path.display(), path.display()))
        });
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

fn write_if_missing(path: PathBuf, content: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    atomic_write(&path, content).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path() -> PathBuf {
        std::env::temp_dir().join(format!("aha-prompt-sync-{}.md", uuid::Uuid::new_v4()))
    }

    #[test]
    fn creates_missing_prompt_and_preserves_existing_file() {
        let missing = test_path();
        write_if_missing(missing.clone(), DEFAULT_SOUL).unwrap();
        assert_eq!(fs::read_to_string(&missing).unwrap(), DEFAULT_SOUL);
        fs::remove_file(missing).unwrap();

        let custom = test_path();
        fs::write(&custom, "# My Agent\n\ncustom\n").unwrap();
        write_if_missing(custom.clone(), DEFAULT_SOUL).unwrap();
        assert_eq!(
            fs::read_to_string(&custom).unwrap(),
            "# My Agent\n\ncustom\n"
        );
        fs::remove_file(custom).unwrap();
    }

    #[test]
    fn atomic_write_creates_and_replaces_file() {
        let path = test_path();
        atomic_write(&path, "v1").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "v1");
        atomic_write(&path, "v2").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "v2");
        // 不残留临时文件
        let tmp = PathBuf::from(format!("{}.tmp-{}", path.display(), std::process::id()));
        assert!(!tmp.exists());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn provider_completeness_requires_key_base_and_model() {
        assert!(validate_provider_completeness("sk-x", "https://api.example/v1", "qwen").is_ok());
        assert!(validate_provider_completeness("", "https://api.example/v1", "qwen").is_err());
        assert!(validate_provider_completeness("sk-x", "  ", "qwen").is_err());
        assert!(validate_provider_completeness("sk-x", "https://api.example/v1", "").is_err());
        let error = validate_provider_completeness("", "u", "m").unwrap_err();
        assert!(error.to_string().starts_with("错误："));
    }
}
