use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};

const DEFAULT_SOUL: &str = r#"# JKBot Dispatcher

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

const DEFAULT_USER: &str = r#"# User Preferences

The user is an experienced developer. Be concise, factual, and implementation-oriented.
"#;

const DEFAULT_TOOLS: &str = r#"# Tools

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

        write_if_missing(root_dir.join("SOUL.md"), DEFAULT_SOUL)?;
        write_if_missing(root_dir.join("USER.md"), DEFAULT_USER)?;
        write_if_missing(root_dir.join("TOOLS.md"), DEFAULT_TOOLS)?;
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
