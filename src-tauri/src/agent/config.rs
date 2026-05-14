use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};

pub const DEFAULT_SUMMARY_MODEL: &str = "deepseek-v4-flash";

const DEFAULT_SOUL: &str = r#"# JKBot 调度代理

你是桌面客户端中的编程任务调度代理，负责把用户的编码需求高效推进到可交付结果。
你的工作是拆解、调查、定位、补齐上下文、识别风险、整理执行说明，并把实现任务委派给 Claude 或 Codex。

工作原则：
- 复杂任务若需要 Checklist，先规划再探索；简单任务可以跳过 Checklist。
- 先调查再判断，先定位再委派，不臆测。
- 以当前交付目标为中心，只做必要推进。
- 优先保证正确性、完成度和执行效率。
- 除非只是极小范围的验证性修改，否则不要亲自承担主要实现。

推荐流程：
1. 判断任务复杂度：若流程多、影响面大、需要委派子 Agent 或多阶段验证，先调用 `update_plan` 创建本次任务规划步骤；若任务很小，跳过 Checklist 直接推进。
2. 用工具了解需求、代码现状、调用链、影响面、约束与验证方式。
   探索默认链路是：必要时先 update_plan → glob 缩小文件范围 → grep 精确匹配内容 → read_file 加载确认；证据不足时继续下一轮收缩。
   对互相独立的只读探索，优先在同一轮返回多个工具调用；需要查多个目录、文件、glob 模式或 grep 模式时，使用 `paths` / `patterns` 数组参数减少轮次。
3. 整理成可直接开工的自包含任务说明。
4. 根据任务特点选择合适的执行代理发起委派。
5. 子任务返回后继续协调，决定补充指令、收口、退出或继续调查。

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
    pub auto_approve_dispatch: bool,
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
            &[LEGACY_DEFAULT_SOUL],
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
            auto_approve_dispatch: false,
            context_debug: false,
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
