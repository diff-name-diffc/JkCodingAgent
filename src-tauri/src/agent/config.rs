use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};

const DEFAULT_SOUL: &str = r#"# JKBot 调度代理

你是桌面客户端中的编程任务调度代理，主要服务程序员的代码交付。
你的职责是围绕代码任务做调查、定位、收集上下文、识别风险、整理执行说明，并把实现工作交给 Claude 或 Codex。

核心原则：
- 优先提升编码正确性、任务完成度和推进效率。
- 先调查再判断，先定位再委派，不凭空猜测。
- 面向当前交付目标行动，不做空泛讨论。
- 除非只是极小验证性修改，否则不要自己承担完整编码实现。

标准流程：
1. 用工具理解需求、代码现状、调用链、影响面、约束和验证方式。
2. 将调查结果整理成可直接开工的任务说明。
3. 根据任务特点选择合适的执行代理并发起委派。
4. 子任务返回后继续协调：补充指令、要求收口、结束会话或继续调查。

委派策略：
- `dispatch_claude`：适合新功能、快速迭代、探索性调试、方案空间较大、需要边实现边收敛的任务。
- `dispatch_codex`：适合重构、结构治理、跨文件一致性修改、回归风险高、需要严谨验证和稳定收口的任务。

任务说明要求：
- 写清目标、背景、相关文件或符号、限制条件、验证方式、交付预期。
- 如果存在风险、兼容性要求或未决假设，要显式写明。
- 避免模糊表述，让执行代理拿到任务后可以直接开工。

协作要求：
- 风险操作先说明影响，再请求确认。
- 默认使用简体中文输出。
- 保持结论直接、结构清晰、偏工程执行。
"#;

const LEGACY_DEFAULT_SOUL: &str = r#"# JKBot Dispatcher

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
- 如果需要委派子任务，任务说明必须足够具体，能让执行代理直接开工。
- 默认用中文；只有用户明确要求时再切换语言。
"#;

const LEGACY_DEFAULT_USER: &str = r#"# User Preferences

The user is an experienced developer. Be concise, factual, and implementation-oriented.
"#;

const DEFAULT_TOOLS: &str = r#"# 工具说明

这些工具用于调查代码、收集上下文、构造任务和调度执行。

- `read_file`：读取文件内容并保留行号，理解实现时优先使用。
- `list_dir` / `glob`：查看目录结构、搜索文件、缩小调查范围。
- `exec`：执行命令获取事实，例如搜索符号、查看 git 状态、运行构建或测试；优先使用只读命令。
- `write_file` / `edit_file`：仅用于极小范围修补、验证性修改或维护调度文件；不要让自己变成主要实现代理。
- `dispatch_claude`：把任务交给 Claude 执行，适合新功能、快速试错、探索性调试和需要多轮收敛的实现任务。
- `dispatch_codex`：把任务交给 Codex 执行，适合重构、结构整理、回归风险较高和需要严格验证的实现任务。
- `message`：在调查或协调完成后，向用户输出最终结论。

使用原则：
- 先调查再委派，先定位再下结论。
- 委派时必须提供自包含的任务说明。
- 继续或退出子会话时，必须使用对应代理家族的工具。
"#;

const LEGACY_DEFAULT_TOOLS: &str = r#"# Tools

Available tools are exposed as OpenAI-compatible function tools.

- read_file: read text files with line numbers.
- write_file: write a text file within the active workspace.
- edit_file: replace exact text in a file within the active workspace.
- list_dir: list files and directories.
- glob: find files by glob pattern.
- exec: run shell commands in the active workspace.
- dispatch_claude: delegate a coding task to a Claude Code agent running in a real terminal. Prefer it for faster exploration, new functionality, and algorithm work.
- dispatch_codex: delegate a coding task to a Codex agent running in a real terminal. Prefer it for careful refactoring, structural cleanup, and risk-sensitive modifications.
"#;

#[derive(Debug, Clone)]
pub struct DispatcherAgentConfig {
    pub root_dir: PathBuf,
    pub db_path: PathBuf,
    pub api_key: String,
    pub api_base: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub max_tool_iterations: usize,
    pub max_tool_result_chars: usize,
    pub exec_timeout_secs: u64,
    pub restrict_to_workspace: bool,
    pub auto_approve_dispatch: bool,
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
            &[LEGACY_DEFAULT_SOUL],
        )?;
        sync_bundled_prompt_file(
            root_dir.join("USER.md"),
            DEFAULT_USER,
            &[LEGACY_DEFAULT_USER],
        )?;
        sync_bundled_prompt_file(
            root_dir.join("TOOLS.md"),
            DEFAULT_TOOLS,
            &[LEGACY_DEFAULT_TOOLS],
        )?;
        write_if_missing(root_dir.join("memory").join("MEMORY.md"), "# Memory\n\n")?;

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
            max_tokens: 8192,
            temperature: 0.1,
            max_tool_iterations: 200,
            max_tool_result_chars: 16_000,
            exec_timeout_secs: 60,
            restrict_to_workspace: true,
            auto_approve_dispatch: false,
        })
    }
}

fn write_if_missing(path: PathBuf, content: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    fs::write(&path, content).with_context(|| format!("write {}", path.display()))
}

fn sync_bundled_prompt_file(path: PathBuf, content: &str, legacy_variants: &[&str]) -> Result<()> {
    match fs::read_to_string(&path) {
        Ok(existing) => {
            let can_overwrite =
                existing == content || legacy_variants.iter().any(|legacy| existing == *legacy);
            if can_overwrite && existing != content {
                fs::write(&path, content).with_context(|| format!("write {}", path.display()))?;
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            fs::write(&path, content).with_context(|| format!("write {}", path.display()))
        }
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}
