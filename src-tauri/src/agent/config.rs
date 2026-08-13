use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

pub const DEFAULT_SUMMARY_MODEL: &str = "deepseek-v4-flash";

pub const DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT: &str = r#"# 普通聊天

你是桌面客户端中的普通聊天助手。
当前会话不是项目 Agent 会话，没有项目目录、项目文件系统或子进程能力。
你可以调用 local_zsh 在受限本地目录 .jkcodingagent/local_env/zsh 中执行 macOS zsh 命令；所有产物应留在该目录，工具会维护 audit.json 审计历史。
如果设置中启用了聊天 MCP 工具，可按工具说明调用这些动态发现的外部工具。
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

const LEGACY_DEFAULT_SOUL: &str = r#"# JKBot 调度代理

你是桌面客户端中的编程任务调度代理，负责把用户的编码需求高效推进到可交付结果。
你的工作是拆解、调查、定位、补齐上下文、识别风险、整理执行说明，并把实现任务委派给 Claude 或 Codex。

工作原则：
- 先调查再判断，先定位再委派，不臆测。
- 以当前交付目标为中心，只做必要推进。
- 优先保证正确性、完成度和执行效率。
- 除非只是极小范围的验证性修改，否则不要亲自承担主要实现。

推荐流程：
1. 用工具了解需求、代码现状、调用链、影响面、约束与验证方式。
   探索默认链路是：glob 缩小文件范围 → grep 精确匹配内容 → read_file 加载确认；证据不足时继续下一轮收缩。
   对互相独立的只读探索，优先在同一轮返回多个工具调用；需要查多个目录、文件、glob 模式或 grep 模式时，使用 `paths` / `patterns` 数组参数减少轮次。
2. 整理成可直接开工的自包含任务说明。
3. 根据任务特点选择合适的执行代理发起委派。
4. 子任务返回后继续协调，决定补充指令、收口、退出或继续调查。

委派策略：
- `dispatch_claude`：适合新功能、快速迭代、探索性调试和需要边实现边收敛的任务。
- `dispatch_codex`：适合重构、结构治理、跨文件一致性修改和高风险收口任务。

任务说明要求：
- 交代清楚目标、背景、相关文件或符号、限制条件、验证方式和交付预期。
- 风险、兼容性要求和未决假设必须显式写明。
- 描述要具体，让执行代理接手后能直接开工。

协作要求：
- 风险操作先说明影响，再请求确认。
- 默认使用简体中文输出。
- 结论直接清晰，偏工程执行。
- 如果两个执行代理可以并行推进不同子问题，可以在同一轮同时调用多个 `dispatch_*`。
- 但同一 session 中，同一 agent 同时最多只允许一个活跃或待启动子进程。
"#;

const LEGACY_DEFAULT_SOUL_V1: &str = r#"# JKBot Dispatcher

You are JKBot, a dispatcher coding agent embedded in a desktop client.
You talk with the user, clarify intent when necessary, make concise plans, and use tools to inspect or modify the active workspace.
When the user requests complex coding tasks, delegate them to a specialist terminal agent.
Choose `dispatch_claude` for faster exploration, greenfield features, algorithm experiments, and broad solution search.
Choose `dispatch_codex` for slower but more careful refactoring, structural cleanup, and regression-sensitive edits.
Respond in Simplified Chinese unless the user explicitly asks for another language.

Engineering rules:
- Prefer simple, direct implementations.
- Read relevant files before editing.
- Keep changes scoped to the user's request.
- Do not invent results. Use tools when local facts are needed.
- For risky operations, explain the impact and ask for confirmation.
- For substantial coding tasks, delegate to Claude or Codex instead of doing everything inline.
"#;

const DEFAULT_USER: &str = r#"# 用户偏好

用户是程序员，偏好高信息密度、面向实现的协作方式。

- 少讲基础概念，优先给事实、路径、符号、原因、风险和可执行结论。
- 调查阶段重证据，执行阶段重交付和验证。
- 如果需要委派子任务，任务说明必须具体到可直接开工。
- 默认用中文；只有用户明确要求时再切换语言。
"#;

const LEGACY_DEFAULT_USER: &str = r#"# User Preferences

The user is an experienced developer. Be concise, factual, and implementation-oriented.
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

impl DispatcherAgentConfig {
    pub fn load() -> Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| anyhow!("failed to resolve home directory"))?;
        let root_dir = home.join(".jkcodingagent");

        fs::create_dir_all(root_dir.join("memory")).context("create ~/.jkcodingagent/memory")?;
        fs::create_dir_all(root_dir.join("skills")).context("create ~/.jkcodingagent/skills")?;

        sync_bundled_prompt_file(
            root_dir.join("SOUL.md"),
            DEFAULT_SOUL,
            &[LEGACY_DEFAULT_SOUL, LEGACY_DEFAULT_SOUL_V1],
        )?;
        sync_bundled_prompt_file(
            root_dir.join("USER.md"),
            DEFAULT_USER,
            &[LEGACY_DEFAULT_USER],
        )?;
        write_if_missing(root_dir.join("memory").join("MEMORY.md"), "# 记忆\n\n")?;

        Ok(Self {
            db_path: root_dir.join("jkbot.sqlite3"),
            root_dir,
            api_key: std::env::var("DASHSCOPE_API_KEY")
                .or_else(|_| std::env::var("OPENAI_API_KEY"))
                .unwrap_or_default(),
            api_base: std::env::var("DASHSCOPE_API_BASE")
                .or_else(|_| std::env::var("OPENAI_API_BASE"))
                .unwrap_or_else(|_| {
                    "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string()
                }),
            model: std::env::var("MODEL_NAME").unwrap_or_else(|_| "qwen3.6-plus".to_string()),
            summary_model: std::env::var("SUMMARY_MODEL_NAME")
                .unwrap_or_else(|_| DEFAULT_SUMMARY_MODEL.to_string()),
            vision_model: std::env::var("VISION_MODEL_NAME").unwrap_or_default(),
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
        anyhow::bail!("错误：模型服务缺少 API 基础地址（Base URL），请先在设置中配置对应模型服务。");
    }
    if model.trim().is_empty() {
        anyhow::bail!("错误：模型服务缺少模型名称，请先在设置中配置对应模型服务。");
    }
    Ok(())
}

/// 原子写：先写同目录临时文件，再 rename 原子替换（G9-16）。
///
/// `fs::write` 直接截断覆盖是非原子的：进程在写入中途崩溃（或磁盘错误）会留下
/// 截断/损坏的 SOUL.md / USER.md，而损坏内容既不等于内置版本也不等于任何 legacy
/// 变体，`sync_bundled_prompt_file` 会把它当成「用户自定义内容」永久保留，
/// 系统无法自愈。同一文件系统下 rename 是原子的：目标文件要么是旧内容、
/// 要么是完整新内容。
fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let tmp_path = PathBuf::from(format!("{}.tmp-{}", path.display(), std::process::id()));
    let result = fs::write(&tmp_path, content)
        .with_context(|| format!("write {}", tmp_path.display()))
        .and_then(|()| {
            fs::rename(&tmp_path, path).with_context(|| {
                format!("rename {} -> {}", tmp_path.display(), path.display())
            })
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

fn sync_bundled_prompt_file(path: PathBuf, content: &str, legacy_variants: &[&str]) -> Result<()> {
    match fs::read_to_string(&path) {
        Ok(existing) => {
            let can_overwrite =
                existing == content || legacy_variants.iter().any(|legacy| existing == *legacy);
            if can_overwrite && existing != content {
                atomic_write(&path, content)
                    .with_context(|| format!("write {}", path.display()))?;
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            atomic_write(&path, content).with_context(|| format!("write {}", path.display()))
        }
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path() -> PathBuf {
        std::env::temp_dir().join(format!("aha-prompt-sync-{}.md", uuid::Uuid::new_v4()))
    }

    #[test]
    fn upgrades_only_exact_bundled_prompt_variants() {
        for legacy in [LEGACY_DEFAULT_SOUL, LEGACY_DEFAULT_SOUL_V1] {
            let path = test_path();
            fs::write(&path, legacy).unwrap();
            sync_bundled_prompt_file(
                path.clone(),
                DEFAULT_SOUL,
                &[LEGACY_DEFAULT_SOUL, LEGACY_DEFAULT_SOUL_V1],
            )
            .unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), DEFAULT_SOUL);
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn preserves_custom_prompt_even_with_legacy_heading() {
        let path = test_path();
        let custom = "# JKBot Dispatcher\n\n用户自定义内容\n";
        fs::write(&path, custom).unwrap();
        sync_bundled_prompt_file(
            path.clone(),
            DEFAULT_SOUL,
            &[LEGACY_DEFAULT_SOUL, LEGACY_DEFAULT_SOUL_V1],
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), custom);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn creates_missing_prompt_and_preserves_unrelated_custom_prompt() {
        let missing = test_path();
        sync_bundled_prompt_file(missing.clone(), DEFAULT_SOUL, &[]).unwrap();
        assert_eq!(fs::read_to_string(&missing).unwrap(), DEFAULT_SOUL);
        fs::remove_file(missing).unwrap();

        let custom = test_path();
        fs::write(&custom, "# My Agent\n\ncustom\n").unwrap();
        sync_bundled_prompt_file(custom.clone(), DEFAULT_SOUL, &[]).unwrap();
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
